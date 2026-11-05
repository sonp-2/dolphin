//! `std:async`: after, every, cancel, run, stop（design.md 5.23）と、非同期の関数の共通処理

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use super::{array_arg, error_value, expect_args, int_arg, num_arg, type_error};
use crate::runtime::data::Data;
use crate::runtime::events::Event;
use crate::runtime::handle::{Forward, FutureOutcome, Handle, HandleKind};
use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register_module(
        "std:async",
        &[
            ("async.after", after),
            ("async.every", every),
            ("async.cancel", cancel),
            ("async.run", run),
            ("async.stop", stop),
            ("async.sleep", sleep),
            ("async.race", race),
            ("async.timeout", timeout),
        ],
    );
}

/// `std:async` を `import` したときに足す、Dolphin で書いた関数（design.md 5.46）
pub const PRELUDE: &str = "async all[@futures] (
    @results := {}
    for @f in @futures (
        arr_push[@results, await @f]
    )
    # @results
)
";

/// 最後の引数のコールバックと、残りの引数に分ける
pub fn split_callback<'v>(name: &str, args: &'v [Value]) -> Result<(&'v [Value], Value), String> {
    let Some((last, rest)) = args.split_last() else {
        return Err(format!("{name} の最後の引数はコールバックの関数です"));
    };
    match last {
        Value::Func(_) | Value::Native(_) => Ok((rest, last.clone())),
        other => Err(format!(
            "{name} の最後の引数はコールバックの関数です（{}が渡されました）",
            other.type_name()
        )),
    }
}

/// 最後の引数がコールバックなら、それと残りの引数に分ける。コールバックがなければ、すべて引数
/// （コールバックを省いた非同期の関数は、future を返す。design.md 5.46）
pub fn split_callback_opt(args: &[Value]) -> (&[Value], Option<Value>) {
    match args.split_last() {
        Some((last @ (Value::Func(_) | Value::Native(_)), rest)) => (rest, Some(last.clone())),
        _ => (args, None),
    }
}

/// `spawn_blocking` のコールバックを省ける版。省いたら future を返す
pub fn spawn_blocking_opt(
    interp: &mut Interpreter<'_>,
    callback: Option<Value>,
    work: impl FnOnce() -> Value + Send + 'static,
) -> Result<Value, String> {
    match callback {
        Some(callback) => spawn_blocking(interp, callback, work),
        None => {
            let future = Handle::new_future();
            let id = interp.events.register(future.clone(), None)?;
            let tx = interp.events.sender();
            thread::spawn(move || {
                let data = Data::from_value(&work());
                let _ = tx.send(Event::Done { id, data });
            });
            Ok(future)
        }
    }
}

/// `work` を補助スレッドで実行し、結果を `callback` に渡すように登録する。戻り値は番号。
/// `work` が作る `Value` は、そのスレッドの中で `Data` にしてから送る
pub fn spawn_blocking(
    interp: &mut Interpreter<'_>,
    callback: Value,
    work: impl FnOnce() -> Value + Send + 'static,
) -> Result<Value, String> {
    let id = interp.events.register(callback, None)?;
    let tx = interp.events.sender();
    thread::spawn(move || {
        let data = Data::from_value(&work());
        let _ = tx.send(Event::Done { id, data });
    });
    Ok(Value::Num(id as f64))
}

fn seconds(name: &str, args: &[Value], allow_zero: bool) -> Result<Duration, String> {
    let n = num_arg(name, args, 0)?;
    if n.is_finite() && (n > 0.0 || (allow_zero && n == 0.0)) {
        Ok(Duration::from_secs_f64(n))
    } else {
        let limit = if allow_zero {
            "0 以上の数"
        } else {
            "0 より大きい数"
        };
        Err(format!("{name} の秒数は{limit}です（{n}）"))
    }
}

fn callback_arg(name: &str, args: &[Value], i: usize) -> Result<Value, String> {
    match &args[i] {
        f @ (Value::Func(_) | Value::Native(_)) => Ok(f.clone()),
        other => Err(type_error(name, i, "関数", other)),
    }
}

fn after(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("async.after", args, 2)?;
    let delay = seconds("async.after", args, true)?;
    let callback = callback_arg("async.after", args, 1)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let id = interp
        .events
        .register(callback, Some((Arc::clone(&cancelled), false)))?;
    let tx = interp.events.sender();
    thread::spawn(move || {
        thread::sleep(delay);
        if !cancelled.load(Ordering::SeqCst) {
            let _ = tx.send(Event::Tick { id });
        }
    });
    Ok(Value::Num(id as f64))
}

fn every(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("async.every", args, 2)?;
    let interval = seconds("async.every", args, false)?;
    let callback = callback_arg("async.every", args, 1)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let id = interp
        .events
        .register(callback, Some((Arc::clone(&cancelled), true)))?;
    let tx = interp.events.sender();
    thread::spawn(move || {
        loop {
            thread::sleep(interval);
            if cancelled.load(Ordering::SeqCst) || tx.send(Event::Tick { id }).is_err() {
                break;
            }
        }
    });
    Ok(Value::Num(id as f64))
}

/// `seconds` 秒後に `nil` で終わる future
fn sleep(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("async.sleep", args, 1)?;
    let delay = seconds("async.sleep", args, true)?;
    let future = Handle::new_future();
    let cancelled = Arc::new(AtomicBool::new(false));
    let id = interp
        .events
        .register(future.clone(), Some((Arc::clone(&cancelled), false)))?;
    let tx = interp.events.sender();
    thread::spawn(move || {
        thread::sleep(delay);
        let _ = tx.send(Event::Tick { id });
    });
    Ok(future)
}

/// future の今の状態。終わっていれば結果（失敗は受け取ったことにする）、まだなら `Some(None)`。
/// future でなければ `None`
fn peek_future(value: &Value) -> Option<Option<FutureOutcome>> {
    if let Value::Handle(handle) = value
        && let HandleKind::Future(state) = &mut *handle.inner.borrow_mut()
    {
        state.observed = true;
        return Some(match &state.outcome {
            FutureOutcome::Pending => None,
            done => Some(done.clone()),
        });
    }
    None
}

/// `future` が終わったとき、`forward` に従って転送先を終わらせるようにする
fn add_forward(future: &Value, forward: Forward) {
    if let Value::Handle(handle) = future
        && let HandleKind::Future(state) = &mut *handle.inner.borrow_mut()
    {
        state.forwards.push(forward);
    }
}

/// `catch[async 関数]` の結果（design.md 5.53）。`result` は `catch` が受け取った値で、
/// future なら、失敗を実行時エラーのエラー値にして成功させる future を返す。future でなければ（始める
/// ときのエラーのエラー値）、それで成功済みの future にする
pub fn catching_future(interp: &mut Interpreter<'_>, result: &Value) -> Value {
    let wrapped = Handle::new_future();
    match peek_future(result) {
        None => interp.settle_future(&wrapped, FutureOutcome::Resolved(result.clone())),
        Some(Some(FutureOutcome::Failed(e))) => interp.settle_future(
            &wrapped,
            FutureOutcome::Resolved(super::error::runtime_error_value(&e)),
        ),
        Some(Some(done)) => interp.settle_future(&wrapped, done),
        Some(None) => add_forward(
            result,
            Forward {
                target: wrapped.clone(),
                replace: None,
                cancel_timer: None,
                catch: true,
            },
        ),
    }
    wrapped
}

/// 最初に終わった future の結果で終わる future（design.md 5.50）
fn race(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("async.race", args, 1)?;
    let futures: Vec<Value> = array_arg("async.race", args, 0)?.borrow().clone();
    if futures.is_empty() {
        return Err("async.race の配列が空です（future が 1 つ以上要ります）".into());
    }
    // 先に全部を検査する（途中で転送だけが残らないように）
    for (i, future) in futures.iter().enumerate() {
        if peek_future(future).is_none() {
            return Err(format!(
                "async.race の {} 番目の要素は future です（{}が渡されました）",
                i + 1,
                future.type_name()
            ));
        }
    }
    let result = Handle::new_future();
    for future in &futures {
        if let Some(Some(outcome)) = peek_future(future) {
            interp.settle_future(&result, outcome);
            break;
        }
        add_forward(
            future,
            Forward {
                target: result.clone(),
                replace: None,
                cancel_timer: None,
                catch: false,
            },
        );
    }
    Ok(result)
}

/// 時間内に終わらなければ、`timeout` のエラー値で終わる future（design.md 5.50）
fn timeout(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("async.timeout", args, 2)?;
    let limit = {
        let n = num_arg("async.timeout", args, 1)?;
        if !(n.is_finite() && n > 0.0) {
            return Err(format!("async.timeout の秒数は 0 より大きい数です（{n}）"));
        }
        Duration::try_from_secs_f64(n)
            .map_err(|_| format!("async.timeout の秒数が大きすぎます（{n}）"))?
    };
    let future = &args[0];
    match peek_future(future) {
        None => return Err(type_error("async.timeout", 0, "future", future)),
        Some(Some(_)) => return Ok(future.clone()),
        Some(None) => {}
    }
    let result = Handle::new_future();
    let timer = Handle::new_future();
    let cancelled = Arc::new(AtomicBool::new(false));
    let id = interp
        .events
        .register(timer.clone(), Some((Arc::clone(&cancelled), false)))?;
    let tx = interp.events.sender();
    thread::spawn(move || {
        thread::sleep(limit);
        let _ = tx.send(Event::Tick { id });
    });
    add_forward(
        future,
        Forward {
            target: result.clone(),
            replace: None,
            cancel_timer: Some(id),
            catch: false,
        },
    );
    add_forward(
        &timer,
        Forward {
            target: result.clone(),
            replace: Some(error_value(
                "timeout",
                format!("{} 秒以内に終わりませんでした", args[1]),
            )),
            cancel_timer: None,
            catch: false,
        },
    );
    Ok(result)
}

fn cancel(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("async.cancel", args, 1)?;
    let id = int_arg("async.cancel", args, 0)?;
    if let Ok(id) = u64::try_from(id) {
        interp.events.cancel(id);
    }
    Ok(Value::Nil)
}

fn run(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("async.run", args, 0)?;
    interp.run_event_loop()?;
    Ok(Value::Nil)
}

fn stop(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("async.stop", args, 0)?;
    interp.events.stop();
    Ok(Value::Nil)
}
