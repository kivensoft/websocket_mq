//! websocket

use std::{net::SocketAddr, sync::Arc, time::{Duration, SystemTime}};

use anyhow::{Context, Result};
use dashmap::DashMap;
use deadpool_redis::{Config, Runtime, redis, Pool};
use futures_util::{StreamExt, SinkExt};
use redis::{Client, AsyncCommands};
use serde::{Serialize, Deserialize};
use serde_json::{Value, Map};
use tokio::{net::{TcpListener, TcpStream}, time::Instant, sync::mpsc::UnboundedSender};
use tokio_tungstenite::{
    tungstenite::{{protocol::Message}, handshake::server::{Request, Response}},
    accept_hdr_async, WebSocketStream,
};
use urlencoding::decode;

use crate::AppConf;

// 应用订阅消息 {前缀}:*
// 应用发送鉴权请求 {前缀}:{AUTH_TOPIC}:{请求ID}
// 应用监听并处理登录回复 {前缀}:{REPLY_TOPIC}:{AUTH_TOPIC}:{请求ID}
// 三方发送统计请求 {前缀}:{STAT_TOPIC}
// 应用回复三方统计请求 {前缀}:{REPLY_TOPIC}:{STAT_TOPIC}

pub const AUTH_TOPIC: &str = "auth";                    // 鉴权订阅子地址
pub const STAT_TOPIC: &str = "stat";                    // 统计信息订阅子地址
pub const REPLY_TOPIC: &str = "reply";                  // 统计信息订阅回复地址
pub const LOGOUT_MSG: &str = r#"{"type":"logout"}"#;    // 鉴权失败后回复的消息
const REDIS_RETRY_INTERVAL: u64 = 5;                    // redis重新连接的间隔时间（单位：秒）
const PING_INTERVAL: u64 = 10;                          // 发送ping的定时任务时间间隔（单位：秒）
const CHAN_SEND_FAIL: &str = "channel send fail";

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ApiResult {
    pub code: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// websocket 客户端信息
struct ClientData {
    login    : bool,
    last_time: u64,
    sender   : UnboundedSender<Message>,
}

// 全局调用参数
struct WsClientParam {
    redis_pre   : String,
    redis_pool  : Pool,
    redis_client: Client,
    ws_clients  : DashMap<u32, ClientData>,
}

/// 启动websocket转发服务
pub async fn start() -> Result<()> {
    let ac = AppConf::get();
    let listener = {
        let socket_addr = ac.listen.parse::<SocketAddr>()?;
        TcpListener::bind(socket_addr).await
            .with_context(|| format!("failed to bind {}", ac.listen))?
    };
    log::info!("startup websocket service: {}", ac.listen);

    let ws_client_param = {
        let (pool, client) = init_pool(&ac.mq, &ac.mq_pass)?;
        Arc::new(WsClientParam {
            redis_pre: ac.mq_pre.clone(),
            redis_pool: pool,
            redis_client: client,
            ws_clients: DashMap::new(),
        })
    };

    // 启动redis客户端连接，并进行订阅
    create_redis_subcribe(ws_client_param.clone());

    // 启动发送ping的定时任务
    create_timer_ping(ws_client_param.clone(), ac.hb_time.parse().unwrap());

    let mut req_id = 0;
    // 循环，处理tcp监听事件
    while let Ok(stream_addr) = listener.accept().await {
        req_id += 1;
        let wcp = ws_client_param.clone();
        tokio::spawn(async move {
            if let Err(e) = on_websocket(req_id, stream_addr, wcp).await {
                log::error!("[WS:{req_id}] on_websocket error: {e:?}");
            }
        });
    }

    log::debug!("terminal websocket service");
    Ok(())
}

// websocket连接处理函数
async fn on_websocket(id: u32, stream_addr: (TcpStream, SocketAddr),
        wsc_param: Arc<WsClientParam>) -> Result<()> {

    log::debug!("[WS:{id}] accept {} tcp connected", stream_addr.1);

    // 升级http协议到websocket，并获取url中query的参数
    let (ws_stream, query) = websocket_accpet(stream_addr.0).await
            .context("websocket handshake failed")?;
    log::debug!("[WS:{id}] webSocket handshake successful");

    let (mut ws_send, mut ws_recv) = ws_stream.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // 客户端加入集合，新建立的客户端登录状态为false
    wsc_param.ws_clients.insert(id, ClientData{
        login: false,
        last_time: SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs(),
        sender: tx,
    });

    // 发送鉴权校验请求消息
    let auth_topic = format!("{}:{}:{}", wsc_param.redis_pre, AUTH_TOPIC, id);
    let auth_params = serde_json::to_string(&query)?;
    wsc_param.redis_pool.get().await?
            .publish(auth_topic, auth_params).await?;

    loop {
        tokio::select! {
            // websocket 收到消息处理过程
            msg = ws_recv.next() => match msg {
                Some(msg) => match msg {
                    Ok(msg) => match msg {
                        Message::Text(msg) => {
                            log::debug!("[WS:{id}] text message: {msg}");
                        },

                        Message::Close(close_frame) => {
                            log::debug!("[WS:{id}] close message: {close_frame:?}");
                            break;
                        },

                        Message::Ping(v) => {
                            log_ping_pong(id, "ping", &v);
                            ws_send.send(Message::Pong(v)).await?;
                        },

                        Message::Pong(v) => {
                            log_ping_pong(id, "pong", &v);
                        },

                        _ => log::trace!("[WS:{id}] unprocessed message type"),
                    },
                    Err(e) => {
                        log::error!("[WS:{id}] receive message error: {e:?}");
                        break;
                    },
                },
                None => break,
            },

            // 通道收到消息，转发给websocket
            msg = rx.recv() => match msg {
                Some(msg @ Message::Close(_)) => {
                    ws_send.send(msg).await?;
                    break;
                },
                Some(msg) => ws_send.send(msg).await?,
                None => break,
            },
        }
    }

    wsc_param.ws_clients.remove(&id);
    log::debug!("[WS:{id}] terminate websocket client connect");

    Ok(())
}

// 进行redis订阅并进行处理
async fn on_redis_message(wsc_param: Arc<WsClientParam>) -> Result<()> {

    let auth_reply_topic_prefix = format!("{}:{}:{}:", wsc_param.redis_pre, REPLY_TOPIC, AUTH_TOPIC);
    let auth_reply_topic = format!("{}*", auth_reply_topic_prefix);
    let stat_topic = format!("{}:{}", wsc_param.redis_pre, STAT_TOPIC);
    let stat_reply_topic = format!("{}:{}:{}", wsc_param.redis_pre, REPLY_TOPIC, STAT_TOPIC);

    // 订阅redis消息
    let mut pubsub = wsc_param.redis_client
            .get_async_connection().await
            .context("get redis connect fail")?
            .into_pubsub();
    let subs = [&wsc_param.redis_pre, &stat_topic];
    let psubs = [&auth_reply_topic];
    pubsub.subscribe(&subs).await.context("redis subscribe fail")?;
    pubsub.psubscribe(&psubs).await.context("redis psubscribe fail")?;
    log::info!("redis subscribe: {subs:?}");
    log::info!("redis psubscribe: {psubs:?}");

    // 进入循环模式，监听并处理redis消息
    let mut pubsub_stream = pubsub.into_on_message();
    loop {
        let msg = match pubsub_stream.next().await {
            Some(msg) => msg,
            None => continue,
        };
        let chan = msg.get_channel_name();
        let payload = match std::str::from_utf8(msg.get_payload_bytes()) {
            Ok(v) => v,
            Err(e) => {
                log::error!("parse redis message to utf8 fail: {e:?}");
                continue
            },
        };
        log::debug!("redis receive on [{chan}]: {payload}");

        // 鉴权回复消息，向websocket客户端回复登录结果
        if chan.starts_with(&auth_reply_topic_prefix) {
            // 解析消息地址中的id，该id关联实际的websocket客户端
            let id = &chan[auth_reply_topic_prefix.len() ..];
            let id = id.parse().context("parse auth message id fail")?;
            let client_data = wsc_param.ws_clients.get_mut(&id);

            if let Some(mut client_data) = client_data {
                let cd = client_data.value_mut();
                let ar: ApiResult = serde_json::from_str(payload)?;

                if ar.code == 200 { // 鉴权成功
                    cd.login = true; // 修改登录状态

                    if let Some(ar_data) = ar.data {
                        let ar_data = serde_json::to_string(&ar_data)?;
                        cd.sender.send(Message::Text(ar_data))?;
                    }
                } else { // 鉴权失败, 关闭客户端
                    cd.sender.send(Message::Text(LOGOUT_MSG.to_owned()))?;
                    cd.sender.send(Message::Close(None))?;
                }
            }
        }
        // 收到统计消息，向redis回复统计结果
        else if chan == &stat_topic {
            let total = wsc_param.ws_clients.len();
            let mut conn = wsc_param.redis_pool.get().await?;
            let reply = serde_json::json!({
                "clientTotal": total,
            });
            let reply = serde_json::to_string(&reply)?;
            conn.publish(&stat_reply_topic, &reply).await?;
        }
        // 需要向websocket客户端广播的消息
        else if chan == &wsc_param.redis_pre {
            let mut total = 0;
            let msg = Message::Text(payload.to_owned());
            for item in wsc_param.ws_clients.iter() {
                let val = item.value();
                if val.login {
                    val.sender.send(msg.clone()).context(CHAN_SEND_FAIL)?;
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
fn create_redis_subcribe(wsc_param: Arc<WsClientParam>) {
    tokio::spawn(async move {
        let interval = Duration::from_secs(REDIS_RETRY_INTERVAL);
        // 连接客户端并进行订阅，断线重连
        while let Err(e) = on_redis_message(wsc_param.clone()).await {
            log::error!("redis on message error: {e:?}");
            tokio::time::sleep(interval).await;
        }
    });
}

// 启动发送ping的定时任务
fn create_timer_ping(wsc_param: Arc<WsClientParam>, ping_expire: u64) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = on_timer(wsc_param.clone(), ping_expire).await {
                log::error!("on_timer error: {e}");
            }
        }
    });
}

// 定时任务，向已连接的客户端发送ping包
async fn on_timer(wsc_param: Arc<WsClientParam>, ping_expire: u64) -> Result<()> {
    static INTERVAL: Duration = Duration::from_secs(PING_INTERVAL);

    let start = Instant::now() + INTERVAL;
    let mut delay = tokio::time::interval_at(start, INTERVAL);
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    delay.tick().await;

    wsc_param.ws_clients.retain(|k, v| {
        // 已经登录或者未登录且过期，不删除
        if v.login || (!v.login && v.last_time + PING_INTERVAL >= now) {
            true
        } else {
            log::debug!("delete expired but not logged in clients: {}", k);
            v.sender.send(Message::Close(None)).expect(CHAN_SEND_FAIL);
            false
        }
    });

    for mut entry in wsc_param.ws_clients.iter_mut() {
        let v = entry.value_mut();
        if v.last_time + ping_expire <= now {
            v.last_time = now;
            v.sender.send(Message::Ping(Vec::with_capacity(0)))?;
            log::trace!("[ping:{}] sending ping messages to clients", entry.key());
        }
    }

    Ok(())
}

// websocket升级连接
async fn websocket_accpet(stream: TcpStream) -> Result<(WebSocketStream<TcpStream>, Value)> {
    let mut query = String::with_capacity(0);
    let ws_stream = accept_hdr_async(stream, |req: &Request, res: Response| {
        if let Some(q) = req.uri().query() {
            query = q.to_owned();
        }

        if log::log_enabled!(log::Level::Trace) {
            log::trace!("accept websocket client connect");
            log::trace!("url: {}", req.uri());
            log::trace!("header:");
            for (ref header, _value) in req.headers() {
                log::trace!("* {}: {:?}", header, _value);
            }
            log::trace!("===============请求头结束");
        }

        Ok(res)
    }).await?;

    let params = parse_url_query(&query).context("parse url query error")?;

    Ok((ws_stream, params))
}

// 解析url中的参数
fn parse_url_query(query: &str) -> Result<Value> {
    if query.is_empty() {
        return Ok(Value::Null);
    }

    let mut params = Map::new();

    for item in query.split('&') {
        let mut s = item.splitn(2, '=');
        let k = decode(s.next().unwrap_or(""))?;
        let v = decode(s.next().unwrap_or(""))?;
        if !k.is_empty() {
            params.insert(String::from(k), Value::String(String::from(v)));
        }
    }

    Ok(Value::Object(params))
}

// redis连接测试
fn try_connect(url: &str) -> Result<Client> {
    let client = redis::Client::open(url)?;
    let mut conn = client.get_connection()?;
    let val: String = redis::cmd("PING").arg(crate::APP_NAME).query(&mut conn)?;
    if val != crate::APP_NAME {
        anyhow::bail!(format!("ping redis error: {url}"));
    }

    Ok(client)
}

// 初始化redis连接池
fn init_pool(host: &str, pass: &str) -> Result<(Pool, Client)> {
    let redis_url = format!("redis://{}@{}", pass, host);

    // 测试redis连接配置的正确性
    let client = try_connect(&redis_url).context("test connect redis error")?;

    let cfg = Config::from_url(redis_url.clone());
    let pool = cfg.create_pool(Some(Runtime::Tokio1)).context("create redis pool fail")?;

    log::info!("connect redis server: {}", redis_url);

    Ok((pool, client))
}

fn log_ping_pong(id: u32, act: &str, text: &[u8]) {
    if let Ok(text) = std::str::from_utf8(&text) {
        if text.is_empty() {
            log::trace!("[WS:{id}] receive {act} message");
        } else {
            log::trace!("[WS:{id}] receive {act} message: {text}");
        }
    }
}
