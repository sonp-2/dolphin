//! トークン列から AST を作る。design.md 4 章の EBNF を再帰下降で実装する
//!
//! 文法の規則 1 つにつき関数 1 つが対応する（`statement`、`block`、`or_expr` など）。
//! 規則の種類ごとにファイルを分けている（文は `stmt`、式は `expr`、…）

mod expr;
mod func;
mod lookahead;
mod pattern;
mod record;
mod stmt;
mod string;
#[cfg(test)]
mod tests;

use super::ast::Program;
use super::lexer::{Token, TokenKind};
use super::span::Span;
use crate::diag::Diagnostic;

type PResult<T> = Result<T, Diagnostic>;

pub fn parse(tokens: &[Token]) -> PResult<Program> {
    let mut parser = Parser { tokens, pos: 0 };
    let stmts = parser.statements_until(&TokenKind::Eof)?;
    Ok(Program { stmts })
}

struct Parser<'t> {
    /// 末尾は必ず Eof
    tokens: &'t [Token],
    pos: usize,
}

impl<'t> Parser<'t> {
    // ---- トークン操作 ----

    fn peek(&self) -> &'t Token {
        &self.tokens[self.pos]
    }

    fn at(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    /// 1 つ読み進めて、読んだトークンを返す。Eof からは進まない
    fn advance(&mut self) -> &'t Token {
        let token = self.peek();
        if token.kind != TokenKind::Eof {
            self.pos += 1;
        }
        token
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokenKind) -> PResult<Span> {
        if self.at(kind) {
            Ok(self.advance().span)
        } else {
            Err(self.unexpected(&kind.to_string()))
        }
    }

    /// 「{expected}が必要です（{今のトークン}がありました）」
    fn unexpected(&self, expected: &str) -> Diagnostic {
        let token = self.peek();
        let diag = Diagnostic::new(
            token.span,
            format!("{expected}が必要です（{}がありました）", token.kind),
        );
        if token.kind == TokenKind::Eof {
            diag.at_eof()
        } else {
            diag
        }
    }

    fn skip_newlines(&mut self) {
        while self.eat(&TokenKind::Newline) {}
    }
}
