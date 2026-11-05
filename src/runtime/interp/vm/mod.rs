//! バイトコード VM（design.md 5.45）
//!
//! 関数の呼び出し 1 回分の枠（`VmFrame`）と値のスタックをヒープに持つ。Dolphin の関数を呼ぶとき
//! Rust の再帰を使わない。組み込み関数が関数値を呼ぶ（`call_value`）ときだけ、入れ子で `run_vm` が動く。
//!
//! 変数の置き場所は、ファイルの本体、無名関数を含む関数では名前で引く `Env` の連なり。
//! 無名関数や分割代入を含まない関数のローカル変数は、値のスタックの枠（`base` からの位置）に置く。`Interpreter` の
//! `current`（いまのモジュール）と `env`（いまのスコープ）は、いま動いている枠のものを指し続ける。
//! 枠を作るときに呼び出し元の分を枠に預け、枠を抜けるときに戻す

//!
//! 分け方: `frame`（枠の作成と呼び出し）、`unwind`（戻りとエラーの巻き戻し、`defer`）、`coroutine`（`async` 関数の流れ）、
//! `step`（命令の振り分け）と、命令の種類ごとの `control`・`collection`・`call`・`for_loop`、二項演算の `arith`

use std::cell::RefCell;
use std::mem;
use std::rc::Rc;

use super::{
    Closure, Env, FuncDef, Interpreter, MAX_DEPTH, Map, Module, RResult, RuntimeError, TraceEntry,
    Value, compare, file_of, func_label, index_get, index_set, int_overflow, module_label,
    not_callable, quoted_label, reject_module, to_key, undefined_name,
};
use crate::runtime::bytecode::{Chunk, Op};
use crate::runtime::compile::compile_function;
use crate::runtime::gc;
use crate::runtime::handle::{FutureOutcome, Handle, HandleKind};
use crate::sema::resolver;
use crate::syntax::ast::{BinaryOp, Pattern, UnaryOp};
use crate::syntax::span::Span;

/// 枠の種類
pub(super) enum FrameKind {
    /// 関数の呼び出し。`def` は呼び出し履歴の名前を作るため、`call_span` は呼び出し位置
    Function { def: Rc<FuncDef>, call_span: Span },
    /// ファイルの本体（`import` したファイルを含む）
    Program,
}

/// 関数の呼び出し 1 回分（またはファイルの本体 1 つ分）の状態
pub(super) struct VmFrame {
    chunk: Rc<Chunk>,
    ip: usize,
    /// 値のスタックで、この枠が使い始める位置
    base: usize,
    kind: FrameKind,
    /// この枠のコードが属するモジュール
    module: Rc<Module>,
    /// 渡された引数の数（既定値を評価するか決める）
    given: usize,
    /// `PushScope` で入る前のスコープ（`PopScope` で戻る）
    scopes: Vec<Rc<Env>>,
    /// `defer` で登録した後始末（関数値と登録位置）。抜けるとき、逆順に呼ぶ（design.md 5.53）
    defers: Vec<(Value, Span)>,
    /// 呼び出し元のモジュールとスコープ。枠を抜けるときに戻す
    caller_module: Rc<Module>,
    caller_env: Rc<Env>,
}

/// 1 命令を実行した後にどうするか
enum Step {
    Next,
    /// 枠が変わった（呼び出しか戻り）。枠から読み直す
    Reload,
    /// 入口の枠が戻った
    Done(Value),
    /// `await` で止まった（`async` 関数の流れの中だけ）
    Suspend,
}

/// `run_vm_exit` の終わり方
pub(super) enum Exit {
    Return(Value),
    Suspend,
}

/// 止められる流れ（`async` 関数の呼び出し 1 つ分。design.md 5.46）。
/// 自分の枠のスタックと値のスタックを持ち、止まっている間はここに預ける
pub(super) struct Coroutine {
    frames: Vec<VmFrame>,
    stack: Vec<Value>,
    depth: usize,
    /// 止まった時点の、いまのモジュールとスコープ
    current: Rc<Module>,
    env: Rc<Env>,
    /// この流れの結果を受け取る future
    future: Value,
}

mod arith;
mod call;
mod collection;
mod control;
mod coroutine;
mod for_loop;
mod frame;
mod matching;
mod step;
mod unwind;

use arith::{binary_values, fast_binary};

impl Interpreter<'_> {
    /// `target` 枚の枠が残るまで（積んだ入口の枠が戻るまで）命令を実行して、その戻り値を返す。
    /// 止められない場所なので、`await` はイベントループを回して待つ
    pub(super) fn run_vm(&mut self, target: usize) -> RResult<Value> {
        match self.run_vm_exit(target)? {
            Exit::Return(value) => Ok(value),
            Exit::Suspend => unreachable!("止められるのは流れの入口の実行だけ"),
        }
    }

    /// `run_vm` と同じ。`async` 関数の流れの入口なら、`await` で止まって `Exit::Suspend` を返すことがある
    fn run_vm_exit(&mut self, target: usize) -> RResult<Exit> {
        loop {
            let (chunk, mut ip, base) = {
                let frame = self.vm_frames.last().expect("実行する枠がある");
                (Rc::clone(&frame.chunk), frame.ip, frame.base)
            };
            loop {
                let op = &chunk.code[ip];
                // よく使う命令は、`step` を通さずここで済ませる（できないときは `step` に任せる）
                match op {
                    Op::Const(i) => {
                        self.stack.push(chunk.consts[*i as usize].clone());
                        ip += 1;
                        continue;
                    }
                    Op::LoadLocal(slot) => {
                        let value = self.stack[base + *slot as usize].clone();
                        self.stack.push(value);
                        ip += 1;
                        continue;
                    }
                    Op::StoreLocal(slot) => {
                        let value = self.pop();
                        self.stack[base + *slot as usize] = value;
                        ip += 1;
                        continue;
                    }
                    Op::LoadVar(n) => {
                        if let Some(value) = self.env.get(&chunk.names[*n as usize]) {
                            self.stack.push(value);
                            ip += 1;
                            continue;
                        }
                    }
                    Op::Declare(n) => {
                        let value = self.pop();
                        self.env.declare_rc(&chunk.names[*n as usize], value);
                        ip += 1;
                        continue;
                    }
                    Op::AssignVar(n) => {
                        let value = self.pop();
                        let name = &chunk.names[*n as usize];
                        if !self.env.assign(name, value) {
                            return Err(self.unwind(undefined_name(name, chunk.spans[ip]), target));
                        }
                        ip += 1;
                        continue;
                    }
                    Op::Pop => {
                        self.pop();
                        ip += 1;
                        continue;
                    }
                    // 外側の match へまとめると、エラー値のときに下の処理へ落ちる流れが変わるのでまとめない
                    #[allow(clippy::collapsible_match)]
                    Op::PopCheckError => {
                        if !matches!(self.stack.last(), Some(Value::Error(_))) {
                            self.stack.pop();
                            ip += 1;
                            continue;
                        }
                    }
                    Op::Binary(bop) => {
                        let len = self.stack.len();
                        if let Some(value) =
                            fast_binary(*bop, &self.stack[len - 2], &self.stack[len - 1])
                        {
                            self.stack.truncate(len - 2);
                            self.stack.push(value);
                            ip += 1;
                            continue;
                        }
                    }
                    Op::JumpIfFalseCond(t) => {
                        if let Some(Value::Bool(b)) = self.stack.last() {
                            if !*b {
                                ip = *t as usize;
                            } else {
                                ip += 1;
                            }
                            self.stack.pop();
                            continue;
                        }
                    }
                    Op::Jump(t) => {
                        ip = *t as usize;
                        continue;
                    }
                    Op::LoopTick => {
                        gc::maybe_collect();
                        ip += 1;
                        continue;
                    }
                    _ => {}
                }
                let span = chunk.spans[ip];
                ip += 1;
                match self.step(op, span, &chunk, &mut ip, base, target) {
                    Ok(Step::Next) => {}
                    Ok(Step::Reload) => break,
                    Ok(Step::Done(value)) => return Ok(Exit::Return(value)),
                    Ok(Step::Suspend) => return Ok(Exit::Suspend),
                    Err(e) => return Err(self.unwind(e, target)),
                }
            }
        }
    }

    pub(super) fn pop(&mut self) -> Value {
        self.stack.pop().expect("スタックに値がある")
    }

    pub(super) fn split_top(&mut self, n: usize) -> Vec<Value> {
        let at = self.stack.len() - n;
        self.stack.split_off(at)
    }

    /// いまの枠の続きの位置を覚える（別の枠へ移る前に）
    pub(super) fn save_ip(&mut self, ip: usize) {
        if let Some(frame) = self.vm_frames.last_mut() {
            frame.ip = ip;
        }
    }
}
