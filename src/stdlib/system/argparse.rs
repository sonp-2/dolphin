//! `os.parse_args`: コマンドライン引数の解析（design.md 5.49）
//!
//! `spec` のマップで宣言し、`os.args[]`（または渡された配列）を解析した結果のマップを返す。
//! `spec` の間違いは実行時エラー、ユーザーの入力の間違いは `usage` のエラー値

use crate::runtime::interp::Interpreter;
use crate::runtime::map::Map;
use crate::runtime::value::Value;
use crate::stdlib::{error_value, new_array, new_map, type_error};

#[derive(Clone, Copy, PartialEq)]
enum Ty {
    Str,
    Num,
    Int,
}

struct Flag {
    name: String,
    short: Option<char>,
    help: String,
}

struct Opt {
    name: String,
    short: Option<char>,
    help: String,
    ty: Ty,
    default: Option<Value>,
    required: bool,
    multiple: bool,
    metavar: String,
}

struct Pos {
    name: String,
    help: String,
    required: bool,
    multiple: bool,
}

struct Spec {
    prog: String,
    description: String,
    flags: Vec<Flag>,
    opts: Vec<Opt>,
    pos: Vec<Pos>,
}

const NAME: &str = "os.parse_args";

// ---- spec の読み取り ----

fn bad(msg: impl std::fmt::Display) -> String {
    format!("{NAME} の spec が正しくありません: {msg}")
}

/// 設定（文字列なら `help` だけ、マップなら各キー）
struct Config<'a> {
    map: Option<std::cell::Ref<'a, Map>>,
    help_only: Option<&'a str>,
}

fn config<'a>(what: &str, value: &'a Value, keys: &[&str]) -> Result<(Config<'a>, bool), String> {
    match value {
        Value::Str(help) => Ok((
            Config {
                map: None,
                help_only: Some(help),
            },
            true,
        )),
        Value::Map(map) => {
            let map = map.borrow();
            for (key, _) in map.iter() {
                if !keys.contains(&&**key) {
                    return Err(bad(format!(
                        "{what} に知らないキー `{key}` があります（使えるのは {}）",
                        keys.join("、")
                    )));
                }
            }
            Ok((
                Config {
                    map: Some(map),
                    help_only: None,
                },
                false,
            ))
        }
        other => Err(bad(format!(
            "{what} は文字列かマップです（{}が渡されました）",
            other.type_name()
        ))),
    }
}

impl Config<'_> {
    fn get(&self, key: &str) -> Option<&Value> {
        self.map.as_ref().and_then(|m| m.get(key))
    }

    fn string(&self, what: &str, key: &str) -> Result<Option<String>, String> {
        if key == "help"
            && let Some(h) = self.help_only
        {
            return Ok(Some(h.to_string()));
        }
        match self.get(key) {
            None => Ok(None),
            Some(Value::Str(s)) => Ok(Some(s.to_string())),
            Some(other) => Err(bad(format!(
                "{what} の `{key}` は文字列です（{}が渡されました）",
                other.type_name()
            ))),
        }
    }

    fn boolean(&self, what: &str, key: &str) -> Result<Option<bool>, String> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(other) => Err(bad(format!(
                "{what} の `{key}` は真偽値です（{}が渡されました）",
                other.type_name()
            ))),
        }
    }

    fn short(&self, what: &str) -> Result<Option<char>, String> {
        let Some(s) = self.string(what, "short")? else {
            return Ok(None);
        };
        let mut chars = s.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c != '-' && !c.is_whitespace() && c != '=' => Ok(Some(c)),
            _ => Err(bad(format!(
                "{what} の `short` は `-` 以外の 1 文字です（\"{s}\"）"
            ))),
        }
    }
}

fn check_name(what: &str, name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.starts_with('-')
        || name.contains(['=', '\0'])
        || name.chars().any(char::is_whitespace)
    {
        return Err(bad(format!("{what} の名前 `{name}` は使えません")));
    }
    Ok(())
}

fn parse_spec(value: &Value) -> Result<Spec, String> {
    let Value::Map(root) = value else {
        return Err(type_error(NAME, 0, "マップ", value));
    };
    let root = root.borrow();
    const KEYS: [&str; 5] = ["name", "description", "flags", "options", "positional"];
    for (key, _) in root.iter() {
        if !KEYS.contains(&&**key) {
            return Err(bad(format!(
                "知らないキー `{key}` があります（使えるのは {}）",
                KEYS.join("、")
            )));
        }
    }
    let text = |key: &str| -> Result<String, String> {
        match root.get(key) {
            None => Ok(String::new()),
            Some(Value::Str(s)) => Ok(s.to_string()),
            Some(other) => Err(bad(format!(
                "`{key}` は文字列です（{}が渡されました）",
                other.type_name()
            ))),
        }
    };
    let mut prog = text("name")?;
    if prog.is_empty() {
        prog = "script".to_string();
    }
    let description = text("description")?;

    let mut spec = Spec {
        prog,
        description,
        flags: Vec::new(),
        opts: Vec::new(),
        pos: Vec::new(),
    };
    let mut seen: Vec<String> = Vec::new();
    let mut shorts: Vec<char> = Vec::new();
    let mut claim = |what: &str, name: &str, short: Option<char>| -> Result<(), String> {
        check_name(what, name)?;
        if name == "help" {
            return Err(bad("`help` は自動で付くので宣言できません"));
        }
        if seen.iter().any(|n| n == name) {
            return Err(bad(format!("名前 `{name}` が重なっています")));
        }
        seen.push(name.to_string());
        if let Some(c) = short {
            if shorts.contains(&c) {
                return Err(bad(format!("短い名前 `-{c}` が重なっています")));
            }
            shorts.push(c);
        }
        Ok(())
    };

    match root.get("flags") {
        None => {}
        Some(Value::Map(flags)) => {
            for (name, value) in flags.borrow().iter() {
                let what = format!("flags の `{name}`");
                let (cfg, _) = config(&what, value, &["short", "help"])?;
                let short = cfg.short(&what)?;
                claim(&what, name, short)?;
                spec.flags.push(Flag {
                    name: name.to_string(),
                    short,
                    help: cfg.string(&what, "help")?.unwrap_or_default(),
                });
            }
        }
        Some(other) => {
            return Err(bad(format!(
                "`flags` はマップです（{}が渡されました）",
                other.type_name()
            )));
        }
    }

    match root.get("options") {
        None => {}
        Some(Value::Map(options)) => {
            for (name, value) in options.borrow().iter() {
                let what = format!("options の `{name}`");
                let (cfg, _) = config(
                    &what,
                    value,
                    &[
                        "short", "help", "type", "default", "required", "multiple", "metavar",
                    ],
                )?;
                let short = cfg.short(&what)?;
                claim(&what, name, short)?;
                let ty = match cfg.string(&what, "type")?.as_deref() {
                    None | Some("str") => Ty::Str,
                    Some("num") => Ty::Num,
                    Some("int") => Ty::Int,
                    Some(other) => {
                        return Err(bad(format!(
                            "{what} の `type` は \"str\" \"num\" \"int\" のどれかです（\"{other}\"）"
                        )));
                    }
                };
                let multiple = cfg.boolean(&what, "multiple")?.unwrap_or(false);
                let default = match cfg.get("default") {
                    None | Some(Value::Nil) => None,
                    Some(v) => {
                        let ok = matches!(
                            (ty, v),
                            (Ty::Str, Value::Str(_))
                                | (Ty::Num, Value::Num(_))
                                | (Ty::Int, Value::Int(_))
                        );
                        if !ok {
                            return Err(bad(format!(
                                "{what} の `default` が `type` に合いません（{}が渡されました）",
                                v.type_name()
                            )));
                        }
                        if multiple {
                            return Err(bad(format!(
                                "{what} は `multiple` なので `default` は使えません"
                            )));
                        }
                        Some(v.clone())
                    }
                };
                let metavar = cfg
                    .string(&what, "metavar")?
                    .unwrap_or_else(|| name.to_uppercase());
                spec.opts.push(Opt {
                    name: name.to_string(),
                    short,
                    help: cfg.string(&what, "help")?.unwrap_or_default(),
                    ty,
                    required: default.is_none()
                        && !multiple
                        && cfg.boolean(&what, "required")?.unwrap_or(false),
                    default,
                    multiple,
                    metavar,
                });
            }
        }
        Some(other) => {
            return Err(bad(format!(
                "`options` はマップです（{}が渡されました）",
                other.type_name()
            )));
        }
    }

    match root.get("positional") {
        None => {}
        Some(Value::Array(items)) => {
            let items = items.borrow();
            for (i, item) in items.iter().enumerate() {
                let what = format!("positional の {i} 番目");
                let pos = match item {
                    Value::Str(name) => Pos {
                        name: name.to_string(),
                        help: String::new(),
                        required: true,
                        multiple: false,
                    },
                    Value::Map(_) => {
                        let (cfg, _) =
                            config(&what, item, &["name", "help", "required", "multiple"])?;
                        let Some(name) = cfg.string(&what, "name")? else {
                            return Err(bad(format!("{what} に `name` がありません")));
                        };
                        Pos {
                            name,
                            help: cfg.string(&what, "help")?.unwrap_or_default(),
                            required: cfg.boolean(&what, "required")?.unwrap_or(true),
                            multiple: cfg.boolean(&what, "multiple")?.unwrap_or(false),
                        }
                    }
                    other => {
                        return Err(bad(format!(
                            "{what} は名前の文字列かマップです（{}が渡されました）",
                            other.type_name()
                        )));
                    }
                };
                claim(&what, &pos.name, None)?;
                if let Some(prev) = spec.pos.last() {
                    if prev.multiple {
                        return Err(bad("`multiple` の位置引数は最後に置きます"));
                    }
                    if !prev.required && pos.required {
                        return Err(bad("省略できる位置引数の後ろに、必須のものは置けません"));
                    }
                }
                spec.pos.push(pos);
            }
        }
        Some(other) => {
            return Err(bad(format!(
                "`positional` は配列です（{}が渡されました）",
                other.type_name()
            )));
        }
    }
    Ok(spec)
}

// ---- 使い方の文字列 ----

fn show_value(v: &Value) -> String {
    match v {
        Value::Str(s) => s.to_string(),
        other => other.to_string(),
    }
}

fn usage(spec: &Spec, h_free: bool) -> String {
    let mut line = format!("使い方: {} [オプション]", spec.prog);
    for p in &spec.pos {
        let dots = if p.multiple { "..." } else { "" };
        if p.required {
            line += &format!(" <{}>{dots}", p.name);
        } else {
            line += &format!(" [{}]{dots}", p.name);
        }
    }
    let mut out = line;
    if !spec.description.is_empty() {
        out += &format!("\n\n{}", spec.description);
    }
    let table = |rows: &[(String, String)]| -> String {
        let width = rows
            .iter()
            .map(|(l, _)| l.chars().count())
            .max()
            .unwrap_or(0);
        let mut text = String::new();
        for (left, help) in rows {
            let pad = " ".repeat(width - left.chars().count());
            let line = if help.is_empty() {
                format!("  {left}")
            } else {
                format!("  {left}{pad}  {help}")
            };
            text += &line;
            text.push('\n');
        }
        text.trim_end().to_string()
    };
    if !spec.pos.is_empty() {
        let rows: Vec<(String, String)> = spec
            .pos
            .iter()
            .map(|p| {
                let mut help = p.help.clone();
                if p.multiple {
                    help = join_note(help, "複数指定できる");
                }
                (p.name.clone(), help)
            })
            .collect();
        out += &format!("\n\n引数:\n{}", table(&rows));
    }
    let mut rows: Vec<(String, String)> = Vec::new();
    let names = |short: Option<char>, name: &str| match short {
        Some(c) => format!("-{c}, --{name}"),
        None => format!("    --{name}"),
    };
    for f in &spec.flags {
        rows.push((names(f.short, &f.name), f.help.clone()));
    }
    for o in &spec.opts {
        let mut help = o.help.clone();
        if let Some(d) = &o.default {
            help = join_note(help, &format!("既定: {}", show_value(d)));
        }
        if o.required {
            help = join_note(help, "必須");
        }
        if o.multiple {
            help = join_note(help, "複数回指定できる");
        }
        rows.push((format!("{} {}", names(o.short, &o.name), o.metavar), help));
    }
    rows.push((
        if h_free { "-h, --help" } else { "    --help" }.to_string(),
        "この使い方を表示して終わる".to_string(),
    ));
    out += &format!("\n\nオプション:\n{}", table(&rows));
    out
}

fn join_note(help: String, note: &str) -> String {
    if help.is_empty() {
        format!("（{note}）")
    } else {
        format!("{help}（{note}）")
    }
}

// ---- 解析 ----

enum Outcome {
    Help,
    Usage(String),
    Done(Value),
}

/// 値として読める引数か（次の引数を、直前のオプションの値にしてよいか）
fn takes_as_value(arg: &str, ty: Ty) -> bool {
    !arg.starts_with('-') || arg == "-" || (ty != Ty::Str && arg.parse::<f64>().is_ok())
}

fn convert(opt: &Opt, shown: &str, raw: &str) -> Result<Value, String> {
    match opt.ty {
        Ty::Str => Ok(Value::Str(raw.into())),
        Ty::Num => match raw.parse::<f64>() {
            Ok(n) if n.is_finite() => Ok(Value::Num(n)),
            _ => Err(format!("`{shown}` の値 `{raw}` は数値ではありません")),
        },
        Ty::Int => raw
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| format!("`{shown}` の値 `{raw}` は整数ではありません")),
    }
}

fn parse(spec: &Spec, args: &[String]) -> Outcome {
    let h_free = !spec
        .flags
        .iter()
        .map(|f| f.short)
        .chain(spec.opts.iter().map(|o| o.short))
        .any(|s| s == Some('h'));
    let mut flag_set = vec![false; spec.flags.len()];
    let mut opt_vals: Vec<Vec<Value>> = vec![Vec::new(); spec.opts.len()];
    let mut positional: Vec<String> = Vec::new();
    let fail = |reason: String| Outcome::Usage(format!("{reason}\n\n{}", usage(spec, h_free)));

    let mut i = 0;
    let mut only_positional = false;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if only_positional || arg == "-" || !arg.starts_with('-') {
            positional.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_positional = true;
            continue;
        }
        if arg == "--help" || (h_free && arg == "-h") {
            return Outcome::Help;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (long, None),
            };
            if let Some(k) = spec.flags.iter().position(|f| f.name == name) {
                if inline.is_some() {
                    return fail(format!("`--{name}` は値を取りません"));
                }
                flag_set[k] = true;
            } else if let Some(k) = spec.opts.iter().position(|o| o.name == name) {
                let opt = &spec.opts[k];
                let shown = format!("--{name}");
                let raw = match inline {
                    Some(v) => v.to_string(),
                    None => match args.get(i) {
                        Some(next) if takes_as_value(next, opt.ty) => {
                            i += 1;
                            next.clone()
                        }
                        _ => return fail(format!("`{shown}` に値がありません")),
                    },
                };
                match convert(opt, &shown, &raw) {
                    Ok(v) => opt_vals[k].push(v),
                    Err(reason) => return fail(reason),
                }
            } else {
                return fail(format!("知らないオプション `--{name}` です"));
            }
            continue;
        }
        // 短いオプション（`-v` `-vq` `-o 値` `-o値`）
        let chars: Vec<char> = arg[1..].chars().collect();
        let mut at = 0;
        while at < chars.len() {
            let c = chars[at];
            at += 1;
            if c == 'h' && h_free {
                return Outcome::Help;
            }
            if let Some(k) = spec.flags.iter().position(|f| f.short == Some(c)) {
                flag_set[k] = true;
            } else if let Some(k) = spec.opts.iter().position(|o| o.short == Some(c)) {
                let opt = &spec.opts[k];
                let shown = format!("-{c}");
                let raw = if at < chars.len() {
                    let rest: String = chars[at..].iter().collect();
                    at = chars.len();
                    rest
                } else {
                    match args.get(i) {
                        Some(next) if takes_as_value(next, opt.ty) => {
                            i += 1;
                            next.clone()
                        }
                        _ => return fail(format!("`{shown}` に値がありません")),
                    }
                };
                match convert(opt, &shown, &raw) {
                    Ok(v) => opt_vals[k].push(v),
                    Err(reason) => return fail(reason),
                }
            } else {
                return fail(format!("知らないオプション `-{c}` です"));
            }
        }
    }

    let mut result = Map::default();
    for (f, set) in spec.flags.iter().zip(&flag_set) {
        result.insert(f.name.as_str().into(), Value::Bool(*set));
    }
    for (o, vals) in spec.opts.iter().zip(opt_vals) {
        let value = if o.multiple {
            new_array(vals)
        } else {
            match vals.into_iter().next_back() {
                Some(v) => v,
                None if o.required => {
                    return fail(format!("`--{}` が必要です", o.name));
                }
                None => o.default.clone().unwrap_or(Value::Nil),
            }
        };
        result.insert(o.name.as_str().into(), value);
    }
    let mut rest = positional.into_iter();
    for p in &spec.pos {
        let value = if p.multiple {
            let all: Vec<Value> = rest.by_ref().map(|s| Value::Str(s.into())).collect();
            if p.required && all.is_empty() {
                return fail(format!("`{}` が足りません", p.name));
            }
            new_array(all)
        } else {
            match rest.next() {
                Some(s) => Value::Str(s.into()),
                None if p.required => return fail(format!("`{}` が足りません", p.name)),
                None => Value::Nil,
            }
        };
        result.insert(p.name.as_str().into(), value);
    }
    if let Some(extra) = rest.next() {
        return fail(format!("余った引数 `{extra}` があります"));
    }
    Outcome::Done(new_map(result))
}

/// `os.parse_args[spec]` / `os.parse_args[spec, args]`
pub fn parse_args(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    if !(1..=2).contains(&args.len()) {
        return Err(format!(
            "{NAME} の引数は 1 個か 2 個です（{} 個渡されました）",
            args.len()
        ));
    }
    let spec = parse_spec(&args[0])?;
    let argv: Vec<String> = match args.get(1) {
        None => interp.args.clone(),
        Some(Value::Array(items)) => {
            let mut v = Vec::new();
            for (i, item) in items.borrow().iter().enumerate() {
                match item {
                    Value::Str(s) => v.push(s.to_string()),
                    other => {
                        return Err(format!(
                            "{NAME} の引数の配列の {i} 番目は文字列です（{}が渡されました）",
                            other.type_name()
                        ));
                    }
                }
            }
            v
        }
        Some(other) => return Err(type_error(NAME, 1, "配列", other)),
    };
    match parse(&spec, &argv) {
        Outcome::Done(value) => Ok(value),
        Outcome::Usage(text) => Ok(error_value("usage", text)),
        Outcome::Help => {
            let h_free = !spec
                .flags
                .iter()
                .map(|f| f.short)
                .chain(spec.opts.iter().map(|o| o.short))
                .any(|s| s == Some('h'));
            writeln!(interp.out, "{}", usage(&spec, h_free)).map_err(|e| e.to_string())?;
            interp.out.flush().map_err(|e| e.to_string())?;
            interp.exit_code = Some(0);
            // `os.exit` と同じ合図
            Err("os.exit".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Spec {
        Spec {
            prog: "t".into(),
            description: "説明".into(),
            flags: vec![
                Flag {
                    name: "verbose".into(),
                    short: Some('v'),
                    help: "詳しく".into(),
                },
                Flag {
                    name: "quiet".into(),
                    short: Some('q'),
                    help: String::new(),
                },
            ],
            opts: vec![
                Opt {
                    name: "out".into(),
                    short: Some('o'),
                    help: "出力".into(),
                    ty: Ty::Str,
                    default: Some(Value::Str("a.out".into())),
                    required: false,
                    multiple: false,
                    metavar: "FILE".into(),
                },
                Opt {
                    name: "n".into(),
                    short: None,
                    help: String::new(),
                    ty: Ty::Int,
                    default: None,
                    required: false,
                    multiple: true,
                    metavar: "N".into(),
                },
            ],
            pos: vec![
                Pos {
                    name: "input".into(),
                    help: String::new(),
                    required: true,
                    multiple: false,
                },
                Pos {
                    name: "rest".into(),
                    help: String::new(),
                    required: false,
                    multiple: true,
                },
            ],
        }
    }

    fn run(args: &[&str]) -> Outcome {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        parse(&spec(), &args)
    }

    fn done(args: &[&str]) -> String {
        match run(args) {
            Outcome::Done(v) => v.to_string(),
            Outcome::Usage(m) => format!("usage: {}", m.lines().next().unwrap()),
            Outcome::Help => "help".into(),
        }
    }

    #[test]
    fn long_short_and_positional() {
        let text = done(&["-vq", "--out=x", "in", "a", "b"]);
        assert!(text.contains("\"verbose\": true"), "{text}");
        assert!(text.contains("\"out\": \"x\""), "{text}");
        assert!(text.contains("\"input\": \"in\""), "{text}");
        let text = done(&["-ox", "in"]);
        assert!(text.contains("\"out\": \"x\""), "{text}");
        let text = done(&["--out", "y", "--", "-v"]);
        assert!(text.contains("\"input\": \"-v\""), "{text}");
    }

    #[test]
    fn defaults_and_multiple() {
        let text = done(&["in"]);
        assert!(text.contains("\"out\": \"a.out\""), "{text}");
        assert!(text.contains("\"verbose\": false"), "{text}");
        let text = done(&["--n", "1", "--n", "-2", "in"]);
        assert!(text.contains("\"n\": {1, -2}"), "{text}");
    }

    #[test]
    fn errors() {
        assert_eq!(done(&[]), "usage: `input` が足りません");
        assert_eq!(
            done(&["--zzz", "in"]),
            "usage: 知らないオプション `--zzz` です"
        );
        assert_eq!(done(&["-z", "in"]), "usage: 知らないオプション `-z` です");
        assert_eq!(done(&["in", "--out"]), "usage: `--out` に値がありません");
        assert_eq!(
            done(&["in", "--n", "x"]),
            "usage: `--n` の値 `x` は整数ではありません"
        );
        assert_eq!(
            done(&["--verbose=1", "in"]),
            "usage: `--verbose` は値を取りません"
        );
    }

    #[test]
    fn help_flags() {
        assert_eq!(done(&["--help"]), "help");
        assert_eq!(done(&["-h"]), "help");
        assert_eq!(done(&["in", "-vh"]), "help");
    }

    #[test]
    fn usage_text() {
        let text = usage(&spec(), true);
        assert!(
            text.starts_with("使い方: t [オプション] <input> [rest]..."),
            "{text}"
        );
        assert!(text.contains("-o, --out FILE"), "{text}");
        assert!(text.contains("（既定: a.out）"), "{text}");
        assert!(text.contains("-h, --help"), "{text}");
    }
}
