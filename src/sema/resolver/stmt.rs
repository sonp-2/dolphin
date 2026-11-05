//! 文の走査（スコープへの宣言、break・continue・return の位置）

use std::collections::HashSet;

use super::{Ctx, RResult, Resolver};
use crate::diag::Diagnostic;
use crate::syntax::ast::{Stmt, StmtKind};

impl<'a> Resolver<'a> {
    pub(super) fn stmts(&self, block: &'a [Stmt], ctx: &mut Ctx<'a>, in_loop: bool) -> RResult {
        block
            .iter()
            .try_for_each(|stmt| self.stmt(stmt, ctx, in_loop))
    }

    /// 新しいスコープに入ってブロックを検査する。`in_loop` は `while` の中か
    /// （`break` / `continue` を使えるか）
    pub(super) fn block(&self, block: &'a [Stmt], ctx: &mut Ctx<'a>, in_loop: bool) -> RResult {
        ctx.scopes.push(HashSet::new());
        let result = self.stmts(block, ctx, in_loop);
        ctx.scopes.pop();
        result
    }

    pub(super) fn stmt(&self, stmt: &'a Stmt, ctx: &mut Ctx<'a>, in_loop: bool) -> RResult {
        match &stmt.kind {
            StmtKind::FuncDef(def) => Err(Diagnostic::new(
                def.span,
                "関数はトップレベルでのみ定義できます",
            )),
            StmtKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.expr(cond, ctx)?;
                self.block(then_block, ctx, in_loop)?;
                match else_block {
                    Some(block) => self.block(block, ctx, in_loop),
                    None => Ok(()),
                }
            }
            StmtKind::While { cond, body } => {
                self.expr(cond, ctx)?;
                self.block(body, ctx, true)
            }
            StmtKind::For { target, iter, body } => {
                // 回す式は、ループ変数の外側で評価される
                self.expr(iter, ctx)?;
                // ループ変数と本体は同じスコープ（`@x` を本体で `:=` し直すとエラー）
                let mut scope = HashSet::new();
                for var in target.vars() {
                    if !scope.insert(var.0.as_str()) {
                        return Err(Diagnostic::new(
                            stmt.span,
                            format!("変数 `{var}` はすでに宣言されています"),
                        ));
                    }
                }
                ctx.scopes.push(scope);
                let result = self.stmts(body, ctx, true);
                ctx.scopes.pop();
                result
            }
            StmtKind::Break | StmtKind::Continue if !in_loop => {
                let keyword = if let StmtKind::Break = stmt.kind {
                    "break"
                } else {
                    "continue"
                };
                Err(Diagnostic::new(
                    stmt.span,
                    format!("`{keyword}` は while か for の中でのみ使えます"),
                ))
            }
            StmtKind::Break | StmtKind::Continue => Ok(()),
            StmtKind::Return(value) => {
                if !ctx.in_func {
                    return Err(Diagnostic::new(stmt.span, "`#` は関数の中でのみ使えます"));
                }
                match value {
                    Some(value) => self.expr(value, ctx),
                    None => Ok(()),
                }
            }
            StmtKind::Declare { target, value } => {
                // `@x := @x + 1` の右辺の `@x` は、宣言する前の `@x`（外側）を指す
                self.expr(value, ctx)?;
                let scope = ctx.scopes.last_mut().expect("スコープは最低 1 つある");
                if !scope.insert(target.0.as_str()) {
                    return Err(Diagnostic::new(
                        stmt.span,
                        format!("変数 `{target}` はすでに宣言されています"),
                    ));
                }
                Ok(())
            }
            StmtKind::Assign {
                target,
                indices,
                value,
            } => {
                if !self.visible(target, ctx) {
                    let message = if indices.is_empty() {
                        format!(
                            "宣言されていない変数 `{target}` に代入しています（`{target} := ...` で宣言してください）"
                        )
                    } else {
                        format!("未定義の変数 `{target}`")
                    };
                    return Err(Diagnostic::new(stmt.span, message));
                }
                for index in indices {
                    self.expr(index, ctx)?;
                }
                self.expr(value, ctx)
            }
            StmtKind::Destructure {
                pattern,
                value,
                declare,
            } => {
                self.expr(value, ctx)?;
                for var in pattern.vars() {
                    if *declare {
                        let scope = ctx.scopes.last_mut().expect("スコープは最低 1 つある");
                        if !scope.insert(var.0.as_str()) {
                            return Err(Diagnostic::new(
                                stmt.span,
                                format!("変数 `{var}` はすでに宣言されています"),
                            ));
                        }
                    } else if !self.visible(var, ctx) {
                        return Err(Diagnostic::new(
                            stmt.span,
                            format!(
                                "宣言されていない変数 `{var}` に代入しています（`:=` で宣言してください）"
                            ),
                        ));
                    }
                }
                Ok(())
            }
            StmtKind::Match { scrutinee, arms } => {
                self.expr(scrutinee, ctx)?;
                arms.iter().try_for_each(|arm| {
                    self.match_arm(arm, ctx, |ctx| self.stmts(&arm.body, ctx, in_loop))
                })
            }
            StmtKind::Expr(expr) => self.expr(expr, ctx),
            StmtKind::Defer(expr) => {
                // 抜けるときに呼ばれる無名関数の中と同じ規則で調べる（`await` は書けない）
                let inner = Ctx {
                    scopes: ctx.scopes.clone(),
                    in_func: true,
                    in_async: false,
                };
                self.expr(expr, &inner)
            }
        }
    }
}
