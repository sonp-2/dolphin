//! 文: ブロック、if・while・for、代入・宣言・式文

use super::super::ast::{Block, Expr, ExprKind, Stmt, StmtKind, StrSegment, Var};
use super::super::lexer::TokenKind;
use super::{Diagnostic, PResult, Parser};

impl<'t> Parser<'t> {
    /// `end` の手前まで、改行で区切られた文を読む。`end` 自体は読まない
    pub(super) fn statements_until(&mut self, end: &TokenKind) -> PResult<Vec<Stmt>> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !self.at(end) && !self.at(&TokenKind::Eof) {
            stmts.push(self.statement()?);
            if self.at(end) || self.at(&TokenKind::Eof) {
                break;
            }
            // 1 行に文は 1 つ（design.md 4.1）
            if !self.at(&TokenKind::Newline) {
                return Err(self.unexpected("改行"));
            }
            self.skip_newlines();
        }
        Ok(stmts)
    }

    /// block = "(" { NEWLINE } [ statement { NEWLINE { NEWLINE } statement } ] { NEWLINE } ")"
    pub(super) fn block(&mut self) -> PResult<Block> {
        self.expect(&TokenKind::LParen)?;
        let stmts = self.statements_until(&TokenKind::RParen)?;
        self.expect(&TokenKind::RParen)?;
        Ok(stmts)
    }

    pub(super) fn statement(&mut self) -> PResult<Stmt> {
        let span = self.peek().span;
        let kind = match self.peek().kind {
            TokenKind::If => return self.if_stmt(),
            TokenKind::While => {
                self.advance();
                let cond = self.expression()?;
                let body = self.block()?;
                StmtKind::While { cond, body }
            }
            // for_stmt = "for" pattern "in" expression block
            TokenKind::For => {
                self.advance();
                if !matches!(self.peek().kind, TokenKind::Var(_) | TokenKind::LBrace) {
                    return Err(self.unexpected("変数（`@x`）かパターン（`{@k, @v}`）"));
                }
                let target = self.pattern()?;
                self.expect(&TokenKind::In)?;
                let iter = self.expression()?;
                let body = self.block()?;
                StmtKind::For { target, iter, body }
            }
            TokenKind::Hash => {
                self.advance();
                let value = match self.peek().kind {
                    TokenKind::Newline | TokenKind::RParen | TokenKind::Eof => None,
                    _ => Some(self.expression()?),
                };
                StmtKind::Return(value)
            }
            TokenKind::Break => {
                self.advance();
                StmtKind::Break
            }
            TokenKind::Continue => {
                self.advance();
                StmtKind::Continue
            }
            TokenKind::Record => return self.record_def(),
            // defer_stmt = "defer" expression
            TokenKind::Defer => {
                self.advance();
                StmtKind::Defer(self.expression()?)
            }
            TokenKind::LBrace if self.looks_like_destructure() => return self.destructure(),
            // `async 名前[…] ( … )` は async 関数の定義。`async [` は無名関数の式
            TokenKind::Async if matches!(self.tokens[self.pos + 1].kind, TokenKind::Ident(_)) => {
                self.advance();
                if !self.looks_like_func_def() {
                    return Err(Diagnostic::new(
                        span,
                        "`async` の後には、関数の定義（`async 名前[引数] ( … )`）を書きます",
                    ));
                }
                return self.func_def_stmt(true, span);
            }
            _ => return self.simple_statement(),
        };
        Ok(Stmt { kind, span })
    }

    /// 代入・関数定義・式文。どれも式から始まるので、まず式として読み、
    /// 続くトークンで区別する
    ///
    /// - `:=` が続く → 宣言。左辺は変数に限る
    /// - `=` が続く → 代入。左辺は変数か `変数{式}...` に限る
    /// - `(` が続き、左辺が呼び出しの形 → 関数定義（design.md 4.1）
    pub(super) fn simple_statement(&mut self) -> PResult<Stmt> {
        let span = self.peek().span;
        if self.looks_like_func_def() {
            return self.func_def_stmt(false, span);
        }
        let expr = self.expression()?;

        let kind = if self.at(&TokenKind::Declare) {
            let ExprKind::Var(target) = expr.kind else {
                return Err(Diagnostic::new(expr.span, "宣言できるのは変数だけです"));
            };
            self.advance();
            let value = self.expression()?;
            StmtKind::Declare { target, value }
        } else if self.at(&TokenKind::Assign) {
            let (target, indices) = assign_target(expr)?;
            self.advance();
            let value = self.expression()?;
            StmtKind::Assign {
                target,
                indices,
                value,
            }
        } else if self.at(&TokenKind::LParen) {
            return Err(self.unexpected("改行"));
        } else {
            StmtKind::Expr(expr)
        };
        Ok(Stmt { kind, span })
    }

    /// if_stmt = "if" expression block [ { NEWLINE } "else" ( block | if_stmt ) ]
    pub(super) fn if_stmt(&mut self) -> PResult<Stmt> {
        let span = self.expect(&TokenKind::If)?;
        let cond = self.expression()?;
        let then_block = self.block()?;
        let else_block = if self.else_follows() {
            if self.at(&TokenKind::If) {
                Some(vec![self.if_stmt()?])
            } else {
                Some(self.block()?)
            }
        } else {
            None
        };
        Ok(Stmt {
            kind: StmtKind::If {
                cond,
                then_block,
                else_block,
            },
            span,
        })
    }

    /// 改行を挟んで `else` が続くなら、`else` まで読み進めて true を返す。
    /// 続かなければ位置を戻す（改行は文の区切りとして残す）
    pub(super) fn else_follows(&mut self) -> bool {
        let saved = self.pos;
        self.skip_newlines();
        if self.eat(&TokenKind::Else) {
            true
        } else {
            self.pos = saved;
            false
        }
    }
}

/// 代入の左辺を (変数, 添字を外側から順に並べたもの) に分解する
fn assign_target(expr: Expr) -> PResult<(Var, Vec<Expr>)> {
    let invalid = || Diagnostic::new(expr.span, "代入できるのは変数か配列の要素だけです");
    let mut indices = Vec::new();
    let mut current = expr.kind;
    loop {
        match current {
            // `@m.key = v` は `@m{"key"} = v`。モジュールかどうかは実行時に分かる
            ExprKind::Member { target, name } => {
                indices.push(Expr {
                    kind: ExprKind::Str(vec![StrSegment::Text(name)]),
                    span: target.span,
                });
                current = target.kind;
            }
            ExprKind::Var(var) => {
                indices.reverse();
                return Ok((var, indices));
            }
            ExprKind::Index { target, index } => {
                indices.push(*index);
                current = target.kind;
            }
            _ => return Err(invalid()),
        }
    }
}
