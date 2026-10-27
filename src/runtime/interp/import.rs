//! import（design.md 5.7）。ファイルと組み込みモジュールを読み込む

use super::*;

impl Interpreter<'_> {
    /// ファイルを読んで実行し、モジュールを返す。同じファイルは 1 回しか実行しない
    pub(super) fn import(&mut self, args: &[Value], span: Span) -> RResult<Value> {
        let rel = match args {
            [Value::Str(rel)] => rel,
            [other] => {
                return Err(RuntimeError::new(
                    span,
                    format!(
                        "import の 1 番目の引数は文字列です（{}が渡されました）",
                        other.type_name()
                    ),
                ));
            }
            _ => {
                return Err(RuntimeError::new(
                    span,
                    format!("import の引数は 1 個です（{} 個渡されました）", args.len()),
                ));
            }
        };

        if rel.starts_with("std:") {
            return self.import_std(rel, span);
        }
        let full = crate::runtime::module::resolve_import(&self.current.dir, rel)
            .map_err(|e| RuntimeError::new(span, e.to_string()))?;
        let shown = self.display_path(&full);
        let key = fs::canonicalize(&full).map_err(|e| {
            let reason = match e.kind() {
                io::ErrorKind::NotFound => "ファイルが見つかりません".to_string(),
                _ => e.to_string(),
            };
            RuntimeError::new(span, format!("`{rel}` を開けません: {reason}"))
        })?;
        match self.modules.get(&key) {
            Some(ModuleState::Loaded(module)) => return Ok(Value::Module(Rc::clone(module))),
            Some(ModuleState::Loading) => {
                let start = self.loading.iter().position(|n| *n == shown).unwrap_or(0);
                let mut chain: Vec<&str> = self.loading[start..].iter().map(|n| &**n).collect();
                chain.push(&shown);
                return Err(RuntimeError::new(
                    span,
                    format!("import が循環しています: {}", chain.join(" → ")),
                ));
            }
            None => {}
        }
        let src = fs::read_to_string(&full)
            .map_err(|e| RuntimeError::new(span, format!("`{rel}` を読み込めません: {e}")))?;

        let origin = Rc::new(Origin {
            path: shown.to_string(),
            src,
        });
        let entry = TraceEntry {
            what: format!("{shown} の読み込み中"),
            span,
            file: file_of(&self.current),
        };
        let dir = full.parent().map(Path::to_path_buf).unwrap_or_default();
        let module = Rc::new(Module::new(
            Rc::clone(&shown),
            dir,
            Some(Rc::clone(&origin)),
        ));

        // 読み込み中のエラーは、そのファイルの行を引用し、import した位置を呼び出し履歴に載せる
        let program = lexer::tokenize(&origin.src)
            .and_then(|tokens| parser::parse(&tokens))
            .and_then(|program| {
                resolver::resolve(&program, |name| self.native_callee(name), |_| false)?;
                Ok(program)
            })
            .map_err(|diag| RuntimeError {
                message: diag.message,
                span: diag.span,
                trace: vec![entry.clone()],
                origin: Some(Rc::clone(&origin)),
                early_return: false,
                exit: false,
            })?;

        self.modules.insert(key.clone(), ModuleState::Loading);
        self.loading.push(shown);
        let importer = mem::replace(&mut self.current, Rc::clone(&module));
        let importer_env = mem::replace(&mut self.env, Rc::clone(&module.globals));
        let result = self.run(&program, false);
        self.env = importer_env;
        self.current = importer;
        self.loading.pop();

        match result {
            Ok(_) => {
                self.modules
                    .insert(key, ModuleState::Loaded(Rc::clone(&module)));
                Ok(Value::Module(module))
            }
            Err(mut e) => {
                // 失敗したファイルは、直して再実行できるよう覚えない
                self.modules.remove(&key);
                if e.origin.is_none() {
                    e.origin = Some(origin);
                }
                e.trace.push(entry);
                Err(e)
            }
        }
    }

    /// 組み込みモジュール（`std:json` など）。同じものは同じモジュールを返す
    pub(super) fn import_std(&mut self, name: &str, span: Span) -> RResult<Value> {
        let Some((&key, functions)) = self.std_registry.get_key_value(name) else {
            let missing_feature = match name {
                "std:http" if !cfg!(feature = "http") => Some("http"),
                "std:crypto" if !cfg!(feature = "crypto") => Some("crypto"),
                "std:regex" if !cfg!(feature = "regex") => Some("regex"),
                "std:ui" if !cfg!(feature = "ui") => Some("ui"),
                "std:audio" if !cfg!(feature = "audio") => Some("audio"),
                "std:sqlite" if !cfg!(feature = "sqlite") => Some("sqlite"),
                "std:ffi" if !cfg!(feature = "ffi") => Some("ffi"),
                "std:image" if !cfg!(feature = "image") => Some("image"),
                _ => None,
            };
            if let Some(feature) = missing_feature {
                return Err(RuntimeError::new(
                    span,
                    format!(
                        "この版には `{name}` が含まれていません（`{feature}` feature を付けてビルドしてください）"
                    ),
                ));
            }
            let mut names: Vec<&str> = self.std_registry.keys().copied().collect();
            names.sort_unstable();
            return Err(RuntimeError::new(
                span,
                format!(
                    "組み込みモジュール `{name}` はありません（あるのは {}）",
                    names.join("、")
                ),
            ));
        };
        if let Some(module) = self.std_loaded.get(key) {
            return Ok(Value::Module(Rc::clone(module)));
        }
        let mut module = Module::new(key.into(), PathBuf::new(), None);
        for &(full, f) in functions {
            let short = full.rsplit('.').next().unwrap_or(full);
            module.natives.insert(short, NativeRef { name: full, f });
        }
        for &(name, value) in self.std_consts.get(key).into_iter().flatten() {
            let value = match value {
                Const::Num(n) => Value::Num(n),
                Const::Str(s) => Value::Str(s.into()),
            };
            module.globals.declare(name, value);
        }
        // Dolphin で書いた関数（`async.all` など）
        if key == "std:async" {
            let tokens = lexer::tokenize(crate::stdlib::async_::PRELUDE)
                .expect("std:async の Dolphin のコードは字句に分けられる");
            let program =
                parser::parse(&tokens).expect("std:async の Dolphin のコードは解析できる");
            for stmt in &program.stmts {
                if let StmtKind::FuncDef(def) = &stmt.kind {
                    module
                        .functions
                        .borrow_mut()
                        .insert(def.name.clone(), Rc::clone(def));
                }
            }
        }
        let module = Rc::new(module);
        self.std_loaded.insert(key, Rc::clone(&module));
        Ok(Value::Module(module))
    }

    /// モジュールの表示名。本体のあるディレクトリからの相対パスにする
    pub(super) fn display_path(&self, full: &Path) -> Rc<str> {
        let path = full.strip_prefix(&self.root_dir).unwrap_or(full);
        path.to_string_lossy().replace('\\', "/").into()
    }
}
