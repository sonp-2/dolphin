//! `for` の命令

use super::*;

impl Interpreter<'_> {
    /// 回す対象を、配列と位置（0）の 2 つにそろえてスタックに積む
    pub(super) fn for_prep(&mut self, span: Span) -> RResult<()> {
        match self.pop() {
            // 回し始めた時点の要素を使う
            Value::Array(items) => {
                let snapshot = items.borrow().clone();
                self.stack
                    .push(Value::Array(Rc::new(RefCell::new(snapshot))));
                self.stack.push(Value::Num(0.0));
            }
            // マップは {キー, 値} の組、文字列は 1 文字（コードポイント）ずつの配列にそろえる
            Value::Map(map) => {
                let pairs: Vec<Value> = map
                    .borrow()
                    .iter()
                    .map(|(k, v)| gc::new_array(vec![Value::Str(Rc::clone(k)), v.clone()]))
                    .collect();
                self.stack.push(Value::Array(Rc::new(RefCell::new(pairs))));
                self.stack.push(Value::Num(0.0));
            }
            Value::Str(text) => {
                let chars: Vec<Value> = text
                    .chars()
                    .map(|c| Value::Str(Rc::from(c.to_string())))
                    .collect();
                self.stack.push(Value::Array(Rc::new(RefCell::new(chars))));
                self.stack.push(Value::Num(0.0));
            }
            other => {
                return Err(RuntimeError::new(
                    span,
                    format!(
                        "for で回せるのは配列・マップ・文字列です（{}でした）",
                        other.type_name()
                    ),
                ));
            }
        }
        Ok(())
    }

    /// 次の要素をスロットに入れる。なければ `end` へ飛ぶ
    #[inline]
    pub(super) fn for_next_local(&mut self, end: u32, slot: u32, ip: &mut usize, base: usize) {
        let len = self.stack.len();
        let (Value::Num(position), Value::Array(items)) =
            (&self.stack[len - 1], &self.stack[len - 2])
        else {
            unreachable!("ForPrep が積んだ値");
        };
        let i = *position as usize;
        let item = items.borrow().get(i).cloned();
        match item {
            None => *ip = end as usize,
            Some(item) => {
                self.stack[len - 1] = Value::Num((i + 1) as f64);
                gc::maybe_collect();
                self.stack[base + slot as usize] = item;
            }
        }
    }

    /// 次の要素を、新しいスコープのループ変数に入れる。なければ `end` へ飛ぶ
    pub(super) fn for_next(&mut self, end: u32, var: &Rc<str>, ip: &mut usize) {
        let len = self.stack.len();
        let (Value::Num(position), Value::Array(items)) =
            (&self.stack[len - 1], &self.stack[len - 2])
        else {
            unreachable!("ForPrep が積んだ値");
        };
        let i = *position as usize;
        let item = items.borrow().get(i).cloned();
        match item {
            None => *ip = end as usize,
            Some(item) => {
                self.stack[len - 1] = Value::Num((i + 1) as f64);
                gc::maybe_collect();
                // ループ変数と本体は、繰り返しごとに新しい同じスコープ
                let inner = Env::new(Some(Rc::clone(&self.env)));
                inner.declare(var, item);
                let outer = mem::replace(&mut self.env, inner);
                self.vm_frames
                    .last_mut()
                    .expect("枠がある")
                    .scopes
                    .push(outer);
            }
        }
    }
}
