//! 式（下ほど優先度が高い）

use std::rc::Rc;

use super::super::ast::{BinaryOp, Expr, ExprKind, FuncDef, UnaryOp, Var};
use super::super::lexer::TokenKind;
use super::super::span::Span;
use super::string::str_segments;
use super::{Diagnostic, PResult, Parser};

impl<'t> Parser<'t> {
    // ---- 式（下ほど優先度が高い） ----

    pub(super) fn expression(&mut self) -> PResult<Expr> {
        self.or_expr()
    }

    /// or_expr = and_expr { "||" and_expr }
    pub(super) fn or_expr(&mut self) -> PResult<Expr> {
        self.left_assoc(Self::and_expr, |kind| match kind {
            TokenKind::OrOr => Some(BinaryOp::Or),
            _ => None,
        })
    }

    /// and_expr = cmp_expr { "&&" cmp_expr }
    pub(super) fn and_expr(&mut self) -> PResult<Expr> {
        self.left_assoc(Self::cmp_expr, |kind| match kind {
            TokenKind::AndAnd => Some(BinaryOp::And),
            _ => None,
        })
    }

    /// cmp_expr = add_expr [ 比較演算子 add_expr ]
    /// 比較は連鎖させない（`1 < @x < 3` はエラー）
    pub(super) fn cmp_expr(&mut self) -> PResult<Expr> {
        let lhs = self.add_expr()?;
        let Some(op) = cmp_op(&self.peek().kind) else {
            return Ok(lhs);
        };
        let span = self.advance().span;
        let rhs = self.add_expr()?;
        if cmp_op(&self.peek().kind).is_some() {
            return Err(Diagnostic::new(
                self.peek().span,
                "比較は続けて書けません（`&&` でつないでください）",
            ));
        }
        Ok(binary(op, lhs, rhs, span))
    }

    /// add_expr = mul_expr { ( "+" | "-" ) mul_expr }
    pub(super) fn add_expr(&mut self) -> PResult<Expr> {
        self.left_assoc(Self::mul_expr, |kind| match kind {
            TokenKind::Plus => Some(BinaryOp::Add),
            TokenKind::Minus => Some(BinaryOp::Sub),
            _ => None,
        })
    }

    /// mul_expr = unary { ( "*" | "/" | "%" ) unary }
    pub(super) fn mul_expr(&mut self) -> PResult<Expr> {
        self.left_assoc(Self::unary, |kind| match kind {
            TokenKind::Star => Some(BinaryOp::Mul),
            TokenKind::Slash => Some(BinaryOp::Div),
            TokenKind::Percent => Some(BinaryOp::Rem),
            _ => None,
        })
    }

    /// 左結合の二項演算 `operand { op operand }` の共通処理
    pub(super) fn left_assoc(
        &mut self,
        operand: fn(&mut Self) -> PResult<Expr>,
        op_of: fn(&TokenKind) -> Option<BinaryOp>,
    ) -> PResult<Expr> {
        let mut lhs = operand(self)?;
        while let Some(op) = op_of(&self.peek().kind) {
            let span = self.advance().span;
            let rhs = operand(self)?;
            lhs = binary(op, lhs, rhs, span);
        }
        Ok(lhs)
    }

    /// unary = ( "-" | "!" ) unary | "await" postfix_without_try { 後置 } | postfix
    ///
    /// `await 式?` は `(await 式)?` と読む（`?` は await の結果に効く）
    pub(super) fn unary(&mut self) -> PResult<Expr> {
        let op = match self.peek().kind {
            TokenKind::Minus => UnaryOp::Neg,
            TokenKind::Bang => UnaryOp::Not,
            TokenKind::Await => {
                let span = self.advance().span;
                let primary = self.primary()?;
                let operand = self.postfix_rest(primary, false)?;
                let awaited = Expr {
                    kind: ExprKind::Await(Box::new(operand)),
                    span,
                };
                return self.postfix_rest(awaited, true);
            }
            _ => return self.postfix(),
        };
        let span = self.advance().span;
        let operand = self.unary()?;
        Ok(Expr {
            kind: ExprKind::Unary {
                op,
                operand: Box::new(operand),
            },
            span,
        })
    }

    /// postfix = primary { "{" expression "}" | "[" [ args ] "]" | "." member | "?" }
    /// member  = IDENT [ "[" [ args ] "]" ]
    pub(super) fn postfix(&mut self) -> PResult<Expr> {
        let expr = self.primary()?;
        self.postfix_rest(expr, true)
    }

    /// `expr` の後ろに続く `{ }` `[ ]` `.` `?` を読む。`allow_try` が false なら `?` の手前で止める
    pub(super) fn postfix_rest(&mut self, mut expr: Expr, allow_try: bool) -> PResult<Expr> {
        loop {
            let span = expr.span;
            if self.eat(&TokenKind::LBrace) {
                let index = self.expression()?;
                self.expect(&TokenKind::RBrace)?;
                expr = Expr {
                    kind: ExprKind::Index {
                        target: Box::new(expr),
                        index: Box::new(index),
                    },
                    span,
                };
            } else if self.eat(&TokenKind::LBracket) {
                let args = self.args(&TokenKind::RBracket)?;
                expr = Expr {
                    kind: ExprKind::CallValue {
                        callee: Box::new(expr),
                        args,
                    },
                    span,
                };
            } else if allow_try && self.eat(&TokenKind::Question) {
                expr = Expr {
                    kind: ExprKind::Try(Box::new(expr)),
                    span,
                };
            } else if self.eat(&TokenKind::Dot) {
                let TokenKind::Ident(name) = &self.peek().kind else {
                    return Err(self.unexpected("名前"));
                };
                let name = name.clone();
                self.advance();
                let target = Box::new(expr);
                let kind = if self.eat(&TokenKind::LBracket) {
                    let args = self.args(&TokenKind::RBracket)?;
                    ExprKind::MemberCall { target, name, args }
                } else {
                    ExprKind::Member { target, name }
                };
                expr = Expr { kind, span };
            } else {
                return Ok(expr);
            }
        }
    }

    /// `( 式 )`。式の前後の改行は読み飛ばす
    pub(super) fn paren_expr(&mut self) -> PResult<Expr> {
        self.expect(&TokenKind::LParen)?;
        self.skip_newlines();
        let expr = self.expression()?;
        self.skip_newlines();
        self.expect(&TokenKind::RParen)?;
        Ok(expr)
    }

    pub(super) fn primary(&mut self) -> PResult<Expr> {
        let token = self.peek();
        let kind = match &token.kind {
            TokenKind::Number(n) => ExprKind::Number(*n),
            TokenKind::Int(n) => ExprKind::Int(*n),
            TokenKind::Str(parts) => ExprKind::Str(str_segments(parts, token.span)?),
            TokenKind::True => ExprKind::Bool(true),
            TokenKind::False => ExprKind::Bool(false),
            TokenKind::Nil => ExprKind::Nil,
            TokenKind::Var(name) => ExprKind::Var(Var(name.clone())),
            // call = IDENT "[" [ args ] "]"。`[` が続かなければ、関数名だけ（関数値）
            TokenKind::Ident(name) => {
                self.advance();
                let kind = if self.eat(&TokenKind::LBracket) {
                    ExprKind::Call {
                        name: name.clone(),
                        args: self.args(&TokenKind::RBracket)?,
                    }
                } else {
                    ExprKind::FuncRef(name.clone())
                };
                return Ok(Expr {
                    kind,
                    span: token.span,
                });
            }
            // cond_expr = "if" expression "(" expression ")" "else" ( "(" expression ")" | cond_expr )
            TokenKind::If => {
                self.advance();
                let cond = self.expression()?;
                let then_expr = self.paren_expr()?;
                if !self.at(&TokenKind::Else) {
                    return Err(self.unexpected("式の `if` の `else`"));
                }
                self.advance();
                let else_expr = if self.at(&TokenKind::If) {
                    self.primary()?
                } else {
                    self.paren_expr()?
                };
                return Ok(Expr {
                    kind: ExprKind::If {
                        cond: Box::new(cond),
                        then_expr: Box::new(then_expr),
                        else_expr: Box::new(else_expr),
                    },
                    span: token.span,
                });
            }
            TokenKind::Match => return self.match_expr(),
            // lambda = [ "async" ] "[" [ params ] "]" block
            TokenKind::LBracket | TokenKind::Async => {
                let is_async = self.at(&TokenKind::Async);
                self.advance();
                if is_async {
                    if !self.at(&TokenKind::LBracket) {
                        return Err(self.unexpected("`[`（`async` の無名関数）"));
                    }
                    self.advance();
                }
                let (params, rest) = self.params()?;
                let body = self.block()?;
                let def = FuncDef {
                    name: String::new(),
                    is_async,
                    record: false,
                    params,
                    rest,
                    body,
                    span: token.span,
                };
                return Ok(Expr {
                    kind: ExprKind::Lambda(Rc::new(def)),
                    span: token.span,
                });
            }
            TokenKind::LBrace => {
                self.advance();
                return self.brace_literal(token.span);
            }
            // "(" expression ")"
            TokenKind::LParen => {
                self.advance();
                let expr = self.expression()?;
                self.expect(&TokenKind::RParen)?;
                return Ok(expr);
            }
            _ => return Err(self.unexpected("式")),
        };
        self.advance();
        Ok(Expr {
            kind,
            span: token.span,
        })
    }

    /// array_lit = "{" [ args ] "}"
    /// map_lit   = "{" ":" "}" | "{" entry { "," entry } "}"
    ///
    /// `{` の後に呼ぶ。最初の要素の後に `:` があればマップ、なければ配列（design.md 4.1）
    pub(super) fn brace_literal(&mut self, span: Span) -> PResult<Expr> {
        let kind = if self.eat(&TokenKind::Colon) {
            self.expect(&TokenKind::RBrace)?;
            ExprKind::Map(Vec::new())
        } else if self.eat(&TokenKind::RBrace) {
            ExprKind::Array(Vec::new())
        } else {
            let first = self.arg_expr()?;
            if matches!(first.kind, ExprKind::Spread(_)) && self.at(&TokenKind::Colon) {
                return Err(Diagnostic::new(first.span, "マップには `...` を使えません"));
            }
            if self.eat(&TokenKind::Colon) {
                let mut entries = vec![(first, self.expression()?)];
                while !self.eat(&TokenKind::RBrace) {
                    if !self.eat(&TokenKind::Comma) {
                        return Err(self.unexpected("`,` か `}`"));
                    }
                    let key = self.expression()?;
                    self.expect(&TokenKind::Colon)?;
                    entries.push((key, self.expression()?));
                }
                ExprKind::Map(entries)
            } else {
                let mut items = vec![first];
                while !self.eat(&TokenKind::RBrace) {
                    if !self.eat(&TokenKind::Comma) {
                        return Err(self.unexpected("`,` か `}`"));
                    }
                    items.push(self.arg_expr()?);
                }
                ExprKind::Array(items)
            }
        };
        Ok(Expr { kind, span })
    }

    /// 引数・配列の要素 1 つ。`...式` は展開
    pub(super) fn arg_expr(&mut self) -> PResult<Expr> {
        let span = self.peek().span;
        if self.eat(&TokenKind::Ellipsis) {
            let inner = self.expression()?;
            return Ok(Expr {
                kind: ExprKind::Spread(Box::new(inner)),
                span,
            });
        }
        self.expression()
    }

    /// args = arg { "," arg }、arg = [ "..." ] expression
    /// 開き括弧の後に呼ぶ。閉じ括弧 `end` まで読み進める
    pub(super) fn args(&mut self, end: &TokenKind) -> PResult<Vec<Expr>> {
        let mut args = Vec::new();
        if self.eat(end) {
            return Ok(args);
        }
        loop {
            args.push(self.arg_expr()?);
            if self.eat(end) {
                return Ok(args);
            }
            if !self.eat(&TokenKind::Comma) {
                return Err(self.unexpected(&format!("`,` か {end}")));
            }
        }
    }
}

fn cmp_op(kind: &TokenKind) -> Option<BinaryOp> {
    match kind {
        TokenKind::Eq => Some(BinaryOp::Eq),
        TokenKind::NotEq => Some(BinaryOp::NotEq),
        TokenKind::Lt => Some(BinaryOp::Lt),
        TokenKind::LtEq => Some(BinaryOp::LtEq),
        TokenKind::Gt => Some(BinaryOp::Gt),
        TokenKind::GtEq => Some(BinaryOp::GtEq),
        _ => None,
    }
}

/// 二項演算の Expr を作る。位置は演算子の位置にする（型エラーで演算子を指すため）
fn binary(op: BinaryOp, lhs: Expr, rhs: Expr, span: Span) -> Expr {
    Expr {
        kind: ExprKind::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        span,
    }
}
