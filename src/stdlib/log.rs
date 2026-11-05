//! `std:log`: debug, info, warn, error, set_level, set_format, set_output（design.md 5.50）
//!
//! 1 行 1 件のログ。text は `時刻 LEVEL msg key=value …`、json は 1 行の JSON。
//! 設定（レベル・形式・出力先）はスレッドごと。既定は info・text・標準エラー

use std::cell::RefCell;
use std::io::Write;

use super::json::to_json_string;
use super::time::iso_utc_ms;
use super::{error_value, expect_args, str_arg, type_error};
use crate::runtime::handle::{HandleKind, StreamSink};
use crate::runtime::interp::Interpreter;
use crate::runtime::map::Map;
use crate::runtime::value::Value;

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register_module(
        "std:log",
        &[
            ("log.debug", |i, a| emit(i, "log.debug", Level::Debug, a)),
            ("log.info", |i, a| emit(i, "log.info", Level::Info, a)),
            ("log.warn", |i, a| emit(i, "log.warn", Level::Warn, a)),
            ("log.error", |i, a| emit(i, "log.error", Level::Error, a)),
            ("log.set_level", set_level),
            ("log.set_format", set_format),
            ("log.set_output", set_output),
        ],
    );
}

/// 低い順。`Off` は何も出さない設定にだけ使う
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Debug,
    Info,
    Warn,
    Error,
    Off,
}

impl Level {
    fn upper(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
            Level::Off => "OFF",
        }
    }
}

struct State {
    level: Level,
    json: bool,
    /// None なら標準エラー
    output: Option<Value>,
}

thread_local! {
    static STATE: RefCell<State> = const {
        RefCell::new(State { level: Level::Info, json: false, output: None })
    };
}

fn set_level(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("log.set_level", args, 1)?;
    let level = match str_arg("log.set_level", args, 0)? {
        "debug" => Level::Debug,
        "info" => Level::Info,
        "warn" => Level::Warn,
        "error" => Level::Error,
        "off" => Level::Off,
        other => {
            return Err(format!(
                "log.set_level のレベルは \"debug\" \"info\" \"warn\" \"error\" \"off\" のどれかです（{other:?}）"
            ));
        }
    };
    STATE.with_borrow_mut(|s| s.level = level);
    Ok(Value::Nil)
}

fn set_format(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("log.set_format", args, 1)?;
    let json = match str_arg("log.set_format", args, 0)? {
        "text" => false,
        "json" => true,
        other => {
            return Err(format!(
                "log.set_format の形式は \"text\" か \"json\" です（{other:?}）"
            ));
        }
    };
    STATE.with_borrow_mut(|s| s.json = json);
    Ok(Value::Nil)
}

fn set_output(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("log.set_output", args, 1)?;
    let output = match &args[0] {
        Value::Nil => None,
        Value::Handle(handle) => match &*handle.inner.borrow() {
            HandleKind::Stream(stream) if stream.writer.is_some() => Some(args[0].clone()),
            HandleKind::Stream(_) => {
                return Err(
                    "log.set_output に渡すのは書き込み用のストリームです（読み込み用でした）"
                        .into(),
                );
            }
            // 閉じたものは、書くときにエラー値になる
            HandleKind::StreamClosed | HandleKind::Closed => Some(args[0].clone()),
            _ => {
                return Err(
                    "log.set_output に渡せるのはストリームのハンドルです（ほかの種類のハンドルでした）"
                        .into(),
                );
            }
        },
        other => return Err(type_error("log.set_output", 0, "ストリームか nil", other)),
    };
    STATE.with_borrow_mut(|s| s.output = output);
    Ok(Value::Nil)
}

fn emit(
    interp: &mut Interpreter<'_>,
    name: &str,
    level: Level,
    args: &[Value],
) -> Result<Value, String> {
    if !(1..=2).contains(&args.len()) {
        return Err(format!(
            "{name} の引数は 1 個か 2 個です（{} 個渡されました）",
            args.len()
        ));
    }
    let fields = match args.get(1) {
        None | Some(Value::Nil) => None,
        Some(Value::Map(map)) => Some(map),
        Some(other) => return Err(type_error(name, 1, "マップ", other)),
    };
    let (min, json) = STATE.with_borrow(|s| (s.level, s.json));
    if level < min {
        return Ok(Value::Nil);
    }
    let msg = args[0].to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    let time = iso_utc_ms(now);
    let mut line = if json {
        format_json(&time, level, &msg, fields.map(|m| m.borrow()).as_deref())
    } else {
        format_text(&time, level, &msg, fields.map(|m| m.borrow()).as_deref())
    };
    line.push('\n');
    Ok(write_line(interp, &line))
}

fn json_string(s: &str) -> String {
    to_json_string("log", &Value::Str(s.into()), None).unwrap_or_else(|_| "\"\"".into())
}

/// text の値。空・空白・`=`・`"`・制御文字を含む文字列は、JSON の文字列として囲む
fn text_value(value: &Value) -> String {
    let rendered = match value {
        Value::Str(s) => s.to_string(),
        Value::Array(_) | Value::Map(_) => {
            to_json_string("log", value, None).unwrap_or_else(|_| value.to_string())
        }
        other => other.to_string(),
    };
    let needs_quote = rendered.is_empty()
        || rendered
            .chars()
            .any(|c| c.is_whitespace() || c == '=' || c == '"' || c.is_control());
    if needs_quote {
        json_string(&rendered)
    } else {
        rendered
    }
}

fn format_text(time: &str, level: Level, msg: &str, fields: Option<&Map>) -> String {
    let mut line = format!(
        "{time} {} {}",
        level.upper(),
        msg.replace('\r', "\\r").replace('\n', "\\n")
    );
    for (key, value) in fields.into_iter().flat_map(|m| m.iter()) {
        line.push(' ');
        line.push_str(key);
        line.push('=');
        line.push_str(&text_value(value));
    }
    line
}

fn format_json(time: &str, level: Level, msg: &str, fields: Option<&Map>) -> String {
    let mut line = format!(
        "{{\"time\":{},\"level\":{},\"msg\":{}",
        json_string(time),
        json_string(&level.upper().to_lowercase()),
        json_string(msg)
    );
    for (key, value) in fields.into_iter().flat_map(|m| m.iter()) {
        let key = if matches!(&**key, "time" | "level" | "msg") {
            format!("field_{key}")
        } else {
            key.to_string()
        };
        // JSON にできない値は、表示した文字列にする
        let encoded =
            to_json_string("log", value, None).unwrap_or_else(|_| json_string(&value.to_string()));
        line.push(',');
        line.push_str(&json_string(&key));
        line.push(':');
        line.push_str(&encoded);
    }
    line.push('}');
    line
}

/// 出力先に 1 行書く。失敗はエラー値（`io`）
fn write_line(interp: &mut Interpreter<'_>, line: &str) -> Value {
    let output = STATE.with_borrow(|s| s.output.clone());
    let result = match &output {
        None => std::io::stderr().write_all(line.as_bytes()),
        Some(Value::Handle(handle)) => match &mut *handle.inner.borrow_mut() {
            HandleKind::Stream(stream) => match &mut stream.writer {
                Some(StreamSink::Writer(w)) => w.write_all(line.as_bytes()),
                Some(StreamSink::InterpOut) => interp.out.write_all(line.as_bytes()),
                None => Ok(()),
            },
            _ => {
                return error_value("io", "ログの出力先のストリームは閉じています");
            }
        },
        Some(_) => Ok(()),
    };
    match result {
        Ok(()) => Value::Nil,
        Err(e) => error_value("io", format!("ログを書けませんでした: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(pairs: &[(&str, Value)]) -> Map {
        let mut map = Map::default();
        for (k, v) in pairs {
            map.insert((*k).into(), v.clone());
        }
        map
    }

    #[test]
    fn text_line() {
        let f = fields(&[
            ("port", Value::Num(8080.0)),
            ("name", Value::Str("a b".into())),
            ("empty", Value::Str("".into())),
            ("ok", Value::Bool(true)),
        ]);
        assert_eq!(
            format_text("T", Level::Warn, "起動\n中", Some(&f)),
            "T WARN 起動\\n中 port=8080 name=\"a b\" empty=\"\" ok=true"
        );
        assert_eq!(format_text("T", Level::Info, "x", None), "T INFO x");
    }

    #[test]
    fn json_line() {
        let f = fields(&[("n", Value::Num(1.0)), ("level", Value::Str("x\"y".into()))]);
        assert_eq!(
            format_json("T", Level::Error, "m", Some(&f)),
            "{\"time\":\"T\",\"level\":\"error\",\"msg\":\"m\",\"n\":1,\"field_level\":\"x\\\"y\"}"
        );
    }

    #[test]
    fn level_order() {
        assert!(Level::Debug < Level::Info && Level::Info < Level::Warn);
        assert!(Level::Error < Level::Off);
    }
}
