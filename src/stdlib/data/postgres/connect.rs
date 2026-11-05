//! 接続とオプション（`postgres.connect` / `postgres.close`）

use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use ::postgres::config::SslMode;
use ::postgres::{Config, NoTls};

use super::error::connect_failure;
use super::{DEFAULT_TIMEOUT, handle_arg};
use crate::runtime::handle::{Handle, HandleKind, PgConn};
use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;
use crate::stdlib::{error_value, expect_args, type_error};

pub(super) fn parse_target(target: &Value) -> Result<Result<Config, Value>, String> {
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
pub(super) fn label_of(config: &Config) -> String {
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

pub(super) fn connect(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
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

pub(super) fn close(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
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
