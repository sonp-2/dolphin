//! 暦の計算（日数と年月日の変換、うるう年、月末、日付と時刻への分解）

pub(super) const MS_PER_DAY: i64 = 86_400_000;
pub(super) const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
pub(super) const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

// ---- 暦の計算（Howard Hinnant の civil_from_days / days_from_civil） ----

/// 1970-01-01 からの日数
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 1970-01-01 からの日数を (年, 月, 日) にする
pub(crate) fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

pub(super) fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub(super) fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// 日付と時刻に分けたもの
#[derive(Debug, PartialEq)]
pub(super) struct Civil {
    pub(super) year: i64,
    pub(super) month: i64,
    pub(super) day: i64,
    pub(super) hour: i64,
    pub(super) minute: i64,
    pub(super) second: i64,
    pub(super) millisecond: i64,
    /// 0（日曜）〜6
    pub(super) weekday: i64,
    /// 1 月 1 日が 1
    pub(super) yday: i64,
}

/// Unix 時刻（秒）を、時差（分）をつけた日付と時刻にする
pub(super) fn civil_from_ts(ts: f64, offset: i64) -> Result<Civil, String> {
    let ms = (ts * 1000.0).round();
    // 1〜9999 年に収まらない値は、ここで弾く（i64 に変換できる範囲でもある）
    if !ms.is_finite() || ms.abs() > 3.0e14 {
        return Err(format!(
            "時刻 {ts} は扱える範囲（1〜9999 年）を超えています"
        ));
    }
    let local = ms as i64 + offset * 60_000;
    let days = local.div_euclid(MS_PER_DAY);
    let in_day = local.rem_euclid(MS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    if !(1..=9999).contains(&year) {
        return Err(format!(
            "時刻 {ts} は扱える範囲（1〜9999 年）を超えています"
        ));
    }
    Ok(Civil {
        year,
        month,
        day,
        hour: in_day / 3_600_000,
        minute: in_day / 60_000 % 60,
        second: in_day / 1000 % 60,
        millisecond: in_day % 1000,
        // 1970-01-01 は木曜日
        weekday: (days + 4).rem_euclid(7),
        yday: days - days_from_civil(year, 1, 1) + 1,
    })
}

/// 日付と時刻（時差つき）から Unix 時刻を作る。範囲外なら、理由を返す
pub(super) fn ts_from_fields(
    (year, month, day): (i64, i64, i64),
    (hour, minute, second, ms): (i64, i64, i64, i64),
    offset: i64,
) -> Result<f64, String> {
    if !(1..=9999).contains(&year) {
        return Err(format!("年 {year} は 1〜9999 の範囲外です"));
    }
    if !(1..=12).contains(&month) {
        return Err(format!("月 {month} は 1〜12 の範囲外です"));
    }
    if !(1..=days_in_month(year, month)).contains(&day) {
        return Err(format!("{year} 年 {month} 月に {day} 日はありません"));
    }
    for (name, value, max) in [("時", hour, 23), ("分", minute, 59), ("秒", second, 59)] {
        if !(0..=max).contains(&value) {
            return Err(format!("{name} {value} は 0〜{max} の範囲外です"));
        }
    }
    if !(0..=999).contains(&ms) {
        return Err(format!("ミリ秒 {ms} は 0〜999 の範囲外です"));
    }
    let total_ms = days_from_civil(year, month, day) * MS_PER_DAY
        + ((hour * 60 + minute) * 60 + second) * 1000
        + ms
        - offset * 60_000;
    Ok(total_ms as f64 / 1000.0)
}

/// UTC の `2026-11-05T03:04:05.678Z`（ミリ秒まで常に付ける。`std:log` が使う）。範囲外は 1970 年
pub(crate) fn iso_utc_ms(ts: f64) -> String {
    let c = civil_from_ts(ts, 0).or_else(|_| civil_from_ts(0.0, 0));
    match c {
        Ok(c) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            c.year, c.month, c.day, c.hour, c.minute, c.second, c.millisecond
        ),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn civil(ts: f64) -> Civil {
        civil_from_ts(ts, 0).unwrap()
    }

    #[test]
    fn known_instants() {
        let c = civil(0.0);
        assert_eq!(
            (c.year, c.month, c.day, c.weekday, c.yday),
            (1970, 1, 1, 4, 1)
        );
        // 2000-02-29（うるう日）
        let c = civil(951_782_400.0);
        assert_eq!(
            (c.year, c.month, c.day, c.weekday, c.yday),
            (2000, 2, 29, 2, 60)
        );
        // 2023-11-14T22:13:20Z（火曜日）
        let c = civil(1_700_000_000.0);
        assert_eq!(
            (
                c.year, c.month, c.day, c.hour, c.minute, c.second, c.weekday
            ),
            (2023, 11, 14, 22, 13, 20, 2)
        );
    }

    #[test]
    fn range_limits() {
        let first = civil(-62_135_596_800.0);
        assert_eq!((first.year, first.month, first.day), (1, 1, 1));
        let last = civil(253_402_300_799.0);
        assert_eq!(
            (
                last.year,
                last.month,
                last.day,
                last.hour,
                last.minute,
                last.second
            ),
            (9999, 12, 31, 23, 59, 59)
        );
        assert!(civil_from_ts(-62_135_596_801.0, 0).is_err());
        assert!(civil_from_ts(253_402_300_800.0, 0).is_err());
        assert!(civil_from_ts(f64::NAN, 0).is_err());
        assert!(civil_from_ts(1e300, 0).is_err());
    }

    #[test]
    fn offsets_and_negative_times() {
        // 日本時間（+9 時間）では、日付が進む
        let c = civil_from_ts(1_700_000_000.0, 540).unwrap();
        assert_eq!((c.day, c.hour, c.minute), (15, 7, 13));
        // 1969-12-31T23:59:59.5Z
        let c = civil(-0.5);
        assert_eq!(
            (c.year, c.month, c.day, c.hour, c.second, c.millisecond),
            (1969, 12, 31, 23, 59, 500)
        );
    }

    #[test]
    fn fields_roundtrip() {
        for ts in [
            0.0,
            1.5,
            -1.0,
            951_782_400.0,
            1_700_000_000.123,
            -62_135_596_800.0,
            253_402_300_799.999,
            -2_208_988_800.0,
        ] {
            let c = civil(ts);
            let back = ts_from_fields(
                (c.year, c.month, c.day),
                (c.hour, c.minute, c.second, c.millisecond),
                0,
            )
            .unwrap();
            assert!((back - ts).abs() < 0.0005, "{ts} → {back}");
        }
        // 時差つきでも往復する
        let c = civil_from_ts(1_700_000_000.0, -300).unwrap();
        let back = ts_from_fields(
            (c.year, c.month, c.day),
            (c.hour, c.minute, c.second, c.millisecond),
            -300,
        )
        .unwrap();
        assert_eq!(back, 1_700_000_000.0);
    }

    #[test]
    fn invalid_fields() {
        let ok = (0, 0, 0, 0);
        assert!(ts_from_fields((2023, 2, 29), ok, 0).is_err());
        assert!(ts_from_fields((2024, 2, 29), ok, 0).is_ok());
        assert!(ts_from_fields((1900, 2, 29), ok, 0).is_err());
        assert!(ts_from_fields((2000, 2, 29), ok, 0).is_ok());
        assert!(ts_from_fields((2023, 13, 1), ok, 0).is_err());
        assert!(ts_from_fields((0, 1, 1), ok, 0).is_err());
        assert!(ts_from_fields((2023, 1, 1), (24, 0, 0, 0), 0).is_err());
    }

    #[test]
    fn iso_utc_ms_is_fixed_width() {
        assert_eq!(iso_utc_ms(0.0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_utc_ms(1_700_000_000.123), "2023-11-14T22:13:20.123Z");
    }
}
