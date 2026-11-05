//! 関数の定義と引数（async、既定値、可変長）

use std::rc::Rc;

use super::super::ast::{FuncDef, Param, Stmt, StmtKind};
use super::super::lexer::TokenKind;
use super::super::span::Span;
use super::{Diagnostic, PResult, Parser};

impl<'t> Parser<'t> {
    /// 関数の定義 `名前[引数] ( … )`。先頭は名前（`looks_like_func_def` が確かめてある）
    pub(super) fn func_def_stmt(&mut self, is_async: bool, span: Span) -> PResult<Stmt> {
        let TokenKind::Ident(name) = &self.peek().kind else {
            unreachable!("looks_like_func_def が確かめた")
        };
        let name = name.clone();
        self.advance();
        self.advance();
        let (params, rest) = self.params()?;
        let body = self.block()?;
        let def = FuncDef {
            name,
            is_async,
            record: false,
            params,
            rest,
            body,
            span,
        };
        Ok(Stmt {
            kind: StmtKind::FuncDef(Rc::new(def)),
            span,
        })
    }

    /// params = [ param { "," param } ] [ "," ] [ "..." VAR ]、param = VAR [ "=" expression ]。
    /// 開き括弧の後に呼ぶ。`]` まで読み進める。既定値のある引数は、ない引数より後ろにだけ書ける。
    /// `...@rest` は最後にだけ書ける
    pub(super) fn params(&mut self) -> PResult<(Vec<Param>, Option<String>)> {
        let mut params: Vec<Param> = Vec::new();
        if self.eat(&TokenKind::RBracket) {
            return Ok((params, None));
        }
        loop {
            if self.eat(&TokenKind::Ellipsis) {
                let TokenKind::Var(name) = &self.peek().kind else {
                    return Err(self.unexpected("変数（`@rest`）"));
                };
                let rest = name.clone();
                self.advance();
                if !self.at(&TokenKind::RBracket) {
                    return Err(Diagnostic::new(
                        self.peek().span,
                        "`...@名前` は引数の最後にだけ書けます",
                    ));
                }
                self.advance();
                return Ok((params, Some(rest)));
            }
            let TokenKind::Var(name) = &self.peek().kind else {
                return Err(self.unexpected("変数（`@x`）"));
            };
            let name = name.clone();
            let name_span = self.advance().span;
            let default = if self.eat(&TokenKind::Assign) {
                Some(self.expression()?)
            } else {
                if params.last().is_some_and(|p| p.default.is_some()) {
                    return Err(Diagnostic::new(
                        name_span,
                        "既定値のない引数は、既定値のある引数より前に書きます",
                    ));
                }
                None
            };
            params.push(Param { name, default });
            if self.eat(&TokenKind::RBracket) {
                return Ok((params, None));
            }
            if !self.eat(&TokenKind::Comma) {
                return Err(self.unexpected("`,` か `]`"));
            }
        }
    }
}
