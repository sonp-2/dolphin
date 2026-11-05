//! `async` 関数の流れ（design.md 5.46）
use super::*;

impl Interpreter<'_> {
    // ---- async 関数の流れ（design.md 5.46） ----

    /// `async` 関数を呼んで、新しい流れを始める。最初の `await` で止まるか、終わるまで、その場で動かす。
    /// 結果を受け取る future を返す
    pub(in super::super) fn start_coroutine(
        &mut self,
        module: &Rc<Module>,
        def: &Rc<FuncDef>,
        parent: &Rc<Env>,
        args: Vec<Value>,
        span: Span,
    ) -> RResult<Value> {
        // その場で始まる流れの入れ子は、いまの深さに積む（Rust の再帰が無限に深くならないように）
        let base = self.depth;
        let (mut frame, env, locals) =
            self.make_function_frame(module, def, parent, args, span, base)?;
        frame.base = 0;
        let future = Handle::new_future();
        let id = self.next_coroutine;
        self.next_coroutine += 1;
        let co = Coroutine {
            frames: vec![frame],
            stack: locals,
            depth: base + 1,
            current: Rc::clone(module),
            env,
            future: future.clone(),
        };
        self.run_coroutine(id, co)?;
        Ok(future)
    }

    /// 流れを動かす。いまの実行の状態は預けておき、止まるか終わったら戻す。`Err` は `os.exit` だけ
    pub(super) fn run_coroutine(&mut self, id: u64, mut co: Coroutine) -> RResult<()> {
        let outer_frames = mem::replace(&mut self.vm_frames, mem::take(&mut co.frames));
        let outer_stack = mem::replace(&mut self.stack, mem::take(&mut co.stack));
        let outer_depth = mem::replace(&mut self.depth, co.depth);
        let outer_current = mem::replace(&mut self.current, Rc::clone(&co.current));
        let outer_env = mem::replace(&mut self.env, Rc::clone(&co.env));
        let outer_coroutine = self.current_coroutine.replace(id);

        let result = self.run_vm_exit(0);

        match &result {
            Ok(Exit::Suspend) => {
                co.frames = mem::take(&mut self.vm_frames);
                co.stack = mem::take(&mut self.stack);
                co.depth = self.depth;
                co.current = Rc::clone(&self.current);
                co.env = Rc::clone(&self.env);
            }
            Ok(Exit::Return(_)) | Err(_) => {}
        }
        self.vm_frames = outer_frames;
        self.stack = outer_stack;
        self.depth = outer_depth;
        self.current = outer_current;
        self.env = outer_env;
        self.current_coroutine = outer_coroutine;

        match result {
            Ok(Exit::Suspend) => {
                self.coroutines.insert(id, co);
            }
            Ok(Exit::Return(value)) => {
                self.settle_future(&co.future, FutureOutcome::Resolved(value));
            }
            Err(e) => {
                if self.exit_code.is_some() {
                    let mut signal = RuntimeError::new(e.span, "os.exit による終了");
                    signal.exit = true;
                    return Err(signal);
                }
                self.settle_future(&co.future, FutureOutcome::Failed(e));
            }
        }
        Ok(())
    }

    /// 再開できる流れを、すべて進める（再開した流れが、さらに別の流れを再開できるようにしても、続ける）
    pub(crate) fn run_ready(&mut self) -> RResult<()> {
        while let Some(id) = self.ready.pop_front() {
            if let Some(co) = self.coroutines.remove(&id) {
                self.run_coroutine(id, co)?;
            }
        }
        Ok(())
    }

    /// future を終わらせて、待っていた流れを再開できる列に入れる
    pub(crate) fn settle_future(&mut self, future: &Value, outcome: FutureOutcome) {
        let Value::Handle(handle) = future else {
            return;
        };
        let mut inner = handle.inner.borrow_mut();
        let HandleKind::Future(state) = &mut *inner else {
            return;
        };
        if !matches!(state.outcome, FutureOutcome::Pending) {
            return;
        }
        state.outcome = outcome.clone();
        self.ready.extend(mem::take(&mut state.waiters));
        let forwards = mem::take(&mut state.forwards);
        drop(inner);
        // 転送先（`async.race` `async.timeout`）にも、同じ結果を渡す
        for forward in forwards {
            if let Some(id) = forward.cancel_timer {
                self.events.cancel(id);
            }
            let outcome = match (&outcome, forward.replace) {
                (FutureOutcome::Failed(e), _) if forward.catch => {
                    FutureOutcome::Resolved(crate::stdlib::error::runtime_error_value(e))
                }
                (FutureOutcome::Resolved(_), Some(value)) => FutureOutcome::Resolved(value),
                _ => outcome.clone(),
            };
            self.settle_future(&forward.target, outcome);
        }
    }

    pub(super) fn add_waiter(&mut self, future: &Value, id: u64) {
        if let Value::Handle(handle) = future
            && let HandleKind::Future(state) = &mut *handle.inner.borrow_mut()
        {
            state.waiters.push(id);
        }
    }

    /// future の状態。終わっていなければ `None`。失敗は受け取ったことにする。future でなければエラー
    pub(super) fn future_outcome(
        &self,
        value: &Value,
        span: Span,
    ) -> RResult<Option<Result<Value, RuntimeError>>> {
        let not_future = || {
            RuntimeError::new(
                span,
                format!(
                    "await できるのは future だけです（{}でした）",
                    value.type_name()
                ),
            )
        };
        let Value::Handle(handle) = value else {
            return Err(not_future());
        };
        let mut inner = handle.inner.borrow_mut();
        let HandleKind::Future(state) = &mut *inner else {
            return Err(not_future());
        };
        Ok(match &state.outcome {
            FutureOutcome::Pending => None,
            FutureOutcome::Resolved(value) => Some(Ok(value.clone())),
            FutureOutcome::Failed(e) => {
                state.observed = true;
                Some(Err(e.clone()))
            }
        })
    }

    /// 止められない場所での `await`: future が終わるまで、イベントループを回して待つ
    pub(super) fn block_on(&mut self, future: &Value, span: Span) -> RResult<Value> {
        loop {
            self.run_ready()?;
            if let Some(outcome) = self.future_outcome(future, span)? {
                return outcome;
            }
            if !self.ready.is_empty() {
                continue;
            }
            if !self.events.has_pending() {
                return Err(RuntimeError::new(
                    span,
                    "await している future が終わる見込みがありません（待っているものがありません）",
                ));
            }
            if let Err(message) = self.wait_one_event() {
                let mut signal = RuntimeError::new(span, message);
                signal.exit = true;
                return Err(signal);
            }
        }
    }

    /// `await`: 終わっていればその値、止められる流れなら止まり、止められない場所なら終わるまで待つ
    pub(super) fn await_op(&mut self, ip: &mut usize, span: Span, target: usize) -> RResult<Step> {
        let future = self.stack.last().cloned().expect("スタックに値がある");
        match self.future_outcome(&future, span)? {
            Some(Ok(value)) => {
                self.pop();
                self.stack.push(value);
            }
            Some(Err(e)) => {
                self.pop();
                return Err(e);
            }
            None => {
                // 流れの入口の実行なら、ここで止める。再開したときに、この命令からやり直す
                if let Some(id) = self.current_coroutine
                    && target == 0
                {
                    self.add_waiter(&future, id);
                    *ip -= 1;
                    self.save_ip(*ip);
                    return Ok(Step::Suspend);
                }
                // 止められない場所（ファイルの最上位など）では、終わるまでイベントループを回す
                let value = self.block_on(&future, span)?;
                self.pop();
                self.stack.push(value);
            }
        }
        Ok(Step::Next)
    }
}
