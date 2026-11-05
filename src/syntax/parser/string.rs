//! 文字列リテラルの展開（`@name`、`@{ }`）

use super::super::ast::{Expr, ExprKind, StrSegment};
use super::super::lexer::{StrPart, TokenKind};
use super::super::span::Span;
use super::{PResult, Parser};

/// 文字列の部分を AST にする。`@name` は文字列の位置を持つ変数、`@{ }` は式として解析する
pub(super) fn str_segments(parts: &[StrPart], span: Span) -> PResult<Vec<StrSegment>> {
    parts
        .iter()
        .map(|part| match part {
            StrPart::Text(text) => Ok(StrSegment::Text(text.clone())),
            StrPart::Var(var) => Ok(StrSegment::Expr(Expr {
                kind: ExprKind::Var(var.clone()),
                span,
            })),
            StrPart::Interp(tokens) => {
                let mut sub = Parser { tokens, pos: 0 };
                sub.skip_newlines();
                let expr = sub.expression()?;
                sub.skip_newlines();
                if !sub.at(&TokenKind::Eof) {
                    return Err(sub.unexpected("`}`"));
                }
                Ok(StrSegment::Expr(expr))
            }
        })
        .collect()
}
