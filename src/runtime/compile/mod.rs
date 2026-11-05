//! AST をバイトコードにコンパイルする（design.md 5.45）
//!
//! 変数の置き場所は、ファイルの本体では名前で引く `Env`。関数のローカル変数は、
//! コンパイル時にスロット（番号）に解決して値のスタックの枠に置く（`compile_function`）。
//! 位置情報は、評価器が使っていた式・文の位置をそのまま命令に付ける
//!
//! 分け方: `stmt`（文）、`for_loop`（`for`）、`expr`（式）、`function`（関数の本体）

use std::collections::HashMap;
use std::rc::Rc;

use super::bytecode::{Chunk, Op};
use super::env::intern;
use super::value::Value;
use crate::syntax::ast::{
    BinaryOp, Expr, ExprKind, FuncDef, Pattern, Stmt, StmtKind, StrSegment, Var,
};
use crate::syntax::span::Span;
mod expr;
mod for_loop;
mod function;
mod stmt;

/// ファイルの本体をコンパイルする。`keep_last` で最後の文が式なら、その値が戻り値になる
pub fn compile_program(stmts: &[Stmt], keep_last: bool) -> Chunk {
    let mut c = Compiler::new(None);
    let end = stmts.last().map_or(Span { line: 1, col: 1 }, |s| s.span);
    match stmts.split_last() {
        Some((last, rest)) if keep_last => {
            c.block(rest);
            match &last.kind {
                StmtKind::Expr(expr) => c.expr(expr),
                _ => {
                    c.stmt(last);
                    c.nil(end);
                }
            }
        }
        _ => {
            c.block(stmts);
            c.nil(end);
        }
    }
    c.emit(Op::Return, end);
    c.chunk
}

/// 関数の本体をコンパイルする。引数の宣言は呼び出し側（VM）が行い、ここは既定値の評価から始まる。
///
/// ローカル変数（引数と、本体で宣言する変数）は、コンパイル時に番号（スロット）に解決して、
/// 呼び出しごとのスタックの枠に置く。ただし関数の中に無名関数（変数を捕まえうる）や分割代入が
/// あるとき、既定値が後ろの引数を読むときは、従来どおり名前で引く `Env` に置く
pub fn compile_function(def: &FuncDef) -> Chunk {
    let mut c = Compiler::new(Some(Slots::new(def)));
    c.function_body(def);
    if let Some(slots) = c.slots.as_ref().filter(|s| !s.bail) {
        c.chunk.locals = Some(slots.count);
        return c.chunk;
    }
    let mut c = Compiler::new(None);
    c.function_body(def);
    c.chunk
}

struct LoopCtx {
    /// ループに入ったときのスコープの深さ
    depth: u32,
    /// `break` の `Jump`（ループの外に飛ぶ）
    breaks: Vec<usize>,
    /// `continue` の `Jump`（次の繰り返しへ飛ぶ）
    continues: Vec<usize>,
}

/// 関数のローカル変数をスロットに解決するための状態
struct Slots {
    /// 外側から内側へのスコープ。名前 → スロット
    scopes: Vec<Vec<(String, u32)>>,
    /// 使ったスロットの数
    count: u32,
    /// まだスコープに入っていない引数の名前（既定値が後ろの引数を読むと、スロットでは同じ意味にできない）
    pending: Vec<String>,
    /// スロットにできないものが見つかった
    bail: bool,
}

impl Slots {
    fn new(def: &FuncDef) -> Self {
        let mut slots = Self {
            scopes: vec![Vec::new()],
            count: def.params.len() as u32,
            pending: def.params.iter().map(|p| p.name.clone()).collect(),
            bail: false,
        };
        // 引数は先頭のスロット、続けて残りの引数（`...@rest`）
        if let Some(rest) = &def.rest {
            let slot = slots.count;
            slots.count += 1;
            slots.scopes[0].push((rest.clone(), slot));
        }
        slots
    }

    fn resolve(&mut self, name: &str) -> Option<u32> {
        if self.pending.iter().any(|p| p == name) {
            self.bail = true;
            return None;
        }
        self.scopes
            .iter()
            .rev()
            .flat_map(|scope| scope.iter().rev())
            .find(|(n, _)| n == name)
            .map(|&(_, slot)| slot)
    }

    /// いまのスコープに変数を作る。同じスコープに同じ名前があれば、そのスロットを使い回す
    fn declare(&mut self, name: &str) -> u32 {
        let scope = self.scopes.last_mut().expect("スコープがある");
        if let Some(&(_, slot)) = scope.iter().find(|(n, _)| n == name) {
            return slot;
        }
        let slot = self.count;
        self.count += 1;
        scope.push((name.to_string(), slot));
        slot
    }
}

struct Compiler {
    chunk: Chunk,
    names: HashMap<String, u32>,
    scope_depth: u32,
    loops: Vec<LoopCtx>,
    /// 関数のローカル変数をスロットにするとき。ファイルの本体とフォールバックでは `None`
    slots: Option<Slots>,
}

impl Compiler {
    fn new(slots: Option<Slots>) -> Self {
        Self {
            chunk: Chunk {
                code: Vec::new(),
                spans: Vec::new(),
                consts: Vec::new(),
                names: Vec::new(),
                locals: None,
                param_names: Vec::new(),
                rest_name: None,
                defs: Vec::new(),
                patterns: Vec::new(),
            },
            names: HashMap::new(),
            scope_depth: 0,
            loops: Vec::new(),
            slots,
        }
    }

    /// スロットにできない書き方が出てきた（そのときは `Env` で作り直す）
    pub(super) fn bail(&mut self) {
        if let Some(slots) = &mut self.slots {
            slots.bail = true;
        }
    }

    /// スタックの値で変数を作る命令
    pub(super) fn emit_declare(&mut self, name: &str, span: Span) {
        if let Some(slots) = &mut self.slots {
            let slot = slots.declare(name);
            self.emit(Op::StoreLocal(slot), span);
        } else {
            let name = self.name(name);
            self.emit(Op::Declare(name), span);
        }
    }

    pub(super) fn emit_load(&mut self, name: &str, span: Span) {
        let slot = self.slots.as_mut().and_then(|s| s.resolve(name));
        if let Some(slot) = slot {
            self.emit(Op::LoadLocal(slot), span);
        } else {
            let name = self.name(name);
            self.emit(Op::LoadVar(name), span);
        }
    }

    pub(super) fn emit_assign(&mut self, name: &str, span: Span) {
        let slot = self.slots.as_mut().and_then(|s| s.resolve(name));
        if let Some(slot) = slot {
            self.emit(Op::StoreLocal(slot), span);
        } else {
            let name = self.name(name);
            self.emit(Op::AssignVar(name), span);
        }
    }

    pub(super) fn emit(&mut self, op: Op, span: Span) -> usize {
        self.chunk.code.push(op);
        self.chunk.spans.push(span);
        self.chunk.code.len() - 1
    }

    pub(super) fn here(&self) -> u32 {
        self.chunk.code.len() as u32
    }

    /// `at` の命令の行き先を、いまの位置にする
    pub(super) fn patch_here(&mut self, at: usize) {
        let target = self.here();
        self.patch(at, target);
    }

    pub(super) fn patch(&mut self, at: usize, to: u32) {
        match &mut self.chunk.code[at] {
            Op::Jump(t)
            | Op::JumpIfFalseCond(t)
            | Op::ShortCircuit(_, t)
            | Op::SkipIfGiven { target: t, .. }
            | Op::ForNext { end: t, .. }
            | Op::ForNextLocal { end: t, .. } => *t = to,
            other => unreachable!("行き先のない命令を書き換えようとした: {other:?}"),
        }
    }

    pub(super) fn name(&mut self, name: &str) -> u32 {
        if let Some(&i) = self.names.get(name) {
            return i;
        }
        let i = self.chunk.names.len() as u32;
        self.chunk.names.push(intern(name));
        self.names.insert(name.to_string(), i);
        i
    }

    pub(super) fn constant(&mut self, value: Value, span: Span) {
        self.chunk.consts.push(value);
        let i = (self.chunk.consts.len() - 1) as u32;
        self.emit(Op::Const(i), span);
    }

    pub(super) fn nil(&mut self, span: Span) {
        self.constant(Value::Nil, span);
    }
}
