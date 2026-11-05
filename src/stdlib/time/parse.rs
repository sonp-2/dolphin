//! 読み取り（`time.parse` と `time.parse_iso`）

use super::super::{error_value, str_arg};
use super::args::check_count;
use super::calendar::{MONTHS, WEEKDAYS, ts_from_fields};
use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;

/// 読み取った項目。`None` は書式に出てこなかった項目
#[derive(Default)]
struct Parsed {
    year: Option<i64>,
    year2: Option<i64>,
    month: Option<i64>,
    day: Option<i64>,
    hour: Option<i64>,
    hour12: Option<i64>,
    pm: Option<bool>,
    minute: Option<i64>,
    second: Option<i64>,
    millisecond: Option<i64>,
    offset: Option<i64>,
}

/// 入力を先頭から読み進めるカーソル
struct Cursor<'a> {
    rest: &'a str,
}

impl Cursor<'_> {
    fn eat(&mut self, expected: char) -> bool {
        if self.rest.starts_with(expected) {
            self.rest = &self.rest[expected.len_utf8()..];
            true
        } else {
            false
        }
    }

    /// ASCII の数字を `min`〜`max` 桁読む
    fn digits(&mut self, min: usize, max: usize) -> Result<i64, String> {
        let len = self
            .rest
            .bytes()
            .take(max)
            .take_while(u8::is_ascii_digit)
            .count();
        if len < min {
            return Err(format!("数字が必要です（`{}` の位置）", self.preview()));
        }
        let (digits, rest) = self.rest.split_at(len);
        self.rest = rest;
        Ok(digits.parse().expect("数字だけなので必ず数値になる"))
    }

    /// 名前の一覧から、大文字小文字を区別せずに 1 つ読む。長い名前（全部）を先に、
    /// なければ 3 文字の略称。戻り値は一覧の位置
    fn name(&mut self, names: &[&str]) -> Result<usize, String> {
        for len in [None, Some(3)] {
            for (i, name) in names.iter().enumerate() {
                let candidate = &name[..len.unwrap_or(name.len())];
                if let Some(head) = self.rest.get(..candidate.len())
                    && head.eq_ignore_ascii_case(candidate)
                {
                    self.rest = &self.rest[candidate.len()..];
                    return Ok(i);
                }
            }
        }
        Err(format!("名前が必要です（`{}` の位置）", self.preview()))
    }

    /// `Z`、`+0900`、`+09:00`、`+09` を読んで、時差（分）にする
    fn zone(&mut self) -> Result<i64, String> {
        if self.eat('Z') || self.eat('z') {
            return Ok(0);
        }
        let sign = if self.eat('+') {
            1
        } else if self.eat('-') {
            -1
        } else {
            return Err(format!(
                "時差（`Z` か `+hh:mm`）が必要です（`{}` の位置）",
                self.preview()
            ));
        };
        let hours = self.digits(2, 2)?;
        self.eat(':');
        let minutes = if self.rest.starts_with(|c: char| c.is_ascii_digit()) {
            self.digits(2, 2)?
        } else {
            0
        };
        if hours > 23 || minutes > 59 {
            return Err("時差が正しくありません".into());
        }
        Ok(sign * (hours * 60 + minutes))
    }

    /// エラーメッセージ用の、いまの位置から先の少しの文字
    fn preview(&self) -> String {
        if self.rest.is_empty() {
            "終わり".to_string()
        } else {
            self.rest.chars().take(6).collect()
        }
    }
}

/// 書式の中身が正しいか。`parse` では使えない `%j` も、ここで弾く
fn check_parse_pattern(pattern: &str) -> Result<(), String> {
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            continue;
        }
        match chars.next() {
            Some(
                'Y' | 'y' | 'm' | 'd' | 'e' | 'H' | 'I' | 'M' | 'S' | 'f' | 'p' | 'a' | 'A' | 'b'
                | 'B' | 'z' | '%',
            ) => {}
            Some('j') => return Err("書式の `%j` は parse では使えません".into()),
            Some(other) => return Err(format!("書式の `%{other}` は使えません")),
            None => return Err("書式が `%` で終わっています".into()),
        }
    }
    Ok(())
}

/// 書式に従って入力を読む。合わなければ、理由を返す
fn parse_with(input: &str, pattern: &str) -> Result<Parsed, String> {
    let mut cursor = Cursor { rest: input };
    let mut parsed = Parsed::default();
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            if !cursor.eat(ch) {
                return Err(format!(
                    "`{ch}` が必要です（`{}` の位置）",
                    cursor.preview()
                ));
            }
            continue;
        }
        match chars.next().expect("書式は検査済み") {
            'Y' => parsed.year = Some(cursor.digits(4, 4)?),
            'y' => parsed.year2 = Some(cursor.digits(2, 2)?),
            'm' => parsed.month = Some(cursor.digits(1, 2)?),
            'd' => parsed.day = Some(cursor.digits(1, 2)?),
            'e' => {
                while cursor.eat(' ') {}
                parsed.day = Some(cursor.digits(1, 2)?);
            }
            'H' => parsed.hour = Some(cursor.digits(1, 2)?),
            'I' => parsed.hour12 = Some(cursor.digits(1, 2)?),
            'M' => parsed.minute = Some(cursor.digits(1, 2)?),
            'S' => parsed.second = Some(cursor.digits(1, 2)?),
            'f' => {
                let len = cursor.rest.bytes().take_while(u8::is_ascii_digit).count();
                if len == 0 {
                    return Err(format!("数字が必要です（`{}` の位置）", cursor.preview()));
                }
                // 3 桁（ミリ秒）に切り詰めるか、0 で埋める
                let frac = format!("{:0<3}", &cursor.rest[..len]);
                parsed.millisecond = Some(frac[..3].parse().expect("数字だけ"));
                cursor.rest = &cursor.rest[len..];
            }
            'p' => {
                let head = cursor.rest.get(..2).map(str::to_ascii_uppercase);
                match head.as_deref() {
                    Some("AM") => parsed.pm = Some(false),
                    Some("PM") => parsed.pm = Some(true),
                    _ => {
                        return Err(format!(
                            "`AM` か `PM` が必要です（`{}` の位置）",
                            cursor.preview()
                        ));
                    }
                }
                cursor.rest = &cursor.rest[2..];
            }
            'a' | 'A' => {
                cursor.name(&WEEKDAYS)?;
            }
            'b' | 'B' => parsed.month = Some(cursor.name(&MONTHS)? as i64 + 1),
            'z' => parsed.offset = Some(cursor.zone()?),
            '%' => {
                if !cursor.eat('%') {
                    return Err(format!("`%` が必要です（`{}` の位置）", cursor.preview()));
                }
            }
            _ => unreachable!("書式は検査済み"),
        }
    }
    if !cursor.rest.is_empty() {
        return Err(format!("余分な文字があります（`{}`）", cursor.preview()));
    }
    Ok(parsed)
}

/// 読み取った項目から Unix 時刻にする。出てこなかった項目は、年月日が 1970-01-01、時刻が 0
fn ts_from_parsed(p: &Parsed) -> Result<f64, String> {
    let year = match (p.year, p.year2) {
        (Some(y), _) => y,
        // 2 桁の年は、69〜99 が 1969〜1999、00〜68 が 2000〜2068（POSIX と同じ）
        (None, Some(y)) => {
            if y >= 69 {
                1900 + y
            } else {
                2000 + y
            }
        }
        (None, None) => 1970,
    };
    let hour = match (p.hour, p.hour12) {
        (Some(h), _) => h,
        (None, Some(h)) if (1..=12).contains(&h) => {
            h % 12 + if p.pm == Some(true) { 12 } else { 0 }
        }
        (None, Some(h)) => return Err(format!("時 {h} は 1〜12 の範囲外です")),
        (None, None) => 0,
    };
    ts_from_fields(
        (year, p.month.unwrap_or(1), p.day.unwrap_or(1)),
        (
            hour,
            p.minute.unwrap_or(0),
            p.second.unwrap_or(0),
            p.millisecond.unwrap_or(0),
        ),
        p.offset.unwrap_or(0),
    )
}

pub(super) fn parse(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.parse", args, 2, 2)?;
    let input = str_arg("time.parse", args, 0)?;
    let pattern = str_arg("time.parse", args, 1)?;
    check_parse_pattern(pattern).map_err(|e| format!("time.parse: {e}"))?;
    let result = parse_with(input, pattern).and_then(|p| ts_from_parsed(&p));
    Ok(match result {
        Ok(ts) => Value::Num(ts),
        Err(message) => error_value(
            "parse",
            format!("{input:?} を `{pattern}` として読めません: {message}"),
        ),
    })
}

/// RFC 3339 を読む。`T` の代わりに小文字の `t` か空白も許す。時差は必須
pub(super) fn parse_iso(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.parse_iso", args, 1, 1)?;
    let input = str_arg("time.parse_iso", args, 0)?;
    let result = parse_iso_str(input);
    Ok(match result {
        Ok(ts) => Value::Num(ts),
        Err(message) => error_value(
            "parse",
            format!("{input:?} を RFC 3339 として読めません: {message}"),
        ),
    })
}

fn parse_iso_str(input: &str) -> Result<f64, String> {
    let mut cursor = Cursor { rest: input };
    let mut p = Parsed {
        year: Some(cursor.digits(4, 4)?),
        ..Parsed::default()
    };
    expect(&mut cursor, '-')?;
    p.month = Some(cursor.digits(2, 2)?);
    expect(&mut cursor, '-')?;
    p.day = Some(cursor.digits(2, 2)?);
    if !(cursor.eat('T') || cursor.eat('t') || cursor.eat(' ')) {
        return Err(format!("`T` が必要です（`{}` の位置）", cursor.preview()));
    }
    p.hour = Some(cursor.digits(2, 2)?);
    expect(&mut cursor, ':')?;
    p.minute = Some(cursor.digits(2, 2)?);
    expect(&mut cursor, ':')?;
    p.second = Some(cursor.digits(2, 2)?);
    if cursor.eat('.') {
        let len = cursor.rest.bytes().take_while(u8::is_ascii_digit).count();
        if len == 0 {
            return Err(format!("数字が必要です（`{}` の位置）", cursor.preview()));
        }
        let frac = format!("{:0<3}", &cursor.rest[..len]);
        p.millisecond = Some(frac[..3].parse().expect("数字だけ"));
        cursor.rest = &cursor.rest[len..];
    }
    p.offset = Some(cursor.zone()?);
    if !cursor.rest.is_empty() {
        return Err(format!("余分な文字があります（`{}`）", cursor.preview()));
    }
    ts_from_parsed(&p)
}

fn expect(cursor: &mut Cursor<'_>, ch: char) -> Result<(), String> {
    if cursor.eat(ch) {
        Ok(())
    } else {
        Err(format!(
            "`{ch}` が必要です（`{}` の位置）",
            cursor.preview()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(input: &str, pattern: &str) -> Result<f64, String> {
        check_parse_pattern(pattern)?;
        parse_with(input, pattern).and_then(|p| ts_from_parsed(&p))
    }

    #[test]
    fn parsing() {
        assert_eq!(
            parsed("2023-11-14 22:13:20", "%Y-%m-%d %H:%M:%S"),
            Ok(1_700_000_000.0)
        );
        assert_eq!(
            parsed("2023-11-14 22:13:20 +0900", "%Y-%m-%d %H:%M:%S %z"),
            Ok(1_700_000_000.0 - 32_400.0)
        );
        assert_eq!(parsed("14 nov 2023", "%d %b %Y"), Ok(1_699_920_000.0));
        assert_eq!(
            parsed("Tuesday, November 14, 2023", "%A, %B %d, %Y"),
            Ok(1_699_920_000.0)
        );
        assert_eq!(parsed("10:13:20.5 PM", "%I:%M:%S.%f %p"), Ok(80_000.5));
        assert_eq!(parsed("12:00 AM", "%I:%M %p"), Ok(0.0));
        assert_eq!(parsed("12:00 PM", "%I:%M %p"), Ok(43_200.0));
        assert_eq!(parsed(" 5", "%e"), Ok(345_600.0));
        // 年がなければ 1970 年
        assert_eq!(parsed("01-02", "%m-%d"), Ok(86_400.0));
        // 2 桁の年の境目
        assert_eq!(parsed("68", "%y"), parsed("2068", "%Y"));
        assert_eq!(parsed("69", "%y"), parsed("1969", "%Y"));
        // %f は 3 桁に切り詰める・埋める
        assert_eq!(parsed("1", "%f"), Ok(0.1));
        assert_eq!(parsed("12345", "%f"), Ok(0.123));
    }

    #[test]
    fn parse_failures() {
        assert!(parsed("2023-13-01", "%Y-%m-%d").is_err());
        assert!(parsed("2023-02-30", "%Y-%m-%d").is_err());
        assert!(parsed("2023-11", "%Y-%m-%d").is_err());
        assert!(parsed("2023-11-14x", "%Y-%m-%d").is_err());
        assert!(parsed("x", "%Y").is_err());
        assert!(parsed("13:00 PM", "%I:%M %p").is_err());
        assert!(parsed("2023", "%j").is_err());
        assert!(parsed("1", "%Q").is_err());
    }

    #[test]
    fn iso_roundtrip() {
        assert_eq!(parse_iso_str("2023-11-14T22:13:20Z"), Ok(1_700_000_000.0));
        assert_eq!(parse_iso_str("2023-11-14t22:13:20z"), Ok(1_700_000_000.0));
        assert_eq!(parse_iso_str("2023-11-14 22:13:20Z"), Ok(1_700_000_000.0));
        assert_eq!(
            parse_iso_str("2023-11-15T07:13:20+09:00"),
            Ok(1_700_000_000.0)
        );
        assert_eq!(
            parse_iso_str("2023-11-14T22:13:20.5-00:00"),
            Ok(1_700_000_000.5)
        );
        assert_eq!(
            parse_iso_str("2023-11-14T22:13:20.123456Z"),
            Ok(1_700_000_000.123)
        );
        assert!(parse_iso_str("2023-11-14T22:13:20").is_err());
        assert!(parse_iso_str("2023-11-14").is_err());
        assert!(parse_iso_str("2023-11-14T22:13:20+9:00").is_err());
        assert!(parse_iso_str("2023-11-14T25:00:00Z").is_err());
        assert!(parse_iso_str("2023-11-14T22:13:20Zx").is_err());
    }
}
