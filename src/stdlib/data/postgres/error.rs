//! エラーの分類（SQLSTATE と接続の状態から、エラー値の種類を決める）

use std::error::Error as StdError;

use crate::runtime::value::Value;
use crate::stdlib::error_value;

pub(super) fn closed() -> Value {
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
pub(super) fn kind_of_sqlstate(code: &str) -> &'static str {
    match code {
        "55P03" | "40P01" | "40001" => "busy",
        c if c.starts_with("28") => "auth",
        c if c.starts_with("08") => "network",
        _ => "sql",
    }
}

pub(super) fn failure(what: &str, e: &::postgres::Error) -> Value {
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
pub(super) fn connect_failure(e: &::postgres::Error) -> Value {
    if let Some(db) = e.as_db_error() {
        let code = db.code().code();
        return error_value(
            kind_of_sqlstate(code),
            format!("接続できません: {} [{code}]", db.message()),
        );
    }
    error_value("network", format!("接続できません: {}", chain(e)))
}
