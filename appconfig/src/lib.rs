pub use getopts::{Options, Matches};
pub use anyhow;


#[cfg(feature="cfg-file")]
mod config;
#[cfg(feature="cfg-file")]
pub use config::Config;
#[cfg(not(feature="cfg-file"))]
pub struct Config;

#[cfg(not(feature="cfg-file"))]
impl Config {
    pub fn get_str(&self, _: &str) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
}

/// Application Parameter Definition Macro.
///
/// ## Examples
///
/// ```rust
/// use appconfig;
///
/// appconfig::appconfig_define!(AppConf,
///     log_level: String => ["L",  "log-level", "LogLevel", "log level(trace/debug/info/warn/error/off)"],
///     log_file : String => ["F",  "log-file", "LogFile", "log filename"],
///     log_max  : String => ["M",  "log-max", "LogFileMaxSize", "log file max size(unit: k/m/g)"],
///     listen   : String => ["l",  "", "Listen", "http service ip:port"],
///     debug    : bool   => ["",  "debug", "", "debug mode"],
/// );
///
/// impl Default for AppConf {
///     fn default() -> Self {
///         AppConf {
///             log_level: String::from("info"),
///             log_file : String::new(),
///             log_max  : String::from("10m"),
///             listen   : String::from("0.0.0.0:8080"),
///             debug    : false,
///         }
///     }
/// }
///
/// let mut ac = AppConf::default();
/// if !appconfig::parse_args(&mut ac, "example application").unwrap();
///     return;
/// }
/// ```
#[macro_export]
macro_rules! appconfig_define {
    // set_opt_flag
    (@set_opt_flag $opts:expr, $short_opt:literal, $long_opt:literal, $opt_name:literal, $desc:literal, $val:expr, bool) => {
        let s = format!("{} (\x1b[34mdefault: \x1b[32m{}\x1b[0m)", $desc, $val);
        $opts.optflag($short_opt, $long_opt, &s)
    };
    (@set_opt_flag $opts:expr, $short_opt:literal, $long_opt:literal, $opt_name:literal, $desc:literal, $val:expr, $_:ty) => {
        let s = match $val.len() {
            0 => String::from($desc),
            _ => format!("{} (\x1b[34mdefault: \x1b[32m{}\x1b[0m)", $desc, $val),
        };
        $opts.optopt($short_opt, $long_opt, &s, $opt_name)
    };

    // get_opt_value
    (@get_opt_value $matches:expr, "help", $out_val:expr, $t:ty) => {};
    (@get_opt_value $matches:expr, "conf-file", $out_val:expr, $t:ty) => {};
    (@get_opt_value $matches:expr, $name:expr, $out_val:expr, String) => {
        if let Some(s) = $matches.opt_str($name) {
            $out_val = s;
        }
    };
    (@get_opt_value $matches:expr, $name:expr, $out_val:expr, bool) => {
        if $matches.opt_present($name) {
            $out_val = true;
        }
    };
    (@get_opt_value $matches:expr, $name:expr, $out_val:expr, $t:ty) => {
        if let Some(s) = $matches.opt_str($name) {
            $out_val = anyhow::Context::with_context(s.parse::<$t>(),
                || format!("program argument {} is not a numbe", $name))?;
        }
    };

    // get_cfg_value
    (@get_cfg_value $cfg: expr, "conf-file", $out_val: expr, $t:ty) => {};
    (@get_cfg_value $cfg: expr, $name: expr, $out_val: expr, String) => {
        if let Ok(s) = $cfg.get_str($name) {
            if let Some(s) = s {
                $out_val = s;
            }
        }
    };
    (@get_cfg_value $cfg: expr, $name: expr, $out_val: expr, bool) => {
        if let Ok(s) = $cfg.get_str($name) {
            if let Some(s) = s {
                $out_val = s.to_lowercase() == "true";
            }
        }
    };
    (@get_cfg_value $cfg: expr, $name: expr, $out_val: expr, $t:ty) => {
        if let Ok(s) = $cfg.get_str($name) {
            if let Some(s) = s {
                $out_val = s.parse::<$t>().with_context(
                    || format!("app config file key {} is not a number", $name))?;
            }
        }
    };

    ( $struct_name:ident, $($field:ident : $type:tt =>
            [$short_opt:literal, $long_opt:tt, $opt_name:literal, $desc:literal]$(,)?)+ ) => {

        #[derive(Debug)]
        pub struct $struct_name {
            $( pub $field: $type,)*
        }

        impl $crate::AppConfig for $struct_name {
            fn to_opts(&self) -> $crate::Options {
                let mut opts = $crate::Options::new();
                $( $crate::appconfig_define!(@set_opt_flag opts, $short_opt, $long_opt, $opt_name, $desc, self.$field, $type); )*
                opts
            }

            fn set_from_getopts(&mut self, matches: &$crate::Matches) -> $crate::anyhow::Result<()> {
                $( $crate::appconfig_define!(@get_opt_value matches, $long_opt, self.$field, $type); )*
                Ok(())
            }

            fn set_from_cfg(&mut self, cfg: &$crate::Config) -> $crate::anyhow::Result<()> {
                $( $crate::appconfig_define!(@get_cfg_value cfg, $long_opt, self.$field, $type); )*
                Ok(())
            }
        }

        impl $struct_name {
            #[cfg(debug_assertions)]
            fn init() -> &'static mut Self {
                unsafe {
                    static mut FLAG: bool = false;
                    if FLAG {
                        panic!("The global variable has already been initialized, and reinitialization is not allowed");
                    }
                    FLAG = true;
                    __APP_CONFIG.write(Self::default())
                }
            }

            #[cfg(not(debug_assertions))]
            fn init() -> &'static mut Self {
                unsafe { __APP_CONFIG.write(Self::default()) }
            }

            fn get() -> &'static Self {
                unsafe { &*__APP_CONFIG.as_ptr() }
            }
        }

        pub static mut __APP_CONFIG: std::mem::MaybeUninit<$struct_name> = std::mem::MaybeUninit::uninit();

    };
}

const C_HELP: &str = "help";
#[cfg(feature="cfg-file")]
const C_CONF_FILE: &str = "conf-file";

pub trait AppConfig {
    fn to_opts(&self) -> getopts::Options;
    fn set_from_getopts(&mut self, matches: &getopts::Matches) -> anyhow::Result<()>;
    fn set_from_cfg(&mut self, cfg: &Config) -> anyhow::Result<()>;
}

pub fn print_banner(banner: &str, use_color: bool) {
    if banner.is_empty() { return; }
    if !use_color { return println!("{}", banner); }

    let mut rng = rand::thread_rng();
    let mut text = Vec::with_capacity(512);
    let mut dyn_color: [u8; 5] = [b'\x1b', b'[', b'3', b'0', b'm'];
    let (mut n1, mut n2) = (0, 0);

    for line in banner.lines() {
        loop {
            let i = rand::Rng::gen_range(&mut rng, 1..8);
            if n1 != i && n2 != i { n1 = n2; n2 = i; break }
        }
        dyn_color[3] = b'0' + n2;
        text.extend_from_slice(&dyn_color);
        text.extend_from_slice(line.as_bytes());
        text.push(b'\n');
    }

    text.extend_from_slice(b"\x1b[0m\n");
    print!("{}", unsafe {std::str::from_utf8_unchecked(&text)});
}

/// Parsing configuration from command line parameters and configuration files
/// and populate it with the variable `ac`
///
/// If the return value is Ok(false), it indicates that the program needs to be terminated immediately.
///
/// Arguments:
///
/// * `app_config`: Output variable, which will be filled after parameter parsing
/// * `banner`: application banner
///
/// Returns:
///
/// Ok(true): success, Ok(false): require terminated, Err(e): error
///
#[inline]
pub fn parse_args<T: AppConfig>(app_config: &mut T, banner: &str) -> anyhow::Result<bool> {
    parse_args_ext(app_config, banner, |_| true)
}


/// Parsing configuration from command line parameters and configuration files
/// and populate it with the variable `ac`
///
/// If the return value is false, it indicates that the program needs to be terminated immediately.
///
/// * `app_config`: application config variable
/// * `banner`: application banner
/// * `f`: A user-defined callback function that checks the validity of parameters.
/// If it returns false, this function will print the help information and return Ok(false)
///
/// Returns:
///
/// Ok(true): success, Ok(false): require terminated, Err(e): error
///
pub fn parse_args_ext<T: AppConfig, F: Fn(&T) -> bool>(app_config: &mut T, version: &str, f: F) -> anyhow::Result<bool> {

    let mut args = std::env::args();
    let prog = args.next().unwrap();

    let mut opts = app_config.to_opts();
    opts.optflag("h", C_HELP, "this help");
    #[cfg(feature="cfg-file")]
    opts.optopt("c",  C_CONF_FILE, "set configuration file", "ConfigFile");

    let matches = match anyhow::Context::context(opts.parse(args), "parse program arguments failed") {
        Ok(m) => m,
        Err(e) => {
            print_usage(&prog, version, &opts);
            return Err(e);
        },
    };

    if matches.opt_present(C_HELP) {
        print_usage(&prog, version, &opts);
        return Ok(false);
    }

    // 参数设置优先级：命令行参数 > 配置文件参数
    // 因此, 先从配置文件读取参数覆盖缺省值, 然后用命令行参数覆盖
    // 从配置文件读取参数, 如果环境变量及命令行未提供配置文件参数, 则允许读取失败, 否则, 读取失败返回错误
    #[cfg(feature="cfg-file")]
    get_from_config_file(app_config, &matches, &prog)?;

    // 从命令行读取参数
    app_config.set_from_getopts(&matches)?;

    if !f(app_config) {
        print_usage(&prog, version, &opts);
        return Ok(false);
    }

    // print_banner(banner, true);

    Ok(true)
}

fn print_usage(prog: &str, version: &str, opts: &getopts::Options) {
    if version.len() > 0 {
        println!("\n{}", version);
    }
    let path = std::path::Path::new(prog);
    let prog = path.file_name().unwrap().to_str().unwrap();
    let brief = format!("\nUsage: \x1b[36m{} \x1b[33m{}\x1b[0m", &prog, "[options]");
    println!("{}", opts.usage(&brief));
}

#[cfg(feature="cfg-file")]
fn get_from_config_file<T: AppConfig>(ac: &mut T, matches: &Matches, prog: &str) -> anyhow::Result<()> {
    let mut conf_is_set = false;
    let mut conf_file = String::new();
    if let Some(cf) = matches.opt_str(C_CONF_FILE) {
        conf_is_set = true;
        conf_file = cf;
    }
    if !conf_is_set {
        let mut path = std::path::PathBuf::from(prog);
        path.set_extension("conf");
        conf_file = path.to_str().ok_or(anyhow::anyhow!("program name error"))?.to_owned();
    }
    match Config::with_file(&conf_file) {
        Ok(cfg) => ac.set_from_cfg(&cfg),
        Err(_) => {
            match conf_is_set {
                true => anyhow::bail!("can't read app config file {conf_file}"),
                false => Ok(())
            }
        },
    }
}
