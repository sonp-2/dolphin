//! `std:yaml`: parse, parse_all, stringify（design.md 5.51）
//!
//! `yaml-rust2`（`yaml` feature）のパーサーのイベントを読んで、値は自前で組み立てる。
//! アンカー・エイリアス・タグは、値にできないので `unsupported` で断る

use std::fmt::Write as _;
use std::rc::Rc;

use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust2::scanner::{Marker, TScalarStyle};

use crate::runtime::interp::Interpreter;
use crate::runtime::map::Map;
use crate::runtime::value::Value;
use crate::stdlib::{error_value, expect_args, new_array, new_map, str_arg};

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register_module(
        "std:yaml",
        &[
            ("yaml.parse", parse),
            ("yaml.parse_all", parse_all),
            ("yaml.stringify", stringify),
        ],
    );
}

/// 入れ子の深さの上限（`std:json` と同じ）
const MAX_DEPTH: usize = 512;

// ---- 読む ----

fn parse(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("yaml.parse", args, 1)?;
    let text = str_arg("yaml.parse", args, 0)?;
    Ok(match load(text) {
        Ok(mut docs) => match docs.len() {
            0 => Value::Nil,
            1 => docs.remove(0),
            _ => error_value(
                "parse",
                "YAML のドキュメントが 2 つ以上あります（`yaml.parse_all` を使ってください）",
            ),
        },
        Err((kind, message)) => error_value(kind, message),
    })
}

fn parse_all(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("yaml.parse_all", args, 1)?;
    let text = str_arg("yaml.parse_all", args, 0)?;
    Ok(match load(text) {
        Ok(docs) => new_array(docs),
        Err((kind, message)) => error_value(kind, message),
    })
}

/// (エラーの種類, メッセージ)
type LoadError = (&'static str, String);

fn load(text: &str) -> Result<Vec<Value>, LoadError> {
    let mut builder = Builder::default();
    let mut parser = Parser::new_from_str(text);
    if let Err(e) = parser.load(&mut builder, true) {
        // 断った理由があれば、そちらが先
        if let Some(err) = builder.error {
            return Err(err);
        }
        let mark = e.marker();
        return Err((
            "parse",
            format!(
                "YAML が正しくありません（{} 行 {} 列）: {}",
                mark.line(),
                mark.col() + 1,
                e.info()
            ),
        ));
    }
    match builder.error {
        Some(err) => Err(err),
        None => Ok(builder.docs),
    }
}

enum Frame {
    Seq(Vec<Value>),
    Map {
        map: Map,
        /// 読んだキー。値を待っているとき Some
        key: Option<Rc<str>>,
    },
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    docs: Vec<Value>,
    error: Option<LoadError>,
}

impl Builder {
    fn fail(&mut self, kind: &'static str, mark: Marker, message: &str) {
        if self.error.is_none() {
            self.error = Some((
                kind,
                format!("{} 行 {} 列: {message}", mark.line(), mark.col() + 1),
            ));
        }
    }

    /// 出来上がった値を、いまの入れ物（なければドキュメント）に入れる
    fn push(&mut self, value: Value) {
        match self.stack.last_mut() {
            None => self.docs.push(value),
            Some(Frame::Seq(items)) => items.push(value),
            Some(Frame::Map { map, key }) => {
                let key = key.take().expect("値の前にキーを読んでいる");
                map.insert(key, value);
            }
        }
    }

    /// マップがキーを待っているか
    fn expects_key(&self) -> bool {
        matches!(self.stack.last(), Some(Frame::Map { key: None, .. }))
    }

    fn start(&mut self, frame: Frame, mark: Marker) {
        if self.expects_key() {
            return self.fail(
                "unsupported",
                mark,
                "配列やマップをキーにはできません（キーは文字列です）",
            );
        }
        if self.stack.len() >= MAX_DEPTH {
            return self.fail(
                "unsupported",
                mark,
                &format!("入れ子が深すぎます（{MAX_DEPTH} 段まで）"),
            );
        }
        self.stack.push(frame);
    }
}

impl MarkedEventReceiver for Builder {
    fn on_event(&mut self, event: Event, mark: Marker) {
        if self.error.is_some() {
            return;
        }
        match event {
            Event::Alias(_) => self.fail(
                "unsupported",
                mark,
                "エイリアス（`*名前`）は Dolphin の値にできません",
            ),
            Event::Scalar(text, style, anchor, tag) => {
                if anchor != 0 {
                    return self.fail(
                        "unsupported",
                        mark,
                        "アンカー（`&名前`）は Dolphin の値にできません",
                    );
                }
                if tag.is_some() {
                    return self.fail(
                        "unsupported",
                        mark,
                        "タグ（`!` `!!`）は Dolphin の値にできません",
                    );
                }
                if let Some(Frame::Map { key, .. }) = self.stack.last_mut()
                    && key.is_none()
                {
                    // キーは、書かれた文字列のまま
                    *key = Some(text.into());
                    return;
                }
                let value = if style == TScalarStyle::Plain {
                    match resolve_plain(&text) {
                        Plain::Null => Value::Nil,
                        Plain::Bool(b) => Value::Bool(b),
                        Plain::Num(n) => Value::Num(n),
                        Plain::NonFinite => {
                            return self.fail(
                                "unsupported",
                                mark,
                                &format!("`{text}` は Dolphin の数値にできません"),
                            );
                        }
                        Plain::Str => Value::Str(text.into()),
                    }
                } else {
                    Value::Str(text.into())
                };
                self.push(value);
            }
            Event::SequenceStart(anchor, tag) | Event::MappingStart(anchor, tag)
                if anchor != 0 || tag.is_some() =>
            {
                let what = if anchor != 0 {
                    "アンカー（`&名前`）"
                } else {
                    "タグ（`!` `!!`）"
                };
                self.fail(
                    "unsupported",
                    mark,
                    &format!("{what}は Dolphin の値にできません"),
                );
            }
            Event::SequenceStart(..) => self.start(Frame::Seq(Vec::new()), mark),
            Event::MappingStart(..) => self.start(
                Frame::Map {
                    map: Map::default(),
                    key: None,
                },
                mark,
            ),
            Event::SequenceEnd => {
                if let Some(Frame::Seq(items)) = self.stack.pop() {
                    self.push(new_array(items));
                }
            }
            Event::MappingEnd => {
                if let Some(Frame::Map { map, .. }) = self.stack.pop() {
                    self.push(new_map(map));
                }
            }
            Event::Nothing
            | Event::StreamStart
            | Event::StreamEnd
            | Event::DocumentStart
            | Event::DocumentEnd => {}
        }
    }
}

/// 引用符なしのスカラーの読み方（YAML 1.2 のコアスキーマ）
#[derive(Debug, PartialEq)]
enum Plain {
    Null,
    Bool(bool),
    Num(f64),
    /// `.inf` `.nan`
    NonFinite,
    Str,
}

fn resolve_plain(s: &str) -> Plain {
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return Plain::Null,
        "true" | "True" | "TRUE" => return Plain::Bool(true),
        "false" | "False" | "FALSE" => return Plain::Bool(false),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" | "-.inf" | "-.Inf" | "-.INF"
        | ".nan" | ".NaN" | ".NAN" => return Plain::NonFinite,
        _ => {}
    }
    if let Some(hex) = s.strip_prefix("0x") {
        return radix(hex, 16);
    }
    if let Some(oct) = s.strip_prefix("0o") {
        return radix(oct, 8);
    }
    if is_decimal(s) {
        return match s.parse::<f64>() {
            Ok(n) if n.is_finite() => Plain::Num(n),
            _ => Plain::NonFinite,
        };
    }
    Plain::Str
}

fn radix(digits: &str, base: u32) -> Plain {
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(base)) {
        return Plain::Str;
    }
    // 16 桁を超える値は f64 に丸める
    let mut n = 0f64;
    for c in digits.chars() {
        n = n * f64::from(base) + f64::from(c.to_digit(base).expect("検査済み"));
    }
    if n.is_finite() {
        Plain::Num(n)
    } else {
        Plain::NonFinite
    }
}

/// `[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?`
fn is_decimal(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'-' | b'+')) {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let start = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i - start
    };
    let int = digits(&mut i);
    let mut frac = 0;
    if b.get(i) == Some(&b'.') {
        i += 1;
        frac = digits(&mut i);
    }
    if int == 0 && frac == 0 {
        return false;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'-' | b'+')) {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return false;
        }
    }
    i == b.len()
}

// ---- 書く ----

fn stringify(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("yaml.stringify", args, 1)?;
    let mut out = String::new();
    let mut seen = Vec::new();
    write_node(&mut out, &args[0], 0, false, &mut seen)?;
    Ok(Value::Str(out.into()))
}

/// `inline` は、直前に `- ` が書いてあって、最初の行のインデントを省くとき
fn write_node(
    out: &mut String,
    value: &Value,
    indent: usize,
    inline: bool,
    seen: &mut Vec<*const ()>,
) -> Result<(), String> {
    let pad = |out: &mut String, n: usize| out.extend(std::iter::repeat_n(' ', n));
    match value {
        Value::Array(items) => {
            let ptr = Rc::as_ptr(items).cast::<()>();
            enter(seen, ptr)?;
            let items = items.borrow();
            if items.is_empty() {
                out.push_str("[]\n");
            } else {
                for (i, item) in items.iter().enumerate() {
                    if !(inline && i == 0) {
                        pad(out, indent);
                    }
                    out.push_str("- ");
                    write_child(out, item, indent + 2, seen)?;
                }
            }
            seen.pop();
        }
        Value::Map(map) => {
            let ptr = Rc::as_ptr(map).cast::<()>();
            enter(seen, ptr)?;
            let map = map.borrow();
            if map.is_empty() {
                out.push_str("{}\n");
            } else {
                for (i, (key, item)) in map.iter().enumerate() {
                    if !(inline && i == 0) {
                        pad(out, indent);
                    }
                    write_string(out, key);
                    out.push(':');
                    match item {
                        Value::Map(m) if !m.borrow().is_empty() => {
                            out.push('\n');
                            write_node(out, item, indent + 2, false, seen)?;
                        }
                        // 配列は、キーと同じインデントに並べる
                        Value::Array(a) if !a.borrow().is_empty() => {
                            out.push('\n');
                            write_node(out, item, indent, false, seen)?;
                        }
                        _ => {
                            out.push(' ');
                            write_scalar_line(out, item)?;
                        }
                    }
                }
            }
            seen.pop();
        }
        other => write_scalar_line(out, other)?,
    }
    Ok(())
}

/// `- ` の後ろの値
fn write_child(
    out: &mut String,
    value: &Value,
    indent: usize,
    seen: &mut Vec<*const ()>,
) -> Result<(), String> {
    match value {
        Value::Array(a) if !a.borrow().is_empty() => write_node(out, value, indent, true, seen),
        Value::Map(m) if !m.borrow().is_empty() => write_node(out, value, indent, true, seen),
        other => write_scalar_line(out, other),
    }
}

/// 空でない配列・マップ以外の値を、改行つきで 1 行に書く
fn write_scalar_line(out: &mut String, value: &Value) -> Result<(), String> {
    match value {
        Value::Nil => out.push_str("null"),
        Value::Bool(b) => write!(out, "{b}").expect("String への書き込みは失敗しない"),
        Value::Num(n) if n.is_finite() => {
            write!(out, "{n}").expect("String への書き込みは失敗しない")
        }
        Value::Num(n) => return Err(format!("yaml.stringify: {n} は YAML にできません")),
        Value::Int(n) => write!(out, "{n}").expect("String への書き込みは失敗しない"),
        Value::Str(s) => write_string(out, s),
        Value::Array(_) => out.push_str("[]"),
        Value::Map(_) => out.push_str("{}"),
        other => {
            return Err(format!(
                "yaml.stringify: {}は YAML にできません",
                other.type_name()
            ));
        }
    }
    out.push('\n');
    Ok(())
}

fn enter(seen: &mut Vec<*const ()>, ptr: *const ()) -> Result<(), String> {
    if seen.contains(&ptr) {
        return Err("yaml.stringify: 循環した配列やマップは YAML にできません".into());
    }
    if seen.len() >= MAX_DEPTH {
        return Err(format!(
            "yaml.stringify: 入れ子が深すぎます（{MAX_DEPTH} 段まで）"
        ));
    }
    seen.push(ptr);
    Ok(())
}

/// 引用符なしで書いて、読み戻しても同じ文字列になるものだけ、そのまま書く
fn is_plain_safe(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let ok_first = first.is_alphanumeric() || first == '_' || first == '/';
    let ok_rest = |c: char| {
        if c.is_ascii() {
            c.is_ascii_alphanumeric() || "_-./+@()=~$%^ ".contains(c)
        } else {
            !c.is_control() && !c.is_whitespace() && c != '\u{feff}'
        }
    };
    ok_first
        && !s.ends_with(' ')
        && !s.contains("  ")
        && s.chars().all(ok_rest)
        && resolve_plain(s) == Plain::Str
}

fn write_string(out: &mut String, s: &str) {
    if is_plain_safe(s) {
        out.push_str(s);
        return;
    }
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}') => {
                write!(out, "\\u{:04x}", c as u32).expect("String への書き込みは失敗しない");
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: &Value) -> String {
        format!("{v}")
    }

    fn rt(src: &str) -> String {
        let docs = load(src).expect("読める");
        let out = {
            let mut s = String::new();
            write_node(&mut s, &docs[0], 0, false, &mut Vec::new()).expect("書ける");
            s
        };
        let again = load(&out).expect("書いたものは読める");
        assert!(docs[0].equals(&again[0]), "往復で変わった: {out}");
        out
    }

    #[test]
    fn scalars_follow_core_schema() {
        assert_eq!(resolve_plain("12"), Plain::Num(12.0));
        assert_eq!(resolve_plain("-1.5e3"), Plain::Num(-1500.0));
        assert_eq!(resolve_plain("0x1F"), Plain::Num(31.0));
        assert_eq!(resolve_plain("0o17"), Plain::Num(15.0));
        assert_eq!(resolve_plain("1_000"), Plain::Str);
        assert_eq!(resolve_plain("yes"), Plain::Str);
        assert_eq!(resolve_plain("."), Plain::Str);
        assert_eq!(resolve_plain("1e"), Plain::Str);
        assert_eq!(resolve_plain(".nan"), Plain::NonFinite);
        assert_eq!(resolve_plain("~"), Plain::Null);
    }

    #[test]
    fn parses_basic() {
        let docs = load("a: 1\nb: [x, true, null]\nc:\n  d: 'q'\n").unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(
            text(&docs[0]),
            r#"{"a": 1, "b": {"x", true, nil}, "c": {"d": "q"}}"#
        );
    }

    #[test]
    fn rejects_anchor_alias_tag() {
        for src in [
            "a: &x 1",
            "a: &x 1\nb: *x",
            "a: !!str 1",
            "&a [1]",
            "!t {a: 1}",
        ] {
            let e = load(src).expect_err(src);
            assert_eq!(e.0, "unsupported", "{src}");
        }
        assert_eq!(load("? [1]\n: 2").expect_err("複合キー").0, "unsupported");
        assert_eq!(load("a: .inf").expect_err("inf").0, "unsupported");
        assert_eq!(load("a: [").expect_err("構文").0, "parse");
    }

    #[test]
    fn multi_documents() {
        assert_eq!(load("---\na\n---\nb\n").unwrap().len(), 2);
        assert_eq!(load("").unwrap().len(), 0);
        assert_eq!(load("# only comment\n").unwrap().len(), 0);
    }

    #[test]
    fn round_trips() {
        let out = rt(
            "name: x\nlist:\n- 1\n- [a, b]\n- {k: v}\n- {}\n- []\nnested:\n  deep:\n    - - 1\n      - 2\n",
        );
        assert!(
            out.starts_with("name: x\nlist:\n- 1\n- - a\n  - b\n"),
            "{out}"
        );
        rt("\"1\": 'true'\n\"\": ' sp '\nmulti: \"a\\nb\\tc\\u0001\"\nja: こんにちは、世界\n");
        rt("- yes\n- 12\n- '12'\n- ~\n- 'null'\n- \"- x\"\n- 'a: b'\n- '#c'\n- 0.5\n");
        rt("42");
        rt("[]");
    }

    #[test]
    fn plain_safe_strings_stay_plain() {
        let mut s = String::new();
        write_string(&mut s, "hello world");
        assert_eq!(s, "hello world");
        for tricky in ["12", "true", "", "a: b", "- x", "x ", "#c", "null", "1e3"] {
            let mut s = String::new();
            write_string(&mut s, tricky);
            assert!(s.starts_with('"'), "{tricky} -> {s}");
        }
    }
}
