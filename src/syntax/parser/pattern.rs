//! パターンと分割代入。`match` のパターン（design.md 5.59）もここで読む

use super::super::ast::{Literal, Pattern, Stmt, StmtKind, Var};
use super::super::lexer::{StrPart, TokenKind};
use super::{Diagnostic, PResult, Parser};

/// 分割代入と `for` で、`match` 専用のパターンを書いたときのメッセージ
const MATCH_ONLY: &str = "このパターンは `match` の中でだけ書けます";

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

    /// pattern = VAR | "_" | "{" [ pattern { "," pattern } ] [ "," ] [ "..." VAR ] "}"
    ///         | "{" STRING ":" pattern { "," STRING ":" pattern } "}"
    ///
    /// 分割代入と `for` のパターン。合わないことがある部分（リテラル・`|`・レコード）は書けない
    pub(super) fn pattern(&mut self) -> PResult<Pattern> {
        self.atom_pattern(false)
    }

    /// match_pattern = atom_pattern { "|" atom_pattern }（design.md 5.59）
    pub(super) fn match_pattern(&mut self) -> PResult<Pattern> {
        let first = self.atom_pattern(true)?;
        if !self.at(&TokenKind::Pipe) {
            return Ok(first);
        }
        let mut alts = vec![first];
        while self.eat(&TokenKind::Pipe) {
            alts.push(self.atom_pattern(true)?);
        }
        Ok(Pattern::Or(alts))
    }

    /// 入れ子の要素。`match` のパターンの中なら or も書ける
    fn nested_pattern(&mut self, in_match: bool) -> PResult<Pattern> {
        if in_match {
            self.match_pattern()
        } else {
            self.pattern()
        }
    }

    /// `in_match` が false なら、分割代入と `for` のパターン（`_` までを書ける）
    fn atom_pattern(&mut self, in_match: bool) -> PResult<Pattern> {
        let token = self.peek();
        match &token.kind {
            TokenKind::Var(name) => {
                let var = Var(name.clone());
                self.advance();
                Ok(Pattern::Var(var))
            }
            TokenKind::Ident(name) if name == "_" => {
                self.advance();
                Ok(Pattern::Wild)
            }
            TokenKind::LBrace => {
                self.advance();
                self.brace_pattern(in_match)
            }
            _ if !in_match => match token.kind {
                TokenKind::Ident(_)
                | TokenKind::Number(_)
                | TokenKind::Int(_)
                | TokenKind::Str(_)
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Nil
                | TokenKind::Minus => Err(Diagnostic::new(token.span, MATCH_ONLY)),
                _ => Err(self.unexpected("パターン")),
            },
            // ここから先は `match` のパターンだけ
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                self.expect(&TokenKind::LBracket)?;
                let mut items = Vec::new();
                loop {
                    items.push(self.match_pattern()?);
                    if self.eat(&TokenKind::RBracket) {
                        break;
                    }
                    if !self.eat(&TokenKind::Comma) {
                        return Err(self.unexpected("`,` か `]`"));
                    }
                }
                Ok(Pattern::Record { name, items })
            }
            TokenKind::Number(n) => {
                let lit = Literal::Number(*n);
                self.advance();
                Ok(Pattern::Lit(lit))
            }
            TokenKind::Int(n) => {
                let lit = Literal::Int(*n);
                self.advance();
                Ok(Pattern::Lit(lit))
            }
            TokenKind::Minus => {
                self.advance();
                let lit = match &self.peek().kind {
                    TokenKind::Number(n) => Literal::Number(-n),
                    TokenKind::Int(n) => Literal::Int(-n),
                    _ => return Err(self.unexpected("数値（負の数のパターン）")),
                };
                self.advance();
                Ok(Pattern::Lit(lit))
            }
            TokenKind::Str(parts) => {
                let lit = match parts.as_slice() {
                    [] => Literal::Str(String::new()),
                    [StrPart::Text(text)] => Literal::Str(text.clone()),
                    _ => {
                        return Err(Diagnostic::new(
                            token.span,
                            "パターンの文字列は、展開を含まない文字列です",
                        ));
                    }
                };
                self.advance();
                Ok(Pattern::Lit(lit))
            }
            TokenKind::True | TokenKind::False => {
                let lit = Literal::Bool(token.kind == TokenKind::True);
                self.advance();
                Ok(Pattern::Lit(lit))
            }
            TokenKind::Nil => {
                self.advance();
                Ok(Pattern::Lit(Literal::Nil))
            }
            _ => Err(self.unexpected("パターン")),
        }
    }

    /// `{` の後。配列かマップのパターン
    fn brace_pattern(&mut self, in_match: bool) -> PResult<Pattern> {
        // `{:}` は任意のマップ（`match` だけ）
        if in_match && self.at(&TokenKind::Colon) {
            self.advance();
            self.expect(&TokenKind::RBrace)?;
            return Ok(Pattern::Map(Vec::new()));
        }
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
                entries.push((key, self.nested_pattern(in_match)?));
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
            items.push(self.nested_pattern(in_match)?);
            if !self.at(&TokenKind::RBrace) && !self.eat(&TokenKind::Comma) {
                return Err(self.unexpected("`,` か `}`"));
            }
        }
        Ok(Pattern::Array { items, rest })
    }
}
