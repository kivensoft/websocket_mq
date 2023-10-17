use anyhow::Context;

mod mq;

const APP_NAME: &str = "websocket_mq";   // 应用程序内部名称
const APP_VER: &str = include_str!(concat!(env!("OUT_DIR"), "/.version"));

const BANNER: &str = r#"
 _       __     __  kivensoft %       __        __     __  _______
| |     / /__  / /_  _________  _____/ /_____  / /_   /  |/  / __ \
| | /| / / _ \/ __ \/ ___/ __ \/ ___/ //_/ _ \/ __/  / /|_/ / / / /
| |/ |/ /  __/ /_/ (__  ) /_/ / /__/ ,< /  __/ /_   / /  / / /_/ /
|__/|__/\___/_.___/____/\____/\___/_/|_|\___/\__/  /_/  /_/\___\_\
"#;

macro_rules! arg_err {
    ($text:literal) => {
        concat!("arg ", $text, " format error")
    }
}

appconfig::appconfig_define!(app_conf, AppConf,
    log_level : String => ["L",  "log-level",  "LogLevel",       "log level(trace/debug/info/warn/error/off)"],
    log_file  : String => ["F",  "log-file",   "LogFile",        "log filename"],
    log_max   : String => ["M",  "log-max",    "LogFileMaxSize", "log file max size(unit: k/m/g)"],
    log_async : bool   => ["",   "log-async",  "",               "enable asynchronous logging"],
    no_console: bool   => ["",   "no-console", "",               "prohibit outputting logs to the console"],
    listen    : String => ["l",  "listen",     "Listen",         "http service ip:port"],
    hb_time   : String => ["b",  "hb_time",    "HbTime",         "websocket heartbreak time(unit: second)"],
    mq        : String => ["m",  "mq",         "MQ",             "redis server ip:port"],
    mq_pass   : String => ["p",  "mq-pass",    "MQPass",         "redis [user]:pass"],
    mq_pre    : String => ["r",  "mq-pre",     "MQPre",          "redis subscribe topic perfix"],
    help_dev  : bool   => ["",   "help-dev",   "",               "View interface development help"],
);

impl Default for AppConf {
    fn default() -> AppConf {
        AppConf {
            log_level : String::from("info"),
            log_file  : String::with_capacity(0),
            log_max   : String::from("10m"),
            log_async : false,
            no_console: false,
            listen    : String::from("127.0.0.1:6402"),
            hb_time   : String::from("40"),
            mq        : String::from("127.0.0.1:6379"),
            mq_pass   : String::with_capacity(0),
            mq_pre    : String::with_capacity(0),
            help_dev  : false,
        }
    }
}

fn init() -> Option<&'static mut AppConf> {
    let version = format!(
            "{}(Message queuing over websocket) version {} CopyLeft Kivensoft 2021-2023.",
            APP_NAME, APP_VER);

    let ac = AppConf::init();
    if !appconfig::parse_args(ac, &version).expect("parse args error") {
        return None;
    }

    if ac.help_dev {
        show_dev_help();
        return None;
    }

    if !ac.mq_pass.is_empty() && !ac.mq_pass.contains(':') {
        println!("Argument mq-pass must have `:`");
        return None;
    }

    ac.hb_time.parse::<u64>().expect(arg_err!("hb_time"));

    let log_level = asynclog::parse_level(&ac.log_level).expect(arg_err!("log-level"));
    let log_max = asynclog::parse_size(&ac.log_max).expect(arg_err!("log-max"));

    if log_level == log::Level::Trace {
        println!("config setting: {ac:#?}\n");
    }

    asynclog::init_log(log_level, ac.log_file.clone(), log_max,
            !ac.no_console, ac.log_async).expect("init log failed");
    asynclog::set_level("mio".to_owned(), log::LevelFilter::Info);
    asynclog::set_level("tokio_tungstenite".to_owned(), log::LevelFilter::Info);
    asynclog::set_level("tungstenite".to_owned(), log::LevelFilter::Info);

    if ac.listen.len() > 0 && ac.listen.as_bytes()[0] == b':' {
        ac.listen.insert_str(0, "0.0.0.0");
    };

    // 在控制台输出logo
    if let Some((s1, s2)) = BANNER.split_once('%') {
        let s2 = &s2[APP_VER.len() - 1..];
        let banner = format!("{s1}{APP_VER}{s2}");
        appconfig::print_banner(&banner, true);
    }

    return Some(ac);
}

fn show_dev_help() {
    use ansicolor::{ac_blue, ac_green, ac_yellow, ac_magenta};
    const APP: &str = "app:ws";

    let app = ac_green!(APP).to_string();
    let api_topic = ac_green!("{}:*", APP).to_string();
    let api_auth_topic = ac_green!("{}:{}:*", APP, mq::AUTH_TOPIC).to_string();
    let api_auth_reply_topic = ac_green!("{}:{}:{}:*", APP, mq::REPLY_TOPIC, mq::AUTH_TOPIC).to_string();
    let api_auth_req = ac_green!("{}:{}:<请求ID>", APP, mq::AUTH_TOPIC).to_string();
    let api_auth_reply = ac_green!("{}:{}:{}:<请求ID>", APP, mq::REPLY_TOPIC, mq::AUTH_TOPIC).to_string();
    let code = ac_yellow!("code");
    let data = ac_yellow!("data");
    let code_200 = ac_magenta!("200");
    let psubscribe = ac_blue!("psubscribe");
    let publish = ac_blue!("publish");
    let api_stat_req = ac_green!("{}:{}", APP, mq::STAT_TOPIC).to_string();
    let api_stat_reply = ac_green!("{}:{}:{}", APP, mq::REPLY_TOPIC, mq::STAT_TOPIC).to_string();

    println!(r#"
接口开发要求：
================================================================================

    1. 假设基地址为`{app}`，则：
        鉴权服务监听地址：`{api_auth_topic}`
        鉴权服务回复地址：`{api_auth_reply_topic}`
        发送鉴权请求：`{api_auth_req}`
        回复鉴权结果：`{api_auth_reply}`

        推送消息监听地址：`{api_topic}`

        统计服务监听地址：`{api_stat_req}`
        统计服务回复地址：`{api_stat_reply}`

    2. 使用`{psubscribe}`命令订阅`{api_auth_topic}`频道，监听鉴权请求，鉴权请
       求的消息格式是websocket发送的请求url中的query格式，消息地址为
       `{api_auth_req}`，接口对参数进行校验，判断鉴权是否通过，向
       `{api_auth_reply}`发送请求处理结果，请求结果格式必须为
       json格式并带有`{code}`和`{data}`字段，{code} == {code_200}表示鉴权成功， {code} != {code_200}
       表示鉴权失败。请求成功后`{data}`字段的内容将返回给客户端。

    3. 通过使用`{publish}`往`{app}`频道发送消息，websocket客户端将实时收到该消息。

    4. 通过往`{api_stat_req}`发送消息，可在`{api_stat_reply}`频道收到统计信息。
"#);
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if init().is_none() {
        return;
    };

    mq::start().await.context("startup server fail").unwrap();
}
