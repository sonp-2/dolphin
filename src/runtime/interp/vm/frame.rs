//! 枠の作成と呼び出し（design.md 5.45）。関数の枠を積む・抜ける
use super::*;

impl Interpreter<'_> {
    /// 関数の命令列。最初に呼ばれたときにコンパイルして覚える
    pub(super) fn chunk_for(&mut self, def: &Rc<FuncDef>) -> Rc<Chunk> {
        let key = Rc::as_ptr(def) as usize;
        if let Some((_, chunk)) = self.chunks.get(&key) {
            return Rc::clone(chunk);
        }
        let chunk = Rc::new(compile_function(def));
        // 定義を持たせておく（解放されて、同じアドレスが別の定義に使われないように）
        self.chunks.insert(key, (Rc::clone(def), Rc::clone(&chunk)));
        chunk
    }

    /// ファイルの本体の枠を積む。スコープは変えない（いまのスコープがファイルのトップレベル）
    pub(in super::super) fn push_program_frame(&mut self, chunk: Chunk) {
        self.vm_frames.push(VmFrame {
            chunk: Rc::new(chunk),
            ip: 0,
            base: self.stack.len(),
            kind: FrameKind::Program,
            module: Rc::clone(&self.current),
            given: 0,
            scopes: Vec::new(),
            defers: Vec::new(),
            caller_module: Rc::clone(&self.current),
            caller_env: Rc::clone(&self.env),
        });
    }

    /// 関数 `def` を呼ぶ枠を積む。`parent` は関数が作られた場所のスコープで、その内側に引数のスコープを作る。
    /// 実行している間は、関数名を `module` から引く
    pub(in super::super) fn push_function_frame(
        &mut self,
        module: &Rc<Module>,
        def: &Rc<FuncDef>,
        parent: &Rc<Env>,
        args: Vec<Value>,
        span: Span,
    ) -> RResult<()> {
        let (frame, env, locals) =
            self.make_function_frame(module, def, parent, args, span, self.depth)?;
        self.stack.extend(locals);
        self.current = Rc::clone(module);
        self.env = env;
        self.depth += 1;
        self.vm_frames.push(frame);
        Ok(())
    }

    /// 関数の枠と、その引数のスコープを作る（積まない）。`depth` はいまの呼び出しの深さ。
    /// ローカル変数をスロットに置く関数なら、スコープは作らず（外側のスコープをそのまま使う）、
    /// スロットの初期値（引数、残りの引数、空き）を返す。呼び出し側が値のスタックに積む
    pub(super) fn make_function_frame(
        &mut self,
        module: &Rc<Module>,
        def: &Rc<FuncDef>,
        parent: &Rc<Env>,
        args: Vec<Value>,
        span: Span,
        depth: usize,
    ) -> RResult<(VmFrame, Rc<Env>, Vec<Value>)> {
        self.check_call(def, args.len(), span, depth)?;

        gc::maybe_collect();
        let chunk = self.chunk_for(def);
        let given = args.len().min(def.params.len());
        let mut args = args;
        let extra = args.split_off(given);
        let (env, locals) = if let Some(count) = chunk.locals {
            args.resize(def.params.len(), Value::Nil);
            if def.rest.is_some() {
                args.push(gc::new_array(extra));
            }
            args.resize(count as usize, Value::Nil);
            (Rc::clone(parent), args)
        } else {
            // 関数のスコープは、そのファイルのトップレベルのスコープの内側に作る
            let env = Env::new(Some(Rc::clone(parent)));
            for (name, value) in chunk.param_names.iter().zip(args) {
                env.declare_rc(name, value);
            }
            if let Some(rest) = &chunk.rest_name {
                env.declare_rc(rest, gc::new_array(extra));
            }
            (env, Vec::new())
        };
        let frame = self.new_function_frame(module, def, chunk, given, span);
        Ok((frame, env, locals))
    }

    pub(super) fn new_function_frame(
        &self,
        module: &Rc<Module>,
        def: &Rc<FuncDef>,
        chunk: Rc<Chunk>,
        given: usize,
        span: Span,
    ) -> VmFrame {
        VmFrame {
            chunk,
            ip: 0,
            base: self.stack.len(),
            kind: FrameKind::Function {
                def: Rc::clone(def),
                call_span: span,
            },
            module: Rc::clone(module),
            given,
            scopes: Vec::new(),
            defers: Vec::new(),
            caller_module: Rc::clone(&self.current),
            caller_env: Rc::clone(&self.env),
        }
    }

    /// `enter_function` と同じだが、引数はスタックの上から `argc` 個にある（`Vec` を作らずに済ませる）。
    /// 引数が定義より多い・`...@rest` がある・`async` のときは、`Vec` を作る道に任せる
    pub(super) fn enter_function_stack(
        &mut self,
        module: &Rc<Module>,
        def: &Rc<FuncDef>,
        parent: &Rc<Env>,
        argc: usize,
        span: Span,
        ip: usize,
    ) -> RResult<Step> {
        // スロットの関数は、積まれた引数をそのままスロットにする
        if !def.is_async && argc <= def.params.len() {
            let chunk = self.chunk_for(def);
            if let Some(count) = chunk.locals {
                self.check_call(def, argc, span, self.depth)?;
                gc::maybe_collect();
                let base = self.stack.len() - argc;
                self.stack.resize(base + count as usize, Value::Nil);
                if def.rest.is_some() {
                    self.stack[base + def.params.len()] = gc::new_array(Vec::new());
                }
                let mut frame = self.new_function_frame(module, def, chunk, argc, span);
                frame.base = base;
                self.save_ip(ip);
                self.current = Rc::clone(module);
                self.env = Rc::clone(parent);
                self.depth += 1;
                self.vm_frames.push(frame);
                return Ok(Step::Reload);
            }
        }
        if def.is_async || def.rest.is_some() || argc > def.params.len() {
            let args = self.split_top(argc);
            return self.enter_function(module, def, parent, args, span, ip);
        }
        self.check_call(def, argc, span, self.depth)?;
        gc::maybe_collect();
        let env = Env::new(Some(Rc::clone(parent)));
        let chunk = self.chunk_for(def);
        let at = self.stack.len() - argc;
        for (name, value) in chunk.param_names.iter().zip(self.stack.drain(at..)) {
            env.declare_rc(name, value);
        }
        let frame = self.new_function_frame(module, def, chunk, argc, span);
        self.save_ip(ip);
        self.current = Rc::clone(module);
        self.env = env;
        self.depth += 1;
        self.vm_frames.push(frame);
        Ok(Step::Reload)
    }

    /// 引数の数と呼び出しの深さを確かめる
    pub(super) fn check_call(
        &self,
        def: &Rc<FuncDef>,
        argc: usize,
        span: Span,
        depth: usize,
    ) -> RResult<()> {
        if argc < def.min_args() || def.max_args().is_some_and(|max| argc > max) {
            return Err(RuntimeError::new(
                span,
                resolver::arity_message(&quoted_label(def), def.min_args(), def.max_args(), argc),
            ));
        }
        if depth >= MAX_DEPTH {
            return Err(RuntimeError::new(
                span,
                format!(
                    "関数の呼び出しが深すぎます（{} 段）。終わらない再帰になっていませんか",
                    MAX_DEPTH
                ),
            ));
        }
        Ok(())
    }

    /// 関数を呼ぶ命令。`async` 関数なら新しい流れを始めて future を積み、普通の関数なら枠を積むだけ
    /// （戻りを待たない）
    pub(super) fn enter_function(
        &mut self,
        module: &Rc<Module>,
        def: &Rc<FuncDef>,
        parent: &Rc<Env>,
        args: Vec<Value>,
        span: Span,
        ip: usize,
    ) -> RResult<Step> {
        if def.is_async {
            let future = self.start_coroutine(module, def, parent, args, span)?;
            self.stack.push(future);
            return Ok(Step::Next);
        }
        self.save_ip(ip);
        self.push_function_frame(module, def, parent, args, span)?;
        Ok(Step::Reload)
    }

    /// 枠を 1 つ抜けて、呼び出し元の状態に戻す
    pub(super) fn pop_frame(&mut self) -> VmFrame {
        let frame = self.vm_frames.pop().expect("抜ける枠がある");
        self.stack.truncate(frame.base);
        self.current = Rc::clone(&frame.caller_module);
        self.env = Rc::clone(&frame.caller_env);
        if let FrameKind::Function { .. } = frame.kind {
            self.depth -= 1;
        }
        frame
    }
}
