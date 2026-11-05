//! 変数の可視性

use super::{Ctx, RResult, Resolver};
use crate::diag::Diagnostic;
use crate::syntax::ast::Var;
use crate::syntax::span::Span;

impl<'a> Resolver<'a> {
    /// 変数を読めるか
    pub(super) fn var(&self, var: &Var, span: Span, ctx: &Ctx<'a>) -> RResult {
        if self.visible(var, ctx) {
            Ok(())
        } else {
            Err(Diagnostic::new(span, format!("未定義の変数 `{var}`")))
        }
    }

    /// この場所から `var` が見えるか。内側のスコープから順に探し、
    /// 関数の中ならトップレベルで宣言される変数も見える
    pub(super) fn visible(&self, var: &Var, ctx: &Ctx<'a>) -> bool {
        let name = var.0.as_str();
        ctx.scopes.iter().any(|scope| scope.contains(name))
            || (ctx.in_func && self.hoisted.contains(name))
            || (self.has_global)(name)
    }
}
