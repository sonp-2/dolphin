//! 書式（`time.format` と `time.iso`）

use super::super::{num_arg, str_arg};
use super::args::{check_count, offset_arg};
use super::calendar::{Civil, MONTHS, WEEKDAYS, civil_from_ts};
use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;

pub(super) fn format(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.format", args, 2, 3)?;
    let ts = num_arg("time.format", args, 0)?;
    let pattern = str_arg("time.format", args, 1)?;
    let offset = offset_arg("time.format", args, 2)?;
    let c = civil_from_ts(ts, offset).map_err(|e| format!("time.format: {e}"))?;
    let text = format_civil(&c, offset, pattern).map_err(|e| format!("time.format: {e}"))?;
    Ok(Value::Str(text.into()))
}

fn format_civil(c: &Civil, offset: i64, pattern: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        let Some(spec) = chars.next() else {
            return Err("書式が `%` で終わっています".into());
        };
        let hour12 = if c.hour % 12 == 0 { 12 } else { c.hour % 12 };
        let piece = match spec {
            'Y' => format!("{:04}", c.year),
            'y' => format!("{:02}", c.year % 100),
            'm' => format!("{:02}", c.month),
            'd' => format!("{:02}", c.day),
            'e' => format!("{:2}", c.day),
            'j' => format!("{:03}", c.yday),
            'H' => format!("{:02}", c.hour),
            'I' => format!("{hour12:02}"),
            'M' => format!("{:02}", c.minute),
            'S' => format!("{:02}", c.second),
            'f' => format!("{:03}", c.millisecond),
            'p' => (if c.hour < 12 { "AM" } else { "PM" }).to_string(),
            'a' => WEEKDAYS[c.weekday as usize][..3].to_string(),
            'A' => WEEKDAYS[c.weekday as usize].to_string(),
            'b' => MONTHS[c.month as usize - 1][..3].to_string(),
            'B' => MONTHS[c.month as usize - 1].to_string(),
            'z' => {
                let sign = if offset < 0 { '-' } else { '+' };
                format!("{sign}{:02}{:02}", offset.abs() / 60, offset.abs() % 60)
            }
            '%' => "%".to_string(),
            other => return Err(format!("書式の `%{other}` は使えません")),
        };
        out.push_str(&piece);
    }
    Ok(out)
}

/// RFC 3339 の文字列にする。ミリ秒が 0 なら小数は省く
pub(super) fn iso(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.iso", args, 1, 2)?;
    let ts = num_arg("time.iso", args, 0)?;
    let offset = offset_arg("time.iso", args, 1)?;
    let c = civil_from_ts(ts, offset).map_err(|e| format!("time.iso: {e}"))?;
    let fraction = if c.millisecond == 0 { "" } else { ".%f" };
    let zone = if offset == 0 { "Z" } else { "%z" };
    let pattern = format!("%Y-%m-%dT%H:%M:%S{fraction}{zone}");
    let mut text = format_civil(&c, offset, &pattern)?;
    if offset != 0 {
        // `+0900` を `+09:00` にする
        text.insert(text.len() - 2, ':');
    }
    Ok(Value::Str(text.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::stdlib::time::calendar::civil_from_ts;

    fn fmt(ts: f64, offset: i64, pattern: &str) -> String {
        format_civil(&civil_from_ts(ts, offset).unwrap(), offset, pattern).unwrap()
    }

    #[test]
    fn formatting() {
        let ts = 1_700_000_000.5;
        assert_eq!(
            fmt(ts, 0, "%Y-%m-%d %H:%M:%S.%f"),
            "2023-11-14 22:13:20.500"
        );
        assert_eq!(
            fmt(ts, 0, "%a %A %b %B %e %j %y %p %I %%"),
            "Tue Tuesday Nov November 14 318 23 PM 10 %"
        );
        assert_eq!(fmt(ts, 540, "%H:%M %z"), "07:13 +0900");
        assert_eq!(fmt(ts, -330, "%z"), "-0530");
        assert_eq!(fmt(0.0, 0, "%I %p"), "12 AM");
        assert_eq!(fmt(43_200.0, 0, "%I %p"), "12 PM");
        assert_eq!(fmt(0.0, 0, "%e"), " 1");
        assert!(format_civil(&civil_from_ts(0.0, 0).unwrap(), 0, "%Q").is_err());
        assert!(format_civil(&civil_from_ts(0.0, 0).unwrap(), 0, "%").is_err());
    }
}
