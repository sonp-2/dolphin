//! 式の走査

use super::{Ctx, RResult, Resolver, arity_message};
use crate::diag::Diagnostic;
use crate::syntax::ast::{Expr, ExprKind, StrSegment};

impl<'a> Resolver<'a> {
    pub(super) fn expr(&self, expr: &'a Expr, ctx: &Ctx<'a>) -> RResult {
        match &expr.kind {
            ExprKind::Number(_) | ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Nil => Ok(()),
            ExprKind::Str(parts) => parts.iter().try_for_each(|part| match part {
                StrSegment::Expr(inner) => self.expr(inner, ctx),
                StrSegment::Text(_) => Ok(()),
            }),
            ExprKind::Var(var) => self.var(var, expr.span, ctx),
            ExprKind::FuncRef(name) => self.callee_arity(name, expr.span).map(|_| ()),
            ExprKind::Lambda(def) => {
                // 外側のスコープを捕まえるので、いまのスコープの上に引数のスコープを重ねる
                let mut inner = Ctx {
                    scopes: ctx.scopes.clone(),
                    in_func: true,
                    in_async: def.is_async,
                };
                self.func_body(def, &mut inner)
            }
            ExprKind::Try(inner) | ExprKind::Spread(inner) => self.expr(inner, ctx),
            ExprKind::Await(inner) => {
                // `async` 関数の中と、ファイルの最上位で書ける
                if ctx.in_func && !ctx.in_async {
                    return Err(Diagnostic::new(
                        expr.span,
                        "`await` は `async` 関数の中か、ファイルの最上位でだけ書けます",
                    ));
                }
                self.expr(inner, ctx)
            }
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee, ctx)?;
                // 腕のスコープを重ねるため、スコープの写しで調べる（変数を作るのは腕のパターンだけ）
                let mut inner = Ctx {
                    scopes: ctx.scopes.clone(),
                    in_func: ctx.in_func,
                    in_async: ctx.in_async,
                };
                arms.iter().try_for_each(|arm| {
                    self.match_arm(arm, &mut inner, |ctx| self.expr(&arm.body, ctx))
                })
            }
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                self.expr(cond, ctx)?;
                self.expr(then_expr, ctx)?;
                self.expr(else_expr, ctx)
            }
            ExprKind::CallValue { callee, args } => {
                self.expr(callee, ctx)?;
                args.iter().try_for_each(|arg| self.expr(arg, ctx))
            }
            ExprKind::Call { name, args } => {
                let arity = self.callee_arity(name, expr.span)?;
                // 組み込み関数（arity が None）の引数の数は実行時に検査する
                // `...` で展開する引数があれば、数は実行時に検査する
                let spread = args.iter().any(|a| matches!(a.kind, ExprKind::Spread(_)));
                if let Some((min, max)) = arity
                    && !spread
                    && (args.len() < min || max.is_some_and(|max| args.len() > max))
                {
                    return Err(Diagnostic::new(
                        expr.span,
                        arity_message(&format!("`{name}`"), min, max, args.len()),
                    ));
                }
                args.iter().try_for_each(|arg| self.expr(arg, ctx))
            }
            ExprKind::Array(items) => items.iter().try_for_each(|item| self.expr(item, ctx)),
            ExprKind::Map(entries) => entries.iter().try_for_each(|(key, value)| {
                self.expr(key, ctx)?;
                self.expr(value, ctx)
            }),
            // モジュールの中身は実行時にしか分からない
            ExprKind::Member { target, .. } => self.expr(target, ctx),
            ExprKind::MemberCall { target, args, .. } => {
                self.expr(target, ctx)?;
                args.iter().try_for_each(|arg| self.expr(arg, ctx))
            }
            ExprKind::Index { target, index } => {
                self.expr(target, ctx)?;
                self.expr(index, ctx)
            }
            ExprKind::Unary { operand, .. } => self.expr(operand, ctx),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs, ctx)?;
                self.expr(rhs, ctx)
            }
        }
    }
}
