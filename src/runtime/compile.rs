//! AST をバイトコードにコンパイルする（design.md 5.45）
//!
//! 変数の置き場所は、ファイルの本体では名前で引く `Env`。関数のローカル変数は、
//! コンパイル時にスロット（番号）に解決して値のスタックの枠に置く（`compile_function`）。
//! 位置情報は、評価器が使っていた式・文の位置をそのまま命令に付ける

use std::collections::HashMap;
use std::rc::Rc;

use super::bytecode::{Chunk, Op};
use super::env::intern;
use super::value::Value;
use crate::syntax::ast::{BinaryOp, Expr, ExprKind, FuncDef, Stmt, StmtKind, StrSegment};
use crate::syntax::span::Span;

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

    fn function_body(&mut self, def: &FuncDef) {
        self.chunk.param_names = def.params.iter().map(|p| intern(&p.name)).collect();
        self.chunk.rest_name = def.rest.as_deref().map(intern);
        for (i, param) in def.params.iter().enumerate() {
            if let Some(default) = &param.default {
                let skip = self.emit(
                    Op::SkipIfGiven {
                        index: i as u32,
                        target: 0,
                    },
                    default.span,
                );
                self.expr(default);
                if self.slots.is_some() {
                    self.emit(Op::StoreLocal(i as u32), default.span);
                } else {
                    let name = self.name(&param.name);
                    self.emit(Op::Declare(name), default.span);
                }
                self.patch_here(skip);
            }
            if let Some(slots) = &mut self.slots {
                slots.pending.retain(|p| p != &param.name);
                slots.scopes[0].push((param.name.clone(), i as u32));
            }
        }
        self.block(&def.body);
        self.nil(def.span);
        self.emit(Op::Return, def.span);
    }

    /// スロットにできない書き方が出てきた（そのときは `Env` で作り直す）
    fn bail(&mut self) {
        if let Some(slots) = &mut self.slots {
            slots.bail = true;
        }
    }

    /// スタックの値で変数を作る命令
    fn emit_declare(&mut self, name: &str, span: Span) {
        if let Some(slots) = &mut self.slots {
            let slot = slots.declare(name);
            self.emit(Op::StoreLocal(slot), span);
        } else {
            let name = self.name(name);
            self.emit(Op::Declare(name), span);
        }
    }

    fn emit_load(&mut self, name: &str, span: Span) {
        let slot = self.slots.as_mut().and_then(|s| s.resolve(name));
        if let Some(slot) = slot {
            self.emit(Op::LoadLocal(slot), span);
        } else {
            let name = self.name(name);
            self.emit(Op::LoadVar(name), span);
        }
    }

    fn emit_assign(&mut self, name: &str, span: Span) {
        let slot = self.slots.as_mut().and_then(|s| s.resolve(name));
        if let Some(slot) = slot {
            self.emit(Op::StoreLocal(slot), span);
        } else {
            let name = self.name(name);
            self.emit(Op::AssignVar(name), span);
        }
    }

    fn emit(&mut self, op: Op, span: Span) -> usize {
        self.chunk.code.push(op);
        self.chunk.spans.push(span);
        self.chunk.code.len() - 1
    }

    fn here(&self) -> u32 {
        self.chunk.code.len() as u32
    }

    /// `at` の命令の行き先を、いまの位置にする
    fn patch_here(&mut self, at: usize) {
        let target = self.here();
        self.patch(at, target);
    }

    fn patch(&mut self, at: usize, to: u32) {
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

    fn name(&mut self, name: &str) -> u32 {
        if let Some(&i) = self.names.get(name) {
            return i;
        }
        let i = self.chunk.names.len() as u32;
        self.chunk.names.push(intern(name));
        self.names.insert(name.to_string(), i);
        i
    }

    fn constant(&mut self, value: Value, span: Span) {
        self.chunk.consts.push(value);
        let i = (self.chunk.consts.len() - 1) as u32;
        self.emit(Op::Const(i), span);
    }

    fn nil(&mut self, span: Span) {
        self.constant(Value::Nil, span);
    }

    // ---- 文 ----

    fn block(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            self.stmt(stmt);
        }
    }

    /// 新しいスコープに入ってブロックを実行する
    /// 変数を作る文を直接含まないブロックは、スコープを作らなくても意味が変わらないので省く
    fn scoped_block(&mut self, stmts: &[Stmt], span: Span) {
        if let Some(slots) = &mut self.slots {
            // スロットのときは、スコープはコンパイル時だけのもの（実行時の命令は要らない）
            slots.scopes.push(Vec::new());
            self.block(stmts);
            self.pop_slot_scope();
            return;
        }
        if !stmts.iter().any(declares_variable) {
            self.block(stmts);
            return;
        }
        self.emit(Op::PushScope, span);
        self.scope_depth += 1;
        self.block(stmts);
        self.scope_depth -= 1;
        self.emit(Op::PopScope, span);
    }

    fn pop_slot_scope(&mut self) {
        if let Some(slots) = &mut self.slots {
            slots.scopes.pop();
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        let span = stmt.span;
        match &stmt.kind {
            // 実行の前に登録済み
            StmtKind::FuncDef(_) => {}
            StmtKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.expr(cond);
                let to_else = self.emit(Op::JumpIfFalseCond(0), cond.span);
                self.scoped_block(then_block, span);
                if let Some(else_block) = else_block {
                    let to_end = self.emit(Op::Jump(0), span);
                    self.patch_here(to_else);
                    self.scoped_block(else_block, span);
                    self.patch_here(to_end);
                } else {
                    self.patch_here(to_else);
                }
            }
            StmtKind::While { cond, body } => {
                let start = self.here();
                self.expr(cond);
                let to_end = self.emit(Op::JumpIfFalseCond(0), cond.span);
                self.emit(Op::LoopTick, span);
                self.loops.push(LoopCtx {
                    depth: self.scope_depth,
                    breaks: Vec::new(),
                    continues: Vec::new(),
                });
                self.scoped_block(body, span);
                self.emit(Op::Jump(start), span);
                let end = self.here();
                self.patch(to_end, end);
                let ctx = self.loops.pop().expect("ループに入っている");
                for at in ctx.breaks {
                    self.patch(at, end);
                }
                for at in ctx.continues {
                    self.patch(at, start);
                }
            }
            StmtKind::For { var, iter, body } => {
                self.expr(iter);
                self.emit(Op::ForPrep, iter.span);
                let next = self.here();
                let for_next = if let Some(slots) = &mut self.slots {
                    // 繰り返しごとの新しいスコープ（ループ変数と本体）は、コンパイル時だけのもの
                    slots.scopes.push(Vec::new());
                    let slot = slots.declare(&var.0);
                    self.emit(Op::ForNextLocal { end: 0, slot }, span)
                } else {
                    let var = self.name(&var.0);
                    self.emit(Op::ForNext { end: 0, var }, span)
                };
                self.loops.push(LoopCtx {
                    depth: self.scope_depth,
                    breaks: Vec::new(),
                    continues: Vec::new(),
                });
                // 繰り返しごとの新しいスコープ（ループ変数と本体）
                if self.slots.is_some() {
                    self.block(body);
                    self.pop_slot_scope();
                } else {
                    self.scope_depth += 1;
                    self.block(body);
                    self.scope_depth -= 1;
                    self.emit(Op::PopScope, span);
                }
                self.emit(Op::Jump(next), span);
                let exit = self.here();
                self.patch(for_next, exit);
                let ctx = self.loops.pop().expect("ループに入っている");
                for at in ctx.breaks {
                    self.patch(at, exit);
                }
                for at in ctx.continues {
                    self.patch(at, next);
                }
                // 準備で積んだ、要素の写しと位置
                self.emit(Op::Pop, span);
                self.emit(Op::Pop, span);
            }
            StmtKind::Return(value) => {
                match value {
                    Some(expr) => self.expr(expr),
                    None => self.nil(span),
                }
                self.emit(Op::Return, span);
            }
            StmtKind::Break => {
                let at = self.leave_loop_scopes(span);
                if let Some(ctx) = self.loops.last_mut() {
                    ctx.breaks.push(at);
                }
            }
            StmtKind::Continue => {
                let at = self.leave_loop_scopes(span);
                if let Some(ctx) = self.loops.last_mut() {
                    ctx.continues.push(at);
                }
            }
            StmtKind::Defer(expr) => {
                // 式を、抜けるときに呼ぶ無名関数にする。変数を捕まえるのでスロットにはできない
                self.bail();
                let body = Stmt {
                    kind: StmtKind::Return(Some(expr.clone())),
                    span,
                };
                self.chunk.defs.push(Rc::new(FuncDef {
                    name: String::new(),
                    is_async: false,
                    params: Vec::new(),
                    rest: None,
                    body: vec![body],
                    span,
                }));
                let i = (self.chunk.defs.len() - 1) as u32;
                self.emit(Op::MakeClosure(i), span);
                self.emit(Op::Defer, span);
            }
            StmtKind::Declare { target, value } => {
                self.expr(value);
                self.emit_declare(&target.0, span);
            }
            StmtKind::Destructure {
                pattern,
                value,
                declare,
            } => {
                self.bail();
                self.expr(value);
                self.chunk.patterns.push(pattern.clone());
                let pattern = (self.chunk.patterns.len() - 1) as u32;
                self.emit(
                    Op::Destructure {
                        pattern,
                        declare: *declare,
                    },
                    span,
                );
            }
            StmtKind::Assign {
                target,
                indices,
                value,
            } => {
                let Some((last, outer)) = indices.split_last() else {
                    self.expr(value);
                    self.emit_assign(&target.0, span);
                    return;
                };
                // 配列とマップは参照なので、読んでいった先を書き換える。
                // `@m{"a"}{"b"} = v` は、外側の添字を読んでいって最後の 1 つだけ書く
                self.emit_load(&target.0, span);
                for index in outer {
                    self.emit(Op::RejectModule, span);
                    self.expr(index);
                    self.emit(
                        Op::IndexGet {
                            target: span,
                            key: index.span,
                        },
                        span,
                    );
                }
                self.emit(Op::RejectModule, span);
                self.expr(last);
                self.expr(value);
                self.emit(
                    Op::IndexSet {
                        target: span,
                        key: last.span,
                    },
                    span,
                );
            }
            StmtKind::Expr(expr) => {
                self.expr(expr);
                self.emit(Op::PopCheckError, expr.span);
            }
        }
    }

    /// `break` / `continue`: ループの中で開いたスコープを閉じて、行き先の決まっていない `Jump` を置く
    fn leave_loop_scopes(&mut self, span: Span) -> usize {
        let depth = self.loops.last().map_or(self.scope_depth, |ctx| ctx.depth);
        for _ in depth..self.scope_depth {
            self.emit(Op::PopScope, span);
        }
        self.emit(Op::Jump(0), span)
    }

    // ---- 式 ----

    fn expr(&mut self, expr: &Expr) {
        let span = expr.span;
        match &expr.kind {
            ExprKind::Number(n) => self.constant(Value::Num(*n), span),
            ExprKind::Int(n) => self.constant(Value::Int(*n), span),
            ExprKind::Bool(b) => self.constant(Value::Bool(*b), span),
            ExprKind::Nil => self.nil(span),
            ExprKind::Str(parts) => {
                if let [StrSegment::Text(text)] = parts.as_slice() {
                    self.constant(Value::Str(text.as_str().into()), span);
                    return;
                }
                for part in parts {
                    match part {
                        StrSegment::Text(text) => {
                            self.constant(Value::Str(text.as_str().into()), span);
                        }
                        StrSegment::Expr(inner) => {
                            self.expr(inner);
                            self.emit(Op::ToDisplay, inner.span);
                        }
                    }
                }
                self.emit(Op::Concat(parts.len() as u32), span);
            }
            ExprKind::Var(var) => self.emit_load(&var.0, span),
            ExprKind::Array(items) => {
                if has_spread(items) {
                    self.build_list(items);
                } else {
                    for item in items {
                        self.expr(item);
                    }
                    self.emit(Op::MakeArray(items.len() as u32), span);
                }
            }
            ExprKind::Map(entries) => {
                for (key, value) in entries {
                    self.expr(key);
                    self.emit(Op::CheckKey, key.span);
                    self.expr(value);
                }
                self.emit(Op::MakeMap(entries.len() as u32), span);
            }
            ExprKind::Member { target, name } => {
                self.expr(target);
                let name = self.name(name);
                self.emit(
                    Op::Member {
                        name,
                        target: target.span,
                    },
                    span,
                );
            }
            ExprKind::FuncRef(name) => {
                let name = self.name(name);
                self.emit(Op::FuncRef(name), span);
            }
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                self.expr(cond);
                let to_else = self.emit(Op::JumpIfFalseCond(0), cond.span);
                self.expr(then_expr);
                let to_end = self.emit(Op::Jump(0), span);
                self.patch_here(to_else);
                self.expr(else_expr);
                self.patch_here(to_end);
            }
            // 引数と配列リテラルの外では、`...` は構文解析で弾かれる
            ExprKind::Spread(inner) => self.expr(inner),
            ExprKind::Try(inner) => {
                self.expr(inner);
                self.emit(Op::Try, span);
            }
            ExprKind::Await(inner) => {
                self.expr(inner);
                self.emit(Op::Await, span);
            }
            ExprKind::Lambda(def) => {
                // 無名関数は、いまのスコープの変数を捕まえる（スロットには置けない）
                self.bail();
                self.chunk.defs.push(def.clone());
                let i = (self.chunk.defs.len() - 1) as u32;
                self.emit(Op::MakeClosure(i), span);
            }
            ExprKind::CallValue { callee, args } => {
                self.expr(callee);
                if has_spread(args) {
                    self.build_list(args);
                    self.emit(
                        Op::CallValueSpread {
                            callee: callee.span,
                        },
                        span,
                    );
                } else {
                    for arg in args {
                        self.expr(arg);
                    }
                    self.emit(
                        Op::CallValue {
                            argc: args.len() as u32,
                            callee: callee.span,
                        },
                        span,
                    );
                }
            }
            ExprKind::MemberCall { target, name, args } => {
                self.expr(target);
                let name = self.name(name);
                if has_spread(args) {
                    self.build_list(args);
                    self.emit(
                        Op::MemberCallSpread {
                            name,
                            target: target.span,
                        },
                        span,
                    );
                } else {
                    for arg in args {
                        self.expr(arg);
                    }
                    self.emit(
                        Op::MemberCall {
                            name,
                            argc: args.len() as u32,
                            target: target.span,
                        },
                        span,
                    );
                }
            }
            ExprKind::Index { target, index } => {
                self.expr(target);
                self.expr(index);
                self.emit(
                    Op::IndexGet {
                        target: target.span,
                        key: index.span,
                    },
                    span,
                );
            }
            ExprKind::Unary { op, operand } => {
                self.expr(operand);
                self.emit(Op::Unary(*op), span);
            }
            ExprKind::Binary { op, lhs, rhs } => {
                if let BinaryOp::And | BinaryOp::Or = op {
                    // 短絡評価。両辺とも真偽値に限る
                    self.expr(lhs);
                    self.emit(Op::LogicOperand(*op), lhs.span);
                    let short = self.emit(Op::ShortCircuit(*op, 0), span);
                    self.expr(rhs);
                    self.emit(Op::LogicOperand(*op), rhs.span);
                    self.patch_here(short);
                } else {
                    self.expr(lhs);
                    self.expr(rhs);
                    self.emit(Op::Binary(*op), span);
                }
            }
            ExprKind::Call { name, args } => {
                let name = self.name(name);
                if has_spread(args) {
                    self.build_list(args);
                    self.emit(Op::CallSpread { name }, span);
                } else {
                    for arg in args {
                        self.expr(arg);
                    }
                    self.emit(
                        Op::Call {
                            name,
                            argc: args.len() as u32,
                        },
                        span,
                    );
                }
            }
        }
    }

    /// 展開（`...式`）を含む並びを、1 つの配列にして積む
    fn build_list(&mut self, items: &[Expr]) {
        let first = items.first().map_or(Span { line: 1, col: 1 }, |e| e.span);
        self.emit(Op::ArrayNew, first);
        for item in items {
            if let ExprKind::Spread(inner) = &item.kind {
                self.expr(inner);
                self.emit(Op::ArrayExtend, inner.span);
            } else {
                self.expr(item);
                self.emit(Op::ArrayAppend, item.span);
            }
        }
    }
}

fn has_spread(items: &[Expr]) -> bool {
    items.iter().any(|e| matches!(e.kind, ExprKind::Spread(_)))
}

/// その文が、いまのスコープに変数を作るか
fn declares_variable(stmt: &Stmt) -> bool {
    matches!(
        &stmt.kind,
        StmtKind::FuncDef(_)
            | StmtKind::Declare { .. }
            | StmtKind::Destructure { declare: true, .. }
    )
}
