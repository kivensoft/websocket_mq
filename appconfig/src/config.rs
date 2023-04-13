use std::borrow::Cow;
use std::fmt::Display;
use std::fs;
use std::path::Path;
use std::str::FromStr;

macro_rules! skip_chars {
    (@sub $c:expr, $c1:expr) => { if $c != $c1 { break; } };
    (@sub $c:expr, $c1:expr, $c2:expr) => { if $c != $c1 && $c != $c2 { break; } };
    (@sub $c:expr, $c1:expr, $c2:expr, $c3:expr) => { if $c != $c1 && $c != $c2 && $c != $c3 { break; } };
    (@sub $c:expr, $c1:expr, $c2:expr, $c3:expr, $c4:expr) => { if $c != $c1 && $c != $c2 && $c != $c3 && $c != $c4 { break; } };

    ($val: expr, $pos: expr, $max: expr, $($t: tt)*) => {
        {
            let mut c = 0;
            while $pos < $max {
                c = $val[$pos];
                skip_chars!(@sub c, $($t)*);
                $pos += 1;
            };
            if $pos >= $max { break; } else { c }
        }
    };
}

macro_rules! util_chars {
    (@sub $c:expr, $c1:expr) => { if $c == $c1 { break; } };
    (@sub $c:expr, $c1:expr, $c2:expr) => { if $c == $c1 || $c == $c2 { break; } };
    (@sub $c:expr, $c1:expr, $c2:expr, $c3:expr) => { if $c == $c1 || $c == $c2 || $c == $c3 { break; } };
    (@sub $c:expr, $c1:expr, $c2:expr, $c3:expr, $c4:expr) => { if $c == $c1 || $c == $c2 || $c == $c3 || $c == $c4 { break; } };
    (@sub $c:expr, $c1:expr, $c2:expr, $c3:expr, $c4:expr, $c5:expr) => { if $c == $c1 || $c == $c2 || $c == $c3 || $c == $c4 || $c == $c5 { break; } };

    ($val: expr, $pos: expr, $max: expr, $($t: tt)*) => {
        {
            let mut c = 0;
            while $pos < $max {
                c = $val[$pos];
                util_chars!(@sub c, $($t)*);
                $pos += 1;
            }
            if $pos >= $max { break; } else { c }
        }
    };
}

struct ConfigItem {
    key_begin: usize,
    key_end: usize,
    val_begin: usize,
    val_end: usize,
}

impl ConfigItem {
    fn new() -> Self { Self {key_begin: 0, key_end: 0, val_begin: 0, val_end: 0} }
}

/// Config Struct
pub struct Config {
    data: Vec<u8>,
    key_values: Vec<ConfigItem>,
}

/// Config Implementation
impl Config {
    pub fn with_file<T: AsRef<Path>>(file: T) -> anyhow::Result<Self> {
        let data = fs::read(file)?;
        std::str::from_utf8(&data)?;
        let kv = Self::parse(&data)?;
        Ok(Self {data, key_values: kv})
    }

    pub fn with_text(text: String) -> anyhow::Result<Self> {
        let data = text.into_bytes();
        let kv = Self::parse(&data)?;
        Ok(Self {data, key_values: kv})
    }

    pub fn with_data(data: Vec<u8>) -> anyhow::Result<Self> {
        let kv = Self::parse(&data)?;
        std::str::from_utf8(&data)?;
        Ok(Self {data, key_values: kv})
    }

    /// Get a value from config as ayn type (That Impls str::FromStr)
    pub fn get<T>(&self, key: &str) -> anyhow::Result<Option<T>>
            where T: FromStr, T::Err: Display {
        match self.get_raw(key) {
            Some(s) => {
                match Self::decode(s)?.parse::<T>() {
                    Ok(v) => Ok(Some(v)),
                    Err(e) => return Err(anyhow::anyhow!("can't parse value error: {e}")),
                }
            },
            None => Ok(None),
        }
    }

    /// Get a value from config as a String
    pub fn get_str(&self, key: &str) -> anyhow::Result<Option<String>> {
        match self.get_raw(key) {
            Some(s) => Ok(Some(Self::decode(s)?.into_owned())),
            None => Ok(None),
        }
    }

    /// Get a value as original data (not escape)
    pub fn get_raw<'a>(&'a self, key: &str) -> Option<&'a [u8]> {
        let key = key.as_bytes();
        for kv in self.key_values.iter() {
            if key == &self.data[kv.key_begin..kv.key_end] {
                return Some(&self.data[kv.val_begin .. kv.val_end]);
            }
        }
        None
    }

    // decode value
    fn decode<'a>(val: &'a [u8]) -> anyhow::Result<Cow<'a, str>> {
        // 删除尾部空白
        let val = Self::trim_whitespace(val);

        // 有转义字符，生成新的转义后的字符串，没有转义字符则返回原串
        if val.contains(&b'\\') {
            let mut v = Vec::with_capacity(val.len() + 32);
            let (mut i, imax) = (0, val.len());
            while i < imax {
                match val[i] {
                    // 转义字符，需要对下一个字符进行判断和处理
                    b'\\' => {
                        i += 1;
                        if i < imax {
                            let c = val[i];
                            // 行尾的'\'，是连接符，跳过下一行的回车换行空白符并继续处理
                            if c == b'\r' || c == b'\n' {
                                skip_chars!(val, i, imax, b'\r', b'\n');
                                skip_chars!(val, i, imax, b' ', b'\t');
                                i -= 1;
                            } else {
                                v.push(Self::escape(c)?);
                            }
                        }
                    },
                    c => v.push(c),
                }
                i += 1;
            }
            // parse之前已经做过utf8有效性检测了，因此这里无需再做1次
            Ok(Cow::Owned(unsafe {String::from_utf8_unchecked(v)}))
        } else {
            // parse之前已经做过utf8有效性检测了，因此这里无需再做1次
            Ok(Cow::Borrowed(unsafe {std::str::from_utf8_unchecked(val)}))
        }
    }

    // 删除尾部的空白字符
    fn trim_whitespace(val: &[u8]) -> &[u8] {
        let mut pos = val.len();
        while pos > 0 {
            let c = val[pos - 1];
            if c != b' ' && c != b'\t' && c != b'\r' && c != b'\n' {
                return &val[..pos];
            }
            pos -= 1;
        }
        val
    }

    // 解析转义字符
    fn escape(v: u8) -> anyhow::Result<u8> {
        let c = match v {
            b't' => b'\t',
            b'r' => b'\r',
            b'n' => b'\n',
            b's' => b' ',
            b'\\' => b'\\',
            _ => anyhow::bail!("The escape character format is not supported \\{v}"),
        };
        Ok(c)
    }

    /// Parse a string into the config
    fn parse(data: &[u8]) -> anyhow::Result<Vec<ConfigItem>> {
        enum ParseStatus { KeyBegin, Comment, Key, Equal, ValBegin, Val, ValContinue }

        let mut result = Vec::with_capacity(64);
        let mut pstate = ParseStatus::KeyBegin;
        let mut curr = ConfigItem::new();
        let (mut i, imax, mut line_no) = (0, data.len(), 1);

        macro_rules! push_str {
            ($vec: expr, $item: expr, $pos: expr, $state: expr, $next_state: expr) => {
                {
                    $state = $next_state;
                    $item.val_end = $pos;
                    $vec.push($item);
                    $item = ConfigItem::new();
                }
            };
        }

        while i < imax {
            let c = data[i];
            if c == b'\n' { line_no += 1 };

            match pstate {
                ParseStatus::KeyBegin => {
                    match skip_chars!(data, i, imax, b' ', b'\t', b'\r', b'\n') {
                        b'#' => pstate = ParseStatus::Comment,
                        b'=' => anyhow::bail!("Not allow start with '=' at line {line_no}"),
                        _ => {
                            curr.key_begin = i;
                            pstate = ParseStatus::Key
                        },
                    }
                    // match c {
                    //     b'#' => pstate = ParseStatus::Comment,
                    //     b'=' => anyhow::bail!("Not allow start with '=' at line {line_no}"),
                    //     b' ' | b'\t' | b'\r' | b'\n' => {},
                    //     _ => {
                    //         pstate = ParseStatus::Key;
                    //         curr.key_begin = i;
                    //     }
                    // }
                },
                ParseStatus::Comment => {
                    util_chars!(data, i, imax, b'\r', b'\n');
                    pstate = ParseStatus::KeyBegin;
                    // match c {
                    //     b'\r' | b'\n' => pstate = ParseStatus::KeyBegin,
                    //     _ => {},
                    // }
                },
                ParseStatus::Key => {
                    let c = util_chars!(data, i, imax, b' ', b'\t', b'=', b'\r', b'\n');
                    curr.key_end = i;
                    match c {
                        b'=' => pstate = ParseStatus::ValBegin,
                        b' ' | b'\t' => pstate = ParseStatus::Equal,
                        b'\r' | b'\n' => anyhow::bail!("Not found field value in line {line_no}"),
                        _ => {},
                    }
                    // match c {
                    //     b' ' | b'\t' => {
                    //         pstate = ParseStatus::Equal;
                    //         curr.key_end = i;
                    //     },
                    //     b'=' => {
                    //         pstate = ParseStatus::ValBegin;
                    //         curr.key_end = i;
                    //     },
                    //     b'\r' | b'\n' | b'#' => anyhow::bail!("Not found field value in line {line_no}"),
                    //     _ => {},
                    // }
                },
                ParseStatus::Equal => {
                    match skip_chars!(data, i, imax, b' ', b'\t') {
                        b'=' => pstate = ParseStatus::ValBegin,
                        _ => anyhow::bail!("Not found '=' in line {line_no}, {i}"),
                    }
                    // match c {
                    //     b'=' => pstate = ParseStatus::ValBegin,
                    //     b' ' | b'\t' => {},
                    //     _ => anyhow::bail!("Not found '=' in line {line_no}, {i}"),
                    // }
                },
                ParseStatus::ValBegin => {
                    match skip_chars!(data, i, imax, b' ', b'\t') {
                        b'\r' | b'\n' => push_str!(result, curr, 0, pstate, ParseStatus::KeyBegin),
                        b'#' => push_str!(result, curr, 0, pstate, ParseStatus::Comment),
                        _ => {
                            pstate = ParseStatus::Val;
                            curr.val_begin = i;
                        },
                    }
                    // match c {
                    //     b'\r' | b'\n' | b'#' => {
                    //         let s = if c != b'#' { ParseStatus::KeyBegin } else { ParseStatus::Comment };
                    //         push_str!(result, curr, 0, pstate, s);
                    //     },
                    //     b' ' | b'\t' => {},
                    //     _ => {
                    //         pstate = ParseStatus::Val;
                    //         curr.val_begin = i;
                    //     },
                    // }
                },
                ParseStatus::Val => {
                    match util_chars!(data, i, imax, b'\r', b'\n', b'#') {
                        c @ (b'\r' | b'\n') => {
                            if data[i - 1] == b'\\' {
                                pstate = ParseStatus::ValContinue;
                                if c == b'\r' && i + 1 < imax && data[i + 1] == b'\n' {
                                    i += 1;
                                }
                            } else {
                                push_str!(result, curr, i, pstate, ParseStatus::KeyBegin);
                            }
                        },
                        b'#' => push_str!(result, curr, i, pstate, ParseStatus::Comment),
                        _ => {},
                    }
                    // match c {
                    //     b'\r' | b'\n' => {
                    //         if data[i - 1] == b'\\' {
                    //             pstate = ParseStatus::ValContinue;
                    //         } else {
                    //             push_str!(result, curr, i, pstate, ParseStatus::KeyBegin);
                    //         }
                    //     },
                    //     b'#' => {
                    //         push_str!(result, curr, i, pstate, ParseStatus::ValComment);
                    //     },
                    //     _ => {},
                    // }
                },
                ParseStatus::ValContinue => {
                    skip_chars!(data, i, imax, b' ', b'\t');
                    pstate = ParseStatus::Val;
                    continue;
                    // match c {
                    //     b' ' | b'\t' => {},
                    //     _ => {
                    //         pstate = ParseStatus::Val;
                    //     }
                    // }
                },
            }
            i += 1;
        }

        match pstate {
            ParseStatus::ValBegin => result.push(curr),
            ParseStatus::Val | ParseStatus::ValContinue => {
                curr.val_end = imax;
                result.push(curr);
            },
            ParseStatus::Key | ParseStatus::Equal => anyhow::bail!("Not found value at line {line_no}"),
            _ => {},
        }

        Ok(result)
    }


}

impl Default for Config {
    fn default() -> Config {
        Config {data: Vec::with_capacity(0), key_values: Vec::with_capacity(0)}
    }
}

#[cfg(test)]
mod tests {
    use crate::Config;

    #[test]
    fn test_config() {
        let cf = Config::with_text(r#"  a = \
        b\\c\s\

        user_name = 中文 \
        输入 #comment

        #abc
age=48#this is age
        this=
      sex=ma\n\\n"#.to_owned()).unwrap();

        assert_eq!("b\\c ", cf.get_str("a").unwrap().unwrap());
        assert_eq!("中文 输入", cf.get_str("user_name").unwrap().unwrap());
        assert_eq!("48", cf.get_str("age").unwrap().unwrap());
        assert_eq!("", cf.get_str("this").unwrap().unwrap());
        assert_eq!("ma\n\\n", cf.get_str("sex").unwrap().unwrap());
    }
}
