//! 引数の検査（個数、時差、年）

use super::super::int_arg;
use crate::runtime::value::Value;

/// 引数の数が `min`〜`max` 個か確かめる
pub(super) fn check_count(
    name: &str,
    args: &[Value],
    min: usize,
    max: usize,
) -> Result<(), String> {
    if (min..=max).contains(&args.len()) {
        return Ok(());
    }
    let expected = if min == max {
        format!("{min} 個")
    } else {
        format!("{min} 個から {max} 個")
    };
    Err(format!(
        "{name} の引数は {expected}です（{} 個渡されました）",
        args.len()
    ))
}

/// `i` 番目の引数が時差（分）。なければ 0（UTC）
pub(super) fn offset_arg(name: &str, args: &[Value], i: usize) -> Result<i64, String> {
    if args.len() <= i {
        return Ok(0);
    }
    let offset = int_arg(name, args, i)?;
    if offset.abs() >= 1440 {
        return Err(format!(
            "{name} の時差 {offset} 分は使えません（-1439〜1439）"
        ));
    }
    Ok(offset)
}

/// `i` 番目の引数が年（1〜9999 の整数）
pub(super) fn year_arg(name: &str, args: &[Value], i: usize) -> Result<i64, String> {
    let year = int_arg(name, args, i)?;
    if (1..=9999).contains(&year) {
        Ok(year)
    } else {
        Err(format!("{name} の年は 1〜9999 の整数です（{year}）"))
    }
}
