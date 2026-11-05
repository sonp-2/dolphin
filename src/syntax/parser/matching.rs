//! `match`（design.md 5.59）。文の位置では腕の本体が文、式の位置では式

use super::super::ast::{Expr, ExprKind, MatchArm, Stmt, StmtKind};
use super::super::lexer::TokenKind;
use super::{PResult, Parser};

impl<'t> Parser<'t> {
    /// match_stmt = "match" expression "(" arms ")"。腕の本体は、ブロックか文 1 つ
    pub(super) fn match_stmt(&mut self) -> PResult<Stmt> {
        let span = self.expect(&TokenKind::Match)?;
        let scrutinee = self.expression()?;
        let arms = self.match_arms(|p| {
            if p.at(&TokenKind::LParen) {
                p.block()
            } else {
                Ok(vec![p.statement()?])
            }
        })?;
        Ok(Stmt {
            kind: StmtKind::Match { scrutinee, arms },
            span,
        })
    }

    /// match_expr = "match" expression "(" arms ")"。腕の本体は式
    pub(super) fn match_expr(&mut self) -> PResult<Expr> {
        let span = self.expect(&TokenKind::Match)?;
        let scrutinee = self.expression()?;
        let arms = self.match_arms(Self::expression)?;
        Ok(Expr {
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            span,
        })
    }

    /// arms = { NEWLINE } [ arm { ( NEWLINE | "," ) { NEWLINE | "," } arm } ] { NEWLINE } ")"
    /// arm  = match_pattern [ "if" expression ] "=>" body
    fn match_arms<B>(&mut self, body: fn(&mut Self) -> PResult<B>) -> PResult<Vec<MatchArm<B>>> {
        self.expect(&TokenKind::LParen)?;
        let mut arms = Vec::new();
        loop {
            let mut separated = false;
            while self.at(&TokenKind::Newline) || self.at(&TokenKind::Comma) {
                self.advance();
                separated = true;
            }
            if self.eat(&TokenKind::RParen) {
                return Ok(arms);
            }
            if !arms.is_empty() && !separated {
                return Err(self.unexpected("改行か `,`"));
            }
            let span = self.peek().span;
            let pattern = self.match_pattern()?;
            let guard = if self.eat(&TokenKind::If) {
                Some(self.expression()?)
            } else {
                None
            };
            self.expect(&TokenKind::FatArrow)?;
            let body = body(self)?;
            arms.push(MatchArm {
                pattern,
                guard,
                body,
                span,
            });
        }
    }
}
