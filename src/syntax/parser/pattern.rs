//! パターンと分割代入

use super::super::ast::{Pattern, Stmt, StmtKind, Var};
use super::super::lexer::{StrPart, TokenKind};
use super::{Diagnostic, PResult, Parser};

impl<'t> Parser<'t> {
    /// destructure = pattern ( ":=" | "=" ) expression
    pub(super) fn destructure(&mut self) -> PResult<Stmt> {
        let span = self.peek().span;
        let pattern = self.pattern()?;
        let declare = self.at(&TokenKind::Declare);
        self.advance();
        let value = self.expression()?;
        Ok(Stmt {
            kind: StmtKind::Destructure {
                pattern,
                value,
                declare,
            },
            span,
        })
    }

    /// pattern = VAR | "{" [ pattern { "," pattern } ] [ "," ] [ "..." VAR ] "}"
    ///         | "{" STRING ":" pattern { "," STRING ":" pattern } "}"
    pub(super) fn pattern(&mut self) -> PResult<Pattern> {
        if let TokenKind::Var(name) = &self.peek().kind {
            let var = Var(name.clone());
            self.advance();
            return Ok(Pattern::Var(var));
        }
        self.expect(&TokenKind::LBrace)?;
        // マップのパターンは、最初の要素が `"キー":`
        let is_map = matches!(&self.peek().kind, TokenKind::Str(_))
            && self.tokens[self.pos + 1].kind == TokenKind::Colon;
        if is_map {
            let mut entries = Vec::new();
            loop {
                let key_span = self.peek().span;
                let TokenKind::Str(parts) = &self.peek().kind else {
                    return Err(self.unexpected("キーの文字列"));
                };
                let key = match parts.as_slice() {
                    [StrPart::Text(text)] => text.clone(),
                    _ => {
                        return Err(Diagnostic::new(
                            key_span,
                            "分割代入のキーは、展開を含まない文字列です",
                        ));
                    }
                };
                self.advance();
                self.expect(&TokenKind::Colon)?;
                entries.push((key, self.pattern()?));
                if self.eat(&TokenKind::RBrace) {
                    return Ok(Pattern::Map(entries));
                }
                self.expect(&TokenKind::Comma)?;
            }
        }
        let mut items = Vec::new();
        let mut rest = None;
        loop {
            if self.eat(&TokenKind::RBrace) {
                break;
            }
            if self.eat(&TokenKind::Ellipsis) {
                let TokenKind::Var(name) = &self.peek().kind else {
                    return Err(self.unexpected("変数（`@rest`）"));
                };
                rest = Some(Var(name.clone()));
                self.advance();
                if !self.at(&TokenKind::RBrace) {
                    return Err(Diagnostic::new(
                        self.peek().span,
                        "`...@名前` はパターンの最後にだけ書けます",
                    ));
                }
                continue;
            }
            items.push(self.pattern()?);
            if !self.at(&TokenKind::RBrace) && !self.eat(&TokenKind::Comma) {
                return Err(self.unexpected("`,` か `}`"));
            }
        }
        Ok(Pattern::Array { items, rest })
    }
}
