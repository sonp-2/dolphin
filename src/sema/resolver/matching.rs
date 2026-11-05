//! `match` の腕とパターンの検査（design.md 5.59）

use std::collections::HashSet;

use super::{Ctx, RResult, Resolver};
use crate::diag::Diagnostic;
use crate::syntax::ast::{MatchArm, Pattern};
use crate::syntax::span::Span;

impl<'a> Resolver<'a> {
    /// 腕 1 つを検査する。パターンの変数・ガード・本体は、同じ新しいスコープに入る。
    /// `body` は、そのスコープの中で本体を検査する
    pub(super) fn match_arm<B>(
        &self,
        arm: &'a MatchArm<B>,
        ctx: &mut Ctx<'a>,
        body: impl FnOnce(&mut Ctx<'a>) -> RResult,
    ) -> RResult {
        let names = self.pattern_names(&arm.pattern, arm.span)?;
        ctx.scopes
            .push(names.iter().copied().collect::<HashSet<_>>());
        let result = (|| {
            if let Some(guard) = &arm.guard {
                self.expr(guard, ctx)?;
            }
            body(ctx)
        })();
        ctx.scopes.pop();
        result
    }

    /// パターンが束縛する変数の名前を左から順に返す。同じ変数が 2 回出る、or の選択肢で
    /// 変数が違う、レコードのフィールド数が合わない、はエラー
    fn pattern_names(&self, pattern: &'a Pattern, span: Span) -> Result<Vec<&'a str>, Diagnostic> {
        let names = self.collect_names(pattern, span)?;
        let mut seen = HashSet::new();
        for name in &names {
            if !seen.insert(*name) {
                return Err(Diagnostic::new(
                    span,
                    format!("変数 `@{name}` はすでに宣言されています"),
                ));
            }
        }
        Ok(names)
    }

    fn collect_names(&self, pattern: &'a Pattern, span: Span) -> Result<Vec<&'a str>, Diagnostic> {
        let mut names = Vec::new();
        match pattern {
            Pattern::Var(var) => names.push(var.0.as_str()),
            Pattern::Wild | Pattern::Lit(_) => {}
            Pattern::Array { items, rest } => {
                for item in items {
                    names.extend(self.collect_names(item, span)?);
                }
                names.extend(rest.iter().map(|v| v.0.as_str()));
            }
            Pattern::Map(entries) => {
                for (_, item) in entries {
                    names.extend(self.collect_names(item, span)?);
                }
            }
            Pattern::Record { name, items } => {
                if let Some(&fields) = self.records.get(name.as_str())
                    && fields != items.len()
                {
                    return Err(Diagnostic::new(
                        span,
                        format!(
                            "レコード `{name}` のフィールドは {fields} 個です（パターンは {} 個）",
                            items.len()
                        ),
                    ));
                }
                for item in items {
                    names.extend(self.collect_names(item, span)?);
                }
            }
            Pattern::Or(alts) => {
                let mut first: Option<Vec<&str>> = None;
                for alt in alts {
                    let alt_names = self.collect_names(alt, span)?;
                    match &first {
                        None => first = Some(alt_names),
                        Some(first) => {
                            let a: HashSet<_> = first.iter().collect();
                            let b: HashSet<_> = alt_names.iter().collect();
                            if a != b {
                                return Err(Diagnostic::new(
                                    span,
                                    "or パターンの選択肢で束縛する変数が違います",
                                ));
                            }
                        }
                    }
                }
                names = first.unwrap_or_default();
            }
        }
        Ok(names)
    }
}
