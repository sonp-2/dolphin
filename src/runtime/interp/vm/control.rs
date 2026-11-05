//! 変数・スコープ・分岐・論理・単項演算の命令

use super::*;

impl Interpreter<'_> {
    pub(super) fn load_var(&mut self, name: &str, span: Span) -> RResult<()> {
        match self.env.get(name) {
            Some(value) => {
                self.stack.push(value);
                Ok(())
            }
            None => Err(undefined_name(name, span)),
        }
    }

    pub(super) fn assign_var(&mut self, name: &str, span: Span) -> RResult<()> {
        let value = self.pop();
        if !self.env.assign(name, value) {
            return Err(undefined_name(name, span));
        }
        Ok(())
    }

    /// 捨てられたエラー値は、失敗に気づかないまま進まないよう実行時エラーにする
    pub(super) fn pop_check_error(&mut self, span: Span) -> RResult<()> {
        if let Value::Error(e) = self.pop() {
            return Err(RuntimeError::new(
                span,
                format!(
                    "エラー値が捨てられました（{}: {}）。変数に入れるか、`?` か `is_error` で扱ってください",
                    e.kind, e.message
                ),
            ));
        }
        Ok(())
    }

    pub(super) fn push_scope(&mut self) {
        let inner = Env::new(Some(Rc::clone(&self.env)));
        let outer = mem::replace(&mut self.env, inner);
        self.vm_frames
            .last_mut()
            .expect("枠がある")
            .scopes
            .push(outer);
    }

    pub(super) fn pop_scope(&mut self) {
        let outer = self
            .vm_frames
            .last_mut()
            .expect("枠がある")
            .scopes
            .pop()
            .expect("閉じるスコープがある");
        self.env = outer;
    }

    pub(super) fn jump_if_false_cond(&mut self, t: u32, ip: &mut usize, span: Span) -> RResult<()> {
        match self.pop() {
            Value::Bool(true) => {}
            Value::Bool(false) => *ip = t as usize,
            other => {
                return Err(RuntimeError::new(
                    span,
                    format!(
                        "条件は真偽値である必要があります（{}でした）",
                        other.type_name()
                    ),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn logic_operand(&mut self, op: BinaryOp, span: Span) -> RResult<()> {
        let top = self.stack.last().expect("スタックに値がある");
        if !matches!(top, Value::Bool(_)) {
            return Err(RuntimeError::new(
                span,
                format!(
                    "`{op}` の両辺は真偽値である必要があります（{}でした）",
                    top.type_name()
                ),
            ));
        }
        Ok(())
    }

    pub(super) fn short_circuit(&mut self, op: BinaryOp, t: u32, ip: &mut usize) {
        let short = matches!(
            (op, self.stack.last()),
            (BinaryOp::And, Some(Value::Bool(false))) | (BinaryOp::Or, Some(Value::Bool(true)))
        );
        if short {
            *ip = t as usize;
        } else {
            self.pop();
        }
    }

    pub(super) fn unary(&mut self, op: UnaryOp, span: Span) -> RResult<()> {
        let value = self.pop();
        let result = match (op, value) {
            (UnaryOp::Neg, Value::Num(n)) => Value::Num(-n),
            (UnaryOp::Neg, Value::Int(n)) => match n.checked_neg() {
                Some(n) => Value::Int(n),
                None => return Err(int_overflow(span)),
            },
            (UnaryOp::Not, Value::Bool(b)) => Value::Bool(!b),
            (op, value) => {
                return Err(RuntimeError::new(
                    span,
                    format!("{}に `{op}` は使えません", value.type_name()),
                ));
            }
        };
        self.stack.push(result);
        Ok(())
    }
}
