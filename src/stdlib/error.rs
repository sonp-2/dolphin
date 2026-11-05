//! エラー値: is_error, err_kind, err_message, err_new, err_data（design.md 8.6・5.39）

use super::{expect_args, str_arg};
use crate::runtime::data::Data;
use crate::runtime::error::RuntimeError;
use crate::runtime::interp::Interpreter;
use crate::runtime::value::{ErrorValue, Value};

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register("is_error", is_error);
    interp.register("err_kind", err_kind);
    interp.register("err_message", err_message);
    interp.register("err_new", err_new);
    interp.register("err_data", err_data);
    interp.register("catch", catch);
}

/// 実行時エラーをエラー値にする（design.md 5.53）。種類は `"runtime"`、詳細に位置と呼び出し履歴
pub fn runtime_error_value(e: &RuntimeError) -> Value {
    let int = |n: usize| Data::Int(i64::try_from(n).unwrap_or(i64::MAX));
    let mut data = vec![
        ("line".to_string(), int(e.span.line)),
        ("col".to_string(), int(e.span.col)),
    ];
    if let Some(origin) = &e.origin {
        data.push(("file".to_string(), Data::Str(origin.path.clone())));
    }
    let trace = e
        .trace
        .iter()
        .map(|t| {
            let mut entry = vec![
                ("what".to_string(), Data::Str(t.what.clone())),
                ("line".to_string(), int(t.span.line)),
                ("col".to_string(), int(t.span.col)),
            ];
            if let Some(file) = &t.file {
                entry.push(("file".to_string(), Data::Str(file.to_string())));
            }
            Data::Map(entry)
        })
        .collect();
    data.push(("trace".to_string(), Data::Array(trace)));
    Value::error_with("runtime", e.message.as_str(), Some(Data::Map(data)))
}

/// `catch[f]`: `f` を引数なしで呼び、実行時エラーが起きたらエラー値にして返す（design.md 5.53）
fn catch(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("catch", args, 1)?;
    let f = &args[0];
    let is_async = match f {
        Value::Func(closure) => closure.def.is_async,
        Value::Native(_) => false,
        other => return Err(super::type_error("catch", 0, "関数", other)),
    };
    let result = match interp.call_value(f, Vec::new()) {
        Ok(value) => value,
        Err(message) => {
            // `os.exit` は捕まえない
            if interp.exit_code.is_some() {
                return Err(message);
            }
            match interp.take_native_error() {
                Some(e) => runtime_error_value(&e),
                None => Value::error("runtime", message),
            }
        }
    };
    if is_async {
        return Ok(super::async_::catching_future(interp, &result));
    }
    Ok(result)
}

fn is_error(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("is_error", args, 1)?;
    Ok(Value::Bool(matches!(args[0], Value::Error(_))))
}

/// エラー値の `kind`（`"not_found"` など）
fn err_kind(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("err_kind", args, 1)?;
    Ok(Value::Str(error_arg("err_kind", args)?.kind.clone()))
}

fn err_message(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("err_message", args, 1)?;
    Ok(Value::Str(error_arg("err_message", args)?.message.clone()))
}

/// 自分でエラー値を作る
fn err_new(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    if !(2..=3).contains(&args.len()) {
        return Err(format!(
            "err_new の引数は 2 個か 3 個です（{} 個渡されました）",
            args.len()
        ));
    }
    let kind = str_arg("err_new", args, 0)?;
    let message = str_arg("err_new", args, 1)?;
    // `nil` は「詳細なし」と同じ
    let data = args
        .get(2)
        .filter(|v| !matches!(v, Value::Nil))
        .map(Data::from_value);
    Ok(Value::error_with(kind, message, data))
}

/// エラー値の詳細データ。なければ `nil`
fn err_data(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("err_data", args, 1)?;
    Ok(match &error_arg("err_data", args)?.data {
        Some(data) => (**data).clone().into_value(),
        None => Value::Nil,
    })
}

fn error_arg<'v>(name: &str, args: &'v [Value]) -> Result<&'v ErrorValue, String> {
    match &args[0] {
        Value::Error(e) => Ok(e),
        other => Err(super::type_error(name, 0, "エラー値", other)),
    }
}
