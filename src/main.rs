mod mq;

// const APP_NAME: &str = "websocket_mq";   // 应用程序内部名称
const APP_VER: &str = "0.9.0";      // 应用程序版本

const BANNER: &str = r#"
 _       __     __     kivensoft      __ 0.9.0  __     __  _______
| |     / /__  / /_  _________  _____/ /_____  / /_   /  |/  / __ \
| | /| / / _ \/ __ \/ ___/ __ \/ ___/ //_/ _ \/ __/  / /|_/ / / / /
| |/ |/ /  __/ /_/ (__  ) /_/ / /__/ ,< /  __/ /_   / /  / / /_/ /
|__/|__/\___/_.___/____/\____/\___/_/|_|\___/\__/  /_/  /_/\___\_\
"#;

appconfig::appconfig_define!(AppConf,
    log_level: String => ["L",  "log-level", "LogLevel",       "log level(trace/debug/info/warn/error/off)"],
    log_file : String => ["F",  "log-file",  "LogFile",        "log filename"],
    log_max  : String => ["M",  "log-max",   "LogFileMaxSize", "log file max size(unit: k/m/g)"],
    listen   : String => ["l",  "listen",    "Listen",         "http service ip:port"],
    mq       : String => ["m",  "mq",        "MQ",             "redis server ip:port"],
    mq_pass  : String => ["p",  "mq-pass",   "MQPass",         "redis [user]:pass"],
    mq_pre   : String => ["r",  "mq-pre",    "MQPre",          "redis subscribe topic perfix"],
    help_dev : bool   => ["",   "help-dev",  "",               "View interface development help"],
);

impl Default for AppConf {
    fn default() -> AppConf {
        AppConf {
            log_level: String::from("info"),
            log_file : String::new(),
            log_max  : String::from("10m"),
            listen   : String::from("0.0.0.0:8080"),
            mq       : String::from("127.0.0.1:6379"),
            mq_pass  : String::new(),
            mq_pre   : String::new(),
            help_dev : false,
        }
    }
}

fn init() -> Option<()> {
    let version = format!("Message queuing service base on websocket version {APP_VER} CopyLeft Kivensoft 2021-2023.");
    let ac = AppConf::init();
    if !appconfig::parse_args(ac, &version).unwrap() {
        return None;
    }
    if ac.help_dev {
        show_dev_help();
        return None;
    }
    if ac.mq_pass.len() > 0 && !ac.mq_pass.contains(':') {
        println!("Argument mq-pass must have `:`");
        return None;
    }

    let log_level = asynclog::parse_level(&ac.log_level).unwrap();
    let log_max = asynclog::parse_size(&ac.log_max).unwrap();

    if log_level == log::Level::Trace {
        println!("config setting: {ac:#?}\n");
    }

    asynclog::init_log(log_level, ac.log_file.clone(), log_max, true, false).expect("init log failed");
    asynclog::set_level("mio".to_owned(), log::LevelFilter::Info);
    asynclog::set_level("tokio_tungstenite".to_owned(), log::LevelFilter::Info);
    asynclog::set_level("tungstenite".to_owned(), log::LevelFilter::Info);

    if ac.listen.len() > 0 && ac.listen.as_bytes()[0] == b':' {
        ac.listen.insert_str(0, "0.0.0.0");
    };

    appconfig::print_banner(BANNER, true);

    return Some(());
}

fn show_dev_help() {
    use ansicolor::{ac_blue, ac_green, ac_yellow, ac_magenta};

    let api = ac_green!("api");
    let psubscribe = ac_blue!("psubscribe");
    let api_auth_topic = ac_green!("api:authentication:*");
    let api_auth_req = ac_green!("api:authentication:<请求ID>");
    let api_auth_reply = ac_green!("api:reply:authentication:<请求ID>");
    let code = ac_yellow!("code");
    let data = ac_yellow!("data");
    let code_200 = ac_magenta!("200");
    let publish = ac_blue!("publish");
    let api_stat_req = ac_green!("api:statistics");
    let api_stat_reply = ac_green!("api:reply:statistics");

    println!(r#"
接口开发要求：
================================================================================

    1. 假设基地址为`{api}`

    2. 使用`{psubscribe}`模式订阅`{api_auth_topic}`频道，监听鉴权请求，鉴权请
       求的消息格式是websocket发送的请求url中的query格式，消息地址为
       `{api_auth_req}`，接口对参数进行校验，判断鉴权是否通过，向
       `{api_auth_reply}`发送请求处理结果，请求结果格式必须为
       json格式并带有`{code}`和`{data}`字段，{code} == {code_200}表示鉴权成功， {code} != {code_200}
       表示鉴权失败。请求成功后`{data}`字段的内容将返回给客户端。

    3. 通过使用`{publish}`往`{api}`频道发送消息，websocket客户端将实时收到该消息。

    4. 通过往`{api_stat_req}`发送消息，可在`{api_stat_reply}`频道收到统计信息。
"#);
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let None = init() { return; }
    let ac = AppConf::get();

    mq::start(&ac.listen, &ac.mq, &ac.mq_pass, &ac.mq_pre).await.unwrap();
}
