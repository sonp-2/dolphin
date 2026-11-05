//! `std:toml`: parse, stringify（design.md 5.51）
//!
//! `toml`（`toml` feature）で読み書きし、`toml::Value` と Dolphin の値を相互に変換する

use std::rc::Rc;

use crate::runtime::interp::Interpreter;
use crate::runtime::map::Map;
use crate::runtime::value::Value;
use crate::stdlib::{error_value, expect_args, new_array, new_map, str_arg, type_error};

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register_module(
        "std:toml",
        &[("toml.parse", parse), ("toml.stringify", stringify)],
    );
}

/// 浮動小数点数のまま正確に表せる整数の上限
const MAX_EXACT: f64 = 9_007_199_254_740_992.0;
/// 入れ子の深さの上限（書くときの再帰を抑える）
const MAX_DEPTH: usize = 512;

// ---- 読む ----

fn parse(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("toml.parse", args, 1)?;
    let text = str_arg("toml.parse", args, 0)?;
    Ok(match load(text) {
        Ok(value) => value,
        Err((kind, message)) => error_value(kind, message),
    })
}

fn load(text: &str) -> Result<Value, (&'static str, String)> {
    let table: toml::Table = toml::from_str(text).map_err(|e| {
        let place = e.span().map_or(String::new(), |span| {
            let before = &text[..span.start.min(text.len())];
            let line = before.matches('\n').count() + 1;
            let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
            format!("（{line} 行 {col} 列）")
        });
        (
            "parse",
            format!("TOML が正しくありません{place}: {}", e.message()),
        )
    })?;
    table_to_map(table)
}

fn table_to_map(table: toml::Table) -> Result<Value, (&'static str, String)> {
    let mut map = Map::default();
    for (key, value) in table {
        map.insert(key.as_str().into(), from_toml(value)?);
    }
    Ok(new_map(map))
}

fn from_toml(value: toml::Value) -> Result<Value, (&'static str, String)> {
    Ok(match value {
        toml::Value::String(s) => Value::Str(s.into()),
        toml::Value::Integer(n) => Value::Num(n as f64),
        toml::Value::Float(n) if n.is_finite() => Value::Num(n),
        toml::Value::Float(n) => {
            return Err((
                "unsupported",
                format!("`{n}` は Dolphin の数値にできません"),
            ));
        }
        toml::Value::Boolean(b) => Value::Bool(b),
        // 日時は、書かれたとおりの文字列にする
        toml::Value::Datetime(d) => Value::Str(d.to_string().into()),
        toml::Value::Array(items) => {
            let items = items.into_iter().map(from_toml).collect::<Result<_, _>>()?;
            new_array(items)
        }
        toml::Value::Table(table) => table_to_map(table)?,
    })
}

// ---- 書く ----

/// 書けなかった理由
enum Fail {
    /// データのせいで書けない（`nil`）。エラー値にする
    Unsupported(String),
    /// プログラムの間違い（実行時エラー）
    Program(String),
}

fn stringify(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("toml.stringify", args, 1)?;
    let Value::Map(map) = &args[0] else {
        return Err(type_error("toml.stringify", 0, "マップ", &args[0]));
    };
    let mut seen = vec![Rc::as_ptr(map).cast::<()>()];
    let table = match to_table(map.borrow().iter(), &mut seen) {
        Ok(table) => table,
        Err(Fail::Unsupported(message)) => return Ok(error_value("unsupported", message)),
        Err(Fail::Program(message)) => return Err(message),
    };
    toml::to_string(&table)
        .map(|s| Value::Str(s.into()))
        .map_err(|e| format!("toml.stringify: {e}"))
}

fn to_table<'a>(
    entries: impl Iterator<Item = (&'a Rc<str>, &'a Value)>,
    seen: &mut Vec<*const ()>,
) -> Result<toml::Table, Fail> {
    let mut table = toml::Table::new();
    for (key, value) in entries {
        table.insert(key.to_string(), to_toml(value, seen)?);
    }
    Ok(table)
}

fn to_toml(value: &Value, seen: &mut Vec<*const ()>) -> Result<toml::Value, Fail> {
    Ok(match value {
        Value::Nil => {
            return Err(Fail::Unsupported(
                "nil は TOML にできません（TOML に null はありません）".into(),
            ));
        }
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Num(n) if !n.is_finite() => {
            return Err(Fail::Program(format!(
                "toml.stringify: {n} は TOML にできません"
            )));
        }
        Value::Num(n) if n.fract() == 0.0 && n.abs() <= MAX_EXACT => {
            toml::Value::Integer(*n as i64)
        }
        Value::Num(n) => toml::Value::Float(*n),
        Value::Int(n) => toml::Value::Integer(*n),
        Value::Str(s) => toml::Value::String(s.to_string()),
        Value::Array(items) => {
            let ptr = Rc::as_ptr(items).cast::<()>();
            enter(seen, ptr)?;
            let converted = items
                .borrow()
                .iter()
                .map(|item| to_toml(item, seen))
                .collect::<Result<Vec<_>, _>>()?;
            seen.pop();
            toml::Value::Array(converted)
        }
        Value::Map(map) => {
            let ptr = Rc::as_ptr(map).cast::<()>();
            enter(seen, ptr)?;
            let table = to_table(map.borrow().iter(), seen)?;
            seen.pop();
            toml::Value::Table(table)
        }
        other => {
            return Err(Fail::Program(format!(
                "toml.stringify: {}は TOML にできません",
                other.type_name()
            )));
        }
    })
}

fn enter(seen: &mut Vec<*const ()>, ptr: *const ()) -> Result<(), Fail> {
    if seen.contains(&ptr) {
        return Err(Fail::Program(
            "toml.stringify: 循環した配列やマップは TOML にできません".into(),
        ));
    }
    if seen.len() >= MAX_DEPTH {
        return Err(Fail::Program(format!(
            "toml.stringify: 入れ子が深すぎます（{MAX_DEPTH} 段まで）"
        )));
    }
    seen.push(ptr);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: &Value) -> String {
        format!("{v}")
    }

    fn write(v: &Value) -> String {
        let Value::Map(map) = v else { panic!() };
        let mut seen = Vec::new();
        let Ok(table) = to_table(map.borrow().iter(), &mut seen) else {
            panic!("変換できる");
        };
        toml::to_string(&table).unwrap()
    }

    #[test]
    fn parses_basic() {
        let v = load(
            "title = \"x\"\nn = 3\nf = 1.5\nok = true\nd = 1979-05-27T07:32:00Z\n\n[a.b]\nc = [1, 2]\n\n[[items]]\nname = \"p\"\n[[items]]\nname = \"q\"\n",
        )
        .unwrap();
        assert_eq!(
            text(&v),
            r#"{"title": "x", "n": 3, "f": 1.5, "ok": true, "d": "1979-05-27T07:32:00Z", "a": {"b": {"c": {1, 2}}}, "items": {{"name": "p"}, {"name": "q"}}}"#
        );
    }

    #[test]
    fn errors() {
        assert_eq!(load("a = ").unwrap_err().0, "parse");
        assert_eq!(load("a = 1\na = 2").unwrap_err().0, "parse");
        assert_eq!(load("a = inf").unwrap_err().0, "unsupported");
        assert_eq!(load("a = nan").unwrap_err().0, "unsupported");
    }

    #[test]
    fn round_trips() {
        let src = "name = \"d\"\nn = 3\nf = 0.25\nlist = [1, 2]\nmixed = [1, \"a\"]\n\n[t]\nk = \"v\"\n\n[t.u]\nx = true\n\n[[arr]]\nid = 1\n\n[[arr]]\nid = 2\n";
        let once = load(src).unwrap();
        let out = write(&once);
        let twice = load(&out).unwrap();
        assert!(once.equals(&twice), "{out}");
    }
}
