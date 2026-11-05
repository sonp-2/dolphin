//! 文のコンパイル（`for` は `for_loop` に分けてある）

use super::*;

impl Compiler {
    pub(super) fn block(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            self.stmt(stmt);
        }
    }

    /// 新しいスコープに入ってブロックを実行する
    /// 変数を作る文を直接含まないブロックは、スコープを作らなくても意味が変わらないので省く
    pub(super) fn scoped_block(&mut self, stmts: &[Stmt], span: Span) {
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

    pub(super) fn pop_slot_scope(&mut self) {
        if let Some(slots) = &mut self.slots {
            slots.scopes.pop();
        }
    }

    pub(super) fn stmt(&mut self, stmt: &Stmt) {
        let span = stmt.span;
        match &stmt.kind {
            // 実行の前に登録済み
            StmtKind::FuncDef(_) => {}
            StmtKind::If {
                cond,
                then_block,
                else_block,
            } => self.if_stmt(cond, then_block, else_block.as_deref(), span),
            StmtKind::While { cond, body } => self.while_stmt(cond, body, span),
            StmtKind::For { target, iter, body } => self.for_stmt(target, iter, body, span),
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
            StmtKind::Defer(expr) => self.defer_stmt(expr, span),
            StmtKind::Declare { target, value } => {
                self.expr(value);
                self.emit_declare(&target.0, span);
            }
            StmtKind::Destructure {
                pattern,
                value,
                declare,
            } => self.destructure_stmt(pattern, value, *declare, span),
            StmtKind::Assign {
                target,
                indices,
                value,
            } => self.assign_stmt(&target.0, indices, value, span),
            StmtKind::Match { scrutinee, arms } => {
                self.match_common(
                    scrutinee,
                    arms,
                    span,
                    |c, body| c.block(body),
                    |body| body.iter().any(declares_variable),
                );
            }
            StmtKind::Expr(expr) => {
                self.expr(expr);
                self.emit(Op::PopCheckError, expr.span);
            }
        }
    }

    fn if_stmt(
        &mut self,
        cond: &Expr,
        then_block: &[Stmt],
        else_block: Option<&[Stmt]>,
        span: Span,
    ) {
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

    fn while_stmt(&mut self, cond: &Expr, body: &[Stmt], span: Span) {
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

    /// `break` / `continue`: ループの中で開いたスコープを閉じて、行き先の決まっていない `Jump` を置く
    fn leave_loop_scopes(&mut self, span: Span) -> usize {
        let depth = self.loops.last().map_or(self.scope_depth, |ctx| ctx.depth);
        for _ in depth..self.scope_depth {
            self.emit(Op::PopScope, span);
        }
        self.emit(Op::Jump(0), span)
    }

    fn defer_stmt(&mut self, expr: &Expr, span: Span) {
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
            record: false,
        }));
        let i = (self.chunk.defs.len() - 1) as u32;
        self.emit(Op::MakeClosure(i), span);
        self.emit(Op::Defer, span);
    }

    fn destructure_stmt(&mut self, pattern: &Pattern, value: &Expr, declare: bool, span: Span) {
        self.bail();
        self.expr(value);
        self.chunk.patterns.push(pattern.clone());
        let pattern = (self.chunk.patterns.len() - 1) as u32;
        self.emit(Op::Destructure { pattern, declare }, span);
    }

    fn assign_stmt(&mut self, target: &str, indices: &[Expr], value: &Expr, span: Span) {
        let Some((last, outer)) = indices.split_last() else {
            self.expr(value);
            self.emit_assign(target, span);
            return;
        };
        // 配列とマップは参照なので、読んでいった先を書き換える。
        // `@m{"a"}{"b"} = v` は、外側の添字を読んでいって最後の 1 つだけ書く
        self.emit_load(target, span);
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
}

/// その文が、いまのスコープに変数を作るか
pub(super) fn declares_variable(stmt: &Stmt) -> bool {
    matches!(
        &stmt.kind,
        StmtKind::FuncDef(_)
            | StmtKind::Declare { .. }
            | StmtKind::Destructure { declare: true, .. }
    )
}
