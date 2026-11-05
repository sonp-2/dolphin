//! 配列・マップ・文字列・クロージャを作る命令

use super::*;

impl Interpreter<'_> {
    pub(super) fn array_append(&mut self) {
        let value = self.pop();
        if let Some(Value::Array(items)) = self.stack.last() {
            items.borrow_mut().push(value);
        }
    }

    pub(super) fn array_extend(&mut self, span: Span) -> RResult<()> {
        match self.pop() {
            Value::Array(source) => {
                if let Some(Value::Array(items)) = self.stack.last() {
                    let more = source.borrow().clone();
                    items.borrow_mut().extend(more);
                }
            }
            other => {
                return Err(RuntimeError::new(
                    span,
                    format!(
                        "`...` で展開できるのは配列だけです（{}でした）",
                        other.type_name()
                    ),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn check_key(&mut self, span: Span) -> RResult<()> {
        let top = self.stack.last().expect("スタックに値がある");
        to_key(top, span)?;
        Ok(())
    }

    pub(super) fn make_map(&mut self, n: usize) {
        let pairs = self.split_top(n * 2);
        let mut map = Map::default();
        let mut it = pairs.into_iter();
        while let (Some(key), Some(value)) = (it.next(), it.next()) {
            let Value::Str(key) = key else {
                unreachable!("CheckKey で文字列だと確かめた");
            };
            map.insert(key, value);
        }
        self.stack.push(gc::new_map(map));
    }

    pub(super) fn tag_record(&mut self, name: &Rc<str>) {
        if let Some(Value::Map(map)) = self.stack.last() {
            map.borrow_mut().tag = Some(Rc::clone(name));
        }
    }

    pub(super) fn concat(&mut self, n: usize) {
        let parts = self.split_top(n);
        let mut s = String::new();
        for part in parts {
            if let Value::Str(text) = part {
                s.push_str(&text);
            }
        }
        self.stack.push(Value::Str(s.into()));
    }

    pub(super) fn reject_module_top(&mut self, span: Span) -> RResult<()> {
        let top = self.stack.last().expect("スタックに値がある");
        reject_module(top, span)?;
        Ok(())
    }

    pub(super) fn make_closure(&mut self, chunk: &Rc<Chunk>, i: usize) {
        let def = Rc::clone(&chunk.defs[i]);
        self.stack.push(Value::Func(Closure::register(Closure {
            def,
            env: Rc::clone(&self.env),
            module: Rc::clone(&self.current),
        })));
    }
}
