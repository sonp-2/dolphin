//! 文の準備と実行（`postgres.exec` / `postgres.query` / `postgres.run_script`）

use std::rc::Rc;

use ::postgres::Statement;
use ::postgres::error::SqlState;
use ::postgres::types::{IsNull, ToSql};
use bytes::BytesMut;

use super::decode::{Raw, decode, int_value};
use super::encode::{Encoded, encode};
use super::error::failure;
use super::{MAX_STATEMENTS, with_conn};
use crate::runtime::handle::PgConn;
use crate::runtime::interp::Interpreter;
use crate::runtime::map::Map;
use crate::runtime::value::Value;
use crate::stdlib::{error_value, expect_args, new_array, new_map, str_arg, type_error};

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

pub(super) fn exec(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
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

pub(super) fn query(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
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

pub(super) fn run_script(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
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
