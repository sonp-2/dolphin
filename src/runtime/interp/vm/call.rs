//! 関数・メソッドを呼ぶ命令
use super::*;

impl Interpreter<'_> {
    /// 展開を含む引数の配列を取り出す
    pub(super) fn pop_spread_args(&mut self) -> Vec<Value> {
        match self.pop() {
            Value::Array(items) => items.borrow().clone(),
            _ => Vec::new(),
        }
    }

    /// `@u.name`: マップなら `@m{"name"}` と同じ。モジュールなら変数、なければ同じ名前の関数を関数値にする
    pub(super) fn member(
        &mut self,
        base: Value,
        name: &str,
        target: Span,
        span: Span,
    ) -> RResult<Value> {
        if let Value::Map(_) = base {
            return index_get(&base, &Value::Str(name.into()), target, span);
        }
        let module = self.as_module(base, target)?;
        // 変数がなければ、同じ名前の関数を関数値として返す（design.md 5.8）
        let value = module.globals.get_own(name).or_else(|| {
            if let Some(native) = module.natives.get(name) {
                return Some(Value::Native(native.clone()));
            }
            let def = module.functions.borrow().get(name).cloned()?;
            Some(Value::Func(Closure::register(Closure {
                def,
                env: Rc::clone(&module.globals),
                module: Rc::clone(&module),
            })))
        });
        value.ok_or_else(|| {
            RuntimeError::new(
                span,
                format!("{} に変数 `@{name}` がありません", module_label(&module)),
            )
        })
    }

    /// `name[args]`: ファイルの関数、`import`、組み込み関数の順に探して呼ぶ
    pub(super) fn call_named(
        &mut self,
        name: &str,
        args: Vec<Value>,
        span: Span,
        ip: usize,
    ) -> RResult<Step> {
        let def = self.current.functions.borrow().get(name).cloned();
        if let Some(def) = def {
            let module = Rc::clone(&self.current);
            let env = Rc::clone(&module.globals);
            return self.enter_function(&module, &def, &env, args, span, ip);
        }
        let value = if name == "import" {
            self.import(&args, span)?
        } else {
            match self.native(name) {
                Some(f) => self.call_native(f, &args, span)?,
                None => {
                    return Err(RuntimeError::new(span, format!("未定義の関数 `{name}`")));
                }
            }
        };
        self.stack.push(value);
        Ok(Step::Next)
    }

    /// 関数値を呼ぶ命令。Dolphin の関数なら枠を積むだけ（戻りを待たない）
    pub(super) fn call_function_op(
        &mut self,
        f: &Value,
        args: Vec<Value>,
        span: Span,
        callee_span: Span,
        ip: usize,
    ) -> RResult<Step> {
        match f {
            Value::Func(closure) => {
                self.enter_function(&closure.module, &closure.def, &closure.env, args, span, ip)
            }
            Value::Native(native) => {
                let value = self.call_native(native.f, &args, span)?;
                self.stack.push(value);
                Ok(Step::Next)
            }
            other => Err(not_callable(other, callee_span)),
        }
    }

    /// `@u.name[args]`: マップの値の関数、モジュールの関数を呼ぶ
    pub(super) fn member_call(
        &mut self,
        base: Value,
        name: &str,
        args: Vec<Value>,
        span: Span,
        target: Span,
        ip: usize,
    ) -> RResult<Step> {
        let module = match base {
            // マップの値の関数を呼ぶ（`@m.handler[1]`）
            Value::Map(_) => {
                let f = index_get(&base, &Value::Str(name.into()), target, span)?;
                return self.call_function_op(&f, args, span, span, ip);
            }
            base => self.as_module(base, target)?,
        };
        let def = module.functions.borrow().get(name).cloned();
        match (def, module.natives.get(name)) {
            (Some(def), _) => {
                let env = Rc::clone(&module.globals);
                self.enter_function(&module, &def, &env, args, span, ip)
            }
            (None, Some(native)) => {
                let value = self.call_native(native.f, &args, span)?;
                self.stack.push(value);
                Ok(Step::Next)
            }
            (None, None) => Err(RuntimeError::new(
                span,
                format!("{} に関数 `{name}` がありません", module_label(&module)),
            )),
        }
    }

    /// `name[args]`（引数はスタックの上）。ファイルの関数ならスタックの引数をそのまま枠にする
    pub(super) fn call_op(
        &mut self,
        name: &Rc<str>,
        argc: usize,
        span: Span,
        ip: usize,
    ) -> RResult<Step> {
        let def = self.current.functions.borrow().get(&**name).cloned();
        if let Some(def) = def {
            let module = Rc::clone(&self.current);
            let env = Rc::clone(&module.globals);
            return self.enter_function_stack(&module, &def, &env, argc, span, ip);
        }
        let args = self.split_top(argc);
        self.call_named(name, args, span, ip)
    }

    /// `f[args]`（関数値と引数はスタックの上）。Dolphin の関数ならスタックの引数をそのまま枠にする
    pub(super) fn call_value_op(
        &mut self,
        argc: usize,
        callee: Span,
        span: Span,
        ip: usize,
    ) -> RResult<Step> {
        let f_at = self.stack.len() - argc - 1;
        if let Value::Func(closure) = &self.stack[f_at] {
            let closure = Rc::clone(closure);
            // 関数値を引数の下から抜く（引数は動かさない）
            self.stack.remove(f_at);
            return self.enter_function_stack(
                &closure.module,
                &closure.def,
                &closure.env,
                argc,
                span,
                ip,
            );
        }
        let args = self.split_top(argc);
        let f = self.pop();
        self.call_function_op(&f, args, span, callee, ip)
    }
}
