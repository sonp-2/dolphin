//! 式のコンパイル

use super::*;

impl Compiler {
    pub(super) fn expr(&mut self, expr: &Expr) {
        let span = expr.span;
        match &expr.kind {
            ExprKind::Number(n) => self.constant(Value::Num(*n), span),
            ExprKind::Int(n) => self.constant(Value::Int(*n), span),
            ExprKind::Bool(b) => self.constant(Value::Bool(*b), span),
            ExprKind::Nil => self.nil(span),
            ExprKind::Str(parts) => self.str_expr(parts, span),
            ExprKind::Var(var) => self.emit_load(&var.0, span),
            ExprKind::Array(items) => {
                if has_spread(items) {
                    self.build_list(items);
                } else {
                    for item in items {
                        self.expr(item);
                    }
                    self.emit(Op::MakeArray(items.len() as u32), span);
                }
            }
            ExprKind::Map(entries) => {
                for (key, value) in entries {
                    self.expr(key);
                    self.emit(Op::CheckKey, key.span);
                    self.expr(value);
                }
                self.emit(Op::MakeMap(entries.len() as u32), span);
            }
            ExprKind::Member { target, name } => {
                self.expr(target);
                let name = self.name(name);
                self.emit(
                    Op::Member {
                        name,
                        target: target.span,
                    },
                    span,
                );
            }
            ExprKind::FuncRef(name) => {
                let name = self.name(name);
                self.emit(Op::FuncRef(name), span);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.match_common(scrutinee, arms, span, |c, body| c.expr(body), |_| false);
            }
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => {
                self.expr(cond);
                let to_else = self.emit(Op::JumpIfFalseCond(0), cond.span);
                self.expr(then_expr);
                let to_end = self.emit(Op::Jump(0), span);
                self.patch_here(to_else);
                self.expr(else_expr);
                self.patch_here(to_end);
            }
            // 引数と配列リテラルの外では、`...` は構文解析で弾かれる
            ExprKind::Spread(inner) => self.expr(inner),
            ExprKind::Try(inner) => {
                self.expr(inner);
                self.emit(Op::Try, span);
            }
            ExprKind::Await(inner) => {
                self.expr(inner);
                self.emit(Op::Await, span);
            }
            ExprKind::Lambda(def) => {
                // 無名関数は、いまのスコープの変数を捕まえる（スロットには置けない）
                self.bail();
                self.chunk.defs.push(def.clone());
                let i = (self.chunk.defs.len() - 1) as u32;
                self.emit(Op::MakeClosure(i), span);
            }
            ExprKind::CallValue { callee, args } => self.call_value_expr(callee, args, span),
            ExprKind::MemberCall { target, name, args } => {
                self.member_call_expr(target, name, args, span);
            }
            ExprKind::Index { target, index } => {
                self.expr(target);
                self.expr(index);
                self.emit(
                    Op::IndexGet {
                        target: target.span,
                        key: index.span,
                    },
                    span,
                );
            }
            ExprKind::Unary { op, operand } => {
                self.expr(operand);
                self.emit(Op::Unary(*op), span);
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary_expr(*op, lhs, rhs, span),
            ExprKind::Call { name, args } => self.call_expr(name, args, span),
        }
    }

    fn str_expr(&mut self, parts: &[StrSegment], span: Span) {
        if let [StrSegment::Text(text)] = parts {
            self.constant(Value::Str(text.as_str().into()), span);
            return;
        }
        for part in parts {
            match part {
                StrSegment::Text(text) => {
                    self.constant(Value::Str(text.as_str().into()), span);
                }
                StrSegment::Expr(inner) => {
                    self.expr(inner);
                    self.emit(Op::ToDisplay, inner.span);
                }
            }
        }
        self.emit(Op::Concat(parts.len() as u32), span);
    }

    fn binary_expr(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr, span: Span) {
        if let BinaryOp::And | BinaryOp::Or = op {
            // 短絡評価。両辺とも真偽値に限る
            self.expr(lhs);
            self.emit(Op::LogicOperand(op), lhs.span);
            let short = self.emit(Op::ShortCircuit(op, 0), span);
            self.expr(rhs);
            self.emit(Op::LogicOperand(op), rhs.span);
            self.patch_here(short);
        } else {
            self.expr(lhs);
            self.expr(rhs);
            self.emit(Op::Binary(op), span);
        }
    }

    fn call_expr(&mut self, name: &str, args: &[Expr], span: Span) {
        let name = self.name(name);
        if has_spread(args) {
            self.build_list(args);
            self.emit(Op::CallSpread { name }, span);
        } else {
            for arg in args {
                self.expr(arg);
            }
            self.emit(
                Op::Call {
                    name,
                    argc: args.len() as u32,
                },
                span,
            );
        }
    }

    fn call_value_expr(&mut self, callee: &Expr, args: &[Expr], span: Span) {
        self.expr(callee);
        if has_spread(args) {
            self.build_list(args);
            self.emit(
                Op::CallValueSpread {
                    callee: callee.span,
                },
                span,
            );
        } else {
            for arg in args {
                self.expr(arg);
            }
            self.emit(
                Op::CallValue {
                    argc: args.len() as u32,
                    callee: callee.span,
                },
                span,
            );
        }
    }

    fn member_call_expr(&mut self, target: &Expr, name: &str, args: &[Expr], span: Span) {
        self.expr(target);
        let name = self.name(name);
        if has_spread(args) {
            self.build_list(args);
            self.emit(
                Op::MemberCallSpread {
                    name,
                    target: target.span,
                },
                span,
            );
        } else {
            for arg in args {
                self.expr(arg);
            }
            self.emit(
                Op::MemberCall {
                    name,
                    argc: args.len() as u32,
                    target: target.span,
                },
                span,
            );
        }
    }

    /// 展開（`...式`）を含む並びを、1 つの配列にして積む
    fn build_list(&mut self, items: &[Expr]) {
        let first = items.first().map_or(Span { line: 1, col: 1 }, |e| e.span);
        self.emit(Op::ArrayNew, first);
        for item in items {
            if let ExprKind::Spread(inner) = &item.kind {
                self.expr(inner);
                self.emit(Op::ArrayExtend, inner.span);
            } else {
                self.expr(item);
                self.emit(Op::ArrayAppend, item.span);
            }
        }
    }
}

fn has_spread(items: &[Expr]) -> bool {
    items.iter().any(|e| matches!(e.kind, ExprKind::Spread(_)))
}
