//! 日付の計算（`is_leap`・`days_in_month`・`add_months`・`start_of`・`diff`・`add`）

use super::super::{error_value, int_arg, num_arg, str_arg, type_error};
use super::args::{check_count, offset_arg, year_arg};
use super::calendar::{civil_from_ts, days_in_month, is_leap, ts_from_fields};
use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;

pub(super) fn is_leap_fn(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.is_leap", args, 1, 1)?;
    Ok(Value::Bool(is_leap(year_arg("time.is_leap", args, 0)?)))
}

pub(super) fn days_in_month_fn(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.days_in_month", args, 2, 2)?;
    let year = year_arg("time.days_in_month", args, 0)?;
    let month = int_arg("time.days_in_month", args, 1)?;
    if !(1..=12).contains(&month) {
        return Err(format!(
            "time.days_in_month の月は 1〜12 の整数です（{month}）"
        ));
    }
    Ok(Value::Num(days_in_month(year, month) as f64))
}

/// `n` か月後。日が月末を超えるなら月末にして、時刻は変えない
pub(super) fn add_months(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.add_months", args, 2, 3)?;
    let ts = num_arg("time.add_months", args, 0)?;
    let n = int_arg("time.add_months", args, 1)?;
    let offset = offset_arg("time.add_months", args, 2)?;
    let c = civil_from_ts(ts, offset).map_err(|e| format!("time.add_months: {e}"))?;
    let total = c.year * 12 + (c.month - 1) + n;
    let (year, month) = (total.div_euclid(12), total.rem_euclid(12) + 1);
    if !(1..=9999).contains(&year) {
        return Ok(error_value(
            "parse",
            format!("{n} か月後は扱える範囲（1〜9999 年）を超えます"),
        ));
    }
    let day = c.day.min(days_in_month(year, month));
    Ok(
        match ts_from_fields(
            (year, month, day),
            (c.hour, c.minute, c.second, c.millisecond),
            offset,
        ) {
            Ok(ts) => Value::Num(ts),
            Err(message) => error_value("parse", message),
        },
    )
}

/// `unit` の始まりの時刻
pub(super) fn start_of(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.start_of", args, 2, 3)?;
    let ts = num_arg("time.start_of", args, 0)?;
    let unit = str_arg("time.start_of", args, 1)?;
    let offset = offset_arg("time.start_of", args, 2)?;
    let c = civil_from_ts(ts, offset).map_err(|e| format!("time.start_of: {e}"))?;
    let (date, time) = match unit {
        "year" => ((c.year, 1, 1), (0, 0, 0, 0)),
        "month" => ((c.year, c.month, 1), (0, 0, 0, 0)),
        "day" => ((c.year, c.month, c.day), (0, 0, 0, 0)),
        "hour" => ((c.year, c.month, c.day), (c.hour, 0, 0, 0)),
        "minute" => ((c.year, c.month, c.day), (c.hour, c.minute, 0, 0)),
        "second" => ((c.year, c.month, c.day), (c.hour, c.minute, c.second, 0)),
        _ => {
            return Err(format!(
                "time.start_of の単位は \"year\" \"month\" \"day\" \"hour\" \"minute\" \"second\" のどれかです（{unit:?}）"
            ));
        }
    };
    ts_from_fields(date, time, offset)
        .map(Value::Num)
        .map_err(|e| format!("time.start_of: {e}"))
}

/// 単位の名前を、その秒数にする（単数形も可）
fn unit_seconds(unit: &str) -> Option<f64> {
    Some(match unit {
        "millisecond" | "milliseconds" => 0.001,
        "second" | "seconds" => 1.0,
        "minute" | "minutes" => 60.0,
        "hour" | "hours" => 3600.0,
        "day" | "days" => 86_400.0,
        "week" | "weeks" => 604_800.0,
        _ => return None,
    })
}

/// `a - b` を `unit` で数える
pub(super) fn diff(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.diff", args, 3, 3)?;
    let a = num_arg("time.diff", args, 0)?;
    let b = num_arg("time.diff", args, 1)?;
    let unit = str_arg("time.diff", args, 2)?;
    let Some(seconds) = unit_seconds(unit) else {
        return Err(format!(
            "time.diff の単位は \"milliseconds\" \"seconds\" \"minutes\" \"hours\" \"days\" \"weeks\" のどれかです（{unit:?}）"
        ));
    };
    Ok(Value::Num((a - b) / seconds))
}

/// 年・月・週・日・時・分・秒・ミリ秒を足す
pub(super) fn add(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.add", args, 2, 3)?;
    let ts = num_arg("time.add", args, 0)?;
    let Value::Map(delta) = &args[1] else {
        return Err(type_error("time.add", 1, "マップ", &args[1]));
    };
    let offset = offset_arg("time.add", args, 2)?;
    let (mut months, mut seconds) = (0i64, 0.0f64);
    for (key, value) in delta.borrow().iter() {
        let Value::Num(n) = value else {
            return Err(format!(
                "time.add の `{key}` は数値です（{}が渡されました）",
                value.type_name()
            ));
        };
        if !n.is_finite() {
            return Err(format!("time.add の `{key}` は有限の数値です（{n}）"));
        }
        match &**key {
            "years" | "months" => {
                if n.fract() != 0.0 || n.abs() > 1e6 {
                    return Err(format!(
                        "time.add の `{key}` は整数です（絶対値 1000000 以下。{n} が渡されました）"
                    ));
                }
                months += *n as i64 * if &**key == "years" { 12 } else { 1 };
            }
            other => match unit_seconds(other).filter(|_| other.ends_with('s')) {
                Some(unit) => seconds += n * unit,
                None => {
                    return Err(format!(
                        "time.add に知らない項目 `{key}` があります（使えるのは years、months、weeks、days、hours、minutes、seconds、milliseconds）"
                    ));
                }
            },
        }
    }
    let mut result = ts;
    if months != 0 {
        let c = civil_from_ts(result, offset).map_err(|e| format!("time.add: {e}"))?;
        let total = c.year * 12 + (c.month - 1) + months;
        let (year, month) = (total.div_euclid(12), total.rem_euclid(12) + 1);
        if !(1..=9999).contains(&year) {
            return Ok(error_value(
                "parse",
                "足した結果が扱える範囲（1〜9999 年）を超えます",
            ));
        }
        let day = c.day.min(days_in_month(year, month));
        match ts_from_fields(
            (year, month, day),
            (c.hour, c.minute, c.second, c.millisecond),
            offset,
        ) {
            Ok(t) => result = t,
            Err(message) => return Ok(error_value("parse", message)),
        }
    }
    result += seconds;
    // 結果が 1〜9999 年に収まるか（`date` などが受け付ける範囲）
    if civil_from_ts(result, 0).is_err() {
        return Ok(error_value(
            "parse",
            "足した結果が扱える範囲（1〜9999 年）を超えます",
        ));
    }
    Ok(Value::Num(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units() {
        assert_eq!(unit_seconds("days"), Some(86_400.0));
        assert_eq!(unit_seconds("day"), Some(86_400.0));
        assert_eq!(unit_seconds("milliseconds"), Some(0.001));
        assert_eq!(unit_seconds("years"), None);
    }
}
