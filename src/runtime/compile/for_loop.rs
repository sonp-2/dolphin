//! `for` 文のコンパイル。ループ変数が単純な名前ならスロット（または繰り返しごとのスコープ）に、
//! パターンなら隠した変数に受けて同じスコープで分割代入する

use super::*;

impl Compiler {
    pub(super) fn for_stmt(&mut self, target: &Pattern, iter: &Expr, body: &[Stmt], span: Span) {
        self.expr(iter);
        self.emit(Op::ForPrep, iter.span);
        let next = self.here();
        // パターンのときは、隠した変数に要素を受け、同じスコープで分割代入する
        // （分割代入があるので、その関数はスロットにしない）
        let hidden;
        let var = match target {
            Pattern::Var(var) => var,
            _ => {
                self.bail();
                hidden = Var("%for".into());
                &hidden
            }
        };
        let for_next = if let (Some(slots), Pattern::Var(_)) = (&mut self.slots, target) {
            // 繰り返しごとの新しいスコープ（ループ変数と本体）は、コンパイル時だけのもの
            slots.scopes.push(Vec::new());
            let slot = slots.declare(&var.0);
            self.emit(Op::ForNextLocal { end: 0, slot }, span)
        } else {
            let var = self.name(&var.0);
            self.emit(Op::ForNext { end: 0, var }, span)
        };
        self.loops.push(LoopCtx {
            depth: self.scope_depth,
            breaks: Vec::new(),
            continues: Vec::new(),
        });
        if !matches!(target, Pattern::Var(_)) {
            self.emit_load("%for", span);
            self.chunk.patterns.push(target.clone());
            let pattern = (self.chunk.patterns.len() - 1) as u32;
            self.emit(
                Op::Destructure {
                    pattern,
                    declare: true,
                },
                span,
            );
        }
        // 繰り返しごとの新しいスコープ（ループ変数と本体）
        if self.slots.is_some() && matches!(target, Pattern::Var(_)) {
            self.block(body);
            self.pop_slot_scope();
        } else {
            self.scope_depth += 1;
            self.block(body);
            self.scope_depth -= 1;
            self.emit(Op::PopScope, span);
        }
        self.emit(Op::Jump(next), span);
        let exit = self.here();
        self.patch(for_next, exit);
        let ctx = self.loops.pop().expect("ループに入っている");
        for at in ctx.breaks {
            self.patch(at, exit);
        }
        for at in ctx.continues {
            self.patch(at, next);
        }
        // 準備で積んだ、要素の写しと位置
        self.emit(Op::Pop, span);
        self.emit(Op::Pop, span);
    }
}
