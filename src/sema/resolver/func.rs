//! 関数の定義の検査

use std::collections::HashSet;

use super::{Callee, Ctx, RResult, Resolver};
use crate::diag::Diagnostic;
use crate::syntax::ast::FuncDef;
use crate::syntax::span::Span;

impl<'a> Resolver<'a> {
    pub(super) fn func_def(&self, def: &'a FuncDef) -> RResult {
        let mut ctx = Ctx {
            scopes: Vec::new(),
            in_func: true,
            in_async: def.is_async,
        };
        self.func_body(def, &mut ctx)
    }

    /// 関数の本体を検査する。`ctx` は関数を囲むスコープ（無名関数が捕まえる範囲）。
    /// 本体は引数と同じスコープ（引数と同じ名前を `:=` で宣言し直すとエラー）
    pub(super) fn func_body(&self, def: &'a FuncDef, ctx: &mut Ctx<'a>) -> RResult {
        // 既定値の式は、それより前の引数を使える
        ctx.scopes.push(HashSet::new());
        for param in &def.params {
            if let Some(default) = &param.default {
                self.expr(default, ctx)?;
            }
            let scope = ctx.scopes.last_mut().expect("push したばかり");
            if !scope.insert(param.name.as_str()) {
                ctx.scopes.pop();
                return Err(Diagnostic::new(
                    def.span,
                    format!("引数 `@{}` が重複しています", param.name),
                ));
            }
        }
        if let Some(rest) = &def.rest {
            let scope = ctx.scopes.last_mut().expect("push したばかり");
            if !scope.insert(rest.as_str()) {
                ctx.scopes.pop();
                return Err(Diagnostic::new(
                    def.span,
                    format!("引数 `@{rest}` が重複しています"),
                ));
            }
        }
        // `break` / `continue` は関数の外側の `while` には効かない
        let result = self.stmts(&def.body, ctx, false);
        ctx.scopes.pop();
        result
    }

    /// 呼び出せる関数か。組み込み関数と以前の入力で定義した関数を含む。
    /// 戻り値は引数の数（組み込み関数は実行時に検査するので None）
    pub(super) fn callee_arity(
        &self,
        name: &str,
        span: Span,
    ) -> Result<Option<(usize, Option<usize>)>, Diagnostic> {
        match self.functions.get(name) {
            Some(&n) => Ok(Some(n)),
            None => match (self.lookup)(name) {
                Some(Callee::User { min, max }) => Ok(Some((min, max))),
                Some(Callee::Native) => Ok(None),
                None => Err(Diagnostic::new(span, format!("未定義の関数 `{name}`"))),
            },
        }
    }
}
