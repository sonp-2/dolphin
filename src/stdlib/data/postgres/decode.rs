//! 値の変換: PostgreSQL → Dolphin（型ごとのバイナリ形式からの復号）

use ::postgres::types::{FromSql, Kind, Type};
use fallible_iterator::FallibleIterator;
use postgres_protocol::types as pt;

use super::encode::format_uuid;
use super::{BoxErr, PG_EPOCH_DAYS, PG_EPOCH_UNIX};
use crate::runtime::value::Value;
use crate::stdlib::data::json::parse_str;
use crate::stdlib::new_array;
use crate::stdlib::time::civil_from_days;

/// 結果の 1 つの値の生のバイト列（NULL なら None）
pub(super) struct Raw<'a>(pub(super) Option<&'a [u8]>);

impl<'a> FromSql<'a> for Raw<'a> {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, BoxErr> {
        Ok(Raw(Some(raw)))
    }

    fn from_sql_null(_: &Type) -> Result<Self, BoxErr> {
        Ok(Raw(None))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

pub(super) fn int_value(n: i64, ints: bool) -> Value {
    if ints {
        Value::Int(n)
    } else {
        Value::Num(n as f64)
    }
}

pub(super) fn decode(raw: &[u8], ty: &Type, ints: bool) -> Result<Value, BoxErr> {
    match ty.kind() {
        Kind::Array(elem) => decode_array(raw, elem, ints),
        Kind::Domain(inner) => decode(raw, inner, ints),
        Kind::Enum(_) => Ok(Value::Str(pt::text_from_sql(raw)?.into())),
        Kind::Simple => decode_simple(raw, ty, ints),
        _ => Err(unsupported(ty)),
    }
}

fn unsupported(ty: &Type) -> BoxErr {
    format!(
        "{} 型の値は読めません（SQL で `::text` を付けて文字列にしてください）",
        ty.name()
    )
    .into()
}

pub(super) fn decode_simple(raw: &[u8], ty: &Type, ints: bool) -> Result<Value, BoxErr> {
    Ok(match *ty {
        Type::BOOL => Value::Bool(pt::bool_from_sql(raw)?),
        Type::INT2 => int_value(i64::from(pt::int2_from_sql(raw)?), ints),
        Type::INT4 => int_value(i64::from(pt::int4_from_sql(raw)?), ints),
        Type::INT8 => int_value(pt::int8_from_sql(raw)?, ints),
        Type::OID => int_value(i64::from(pt::oid_from_sql(raw)?), ints),
        Type::FLOAT4 => Value::Num(f64::from(pt::float4_from_sql(raw)?)),
        Type::FLOAT8 => Value::Num(pt::float8_from_sql(raw)?),
        Type::NUMERIC => Value::Str(numeric_from_sql(raw)?.into()),
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN => {
            Value::Str(pt::text_from_sql(raw)?.into())
        }
        Type::CHAR => Value::Str(String::from_utf8_lossy(raw).into_owned().into()),
        Type::BYTEA => Value::bytes(pt::bytea_from_sql(raw)),
        Type::UUID => Value::Str(format_uuid(&pt::uuid_from_sql(raw)?).into()),
        Type::DATE => {
            let days = pt::date_from_sql(raw)?;
            match days {
                i32::MAX => Value::Str("infinity".into()),
                i32::MIN => Value::Str("-infinity".into()),
                d => {
                    let (y, m, day) = civil_from_days(i64::from(d) + PG_EPOCH_DAYS);
                    Value::Str(format!("{y:04}-{m:02}-{day:02}").into())
                }
            }
        }
        Type::TIME => {
            let micros = pt::time_from_sql(raw)?;
            let secs = micros / 1_000_000;
            let frac = micros % 1_000_000;
            let mut s = format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
            if frac != 0 {
                s.push_str(format!(".{frac:06}").trim_end_matches('0'));
            }
            Value::Str(s.into())
        }
        Type::TIMESTAMP | Type::TIMESTAMPTZ => match pt::timestamp_from_sql(raw)? {
            i64::MAX => Value::Str("infinity".into()),
            i64::MIN => Value::Str("-infinity".into()),
            micros => Value::Num((micros + PG_EPOCH_UNIX * 1_000_000) as f64 / 1e6),
        },
        Type::JSON => json_value(pt::text_from_sql(raw)?)?,
        Type::JSONB => {
            let (version, body) = raw.split_first().ok_or("jsonb が空です")?;
            if *version != 1 {
                return Err(format!("jsonb のバージョン {version} には対応していません").into());
            }
            json_value(pt::text_from_sql(body)?)?
        }
        _ => return Err(unsupported(ty)),
    })
}

fn json_value(text: &str) -> Result<Value, BoxErr> {
    parse_str(text, false).map_err(|(_, message)| format!("JSON が読めません: {message}").into())
}

pub(super) fn decode_array(raw: &[u8], elem: &Type, ints: bool) -> Result<Value, BoxErr> {
    let array = pt::array_from_sql(raw)?;
    let dims: Vec<usize> = array
        .dimensions()
        .map(|d| Ok(usize::try_from(d.len).unwrap_or(0)))
        .collect()?;
    let mut flat = Vec::new();
    let mut values = array.values();
    while let Some(item) = values.next()? {
        flat.push(match item {
            Some(bytes) => decode(bytes, elem, ints)?,
            None => Value::Nil,
        });
    }
    if dims.is_empty() {
        return Ok(new_array(Vec::new()));
    }
    Ok(nest(&mut flat.into_iter(), &dims))
}

fn nest(flat: &mut std::vec::IntoIter<Value>, dims: &[usize]) -> Value {
    let (first, rest) = dims.split_first().expect("次元が 1 つ以上ある");
    let mut items = Vec::with_capacity(*first);
    for _ in 0..*first {
        if rest.is_empty() {
            items.push(flat.next().unwrap_or(Value::Nil));
        } else {
            items.push(nest(flat, rest));
        }
    }
    new_array(items)
}

/// `numeric` のバイナリ形式を 10 進数の文字列にする
pub(super) fn numeric_from_sql(raw: &[u8]) -> Result<String, BoxErr> {
    if raw.len() < 8 {
        return Err("numeric が短すぎます".into());
    }
    let be16 = |i: usize| i16::from_be_bytes([raw[i], raw[i + 1]]);
    let ndigits = usize::try_from(be16(0)).map_err(|_| "numeric が正しくありません")?;
    let weight = i32::from(be16(2));
    let sign = u16::from_be_bytes([raw[4], raw[5]]);
    let dscale = usize::from(u16::from_be_bytes([raw[6], raw[7]]));
    if raw.len() != 8 + ndigits * 2 {
        return Err("numeric の長さが正しくありません".into());
    }
    match sign {
        0xC000 => return Ok("NaN".into()),
        0xD000 => return Ok("Infinity".into()),
        0xF000 => return Ok("-Infinity".into()),
        0 | 0x4000 => {}
        _ => return Err("numeric の符号が正しくありません".into()),
    }
    let digit = |i: i32| -> i16 {
        if i >= 0 && (i as usize) < ndigits {
            be16(8 + i as usize * 2)
        } else {
            0
        }
    };
    let mut s = String::new();
    if sign == 0x4000 {
        s.push('-');
    }
    if weight < 0 {
        s.push('0');
    } else {
        for i in 0..=weight {
            let d = digit(i);
            if i == 0 {
                s.push_str(&d.to_string());
            } else {
                s.push_str(&format!("{d:04}"));
            }
        }
    }
    if dscale > 0 {
        s.push('.');
        let mut frac = String::new();
        let mut i = weight + 1;
        while frac.len() < dscale {
            frac.push_str(&format!("{:04}", digit(i)));
            i += 1;
        }
        frac.truncate(dscale);
        s.push_str(&frac);
    }
    Ok(s)
}
