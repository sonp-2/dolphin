use ::postgres::config::SslMode;
use ::postgres::types::{IsNull, Kind, Type};
use bytes::BytesMut;

use super::connect::{label_of, parse_target};
use super::decode::*;
use super::encode::*;
use super::error::kind_of_sqlstate;
use crate::runtime::value::Value;
use crate::stdlib::new_array;

fn numeric(text: &str) -> Vec<u8> {
    let mut buf = BytesMut::new();
    numeric_to_sql(text, &mut buf).unwrap();
    buf.to_vec()
}

#[test]
fn numeric_roundtrip() {
    for text in [
        "0",
        "1",
        "-1",
        "12.34",
        "-12.340",
        "0.0001",
        "100000",
        "123456789.123456789",
        "0.5",
        "10000.0001",
        "99999999999999999999.99",
    ] {
        assert_eq!(numeric_from_sql(&numeric(text)).unwrap(), text, "{text}");
    }
    assert_eq!(numeric_from_sql(&numeric("NaN")).unwrap(), "NaN");
    // 指数・符号・先頭の 0
    assert_eq!(numeric_from_sql(&numeric("1e3")).unwrap(), "1000");
    assert_eq!(numeric_from_sql(&numeric("1.5e-3")).unwrap(), "0.0015");
    assert_eq!(numeric_from_sql(&numeric("+007.50")).unwrap(), "7.50");
    assert_eq!(numeric_from_sql(&numeric("-0")).unwrap(), "0");
}

#[test]
fn numeric_wire_format() {
    // 12.34 = ndigits 2, weight 0, sign 0, dscale 2, digits [12, 3400]
    assert_eq!(
        numeric("12.34"),
        [0, 2, 0, 0, 0, 0, 0, 2, 0, 12, 0x0D, 0x48]
    );
    // 0 は桁なし
    assert_eq!(numeric("0"), [0, 0, 0, 0, 0, 0, 0, 0]);
    assert!(numeric_to_sql("abc", &mut BytesMut::new()).is_err());
    assert!(numeric_to_sql("", &mut BytesMut::new()).is_err());
    assert!(numeric_to_sql("1.2.3", &mut BytesMut::new()).is_err());
}

#[test]
fn date_and_time() {
    assert_eq!(parse_date("2000-01-01").unwrap(), 0);
    assert_eq!(parse_date("2000-01-02").unwrap(), 1);
    assert_eq!(parse_date("1999-12-31").unwrap(), -1);
    assert!(parse_date("2026-02-30").is_err());
    assert!(parse_date("2026-13-01").is_err());
    assert!(parse_date("hello").is_err());
    let days = parse_date("2026-10-20").unwrap();
    let v = decode_simple(&days.to_be_bytes(), &Type::DATE, false).unwrap();
    assert!(v.equals(&Value::Str("2026-10-20".into())));

    assert_eq!(parse_time("00:00:01").unwrap(), 1_000_000);
    assert_eq!(parse_time("10:30:00.5").unwrap(), 37_800_500_000);
    assert!(parse_time("25:00:00").is_err());
    assert!(parse_time("10:60").is_err());
    let v = decode_simple(&37_800_500_000_i64.to_be_bytes(), &Type::TIME, false).unwrap();
    assert!(v.equals(&Value::Str("10:30:00.5".into())));
    let v = decode_simple(&3_600_000_000_i64.to_be_bytes(), &Type::TIME, false).unwrap();
    assert!(v.equals(&Value::Str("01:00:00".into())));
}

#[test]
fn timestamps() {
    // Unix 時刻 946684800 = 2000-01-01 00:00:00 UTC = PostgreSQL の 0
    assert_eq!(unix_to_pg_micros(946_684_800.0).unwrap(), 0);
    assert_eq!(unix_to_pg_micros(946_684_801.5).unwrap(), 1_500_000);
    let v = decode_simple(&1_500_000_i64.to_be_bytes(), &Type::TIMESTAMPTZ, false).unwrap();
    assert!(v.equals(&Value::Num(946_684_801.5)));
    let v = decode_simple(&i64::MAX.to_be_bytes(), &Type::TIMESTAMP, false).unwrap();
    assert!(v.equals(&Value::Str("infinity".into())));
}

#[test]
fn uuid_roundtrip() {
    let s = "123e4567-e89b-12d3-a456-426614174000";
    assert_eq!(format_uuid(&parse_uuid(s).unwrap()), s);
    assert!(parse_uuid("not-a-uuid").is_err());
}

#[test]
fn scalar_encoding() {
    let mut buf = BytesMut::new();
    assert!(matches!(
        encode(&Value::Nil, &Type::INT4, &mut buf).unwrap(),
        IsNull::Yes
    ));
    encode(&Value::Int(7), &Type::INT4, &mut buf).unwrap();
    assert_eq!(&buf[..], &[0, 0, 0, 7]);
    // 整数でない数値は int には入らない
    assert!(encode(&Value::Num(1.5), &Type::INT4, &mut BytesMut::new()).is_err());
    // 範囲外
    assert!(encode(&Value::Int(70_000), &Type::INT2, &mut BytesMut::new()).is_err());
    // 型違い
    assert!(encode(&Value::Str("1".into()), &Type::INT4, &mut BytesMut::new()).is_err());
    assert!(encode(&Value::Int(1), &Type::TEXT, &mut BytesMut::new()).is_err());
    // 整数の数値は float にも numeric にも入る
    assert!(encode(&Value::Int(1), &Type::FLOAT8, &mut BytesMut::new()).is_ok());
    assert!(encode(&Value::Num(1.25), &Type::NUMERIC, &mut BytesMut::new()).is_ok());
    // jsonb は版の 1 バイトが先頭に付く
    let mut buf = BytesMut::new();
    encode(&Value::Bool(true), &Type::JSONB, &mut buf).unwrap();
    assert_eq!(&buf[..], b"\x01true");
    let mut buf = BytesMut::new();
    encode(&Value::Str("a".into()), &Type::JSON, &mut buf).unwrap();
    assert_eq!(&buf[..], b"\"a\"");
}

#[test]
fn scalar_decoding() {
    assert!(
        decode_simple(&[0, 0, 0, 7], &Type::INT4, false)
            .unwrap()
            .equals(&Value::Num(7.0))
    );
    assert!(
        decode_simple(&[0, 0, 0, 7], &Type::INT4, true)
            .unwrap()
            .equals(&Value::Int(7))
    );
    assert!(
        decode_simple(&[1], &Type::BOOL, false)
            .unwrap()
            .equals(&Value::Bool(true))
    );
    assert!(
        decode_simple(b"hi", &Type::TEXT, false)
            .unwrap()
            .equals(&Value::Str("hi".into()))
    );
    assert_eq!(
        decode_simple(b"\x01{\"a\": 1}", &Type::JSONB, false)
            .map(|v| v.type_name())
            .unwrap(),
        "マップ"
    );
    // 対応しない型は、キャストを案内するエラー
    let e = decode_simple(&[0; 16], &Type::INTERVAL, false).unwrap_err();
    assert!(e.to_string().contains("::text"), "{e}");
}

#[test]
fn array_roundtrip() {
    let arr = |items: Vec<Value>| new_array(items);
    let value = arr(vec![Value::Int(1), Value::Nil, Value::Int(3)]);
    let ty = Type::INT4_ARRAY;
    let mut buf = BytesMut::new();
    encode(&value, &ty, &mut buf).unwrap();
    let Kind::Array(elem) = ty.kind() else {
        panic!()
    };
    let back = decode_array(&buf, elem, true).unwrap();
    assert!(back.equals(&value));

    // 2 次元
    let value = arr(vec![
        arr(vec![Value::Str("a".into()), Value::Str("b".into())]),
        arr(vec![Value::Str("c".into()), Value::Nil]),
    ]);
    let mut buf = BytesMut::new();
    encode(&value, &Type::TEXT_ARRAY, &mut buf).unwrap();
    assert!(
        decode_array(&buf, &Type::TEXT, false)
            .unwrap()
            .equals(&value)
    );

    // 空の配列
    let empty = arr(vec![]);
    let mut buf = BytesMut::new();
    encode(&empty, &Type::TEXT_ARRAY, &mut buf).unwrap();
    assert!(
        decode_array(&buf, &Type::TEXT, false)
            .unwrap()
            .equals(&empty)
    );

    // 長方形でない配列は送れない
    let ragged = arr(vec![
        arr(vec![Value::Int(1), Value::Int(2)]),
        arr(vec![Value::Int(3)]),
    ]);
    assert!(encode(&ragged, &Type::INT4_ARRAY, &mut BytesMut::new()).is_err());
    // 配列でない値
    assert!(encode(&Value::Int(1), &Type::INT4_ARRAY, &mut BytesMut::new()).is_err());
}

#[test]
fn sqlstate_kinds() {
    assert_eq!(kind_of_sqlstate("42601"), "sql");
    assert_eq!(kind_of_sqlstate("23505"), "sql");
    assert_eq!(kind_of_sqlstate("28P01"), "auth");
    assert_eq!(kind_of_sqlstate("40001"), "busy");
    assert_eq!(kind_of_sqlstate("40P01"), "busy");
    assert_eq!(kind_of_sqlstate("55P03"), "busy");
    assert_eq!(kind_of_sqlstate("08006"), "network");
}

#[test]
fn target_parsing() {
    let url = Value::Str("postgres://u:p@db.example:6432/app?sslmode=require".into());
    let config = parse_target(&url).unwrap().unwrap();
    assert_eq!(label_of(&config), "db.example:6432/app");
    assert_eq!(config.get_user(), Some("u"));
    assert_eq!(config.get_ssl_mode(), SslMode::Require);
    // 既定値
    let config = parse_target(&Value::Str("postgres:///".into()))
        .unwrap()
        .unwrap();
    assert_eq!(label_of(&config), "localhost:5432");
    assert_eq!(config.get_user(), Some("postgres"));
    // 読めない URL は parse のエラー値
    let bad = parse_target(&Value::Str("postgres://h/db?sslmode=bogus".into()))
        .unwrap()
        .unwrap_err();
    assert_eq!(err_kind(&bad), "parse");
    // 型の間違いは実行時エラー
    assert!(parse_target(&Value::Int(1)).is_err());
}

fn err_kind(v: &Value) -> String {
    match v {
        Value::Error(e) => e.kind.to_string(),
        other => panic!("エラー値ではありません: {other:?}"),
    }
}
