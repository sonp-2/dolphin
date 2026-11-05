//! `str_format`（design.md 5.48）。Python の format_spec の最小サブセット
//!
//! テンプレートの `{}` `{0}` `{:spec}` `{0:spec}` に引数を埋める。`{{` `}}` は `{` `}` そのもの。
//! spec は `[[fill]align][0][width][,][.prec][type]`

use super::{expect_args_at_least, str_arg};
use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register("str_format", str_format);
}

/// 幅の上限
const WIDTH_MAX: usize = 1_000_000;
/// 精度の上限
const PREC_MAX: usize = 1000;

#[derive(Default)]
struct Spec {
    fill: Option<char>,
    align: Option<char>,
    zero: bool,
    width: usize,
    comma: bool,
    prec: Option<usize>,
    ty: Option<char>,
}

fn str_format(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args_at_least("str_format", args, 1)?;
    let template = str_arg("str_format", args, 0)?;
    let values = &args[1..];
    Ok(Value::Str(format_template(template, values)?.into()))
}

pub fn format_template(template: &str, values: &[Value]) -> Result<String, String> {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut auto = 0usize;
    // 0: まだ決まっていない、1: 自動（`{}`）、2: 番号（`{0}`）
    let mut mode = 0u8;
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' if chars.get(i + 1) == Some(&'{') => {
                out.push('{');
                i += 2;
            }
            '}' if chars.get(i + 1) == Some(&'}') => {
                out.push('}');
                i += 2;
            }
            '}' => return Err("str_format のテンプレートに、対応しない } があります".into()),
            '{' => {
                let Some(len) = chars[i + 1..].iter().position(|&c| c == '}') else {
                    return Err("str_format のテンプレートに、閉じていない { があります".into());
                };
                let field: String = chars[i + 1..i + 1 + len].iter().collect();
                i += len + 2;
                let (index, spec) = match field.split_once(':') {
                    Some((index, spec)) => (index, spec),
                    None => (field.as_str(), ""),
                };
                let n = if index.is_empty() {
                    if mode == 2 {
                        return Err("str_format の {} と {0} は混ぜられません".into());
                    }
                    mode = 1;
                    auto += 1;
                    auto - 1
                } else {
                    if mode == 1 {
                        return Err("str_format の {} と {0} は混ぜられません".into());
                    }
                    mode = 2;
                    index.parse::<usize>().map_err(|_| {
                        format!("str_format の {{{field}}} の番号が読めません（{index:?}）")
                    })?
                };
                let Some(value) = values.get(n) else {
                    return Err(format!(
                        "str_format の引数が足りません（{n} 番目が必要で、{} 個渡されました）",
                        values.len()
                    ));
                };
                out.push_str(&format_one(value, &parse_spec(spec)?)?);
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(out)
}

fn parse_spec(spec: &str) -> Result<Spec, String> {
    let bad = |why: &str| format!("str_format の書式 {spec:?} は使えません（{why}）");
    let c: Vec<char> = spec.chars().collect();
    let mut s = Spec::default();
    let mut i = 0;
    let is_align = |c: char| matches!(c, '<' | '>' | '^');
    if c.len() >= 2 && is_align(c[1]) {
        s.fill = Some(c[0]);
        s.align = Some(c[1]);
        i = 2;
    } else if !c.is_empty() && is_align(c[0]) {
        s.align = Some(c[0]);
        i = 1;
    }
    if c.get(i) == Some(&'0') {
        s.zero = true;
        i += 1;
    }
    let digits = |i: &mut usize| -> Option<usize> {
        let start = *i;
        while c.get(*i).is_some_and(char::is_ascii_digit) {
            *i += 1;
        }
        let text: String = c[start..*i].iter().collect();
        text.parse().ok()
    };
    if let Some(w) = digits(&mut i) {
        s.width = w;
    } else if c.get(i).is_some_and(char::is_ascii_digit) {
        return Err(bad("幅が大きすぎます"));
    }
    if c.get(i) == Some(&',') {
        s.comma = true;
        i += 1;
    }
    if c.get(i) == Some(&'.') {
        i += 1;
        match digits(&mut i) {
            Some(p) => s.prec = Some(p),
            None => return Err(bad(". の後に精度の数字が必要です")),
        }
    }
    if let Some(&t) = c.get(i) {
        if !matches!(t, 'd' | 'f' | 'x' | 'X' | 'b' | 'o' | 's') {
            return Err(bad("型は d f x X b o s のどれかです"));
        }
        s.ty = Some(t);
        i += 1;
    }
    if i != c.len() {
        return Err(bad("読めない文字があります"));
    }
    if s.width > WIDTH_MAX {
        return Err(bad(&format!("幅は {WIDTH_MAX} までです")));
    }
    if s.prec.is_some_and(|p| p > PREC_MAX) {
        return Err(bad(&format!("精度は {PREC_MAX} までです")));
    }
    Ok(s)
}

/// 整数として読めるか（整数、または小数部のない数値）。符号と絶対値
fn integer_of(value: &Value) -> Option<(bool, u64)> {
    match value {
        Value::Int(n) => Some((*n < 0, n.unsigned_abs())),
        Value::Num(n) if n.fract() == 0.0 && n.abs() < 9_223_372_036_854_775_808.0 => {
            Some((*n < 0.0, n.abs() as u64))
        }
        _ => None,
    }
}

/// 整数部の 3 桁ごとに `,` を入れる（`digits` は数字だけの並び）
fn group(digits: &str) -> String {
    let mut out = String::new();
    for (k, c) in digits.chars().enumerate() {
        if k > 0 && (digits.len() - k) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn format_one(value: &Value, spec: &Spec) -> Result<String, String> {
    let mismatch = |t: char, want: &str| {
        format!(
            "str_format の型 {t} には{want}が必要です（{}が渡されました）",
            value.type_name()
        )
    };
    // (符号, 本体, 数値か)
    let (sign, body, numeric): (&str, String, bool) = match spec.ty {
        Some('s') => ("", truncate(value.to_string(), spec.prec), false),
        Some(t @ ('d' | 'x' | 'X' | 'b' | 'o')) => {
            let Some((neg, abs)) = integer_of(value) else {
                return Err(mismatch(t, "整数"));
            };
            if spec.prec.is_some() {
                return Err(format!("str_format の型 {t} に精度は付けられません"));
            }
            let text = match t {
                'd' => abs.to_string(),
                'x' => format!("{abs:x}"),
                'X' => format!("{abs:X}"),
                'b' => format!("{abs:b}"),
                _ => format!("{abs:o}"),
            };
            if spec.comma && t != 'd' {
                return Err(format!("str_format の型 {t} に , は付けられません"));
            }
            (if neg { "-" } else { "" }, text, true)
        }
        Some('f') => {
            let n = match value {
                Value::Num(n) => *n,
                Value::Int(n) => *n as f64,
                _ => return Err(mismatch('f', "数値")),
            };
            let text = format!("{:.*}", spec.prec.unwrap_or(6), n.abs());
            (
                if n.is_sign_negative() && !n.is_nan() {
                    "-"
                } else {
                    ""
                },
                text,
                true,
            )
        }
        _ => match value {
            Value::Num(_) | Value::Int(_) => {
                if spec.prec.is_some() {
                    return Err("str_format の数値に精度を付けるには型 f が必要です".into());
                }
                let text = value.to_string();
                match text.strip_prefix('-') {
                    Some(rest) => ("-", rest.to_string(), true),
                    None => ("", text, true),
                }
            }
            _ => ("", truncate(value.to_string(), spec.prec), false),
        },
    };
    let mut body = body;
    if spec.comma {
        if !numeric {
            return Err("str_format の , は数値にだけ付けられます".into());
        }
        let int_len = body.find('.').unwrap_or(body.len());
        if body[..int_len].chars().all(|c| c.is_ascii_digit()) {
            body = format!("{}{}", group(&body[..int_len]), &body[int_len..]);
        }
    }
    let used = sign.chars().count() + body.chars().count();
    let missing = spec.width.saturating_sub(used);
    if spec.zero {
        if !numeric {
            return Err("str_format の 0 は数値にだけ付けられます".into());
        }
        if spec.comma {
            return Err("str_format の 0 と , は一緒に使えません".into());
        }
        if spec.align.is_some() {
            return Err("str_format の 0 と配置（< > ^）は一緒に使えません".into());
        }
        return Ok(format!("{sign}{}{body}", "0".repeat(missing)));
    }
    let fill = spec.fill.unwrap_or(' ');
    let align = spec.align.unwrap_or(if numeric { '>' } else { '<' });
    let (left, right) = match align {
        '<' => (0, missing),
        '^' => (missing / 2, missing - missing / 2),
        _ => (missing, 0),
    };
    let rep = |n: usize| std::iter::repeat_n(fill, n).collect::<String>();
    Ok(format!("{}{sign}{body}{}", rep(left), rep(right)))
}

fn truncate(s: String, prec: Option<usize>) -> String {
    match prec {
        Some(p) => s.chars().take(p).collect(),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(t: &str, v: Vec<Value>) -> Result<String, String> {
        format_template(t, &v)
    }
    fn n(x: f64) -> Value {
        Value::Num(x)
    }
    fn s(x: &str) -> Value {
        Value::Str(x.into())
    }

    #[test]
    fn basics() {
        assert_eq!(f("{} と {}", vec![s("a"), n(1.0)]).unwrap(), "a と 1");
        assert_eq!(f("{1}{0}{1}", vec![s("a"), s("b")]).unwrap(), "bab");
        assert_eq!(f("{{}} {}", vec![n(2.5)]).unwrap(), "{} 2.5");
    }

    #[test]
    fn align_and_width() {
        assert_eq!(f("[{:>5}]", vec![s("ab")]).unwrap(), "[   ab]");
        assert_eq!(f("[{:<5}]", vec![n(7.0)]).unwrap(), "[7    ]");
        assert_eq!(f("[{:^6}]", vec![s("ab")]).unwrap(), "[  ab  ]");
        assert_eq!(f("[{:*^7}]", vec![s("ab")]).unwrap(), "[**ab***]");
        assert_eq!(f("[{:5}]", vec![n(7.0)]).unwrap(), "[    7]");
        assert_eq!(f("[{:5}]", vec![s("ab")]).unwrap(), "[ab   ]");
    }

    #[test]
    fn numbers() {
        assert_eq!(f("{:.2f}", vec![n(3.14159)]).unwrap(), "3.14");
        assert_eq!(f("{:8.2f}", vec![n(-3.14159)]).unwrap(), "   -3.14");
        assert_eq!(f("{:,}", vec![n(1234567.0)]).unwrap(), "1,234,567");
        assert_eq!(f("{:,.1f}", vec![n(-1234.56)]).unwrap(), "-1,234.6");
        assert_eq!(f("{:05d}", vec![n(42.0)]).unwrap(), "00042");
        assert_eq!(f("{:05d}", vec![n(-42.0)]).unwrap(), "-0042");
        assert_eq!(
            f("{:x} {:X} {:b} {:o}", vec![n(255.0); 4]).unwrap(),
            "ff FF 11111111 377"
        );
        assert_eq!(f("{:d}", vec![Value::Int(-5)]).unwrap(), "-5");
        assert_eq!(f("{:.3}", vec![s("abcdef")]).unwrap(), "abc");
    }

    #[test]
    fn errors() {
        assert!(f("{", vec![]).is_err());
        assert!(f("}", vec![]).is_err());
        assert!(f("{}", vec![]).is_err());
        assert!(f("{}{0}", vec![n(1.0), n(2.0)]).is_err());
        assert!(f("{:d}", vec![n(1.5)]).is_err());
        assert!(f("{:f}", vec![s("x")]).is_err());
        assert!(f("{:q}", vec![n(1.0)]).is_err());
        assert!(f("{:,}", vec![s("x")]).is_err());
        assert!(f("{:05}", vec![s("x")]).is_err());
        assert!(f("{:9999999999}", vec![n(1.0)]).is_err());
    }
}
