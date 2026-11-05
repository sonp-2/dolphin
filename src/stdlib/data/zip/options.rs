//! オプションのマップ（`level`・`gzip`・`max_size`）の読み取り

use super::{DEFAULT_LEVEL, DEFAULT_MAX_SIZE};
use crate::runtime::value::Value;
use crate::stdlib::type_error;

pub(super) struct Opts {
    pub(super) level: u32,
    pub(super) gzip: bool,
    pub(super) max_size: u64,
}

/// 第 `n` 引数までが必須で、その次にオプションのマップを 1 つ取れる。`allowed` 以外のキーは間違い
pub(super) fn options(
    name: &str,
    args: &[Value],
    n: usize,
    allowed: &[&str],
) -> Result<Opts, String> {
    if args.len() != n && args.len() != n + 1 {
        return Err(format!(
            "{name} の引数は {n} 個か {} 個です（{} 個渡されました）",
            n + 1,
            args.len()
        ));
    }
    let mut opts = Opts {
        level: DEFAULT_LEVEL,
        gzip: false,
        max_size: DEFAULT_MAX_SIZE,
    };
    let Some(options) = args.get(n) else {
        return Ok(opts);
    };
    let Value::Map(options) = options else {
        return Err(type_error(name, n, "マップ", options));
    };
    for (key, value) in options.borrow().iter() {
        if !allowed.contains(&&**key) {
            return Err(format!("{name} のオプション `{key}` は知りません"));
        }
        match (&**key, value) {
            ("level", Value::Num(x)) if x.fract() == 0.0 && (0.0..=9.0).contains(x) => {
                opts.level = *x as u32;
            }
            ("level", Value::Num(x)) => {
                return Err(format!("{name} の `level` は 0〜9 の整数です（{x}）"));
            }
            ("gzip", Value::Bool(b)) => opts.gzip = *b,
            ("max_size", Value::Num(x)) if x.is_finite() && *x >= 0.0 => {
                opts.max_size = *x as u64;
            }
            ("max_size", Value::Int(x)) if *x >= 0 => opts.max_size = *x as u64,
            ("max_size", Value::Num(_) | Value::Int(_)) => {
                return Err(format!("{name} の `max_size` は 0 以上の数です"));
            }
            (_, other) => {
                let expected = match &**key {
                    "gzip" => "真偽値",
                    _ => "数値",
                };
                return Err(format!(
                    "{name} のオプション `{key}` は{expected}です（{}が渡されました）",
                    other.type_name()
                ));
            }
        }
    }
    Ok(opts)
}
