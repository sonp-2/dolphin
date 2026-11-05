//! `std:postgres`: connect, close, exec, query, run_script（design.md 5.47）
//!
//! 純 Rust の同期クライアント `postgres`（`postgres` feature）を使う。
//! パラメータと結果は、サーバーが決めた型に合わせて、バイナリ形式で自前に変換する

use std::collections::HashMap;
use std::error::Error as StdError;
use std::rc::Rc;
use std::str::FromStr;
use std::time::Duration;

use ::postgres::config::SslMode;
use ::postgres::error::SqlState;
use ::postgres::types::{FromSql, IsNull, Kind, ToSql, Type, to_sql_checked};
use ::postgres::{Config, NoTls, Statement};
use bytes::BytesMut;
use fallible_iterator::FallibleIterator;
use postgres_protocol::types as pt;

use crate::runtime::handle::{Handle, HandleKind, PgConn};
use crate::runtime::interp::Interpreter;
use crate::runtime::map::Map;
use crate::runtime::value::Value;
use crate::stdlib::data::json::{parse_str, to_json_string};
use crate::stdlib::time::{civil_from_days, days_from_civil};
use crate::stdlib::{error_value, expect_args, new_array, new_map, str_arg, type_error};

/// 接続の待ち時間の既定（秒）
const DEFAULT_TIMEOUT: f64 = 10.0;
/// 覚えておく準備済みの文の数の上限
const MAX_STATEMENTS: usize = 64;
/// 2000-01-01 の Unix 時刻（秒）。PostgreSQL の日時の起点
const PG_EPOCH_UNIX: i64 = 946_684_800;
/// 1970-01-01 から 2000-01-01 までの日数
const PG_EPOCH_DAYS: i64 = 10_957;

type BoxErr = Box<dyn StdError + Sync + Send>;

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register_module(
        "std:postgres",
        &[
            ("postgres.connect", connect),
            ("postgres.close", close),
            ("postgres.exec", exec),
            ("postgres.query", query),
            ("postgres.run_script", run_script),
        ],
    );
}

// ---- 引数・エラー ----

fn handle_arg<'v>(name: &str, args: &'v [Value], i: usize) -> Result<&'v Rc<Handle>, String> {
    match &args[i] {
        Value::Handle(h) => Ok(h),
        other => Err(type_error(name, i, "ハンドル", other)),
    }
}

fn closed() -> Value {
    error_value("closed", "接続はすでに閉じています")
}

/// エラーとその原因を `: ` でつないだ文字列
fn chain(e: &dyn StdError) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let part = s.to_string();
        if !text.contains(&part) {
            text.push_str(": ");
            text.push_str(&part);
        }
        source = s.source();
    }
    text
}

fn has_io_source(e: &dyn StdError) -> bool {
    let mut source = e.source();
    while let Some(s) = source {
        if s.is::<std::io::Error>() {
            return true;
        }
        source = s.source();
    }
    false
}

/// SQLSTATE から、エラー値の種類を決める（design.md 5.47）
fn kind_of_sqlstate(code: &str) -> &'static str {
    match code {
        "55P03" | "40P01" | "40001" => "busy",
        c if c.starts_with("28") => "auth",
        c if c.starts_with("08") => "network",
        _ => "sql",
    }
}

fn failure(what: &str, e: &::postgres::Error) -> Value {
    if let Some(db) = e.as_db_error() {
        let code = db.code().code();
        return error_value(
            kind_of_sqlstate(code),
            format!("{what}: {} [{code}]", db.message()),
        );
    }
    let kind = if e.is_closed() || has_io_source(e) {
        "network"
    } else {
        "sql"
    };
    error_value(kind, format!("{what}: {}", chain(e)))
}

/// 接続の失敗。サーバーが返したもの（認証など）以外は、すべて `network`
fn connect_failure(e: &::postgres::Error) -> Value {
    if let Some(db) = e.as_db_error() {
        let code = db.code().code();
        return error_value(
            kind_of_sqlstate(code),
            format!("接続できません: {} [{code}]", db.message()),
        );
    }
    error_value("network", format!("接続できません: {}", chain(e)))
}

fn with_conn(
    name: &str,
    args: &[Value],
    f: impl FnOnce(&mut PgConn) -> Result<Value, String>,
) -> Result<Value, String> {
    let handle = handle_arg(name, args, 0)?;
    let mut inner = handle.inner.borrow_mut();
    match &mut *inner {
        HandleKind::Postgres(conn) => f(conn),
        HandleKind::PostgresClosed => Ok(closed()),
        _ => Err(format!(
            "{name} に渡せるのは PostgreSQL の接続のハンドルです（{handle}でした）"
        )),
    }
}

// ---- connect / close ----

/// 接続先の指定（URL かマップ）を読む。読めなければエラー値（`parse`）。型の間違いは実行時エラー
fn parse_target(target: &Value) -> Result<Result<Config, Value>, String> {
    const NAME: &str = "postgres.connect";
    let mut config = match target {
        Value::Str(url) => match Config::from_str(url) {
            Ok(config) => config,
            Err(e) => {
                return Ok(Err(error_value(
                    "parse",
                    format!("接続先の URL が読めません: {e}"),
                )));
            }
        },
        Value::Map(map) => {
            let mut config = Config::new();
            for (key, value) in map.borrow().iter() {
                match (&**key, value) {
                    ("host", Value::Str(s)) => {
                        config.host(s);
                    }
                    ("user", Value::Str(s)) => {
                        config.user(s);
                    }
                    ("password", Value::Str(s)) => {
                        config.password(s.as_bytes());
                    }
                    ("dbname", Value::Str(s)) => {
                        config.dbname(s);
                    }
                    ("port", Value::Int(n)) if (1..=65535).contains(n) => {
                        config.port(*n as u16);
                    }
                    ("port", Value::Num(n)) if n.fract() == 0.0 && (1.0..=65535.0).contains(n) => {
                        config.port(*n as u16);
                    }
                    ("port", Value::Int(_) | Value::Num(_)) => {
                        return Err(format!("{NAME} の `port` は 1〜65535 です"));
                    }
                    ("host" | "user" | "password" | "dbname" | "port", other) => {
                        return Err(format!(
                            "{NAME} の接続先 `{key}` の型が違います（{}が渡されました）",
                            other.type_name()
                        ));
                    }
                    _ => return Err(format!("{NAME} の接続先 `{key}` は知りません")),
                }
            }
            config
        }
        other => return Err(type_error(NAME, 0, "文字列かマップ", other)),
    };
    if config.get_hosts().is_empty() {
        config.host("localhost");
    }
    if config.get_user().is_none() {
        config.user("postgres");
    }
    Ok(Ok(config))
}

/// `options` を読んで config に反映する。返すのは `int` の指定
fn apply_options(config: &mut Config, options: &Value) -> Result<bool, String> {
    const NAME: &str = "postgres.connect";
    let Value::Map(options) = options else {
        return Err(type_error(NAME, 1, "マップ", options));
    };
    let mut ints = false;
    let mut timeout = DEFAULT_TIMEOUT;
    for (key, value) in options.borrow().iter() {
        match (&**key, value) {
            ("int", Value::Bool(b)) => ints = *b,
            ("timeout", Value::Num(n)) => {
                if !(n.is_finite() && *n > 0.0) {
                    return Err(format!("{NAME} の `timeout` は 0 より大きい数です（{n}）"));
                }
                timeout = *n;
            }
            ("timeout", Value::Int(n)) => {
                if *n <= 0 {
                    return Err(format!("{NAME} の `timeout` は 0 より大きい数です（{n}）"));
                }
                timeout = *n as f64;
            }
            ("application_name", Value::Str(s)) => {
                config.application_name(s);
            }
            ("tls", Value::Str(s)) => {
                let mode = match &**s {
                    "disable" => SslMode::Disable,
                    "prefer" => SslMode::Prefer,
                    "require" => SslMode::Require,
                    other => {
                        return Err(format!(
                            "{NAME} の `tls` は \"disable\" \"prefer\" \"require\" のどれかです（\"{other}\"）"
                        ));
                    }
                };
                config.ssl_mode(mode);
            }
            ("int" | "timeout" | "application_name" | "tls", other) => {
                return Err(format!(
                    "{NAME} のオプション `{key}` の型が違います（{}が渡されました）",
                    other.type_name()
                ));
            }
            _ => return Err(format!("{NAME} のオプション `{key}` は知りません")),
        }
    }
    // 巨大な値で Duration が作れずに落ちないように、1 日で打ち切る
    config.connect_timeout(Duration::from_secs_f64(timeout.min(86_400.0)));
    Ok(ints)
}

/// 表示用（パスワードは含めない）。`localhost:5432/appdb`
fn label_of(config: &Config) -> String {
    use ::postgres::config::Host;
    let host = match config.get_hosts().first() {
        Some(Host::Tcp(h)) => h.clone(),
        #[cfg(unix)]
        Some(Host::Unix(p)) => p.display().to_string(),
        None => "localhost".to_string(),
    };
    let port = config.get_ports().first().copied().unwrap_or(5432);
    match config.get_dbname() {
        Some(db) => format!("{host}:{port}/{db}"),
        None => format!("{host}:{port}"),
    }
}

fn tls_connector() -> tokio_postgres_rustls::MakeRustlsConnect {
    use std::sync::{Arc, OnceLock};

    static CONFIG: OnceLock<rustls::ClientConfig> = OnceLock::new();
    let config = CONFIG.get_or_init(|| {
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("ring は TLS 1.2 と 1.3 に対応している")
            .with_root_certificates(roots)
            .with_no_client_auth()
    });
    tokio_postgres_rustls::MakeRustlsConnect::new(config.clone())
}

fn connect(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    if !(1..=2).contains(&args.len()) {
        return Err(format!(
            "postgres.connect の引数は 1 個か 2 個です（{} 個渡されました）",
            args.len()
        ));
    }
    let mut config = match parse_target(&args[0])? {
        Ok(config) => config,
        Err(error) => {
            // 型の間違いを先に知らせるため、options の検査はここでも行う
            if let Some(options) = args.get(1) {
                apply_options(&mut Config::new(), options)?;
            }
            return Ok(error);
        }
    };
    let ints = match args.get(1) {
        Some(options) => apply_options(&mut config, options)?,
        None => {
            config.connect_timeout(Duration::from_secs_f64(DEFAULT_TIMEOUT));
            false
        }
    };
    let label = label_of(&config);
    let client = if config.get_ssl_mode() == SslMode::Disable {
        config.connect(NoTls)
    } else {
        config.connect(tls_connector())
    };
    let client = match client {
        Ok(client) => client,
        Err(e) => return Ok(connect_failure(&e)),
    };
    Ok(Value::Handle(Handle::new(HandleKind::Postgres(Box::new(
        PgConn {
            client,
            label,
            ints,
            statements: HashMap::new(),
        },
    )))))
}

fn close(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("postgres.close", args, 1)?;
    let handle = handle_arg("postgres.close", args, 0)?;
    if !matches!(
        &*handle.inner.borrow(),
        HandleKind::Postgres(_) | HandleKind::PostgresClosed
    ) {
        return Err(format!(
            "postgres.close に渡せるのは PostgreSQL の接続のハンドルです（{handle}でした）"
        ));
    }
    handle.close();
    Ok(Value::Nil)
}

// ---- 値の変換: Dolphin → PostgreSQL ----

/// 変換済みのパラメータ 1 つ（バイナリ形式。None は NULL）。`Value` は別のスレッドに渡せないので、先に変換しておく
#[derive(Debug)]
struct Encoded(Option<Vec<u8>>);

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

fn encode(value: &Value, ty: &Type, out: &mut BytesMut) -> Result<IsNull, BoxErr> {
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

fn encode_array(value: &Value, ty: &Type, elem: &Type, out: &mut BytesMut) -> Result<(), BoxErr> {
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

fn parse_uuid(s: &str) -> Result<[u8; 16], BoxErr> {
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

fn format_uuid(b: &[u8]) -> String {
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
fn parse_date(s: &str) -> Result<i32, BoxErr> {
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
fn parse_time(s: &str) -> Result<i64, BoxErr> {
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

fn unix_to_pg_micros(secs: f64) -> Result<i64, BoxErr> {
    let micros = (secs * 1e6).round();
    if !micros.is_finite() || micros.abs() >= 9.0e18 {
        return Err("日時が範囲を出ています".into());
    }
    Ok(micros as i64 - PG_EPOCH_UNIX * 1_000_000)
}

/// 10 進数の文字列を、`numeric` のバイナリ形式にする
fn numeric_to_sql(text: &str, out: &mut BytesMut) -> Result<(), BoxErr> {
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

// ---- 値の変換: PostgreSQL → Dolphin ----

/// 結果の 1 つの値の生のバイト列（NULL なら None）
struct Raw<'a>(Option<&'a [u8]>);

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

fn int_value(n: i64, ints: bool) -> Value {
    if ints {
        Value::Int(n)
    } else {
        Value::Num(n as f64)
    }
}

fn decode(raw: &[u8], ty: &Type, ints: bool) -> Result<Value, BoxErr> {
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

fn decode_simple(raw: &[u8], ty: &Type, ints: bool) -> Result<Value, BoxErr> {
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

fn decode_array(raw: &[u8], elem: &Type, ints: bool) -> Result<Value, BoxErr> {
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
fn numeric_from_sql(raw: &[u8]) -> Result<String, BoxErr> {
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

// ---- 文の準備と実行 ----

fn sql_args<'v>(name: &str, args: &'v [Value]) -> Result<(&'v str, Option<&'v Value>), String> {
    if !(2..=3).contains(&args.len()) {
        return Err(format!(
            "{name} の引数は 2 個か 3 個です（{} 個渡されました）",
            args.len()
        ));
    }
    Ok((str_arg(name, args, 1)?, args.get(2)))
}

/// `params` を検査して、値の並びにする。配列と `nil` 以外は実行時エラー
fn param_values(name: &str, params: Option<&Value>) -> Result<Vec<Value>, String> {
    match params {
        None | Some(Value::Nil) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let items = items.borrow();
            for item in items.iter() {
                match item {
                    Value::Nil
                    | Value::Bool(_)
                    | Value::Int(_)
                    | Value::Num(_)
                    | Value::Str(_)
                    | Value::Bytes(_)
                    | Value::Array(_)
                    | Value::Map(_) => {}
                    other => {
                        return Err(format!(
                            "{name} のパラメータに渡せるのは nil・真偽値・数値・整数・文字列・バイト列・配列・マップです（{}が渡されました）",
                            other.type_name()
                        ));
                    }
                }
            }
            Ok(items.clone())
        }
        Some(Value::Map(_)) => Err(format!(
            "{name} のパラメータは配列です（PostgreSQL に名前付きのパラメータはありません。`$1` `$2` に順番に入れます）"
        )),
        Some(other) => Err(type_error(name, 2, "配列", other)),
    }
}

/// 準備済みの文を取る。なければ準備して覚える
fn prepare(conn: &mut PgConn, sql: &str) -> Result<Statement, ::postgres::Error> {
    if let Some(stmt) = conn.statements.get(sql) {
        return Ok(stmt.clone());
    }
    let stmt = conn.client.prepare(sql)?;
    if conn.statements.len() >= MAX_STATEMENTS {
        conn.statements.clear();
    }
    conn.statements.insert(sql.to_string(), stmt.clone());
    Ok(stmt)
}

/// 覚えておいた文が、スキーマの変更で使えなくなったときのエラーか
fn is_stale(e: &::postgres::Error) -> bool {
    e.code() == Some(&SqlState::INVALID_SQL_STATEMENT_NAME)
        || (e.code() == Some(&SqlState::FEATURE_NOT_SUPPORTED)
            && e.as_db_error()
                .is_some_and(|d| d.message().contains("cached plan")))
}

enum Outcome {
    Rows(Vec<::postgres::Row>),
    Count(u64),
}

/// 文を準備して実行する。失敗はエラー値
fn run(conn: &mut PgConn, sql: &str, params: &[Value], want_rows: bool) -> Result<Outcome, Value> {
    for attempt in 0..2 {
        let stmt = prepare(conn, sql).map_err(|e| failure("SQL を準備できません", &e))?;
        if stmt.params().len() != params.len() {
            return Err(error_value(
                "sql",
                format!(
                    "パラメータの数が合いません（必要 {} 個、渡されたのは {} 個）",
                    stmt.params().len(),
                    params.len()
                ),
            ));
        }
        // サーバーが決めた型に合わせて、先にバイナリ形式にする
        let mut wrapped = Vec::with_capacity(params.len());
        for (i, (value, ty)) in params.iter().zip(stmt.params()).enumerate() {
            let mut buf = BytesMut::new();
            match encode(value, ty, &mut buf) {
                Ok(IsNull::Yes) => wrapped.push(Encoded(None)),
                Ok(IsNull::No) => wrapped.push(Encoded(Some(buf.to_vec()))),
                Err(e) => {
                    return Err(error_value("sql", format!("パラメータ ${}: {e}", i + 1)));
                }
            }
        }
        let refs: Vec<&(dyn ToSql + Sync)> = wrapped.iter().map(|p| p as _).collect();
        let result = if want_rows {
            conn.client.query(&stmt, &refs).map(Outcome::Rows)
        } else {
            conn.client.execute(&stmt, &refs).map(Outcome::Count)
        };
        match result {
            Ok(outcome) => return Ok(outcome),
            Err(e) if attempt == 0 && is_stale(&e) => conn.statements.clear(),
            Err(e) => return Err(failure("SQL を実行できません", &e)),
        }
    }
    unreachable!("2 回目の失敗は返る")
}

fn exec(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let (sql, params) = sql_args("postgres.exec", args)?;
    let params = param_values("postgres.exec", params)?;
    with_conn("postgres.exec", args, |conn| {
        Ok(match run(conn, sql, &params, false) {
            Ok(Outcome::Count(n)) => {
                let mut result = Map::default();
                result.insert("changes".into(), int_value(n as i64, conn.ints));
                new_map(result)
            }
            Ok(Outcome::Rows(_)) => unreachable!("exec は行を取らない"),
            Err(error) => error,
        })
    })
}

fn query(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let (sql, params) = sql_args("postgres.query", args)?;
    let params = param_values("postgres.query", params)?;
    with_conn("postgres.query", args, |conn| {
        let rows = match run(conn, sql, &params, true) {
            Ok(Outcome::Rows(rows)) => rows,
            Ok(Outcome::Count(_)) => unreachable!("query は行を取る"),
            Err(error) => return Ok(error),
        };
        let ints = conn.ints;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut map = Map::default();
            for (i, column) in row.columns().iter().enumerate() {
                let raw: Raw<'_> = match row.try_get(i) {
                    Ok(raw) => raw,
                    Err(e) => return Ok(failure("結果を読めません", &e)),
                };
                let value = match raw.0 {
                    None => Value::Nil,
                    Some(bytes) => match decode(bytes, column.type_(), ints) {
                        Ok(v) => v,
                        Err(e) => {
                            return Ok(error_value(
                                "sql",
                                format!("列 `{}` を読めません: {e}", column.name()),
                            ));
                        }
                    },
                };
                map.insert(Rc::from(column.name()), value);
            }
            out.push(new_map(map));
        }
        Ok(new_array(out))
    })
}

fn run_script(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("postgres.run_script", args, 2)?;
    let sql = str_arg("postgres.run_script", args, 1)?;
    with_conn("postgres.run_script", args, |conn| {
        // 文を作り直させるため、覚えている文は捨てる（スキーマが変わりうる）
        conn.statements.clear();
        Ok(match conn.client.batch_execute(sql) {
            Ok(()) => Value::Nil,
            Err(e) => failure("SQL を実行できません", &e),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
