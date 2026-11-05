//! バイト列: bytes_from_str, bytes_from_arr, bytes_new, bytes_concat, bytes_len, bytes_slice,
//! bytes_to_arr, bytes_find, bytes_to_str（design.md 5.14）
//!
//! バイト列は変更できない（`Value::Bytes`）。読み取りは `@b{i}`

use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;
use crate::stdlib::deprecated::legacy;
use crate::stdlib::{
    array_arg, bytes_arg, data_arg, error_value, expect_args, int_arg, new_array, str_arg,
    type_error,
};

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register("bytes_from_str", str_to_bytes);
    interp.register("str_to_bytes", legacy!("str_to_bytes", str_to_bytes));
    interp.register("bytes_from_arr", bytes_from_arr);
    interp.register("bytes_new", bytes_new);
    interp.register("bytes_concat", bytes_concat);
    interp.register("bytes_len", bytes_len);
    interp.register("bytes_slice", bytes_slice);
    interp.register("bytes_to_arr", bytes_to_arr);
    interp.register("bytes_find", bytes_find);
    interp.register("bytes_to_str", bytes_to_str);
    interp.register("bytes_get_int", bytes_get_int);
    interp.register("bytes_set_int", bytes_set_int);
}

/// 作れるバイト列の大きさの上限
const BYTES_MAX: i64 = 100_000_000;

/// 文字列を UTF-8 のバイト列にする
fn str_to_bytes(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("bytes_from_str", args, 1)?;
    Ok(Value::bytes(str_arg("bytes_from_str", args, 0)?.as_bytes()))
}

/// 0〜255 の整数の配列から作る
fn bytes_from_arr(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("bytes_from_arr", args, 1)?;
    let items = array_arg("bytes_from_arr", args, 0)?.borrow();
    let mut bytes = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        match item {
            Value::Num(n) if n.fract() == 0.0 && (0.0..=255.0).contains(n) => bytes.push(*n as u8),
            Value::Num(n) => {
                return Err(format!(
                    "bytes_from_arr の {i} 番目は 0〜255 の整数です（{n}）"
                ));
            }
            other => {
                return Err(format!(
                    "bytes_from_arr の {i} 番目は数値です（{}が渡されました）",
                    other.type_name()
                ));
            }
        }
    }
    Ok(Value::bytes(bytes))
}

/// 0 で埋めた `n` バイト
fn bytes_new(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("bytes_new", args, 1)?;
    let n = int_arg("bytes_new", args, 0)?;
    if !(0..=BYTES_MAX).contains(&n) {
        return Err(format!("bytes_new の大きさは 0〜{BYTES_MAX} です（{n}）"));
    }
    Ok(Value::bytes(vec![0u8; n as usize]))
}

/// つなげる。どちらも、バイト列か文字列（UTF-8）
fn bytes_concat(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("bytes_concat", args, 2)?;
    let mut bytes = data_arg("bytes_concat", args, 0)?.into_owned();
    bytes.extend_from_slice(&data_arg("bytes_concat", args, 1)?);
    Ok(Value::bytes(bytes))
}

fn bytes_len(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("bytes_len", args, 1)?;
    Ok(Value::Num(bytes_arg("bytes_len", args, 0)?.len() as f64))
}

/// `start` から `end` の手前まで
fn bytes_slice(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("bytes_slice", args, 3)?;
    let bytes = bytes_arg("bytes_slice", args, 0)?;
    let start = int_arg("bytes_slice", args, 1)?;
    let end = int_arg("bytes_slice", args, 2)?;
    let len = bytes.len() as i64;
    if !(0 <= start && start <= end && end <= len) {
        return Err(format!(
            "bytes_slice の範囲 {start}〜{end} は使えません（0 ≦ 開始 ≦ 終了 ≦ {len}）"
        ));
    }
    Ok(Value::bytes(&bytes[start as usize..end as usize]))
}

/// 0〜255 の数値の配列にする
fn bytes_to_arr(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("bytes_to_arr", args, 1)?;
    let bytes = bytes_arg("bytes_to_arr", args, 0)?;
    Ok(new_array(
        bytes.iter().map(|&b| Value::Num(f64::from(b))).collect(),
    ))
}

// ---- 整数の読み書き（design.md 5.50） ----

/// `"le"` か `"be"`。true ならリトルエンディアン
fn endian_arg(name: &str, args: &[Value], i: usize) -> Result<bool, String> {
    match str_arg(name, args, i)? {
        "le" => Ok(true),
        "be" => Ok(false),
        other => Err(format!(
            "{name} のバイト順は \"le\" か \"be\" です（{other:?}）"
        )),
    }
}

fn size_arg(name: &str, args: &[Value], i: usize) -> Result<usize, String> {
    let size = int_arg(name, args, i)?;
    if !(1..=8).contains(&size) {
        return Err(format!("{name} の大きさは 1〜8 バイトです（{size}）"));
    }
    Ok(size as usize)
}

fn offset_in(
    name: &str,
    args: &[Value],
    i: usize,
    len: usize,
    size: usize,
) -> Result<usize, String> {
    let offset = int_arg(name, args, i)?;
    if offset < 0 || offset as usize + size > len {
        return Err(format!(
            "{name} の位置 {offset} から {size} バイトは範囲外です（長さ {len}）"
        ));
    }
    Ok(offset as usize)
}

/// `offset` から `size` バイトを整数として読む。1〜6 バイトは数値、7〜8 バイトは `int`
fn bytes_get_int(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    const NAME: &str = "bytes_get_int";
    if !(4..=5).contains(&args.len()) {
        return Err(format!(
            "{NAME} の引数は 4 個か 5 個です（{} 個渡されました）",
            args.len()
        ));
    }
    let bytes = bytes_arg(NAME, args, 0)?;
    let size = size_arg(NAME, args, 2)?;
    let offset = offset_in(NAME, args, 1, bytes.len(), size)?;
    let little = endian_arg(NAME, args, 3)?;
    let signed = match args.get(4) {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(other) => return Err(type_error(NAME, 4, "真偽値", other)),
    };
    let mut raw = [0u8; 8];
    raw[..size].copy_from_slice(&bytes[offset..offset + size]);
    if !little {
        raw[..size].reverse();
    }
    let mut value = u64::from_le_bytes(raw);
    if signed && size < 8 && value >> (size * 8 - 1) & 1 == 1 {
        value |= u64::MAX << (size * 8);
    }
    if size <= 6 {
        // 6 バイトまでは数値に正確に収まる
        let n = if signed {
            value as i64 as f64
        } else {
            value as f64
        };
        return Ok(Value::Num(n));
    }
    if !signed && size == 8 && value > i64::MAX as u64 {
        return Ok(error_value(
            "range",
            format!("{NAME}: {value} は int（-2^63〜2^63-1）に収まりません"),
        ));
    }
    Ok(Value::Int(value as i64))
}

/// `offset` から `size` バイトを `value` で置き換えた、新しいバイト列
fn bytes_set_int(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    const NAME: &str = "bytes_set_int";
    expect_args(NAME, args, 5)?;
    let bytes = bytes_arg(NAME, args, 0)?;
    let size = size_arg(NAME, args, 2)?;
    let offset = offset_in(NAME, args, 1, bytes.len(), size)?;
    let little = endian_arg(NAME, args, 4)?;
    let value: i128 = match &args[3] {
        Value::Int(n) => i128::from(*n),
        Value::Num(n) if n.fract() == 0.0 && n.abs() < 9.3e18 => *n as i128,
        Value::Num(n) => {
            return Err(format!("{NAME} の値は整数です（{n} が渡されました）"));
        }
        other => return Err(type_error(NAME, 3, "整数", other)),
    };
    let bits = size * 8;
    let (min, max) = (-(1i128 << (bits - 1)), (1i128 << bits) - 1);
    if !(min..=max).contains(&value) {
        return Err(format!(
            "{NAME} の値 {value} は {size} バイトに収まりません（{min}〜{max}）"
        ));
    }
    let mut raw = (value as u64).to_le_bytes();
    if !little {
        raw[..size].reverse();
    }
    let mut out = bytes.to_vec();
    out[offset..offset + size].copy_from_slice(&raw[..size]);
    Ok(Value::bytes(out))
}

// 最初に見つかった位置（バイト単位）。なければ -1
fn bytes_find(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("bytes_find", args, 2)?;
    let bytes = bytes_arg("bytes_find", args, 0)?;
    let sub = data_arg("bytes_find", args, 1)?;
    let pos = if sub.is_empty() {
        Some(0)
    } else {
        bytes.windows(sub.len()).position(|w| w == &*sub)
    };
    Ok(Value::Num(pos.map_or(-1.0, |p| p as f64)))
}

/// UTF-8 の文字列にする。UTF-8 でなければ `parse` のエラー値
fn bytes_to_str(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("bytes_to_str", args, 1)?;
    let bytes = bytes_arg("bytes_to_str", args, 0)?;
    Ok(match std::str::from_utf8(bytes) {
        Ok(s) => Value::Str(s.into()),
        Err(e) => error_value(
            "parse",
            format!("UTF-8 として読めません（{} バイト目）", e.valid_up_to() + 1),
        ),
    })
}
