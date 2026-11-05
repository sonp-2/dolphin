//! OS のタイムゾーン（`time.local_offset`）。Windows と Unix で取り方が違う

use std::time::{SystemTime, UNIX_EPOCH};

use super::super::{error_value, num_arg};
use super::args::check_count;
use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;

/// OS のタイムゾーンの、UTC からの分。`ts` の時点の値（夏時間を反映）
#[cfg(windows)]
fn os_offset_minutes(ts: f64) -> Result<i64, String> {
    use super::calendar::{civil_from_ts, days_from_civil};
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct WinTime {
        year: u16,
        month: u16,
        weekday: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        millisecond: u16,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SystemTimeToTzSpecificLocalTime(
            zone: *const std::ffi::c_void,
            utc: *const WinTime,
            local: *mut WinTime,
        ) -> i32;
    }
    // Windows が扱えるのは 1601 年から。範囲外は今の値にする
    let c = match civil_from_ts(ts, 0) {
        Ok(c) if c.year >= 1601 => c,
        _ => civil_from_ts(now_secs(), 0)?,
    };
    let utc = WinTime {
        year: c.year as u16,
        month: c.month as u16,
        weekday: c.weekday as u16,
        day: c.day as u16,
        hour: c.hour as u16,
        minute: c.minute as u16,
        second: c.second as u16,
        millisecond: 0,
    };
    let mut local = WinTime::default();
    // SAFETY: 渡すのはどちらも、このスタック上の正しく初期化された構造体。null の zone は「今のタイムゾーン」
    let ok = unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) };
    if ok == 0 {
        return Err("OS のタイムゾーンを取得できませんでした".into());
    }
    let minutes = |t: &WinTime| {
        days_from_civil(i64::from(t.year), i64::from(t.month), i64::from(t.day)) * 1440
            + i64::from(t.hour) * 60
            + i64::from(t.minute)
    };
    Ok(minutes(&local) - minutes(&utc))
}

#[cfg(all(unix, target_pointer_width = "64"))]
fn os_offset_minutes(ts: f64) -> Result<i64, String> {
    use std::ffi::{c_char, c_int, c_long};
    #[repr(C)]
    struct Tm {
        sec: c_int,
        min: c_int,
        hour: c_int,
        mday: c_int,
        mon: c_int,
        year: c_int,
        wday: c_int,
        yday: c_int,
        isdst: c_int,
        gmtoff: c_long,
        zone: *const c_char,
    }
    unsafe extern "C" {
        fn localtime_r(time: *const i64, out: *mut Tm) -> *mut Tm;
    }
    let time = ts.floor() as i64;
    let mut tm = Tm {
        sec: 0,
        min: 0,
        hour: 0,
        mday: 0,
        mon: 0,
        year: 0,
        wday: 0,
        yday: 0,
        isdst: 0,
        gmtoff: 0,
        zone: std::ptr::null(),
    };
    // SAFETY: どちらもこのスタック上の正しい構造体。localtime_r はスレッドセーフ
    let result = unsafe { localtime_r(&time, &mut tm) };
    if result.is_null() {
        return Err("OS のタイムゾーンを取得できませんでした".into());
    }
    // c_long は 64 ビット Unix では i64、32 ビットでは i32 なので、変換は消さない
    #[allow(clippy::useless_conversion)]
    let gmtoff = i64::from(tm.gmtoff);
    Ok(gmtoff / 60)
}

#[cfg(not(any(windows, all(unix, target_pointer_width = "64"))))]
fn os_offset_minutes(_ts: f64) -> Result<i64, String> {
    Err("この環境ではタイムゾーンを取得できません".into())
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

pub(super) fn local_offset(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    check_count("time.local_offset", args, 0, 1)?;
    let ts = if args.is_empty() {
        now_secs()
    } else {
        let ts = num_arg("time.local_offset", args, 0)?;
        if !ts.is_finite() {
            return Err(format!("time.local_offset の時刻 {ts} は使えません"));
        }
        ts
    };
    Ok(match os_offset_minutes(ts) {
        Ok(minutes) => Value::Num(minutes as f64),
        Err(message) => error_value("io", message),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_offset_is_a_valid_offset() {
        // 環境で値は変わるが、時差として使える範囲に収まる
        for ts in [0.0, 1_700_000_000.0, 1_750_000_000.5, -1.0e9, 3.0e11] {
            let minutes = os_offset_minutes(ts).unwrap();
            assert!(minutes.abs() < 1440, "{ts}: {minutes}");
        }
    }
}
