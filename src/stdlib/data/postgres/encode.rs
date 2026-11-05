//! 値の変換: Dolphin → PostgreSQL（型ごとのバイナリ形式への符号化）

use ::postgres::types::{IsNull, Kind, ToSql, Type, to_sql_checked};
use bytes::BytesMut;
use postgres_protocol::types as pt;

use super::{BoxErr, PG_EPOCH_DAYS, PG_EPOCH_UNIX};
use crate::runtime::value::Value;
use crate::stdlib::data::json::to_json_string;
use crate::stdlib::time::{civil_from_days, days_from_civil};

/// 変換済みのパラメータ 1 つ（バイナリ形式。None は NULL）。`Value` は別のスレッドに渡せないので、先に変換しておく
#[derive(Debug)]
pub(super) struct Encoded(pub(super) Option<Vec<u8>>);

impl ToSql for Encoded {
    fn to_sql(&self, _: &Type, out: &mut BytesMut) -> Result<IsNull, BoxErr> {
        match &self.0 {
            None => Ok(IsNull::Yes),
            Some(bytes) => {
                out.extend_from_slice(bytes);
                Ok(IsNull::No)
            }
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

fn mismatch(value: &Value, ty: &Type) -> BoxErr {
    format!(
        "{}を {} 型のパラメータには渡せません",
        value.type_name(),
        ty.name()
    )
    .into()
}

fn as_i64(value: &Value, ty: &Type) -> Result<i64, BoxErr> {
    match value {
        Value::Int(n) => Ok(*n),
        Value::Num(n) if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_992.0 => Ok(*n as i64),
        other => Err(mismatch(other, ty)),
    }
}

fn as_f64(value: &Value, ty: &Type) -> Result<f64, BoxErr> {
    match value {
        Value::Int(n) => Ok(*n as f64),
        Value::Num(n) => Ok(*n),
        other => Err(mismatch(other, ty)),
    }
}

fn range_error(n: i64, ty: &Type) -> BoxErr {
    format!("{n} は {} 型の範囲を出ています", ty.name()).into()
}

pub(super) fn encode(value: &Value, ty: &Type, out: &mut BytesMut) -> Result<IsNull, BoxErr> {
    if matches!(value, Value::Nil) {
        return Ok(IsNull::Yes);
    }
    match ty.kind() {
        Kind::Array(elem) => encode_array(value, ty, elem, out)?,
        Kind::Domain(inner) => return encode(value, inner, out),
        Kind::Enum(_) => match value {
            Value::Str(s) => pt::text_to_sql(s, out),
            other => return Err(mismatch(other, ty)),
        },
        Kind::Simple => encode_simple(value, ty, out)?,
        _ => {
            return Err(format!("{} 型のパラメータには対応していません", ty.name()).into());
        }
    }
    Ok(IsNull::No)
}

fn encode_simple(value: &Value, ty: &Type, out: &mut BytesMut) -> Result<(), BoxErr> {
    match *ty {
        Type::BOOL => match value {
            Value::Bool(b) => pt::bool_to_sql(*b, out),
            other => return Err(mismatch(other, ty)),
        },
        Type::INT2 => {
            let n = as_i64(value, ty)?;
            pt::int2_to_sql(i16::try_from(n).map_err(|_| range_error(n, ty))?, out);
        }
        Type::INT4 => {
            let n = as_i64(value, ty)?;
            pt::int4_to_sql(i32::try_from(n).map_err(|_| range_error(n, ty))?, out);
        }
        Type::INT8 => pt::int8_to_sql(as_i64(value, ty)?, out),
        Type::OID => {
            let n = as_i64(value, ty)?;
            pt::oid_to_sql(u32::try_from(n).map_err(|_| range_error(n, ty))?, out);
        }
        Type::FLOAT4 => pt::float4_to_sql(as_f64(value, ty)? as f32, out),
        Type::FLOAT8 => pt::float8_to_sql(as_f64(value, ty)?, out),
        Type::NUMERIC => {
            let text = match value {
                Value::Int(n) => n.to_string(),
                Value::Num(n) => {
                    if n.is_nan() {
                        "NaN".to_string()
                    } else if n.is_infinite() {
                        return Err("無限大は numeric にできません".into());
                    } else {
                        n.to_string()
                    }
                }
                Value::Str(s) => s.to_string(),
                other => return Err(mismatch(other, ty)),
            };
            numeric_to_sql(&text, out)?;
        }
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN => match value {
            Value::Str(s) => pt::text_to_sql(s, out),
            other => return Err(mismatch(other, ty)),
        },
        Type::BYTEA => match value {
            Value::Bytes(b) => pt::bytea_to_sql(b, out),
            other => return Err(mismatch(other, ty)),
        },
        Type::UUID => match value {
            Value::Str(s) => pt::uuid_to_sql(parse_uuid(s)?, out),
            other => return Err(mismatch(other, ty)),
        },
        Type::DATE => match value {
            Value::Str(s) => pt::date_to_sql(parse_date(s)?, out),
            other => return Err(mismatch(other, ty)),
        },
        Type::TIME => match value {
            Value::Str(s) => pt::time_to_sql(parse_time(s)?, out),
            other => return Err(mismatch(other, ty)),
        },
        Type::TIMESTAMP | Type::TIMESTAMPTZ => {
            let secs = as_f64(value, ty)?;
            pt::timestamp_to_sql(unix_to_pg_micros(secs)?, out);
        }
        Type::JSON | Type::JSONB => {
            let text = to_json_string("postgres のパラメータ", value, None)?;
            if *ty == Type::JSONB {
                out.extend_from_slice(&[1]);
            }
            out.extend_from_slice(text.as_bytes());
        }
        _ => {
            return Err(format!(
                "{} 型のパラメータには対応していません（対応する型は design.md 5.47）",
                ty.name()
            )
            .into());
        }
    }
    Ok(())
}

pub(super) fn encode_array(
    value: &Value,
    ty: &Type,
    elem: &Type,
    out: &mut BytesMut,
) -> Result<(), BoxErr> {
    let Value::Array(items) = value else {
        return Err(mismatch(value, ty));
    };
    // JSON の配列の要素は、配列でも 1 つの値
    let flat_elements = matches!(*elem, Type::JSON | Type::JSONB);
    let mut dims: Vec<i32> = Vec::new();
    let mut leaves: Vec<Value> = Vec::new();
    if !items.borrow().is_empty() {
        // 最初の要素をたどって次元を決める
        let mut cur = items.borrow().clone();
        loop {
            dims.push(i32::try_from(cur.len()).map_err(|_| "配列が大きすぎます")?);
            if flat_elements {
                break;
            }
            let next = match cur.first() {
                Some(Value::Array(a)) => a.borrow().clone(),
                _ => break,
            };
            cur = next;
        }
        if dims.contains(&0) {
            return Err("空の配列を入れ子にはできません".into());
        }
        collect(&items.borrow(), 0, &dims, flat_elements, &mut leaves)?;
    }
    let dimensions = dims.iter().map(|&len| pt::ArrayDimension {
        len,
        lower_bound: 1,
    });
    pt::array_to_sql(
        dimensions,
        elem.oid(),
        leaves.iter(),
        |v, buf| {
            Ok::<_, BoxErr>(match encode(v, elem, buf)? {
                IsNull::Yes => postgres_protocol::IsNull::Yes,
                IsNull::No => postgres_protocol::IsNull::No,
            })
        },
        out,
    )?;
    Ok(())
}

/// 入れ子の配列が長方形であることを確かめながら、平らな要素の並びにする
fn collect(
    items: &[Value],
    level: usize,
    dims: &[i32],
    flat_elements: bool,
    leaves: &mut Vec<Value>,
) -> Result<(), BoxErr> {
    let ragged = || -> BoxErr {
        "多次元の配列は、各次元の長さが揃っている必要があります".into()
    };
    if usize::try_from(dims[level]).ok() != Some(items.len()) {
        return Err(ragged());
    }
    for item in items {
        if level + 1 == dims.len() {
            if !flat_elements && matches!(item, Value::Array(_)) {
                return Err(ragged());
            }
            leaves.push(item.clone());
        } else {
            let Value::Array(inner) = item else {
                return Err(ragged());
            };
            collect(&inner.borrow(), level + 1, dims, flat_elements, leaves)?;
        }
    }
    Ok(())
}

pub(super) fn parse_uuid(s: &str) -> Result<[u8; 16], BoxErr> {
    let hex: Vec<u8> = s.bytes().filter(|&b| b != b'-').collect();
    if hex.len() != 32 || !hex.iter().all(u8::is_ascii_hexdigit) {
        return Err(format!("`{s}` は UUID として読めません").into());
    }
    let mut out = [0u8; 16];
    for (i, pair) in hex.chunks(2).enumerate() {
        let digit = |b: u8| (b as char).to_digit(16).expect("16 進数の桁") as u8;
        out[i] = digit(pair[0]) * 16 + digit(pair[1]);
    }
    Ok(out)
}

pub(super) fn format_uuid(b: &[u8]) -> String {
    let mut s = String::with_capacity(36);
    for (i, byte) in b.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            s.push('-');
        }
        s.push_str(&format!("{byte:02x}"));
    }
    s
}

fn digits_of(s: &str, min: usize, max: usize) -> Option<i64> {
    if s.len() < min || s.len() > max || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// `2026-10-20` → PostgreSQL の日付（2000-01-01 からの日数）
pub(super) fn parse_date(s: &str) -> Result<i32, BoxErr> {
    let bad =
        || -> BoxErr { format!("`{s}` は日付（YYYY-MM-DD）として読めません").into() };
    let mut parts = s.split('-');
    let (y, m, d) = (
        digits_of(parts.next().unwrap_or(""), 1, 4).ok_or_else(bad)?,
        digits_of(parts.next().unwrap_or(""), 1, 2).ok_or_else(bad)?,
        digits_of(parts.next().unwrap_or(""), 1, 2).ok_or_else(bad)?,
    );
    if parts.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) || y < 1 {
        return Err(bad());
    }
    // 2 月 30 日のような存在しない日を弾く
    let days = days_from_civil(y, m, d);
    if civil_from_days(days) != (y, m, d) {
        return Err(bad());
    }
    Ok((days - PG_EPOCH_DAYS) as i32)
}

/// `10:30:00.5` → 真夜中からのマイクロ秒
pub(super) fn parse_time(s: &str) -> Result<i64, BoxErr> {
    let bad = || -> BoxErr {
        format!("`{s}` は時刻（HH:MM:SS[.ffffff]）として読めません").into()
    };
    let (hms, frac) = match s.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    let mut parts = hms.split(':');
    let h = digits_of(parts.next().unwrap_or(""), 1, 2).ok_or_else(bad)?;
    let m = digits_of(parts.next().unwrap_or(""), 1, 2).ok_or_else(bad)?;
    let sec = match parts.next() {
        Some(p) => digits_of(p, 1, 2).ok_or_else(bad)?,
        None => 0,
    };
    if parts.next().is_some() || h > 24 || m > 59 || sec > 59 || (h == 24 && (m, sec) != (0, 0)) {
        return Err(bad());
    }
    let micros = match frac {
        Some(f) => {
            let digits = digits_of(f, 1, 6).ok_or_else(bad)?;
            digits * 10_i64.pow(6 - f.len() as u32)
        }
        None => 0,
    };
    Ok(((h * 60 + m) * 60 + sec) * 1_000_000 + micros)
}

pub(super) fn unix_to_pg_micros(secs: f64) -> Result<i64, BoxErr> {
    let micros = (secs * 1e6).round();
    if !micros.is_finite() || micros.abs() >= 9.0e18 {
        return Err("日時が範囲を出ています".into());
    }
    Ok(micros as i64 - PG_EPOCH_UNIX * 1_000_000)
}

/// 10 進数の文字列を、`numeric` のバイナリ形式にする
pub(super) fn numeric_to_sql(text: &str, out: &mut BytesMut) -> Result<(), BoxErr> {
    let bad = || -> BoxErr { format!("`{text}` は numeric として読めません").into() };
    let t = text.trim();
    if t.eq_ignore_ascii_case("nan") {
        out.extend_from_slice(&0_i16.to_be_bytes());
        out.extend_from_slice(&0_i16.to_be_bytes());
        out.extend_from_slice(&0xC000_u16.to_be_bytes());
        out.extend_from_slice(&0_u16.to_be_bytes());
        return Ok(());
    }
    let (negative, body) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (mantissa, exponent) = match body.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().map_err(|_| bad())?),
        None => (body, 0),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (int_part.is_empty() && frac_part.is_empty())
        || !int_part
            .bytes()
            .chain(frac_part.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return Err(bad());
    }
    // 指数を小数点の位置に反映する
    let mut digits: Vec<u8> = int_part
        .bytes()
        .chain(frac_part.bytes())
        .map(|b| b - b'0')
        .collect();
    let mut point = int_part.len() as i64 + i64::from(exponent);
    if point < 0 {
        let pad = (-point) as usize;
        if pad > 100_000 {
            return Err(bad());
        }
        digits.splice(0..0, std::iter::repeat_n(0, pad));
        point = 0;
    }
    let point = point as usize;
    if point > digits.len() {
        if point - digits.len() > 100_000 {
            return Err(bad());
        }
        digits.resize(point, 0);
    }
    let dscale = digits.len() - point;
    let mut int_digits = digits[..point].to_vec();
    let mut frac_digits = digits[point..].to_vec();
    // 整数部は 4 桁ごとに右から、小数部は左から区切る
    while !int_digits.len().is_multiple_of(4) {
        int_digits.insert(0, 0);
    }
    while !frac_digits.len().is_multiple_of(4) {
        frac_digits.push(0);
    }
    let mut groups: Vec<i16> = int_digits
        .chunks(4)
        .chain(frac_digits.chunks(4))
        .map(|c| c.iter().fold(0i16, |acc, &d| acc * 10 + i16::from(d)))
        .collect();
    let mut weight = (int_digits.len() / 4) as i32 - 1;
    while groups.first() == Some(&0) {
        groups.remove(0);
        weight -= 1;
    }
    while groups.last() == Some(&0) {
        groups.pop();
    }
    if groups.is_empty() {
        weight = 0;
    }
    let ndigits = i16::try_from(groups.len()).map_err(|_| bad())?;
    let weight = i16::try_from(weight).map_err(|_| bad())?;
    let sign: u16 = if negative && !groups.is_empty() {
        0x4000
    } else {
        0
    };
    out.extend_from_slice(&ndigits.to_be_bytes());
    out.extend_from_slice(&weight.to_be_bytes());
    out.extend_from_slice(&sign.to_be_bytes());
    out.extend_from_slice(&(u16::try_from(dscale).map_err(|_| bad())?).to_be_bytes());
    for g in groups {
        out.extend_from_slice(&g.to_be_bytes());
    }
    Ok(())
}
