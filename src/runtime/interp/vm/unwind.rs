//! 戻りとエラーの巻き戻し、`defer`
use super::*;

impl Interpreter<'_> {
    /// エラーのために、`target` の枠まで（`target` は残す）枠を抜ける。別のファイルの関数で起きた
    /// エラーはそのファイルの行を引用し、呼び出し履歴に「f の中」を足す
    pub(super) fn unwind(&mut self, mut e: RuntimeError, target: usize) -> RuntimeError {
        while self.vm_frames.len() > target {
            // 実行時エラーで抜ける枠の後始末。元のエラーを残す（`os.exit` のときは走らない）
            if !e.exit
                && let Some(d) = self.run_defers()
                && d.exit
            {
                e = d;
            }
            let frame = self.pop_frame();
            if let FrameKind::Function { def, call_span } = frame.kind {
                // 出どころは、いちばん内側の関数のファイルで決まる（メインのファイルは出どころ `None` なので、
                // 呼び出し履歴が空かどうかで「まだ決めていない」を見分ける。決まった後に、外側の
                // `import` したファイルの関数が上書きしないように）
                if e.origin.is_none() && e.trace.is_empty() {
                    e.origin = frame.module.origin.clone();
                }
                e.trace.push(TraceEntry {
                    what: format!("{} の中", func_label(&def)),
                    span: call_span,
                    file: file_of(&self.current),
                });
            }
        }
        e
    }

    pub(super) fn do_return(&mut self, value: Value, target: usize) -> RResult<Step> {
        if self.vm_frames.last().is_some_and(|f| !f.defers.is_empty())
            && let Some(e) = self.run_defers()
        {
            return Err(e);
        }
        self.pop_frame();
        Ok(if self.vm_frames.len() == target {
            Step::Done(value)
        } else {
            self.stack.push(value);
            Step::Reload
        })
    }

    /// いまの枠の `defer` を、登録の逆順にすべて呼ぶ（design.md 5.53）。値は捨てる。
    /// 戻り値は、最初に起きたエラー。`os.exit` の合図はすぐに返す（残りは走らない）
    pub(super) fn run_defers(&mut self) -> Option<RuntimeError> {
        let mut first = None;
        while let Some((f, span)) = self.vm_frames.last_mut().and_then(|fr| fr.defers.pop()) {
            match self.call_function(&f, Vec::new(), span, span) {
                Ok(_) => {}
                Err(e) if e.exit => return Some(e),
                Err(e) => {
                    first.get_or_insert(e);
                }
            }
        }
        first
    }

    /// `?`: エラー値なら、関数の中ではその値を返し、関数の外ではエラーで止まる
    pub(super) fn try_op(&mut self, span: Span, target: usize) -> RResult<Step> {
        match self.pop() {
            Value::Error(e) => {
                let in_func = matches!(
                    self.vm_frames.last().map(|f| &f.kind),
                    Some(FrameKind::Function { .. })
                );
                if in_func {
                    // いまの関数から、そのエラー値を返す
                    return self.do_return(Value::Error(e), target);
                }
                // 関数の外では、エラーで止まる
                Err(RuntimeError::new(span, &*e.message))
            }
            value => {
                self.stack.push(value);
                Ok(Step::Next)
            }
        }
    }

    pub(super) fn defer_op(&mut self, span: Span) {
        let f = self.pop();
        self.vm_frames
            .last_mut()
            .expect("枠がある")
            .defers
            .push((f, span));
    }
}
