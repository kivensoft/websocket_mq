use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::{Duration, SystemTime}};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use futures_util::{StreamExt, TryStreamExt, future};
use redis::{Client, AsyncCommands};
use serde::{Serialize, Deserialize};
use tokio::{net::{TcpListener, TcpStream}, time::{Instant, Interval}};
use tokio_tungstenite::tungstenite::{protocol::Message};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use futures_channel::mpsc::UnboundedSender;

// 应用订阅消息 {前缀}:*
// 应用发送鉴权请求 {前缀}:{AUTH_TOPIC}:{请求ID}
// 应用监听并处理登录回复 {前缀}:{REPLY_TOPIC}:{AUTH_TOPIC}:{请求ID}
// 三方发送统计请求 {前缀}:{STAT_TOPIC}
// 应用回复三方统计请求 {前缀}:{REPLY_TOPIC}:{STAT_TOPIC}

const AUTH_TOPIC: &str = "authentication";          // 鉴权订阅子地址
const STAT_TOPIC: &str = "statistics";              // 统计信息订阅子地址
const REPLY_TOPIC: &str = "reply";                  // 统计信息订阅回复地址
const LOGOUT_MSG: &str = r#"{"type":"logout"}"#;    // 鉴权失败后回复的消息
const REDIS_RETRY_INTERVAL: u64 = 5;                // redis重新连接的间隔时间（单位：秒）
const PING_INTERVAL: u64 = 10;                      // 发送ping的定时任务时间间隔（单位：秒）
const PING_EXPIRE: u64 = 40;                        // 两次发送ping的最小时间间隔

const CHAN_SEND_FAIL: &str = "channel send fail";

type ClientMap = HashMap<u32, ClientData>;
type SharedClients = Arc<Mutex<ClientMap>>;
// type RedisConn = Arc<Mutex<Connection>>;

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ApiResult {
    pub code: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

struct ClientData {
    login:      bool,
    last_time:  u64,
    sender:     UnboundedSender<Message>,
}

/// 启动websocket转发服务
///
/// 参数:
///
/// * `ws_addr` websocket监听地址:端口
/// * `redis_addr` redis连接地址:端口
/// * `redis_pass` redis连接的用户名:密码
/// * `redis_pre` redis订阅主题前缀
pub async fn start(ws_addr: &str, redis_addr: &str, redis_pass: &str, redis_pre: &str) -> Result<()> {
    let socket_addr = ws_addr.parse::<SocketAddr>()?;
    let listener = TcpListener::bind(socket_addr).await.expect(&format!("Failed to bind {ws_addr}"));
    log::info!("启动websocket服务: {ws_addr}");

    let ws_clients: SharedClients = Arc::new(Mutex::new(HashMap::new()));

    // 启动redis客户端连接，并进行订阅
    let redis_url = format!("redis://{}@{}", redis_pass, redis_addr);
    let redis_client = redis::Client::open(redis_url.as_str()).with_context(|| format!("连接redis失败: {redis_url}"))?;
    log::info!("连接redis服务: {redis_url}");
    create_redis_subcribe(redis_client.clone(), redis_pre, ws_clients.clone());

    // 启动发送ping的定时任务
    create_timer_ping(ws_clients.clone());

    let (mut req_id, redis_pre) = (0, redis_pre.to_owned());
    while let Ok((stream, addr)) = listener.accept().await {
        req_id += 1;
        let (wscs, rc, pre) = (ws_clients.clone(), redis_client.clone(), redis_pre.clone());
        tokio::spawn(async move {
            if let Err(e) = ws_on_conn(req_id, stream, addr, wscs, rc, pre).await {
                log::error!("[WS:{req_id}] 客户端处理过程发生错误: {e}");
            }
        });
    }

    log::debug!("结束websocket服务");
    Ok(())
}

// websocket连接处理函数
async fn ws_on_conn(id: u32, stream: TcpStream, addr: SocketAddr, clients: SharedClients, redis_client: Client, redis_pre: String) -> Result<()> {
    log::debug!("[WS:{id}] 接受来自客户端{}的连接", addr);

    // 获取url中的参数，用于鉴权
    let req_param = Arc::new(Mutex::new(String::new()));
    let rq2 = req_param.clone();
    let ws_stream = tokio_tungstenite::accept_hdr_async(stream, |req: &Request, res: Response| {
        if let Some(query) = req.uri().query() {
            *rq2.lock() = query.to_owned();
        }
        log_request(req);
        Ok(res)
    }).await.expect("websocket握手失败");
    log::debug!("[WS:{id}] WebSocket握手成功");

    let (ws_send, ws_recv) = ws_stream.split();
    let (tx, rx) = futures_channel::mpsc::unbounded();
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
    // 客户端加入集合，新建立的客户端登录状态为false
    clients.lock().insert(id, ClientData{login: false, last_time: now, sender: tx});

    // 发送鉴权校验请求消息
    let auth_topic = format!("{redis_pre}:{AUTH_TOPIC}:{id}");
    let mut publish_conn = redis_client.get_async_connection().await?;
    let req_param = req_param.lock().clone();
    publish_conn.publish(&auth_topic, &req_param).await?;

    // 生成收到消息的处理流
    let incoming = ws_recv.try_for_each(|msg| {
        match msg {
            Message::Text(msg) => {
                log::debug!("[WS:{id}] 收到websocket文本消息: {msg}");
            },
            Message::Close(close_frame) => {
                log::debug!("[WS:{id}] 收到websocket关闭连接消息: {close_frame:?}");
                let client_data = clients.lock().remove(&id);
                if let Some(client_data) = client_data {
                    client_data.sender.unbounded_send(Message::Close(close_frame)).expect(CHAN_SEND_FAIL);
                }
            },
            Message::Ping(v) => {
                log::trace!("[WS:{id}] 收到ping包");
                let clients = clients.lock();
                let client_data = clients.get(&id);
                if let Some(client_data) = client_data {
                    client_data.sender.unbounded_send(Message::Pong(v)).expect(CHAN_SEND_FAIL);
                }
            },
            Message::Pong(_) => {
                log::trace!("[WS:{id}] 收到pong包");
            },
            _ => {},
        }

        future::ok(())
    });

    let receive = rx.map(Ok).forward(ws_send);

    futures_util::pin_mut!(incoming, receive);
    future::select(incoming, receive).await;

    clients.lock().remove(&id);
    log::debug!("[WS:{id}] 结束websocket处理");

    Ok(())
}

// 记录请求头部的日志函数
fn log_request(req: &Request) {
    if log::log_enabled!(log::Level::Trace) {
        log::trace!("接受新的websocket客户端连接");
        log::trace!("请求路径: {}", req.uri().path());
        log::trace!("请求头部内容如下:");
        for (ref header, _value) in req.headers() {
            log::trace!("* {}: {:?}", header, _value);
        }
        log::trace!("===============请求头结束");
    }
}

// 进行redis订阅并进行处理
async fn subscribe(redis_client: Client, redis_pre: String, ws_clients: SharedClients) -> Result<()> {
    const SUB_FAIL: &str = "订阅消息失败";

    let auth_reply_topic_prefix = format!("{}:{}:{}:", redis_pre, REPLY_TOPIC, AUTH_TOPIC);
    let auth_reply_topic = format!("{}*", auth_reply_topic_prefix);
    let stat_topic = format!("{}:{}", redis_pre, STAT_TOPIC);
    let stat_reply_topic = format!("{}:{}:{}", redis_pre, REPLY_TOPIC, STAT_TOPIC);

    let mut pubsub_conn = redis_client.get_async_connection().await.context("切换redis异步方式失败")?.into_pubsub();
    pubsub_conn.subscribe(&redis_pre).await.context(SUB_FAIL)?;
    pubsub_conn.subscribe(&stat_topic).await.context(SUB_FAIL)?;
    pubsub_conn.psubscribe(&auth_reply_topic).await.context(SUB_FAIL)?;
    log::info!("[redis] 订阅redis消息: {redis_pre}, {stat_topic}, {auth_reply_topic}");

    let mut pubsub_stream = pubsub_conn.on_message();
    loop {
        let msg = pubsub_stream.next().await.context("获取订阅消息失败")?;
        let addr = msg.get_channel_name();
        let msg: String = msg.get_payload().context("获取订阅消息内容失败")?;
        log::debug!("[redis] 收到{addr}的订阅消息: {msg}");

        // 鉴权回复消息
        if addr.starts_with(&auth_reply_topic_prefix) {
            let id = &addr[auth_reply_topic_prefix.len() ..];
            let id = id.parse().context("解析鉴权响应消息的id失败")?;
            let mut ws_clients = ws_clients.lock();
            let client_data = ws_clients.get_mut(&id);
            if let Some(client_data) = client_data {
                let ar: ApiResult = serde_json::from_str(&msg)?;
                if ar.code == 200 { // 鉴权成功
                    client_data.login = true; // 修改登录状态
                    if let Some(ar_data) = ar.data {
                        let ar_data = serde_json::to_string(&ar_data)?;
                        client_data.sender.unbounded_send(Message::Text(ar_data))?;
                    }
                } else { // 鉴权失败
                    client_data.sender.unbounded_send(Message::Text(LOGOUT_MSG.to_owned()))?;
                    client_data.sender.unbounded_send(Message::Close(None))?;
                }
            }
        }
        // 统计消息
        else if addr == &stat_topic {
            let total = ws_clients.lock().len();
            let mut publish_conn = redis_client.get_async_connection().await?;
            let reply = serde_json::json!({
                "clientTotal": total,
            });
            let reply = serde_json::to_string(&reply)?;
            publish_conn.publish(&stat_reply_topic, &reply).await?;
        }
        // 需要向客户端广播的消息
        else if addr == &redis_pre {
            let mut total = 0;
            let msg = Message::Text(msg);
            for (_, v) in &*ws_clients.lock() {
                if v.login {
                    v.sender.unbounded_send(msg.clone()).context(CHAN_SEND_FAIL)?;
                    total += 1;
                }
            }
            log::debug!("[redis] 转发消息到客户端成功, 转发数量: {total}, 转发内容: {msg}");
        }
        else {
            log::warn!("不认识的消息，请检查代码");
        }
    }
}

// 启动redis客户端连接，并进行订阅
fn create_redis_subcribe(redis_client: Client, redis_pre: &str, ws_clients: SharedClients) {
    let redis_pre = redis_pre.to_owned();
    tokio::spawn(async move {
        // 连接客户端并进行订阅，断线重连
        while let Err(e) = subscribe(redis_client.clone(), redis_pre.clone(), ws_clients.clone()).await {
            log::error!("{e}");
            tokio::time::sleep(Duration::from_secs(REDIS_RETRY_INTERVAL)).await;
        }
    });
}

// 启动发送ping的定时任务
fn create_timer_ping(ws_clients: SharedClients) {
    let start = Instant::now() + Duration::from_secs(PING_INTERVAL);
    let interval = Duration::from_secs(PING_INTERVAL);
    let mut interval = tokio::time::interval_at(start, interval);
    tokio::spawn(async move {
        loop {
            if let Err(e) = on_timer(&mut interval, &ws_clients).await {
                log::error!("on_timer error: {e}");
            }
        }
    });

}

// 定时任务，向已连接的客户端发送ping包
async fn on_timer(interval: &mut Interval, ws_clients: &SharedClients) -> Result<()> {
    interval.tick().await;
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
    let mut ws_clients = ws_clients.lock();

    ws_clients.retain(|k, v| {
        if v.login || (!v.login && v.last_time + PING_INTERVAL >= now) {
            true
        } else {
            log::debug!("删除过期登录不成功的客户端: {}", k);
            v.sender.unbounded_send(Message::Close(None)).expect(CHAN_SEND_FAIL);
            false
        }
    });

    for (k, v) in &mut *ws_clients {
        if v.last_time + PING_EXPIRE <= now {
            v.last_time = now;
            v.sender.unbounded_send(Message::Ping(b"".to_vec())).context(CHAN_SEND_FAIL)?;
            log::trace!("[ping] 向客户端{k}发送ping消息");
        }
    }

    Ok(())
}
