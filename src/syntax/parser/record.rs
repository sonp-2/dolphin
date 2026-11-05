//! record の定義

use std::rc::Rc;

use super::super::ast::{FuncDef, Stmt, StmtKind};
use super::super::lexer::TokenKind;
use super::{Diagnostic, PResult, Parser};

impl<'t> Parser<'t> {
    /// record_def = "record" IDENT "[" VAR { "," VAR } "]"（design.md 5.55）。
    /// コンストラクタ関数として表す（本体は空）
    pub(super) fn record_def(&mut self) -> PResult<Stmt> {
        let span = self.advance().span;
        let TokenKind::Ident(name) = &self.peek().kind else {
            return Err(self.unexpected("レコードの名前"));
        };
        let name = name.clone();
        self.advance();
        self.expect(&TokenKind::LBracket)?;
        let (params, rest) = self.params()?;
        if rest.is_some() {
            return Err(Diagnostic::new(
                span,
                "レコードのフィールドに `...@名前` は書けません",
            ));
        }
        if params.is_empty() {
            return Err(Diagnostic::new(
                span,
                "レコードにはフィールドが 1 つ以上必要です",
            ));
        }
        if params.iter().any(|p| p.default.is_some()) {
            return Err(Diagnostic::new(
                span,
                "レコードのフィールドに既定値は書けません",
            ));
        }
        let def = FuncDef {
            name,
            is_async: false,
            record: true,
            params,
            rest: None,
            body: Vec::new(),
            span,
        };
        Ok(Stmt {
            kind: StmtKind::FuncDef(Rc::new(def)),
            span,
        })
    }
}
