//! 先読み: 文の先頭を見て、関数定義か分割代入かを見分ける

use super::super::lexer::TokenKind;
use super::Parser;

impl<'t> Parser<'t> {
    /// 文の先頭の `{ … }` の直後が `:=` か `=` か（分割代入）
    pub(super) fn looks_like_destructure(&self) -> bool {
        let mut depth = 0;
        for (i, token) in self.tokens.iter().enumerate().skip(self.pos) {
            match token.kind {
                TokenKind::LBracket | TokenKind::LBrace | TokenKind::LParen => depth += 1,
                TokenKind::RBracket | TokenKind::RBrace | TokenKind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return self.tokens.get(i + 1).is_some_and(|t| {
                            matches!(t.kind, TokenKind::Declare | TokenKind::Assign)
                        });
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
        }
        false
    }

    /// 文の先頭が `IDENT "[" … "]" "("` の形か。`]` の直後が `(` なら関数定義（design.md 4.1）。
    /// 引数に既定値（`@b = 1`）を書けるので、式として読む前に見分ける
    pub(super) fn looks_like_func_def(&self) -> bool {
        if !matches!(self.peek().kind, TokenKind::Ident(_))
            || self.tokens[self.pos + 1].kind != TokenKind::LBracket
        {
            return false;
        }
        let mut depth = 0;
        for (i, token) in self.tokens.iter().enumerate().skip(self.pos + 1) {
            match token.kind {
                TokenKind::LBracket | TokenKind::LBrace | TokenKind::LParen => depth += 1,
                TokenKind::RBracket | TokenKind::RBrace | TokenKind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return self
                            .tokens
                            .get(i + 1)
                            .is_some_and(|t| t.kind == TokenKind::LParen);
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
        }
        false
    }
}
