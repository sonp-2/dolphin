//! `match` のコンパイル（design.md 5.59）
//!
//! 対象の値は隠した変数に持ち、腕ごとに `MatchPat`（合えば束縛する値を積む）→ 変数の宣言
//! → ガード → 本体の順に置く。腕のスコープは、変数が通常の `Declare` / `StoreLocal` で
//! 作られるので、スロットでも `Env` でも同じ並びで動く

use super::*;

impl Compiler {
    /// 文（本体がブロック）と式（本体が式）で共通の並び。`emit_body` が本体を出力し、
    /// `declares` は本体がスコープに変数を作るか（パターンの変数がないときのスコープの要否）
    pub(super) fn match_common<B>(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm<B>],
        span: Span,
        emit_body: fn(&mut Self, &B),
        declares: fn(&B) -> bool,
    ) {
        self.expr(scrutinee);
        let hidden = format!("%match{}", self.hidden);
        self.hidden += 1;
        self.enter_scope(span);
        self.emit_declare(&hidden, span);

        let mut ends = Vec::new();
        for arm in arms {
            self.emit_load(&hidden, arm.span);
            let vars = arm.pattern.vars();
            self.chunk.matches.push(MatchInfo {
                pattern: arm.pattern.clone(),
                vars: vars.iter().map(|v| intern(&v.0)).collect(),
            });
            let pattern = (self.chunk.matches.len() - 1) as u32;
            let test = self.emit(Op::MatchPat { pattern, fail: 0 }, arm.span);

            let scoped = !vars.is_empty() || declares(&arm.body);
            if scoped {
                self.enter_scope(arm.span);
            }
            // 積まれた値は、最後の変数が一番上
            for var in vars.iter().rev() {
                self.emit_declare(&var.0, arm.span);
            }
            let guard_fail = arm.guard.as_ref().map(|guard| {
                self.expr(guard);
                self.emit(Op::JumpIfFalseCond(0), guard.span)
            });
            emit_body(self, &arm.body);
            if scoped {
                self.leave_scope(arm.span);
            }
            ends.push(self.emit(Op::Jump(0), arm.span));
            if let Some(guard_fail) = guard_fail {
                self.patch_here(guard_fail);
                // ガードが偽のときも、腕のスコープを閉じてから次の腕へ
                if scoped && self.slots.is_none() {
                    self.emit(Op::PopScope, arm.span);
                }
            }
            self.patch_here(test);
        }
        self.emit_load(&hidden, span);
        self.emit(Op::MatchFail, span);
        for at in ends {
            self.patch_here(at);
        }
        self.leave_scope(span);
    }

    /// 新しいスコープに入る。スロットのときはコンパイル時だけのもの
    fn enter_scope(&mut self, span: Span) {
        if let Some(slots) = &mut self.slots {
            slots.scopes.push(Vec::new());
        } else {
            self.emit(Op::PushScope, span);
            self.scope_depth += 1;
        }
    }

    fn leave_scope(&mut self, span: Span) {
        if self.slots.is_some() {
            self.pop_slot_scope();
        } else {
            self.scope_depth -= 1;
            self.emit(Op::PopScope, span);
        }
    }
}
