//! `std:time`: now, monotonic, sleep, date, timestamp, format, parse, iso, parse_iso
//! （design.md 5.13）と、is_leap, days_in_month, add_months, start_of（5.30）、
//! local_offset, diff, add（5.50）
//!
//! 時刻は Unix 時刻（秒。小数）。暦はグレゴリオ暦で、年は 1〜9999、うるう秒は扱わない。
//! 時差は「UTC からの分」で、省略は UTC。外部クレートは使わず、暦の計算は自前

mod args;
mod arith;
mod calendar;
mod format;
mod offset;
mod parse;

use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{error_value, new_map, num_arg, type_error};
use crate::runtime::interp::Interpreter;
use crate::runtime::map::Map;
use crate::runtime::value::Value;

use args::{check_count, offset_arg};
use arith::{add, add_months, days_in_month_fn, diff, is_leap_fn, start_of};
use calendar::{civil_from_ts, ts_from_fields};
use format::{format, iso};
use offset::local_offset;
use parse::{parse, parse_iso};

pub(super) use calendar::iso_utc_ms;
#[cfg(feature = "postgres")]
pub(crate) use calendar::{civil_from_days, days_from_civil};

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register_module(
        "std:time",
        &[
            ("time.now", now),
            ("time.monotonic", monotonic),
            ("time.sleep", sleep),
            ("time.date", date),
            ("time.timestamp", timestamp),
            ("time.format", format),
            ("time.parse", parse),
            ("time.iso", iso),
            ("time.parse_iso", parse_iso),
            ("time.is_leap", is_leap_fn),
            ("time.days_in_month", days_in_month_fn),
            ("time.add_months", add_months),
            ("time.start_of", start_of),
            ("time.local_offset", local_offset),
            ("time.diff", diff),
            ("time.add", add),
        ],
    );
}

// ---- 関数 ----

/// 現在時刻（ミリ秒の精度）
fn now(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.now", args, 0, 0)?;
    let ms = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_millis() as f64,
        Err(before) => -(before.duration().as_millis() as f64),
    };
    Ok(Value::Num(ms / 1000.0))
}

/// 経過時間の計測用の秒。起点は不定で、戻らない
fn monotonic(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.monotonic", args, 0, 0)?;
    static START: OnceLock<Instant> = OnceLock::new();
    Ok(Value::Num(
        START.get_or_init(Instant::now).elapsed().as_secs_f64(),
    ))
}

/// 待つ。インタプリタのスレッドが止まる
fn sleep(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.sleep", args, 1, 1)?;
    let seconds = num_arg("time.sleep", args, 0)?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(format!("time.sleep の秒数は 0 以上の数です（{seconds}）"));
    }
    // 待つ前に、ここまでの出力を見せる
    interp.out.flush().map_err(|e| e.to_string())?;
    thread::sleep(Duration::try_from_secs_f64(seconds).unwrap_or(Duration::MAX));
    Ok(Value::Nil)
}

fn date(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.date", args, 1, 2)?;
    let ts = num_arg("time.date", args, 0)?;
    let c = civil_from_ts(ts, offset_arg("time.date", args, 1)?)
        .map_err(|e| format!("time.date: {e}"))?;
    let mut map = Map::default();
    for (key, value) in [
        ("year", c.year),
        ("month", c.month),
        ("day", c.day),
        ("hour", c.hour),
        ("minute", c.minute),
        ("second", c.second),
        ("millisecond", c.millisecond),
        ("weekday", c.weekday),
        ("yday", c.yday),
    ] {
        map.insert(key.into(), Value::Num(value as f64));
    }
    Ok(new_map(map))
}

/// `date` の逆。範囲外は `parse` のエラー値
fn timestamp(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.timestamp", args, 1, 2)?;
    let Value::Map(fields) = &args[0] else {
        return Err(type_error("time.timestamp", 0, "マップ", &args[0]));
    };
    let fields = fields.borrow();
    const KEYS: [&str; 7] = [
        "year",
        "month",
        "day",
        "hour",
        "minute",
        "second",
        "millisecond",
    ];
    for (key, _) in fields.iter() {
        // `date` の結果をそのまま渡せるよう、`weekday` と `yday` は受け取って無視する
        if !KEYS.contains(&&**key) && !matches!(&**key, "weekday" | "yday") {
            return Err(format!(
                "time.timestamp に知らない項目 `{key}` があります（使えるのは {}）",
                KEYS.join("、")
            ));
        }
    }
    let field = |key: &str, default: Option<i64>| -> Result<i64, String> {
        match fields.get(key) {
            Some(Value::Num(n)) if n.fract() == 0.0 && n.abs() < 1e15 => Ok(*n as i64),
            Some(other) => Err(format!(
                "time.timestamp の `{key}` は整数です（{}が渡されました）",
                other.type_name()
            )),
            None => default.ok_or_else(|| format!("time.timestamp に `{key}` がありません")),
        }
    };
    let date = (
        field("year", None)?,
        field("month", None)?,
        field("day", None)?,
    );
    let time = (
        field("hour", Some(0))?,
        field("minute", Some(0))?,
        field("second", Some(0))?,
        field("millisecond", Some(0))?,
    );
    let offset = offset_arg("time.timestamp", args, 1)?;
    Ok(match ts_from_fields(date, time, offset) {
        Ok(ts) => Value::Num(ts),
        Err(message) => error_value("parse", format!("日付と時刻が正しくありません: {message}")),
    })
}
