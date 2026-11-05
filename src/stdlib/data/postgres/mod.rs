//! `std:postgres`: connect, close, exec, query, run_script（design.md 5.47）
//!
//! 純 Rust の同期クライアント `postgres`（`postgres` feature）を使う。
//! パラメータと結果は、サーバーが決めた型に合わせて、バイナリ形式で自前に変換する

mod connect;
mod decode;
mod encode;
mod error;
mod exec;

use std::error::Error as StdError;
use std::rc::Rc;

use crate::runtime::handle::{Handle, HandleKind, PgConn};
use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;
use crate::stdlib::type_error;
use error::closed;

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
            ("postgres.connect", connect::connect),
            ("postgres.close", connect::close),
            ("postgres.exec", exec::exec),
            ("postgres.query", exec::query),
            ("postgres.run_script", exec::run_script),
        ],
    );
}

fn handle_arg<'v>(name: &str, args: &'v [Value], i: usize) -> Result<&'v Rc<Handle>, String> {
    match &args[i] {
        Value::Handle(h) => Ok(h),
        other => Err(type_error(name, i, "ハンドル", other)),
    }
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

#[cfg(test)]
mod tests;
