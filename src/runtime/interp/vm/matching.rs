//! `match` の命令（design.md 5.59）

use super::*;
use crate::runtime::bytecode::MatchInfo;
use crate::runtime::value::Repr;
use crate::syntax::ast::Literal;

/// エラーメッセージに入れる値の表示の長さの上限（文字数）
const SHOWN_VALUE_MAX: usize = 60;

impl Interpreter<'_> {
    /// スタックの値をパターンに当てる。合えば束縛する値を積み、合わなければ `fail` へ飛ぶ
    pub(super) fn match_pat(&mut self, info: &MatchInfo, fail: u32, ip: &mut usize) {
        let value = self.pop();
        let mut binds: Vec<Option<Value>> = vec![None; info.vars.len()];
        if matches_pattern(&info.pattern, &value, &info.vars, &mut binds) {
            self.stack.extend(
                binds
                    .into_iter()
                    .map(|v| v.expect("パターンの変数はすべて束縛される")),
            );
        } else {
            *ip = fail as usize;
        }
    }

    /// どの腕にも合わなかった
    pub(super) fn match_fail(&mut self, span: Span) -> RuntimeError {
        let value = self.pop();
        let shown = Repr(&value).to_string();
        let shown = if shown.chars().count() > SHOWN_VALUE_MAX {
            let head: String = shown.chars().take(SHOWN_VALUE_MAX).collect();
            format!("{head}…")
        } else {
            shown
        };
        RuntimeError::new(span, format!("match に合う腕がありません（値: {shown}）"))
    }
}

/// `value` が `pattern` に合うか。合えば、変数に入る値を `binds`（`vars` の順）に入れる
fn matches_pattern(
    pattern: &Pattern,
    value: &Value,
    vars: &[Rc<str>],
    binds: &mut [Option<Value>],
) -> bool {
    match pattern {
        Pattern::Wild => true,
        Pattern::Var(var) => {
            bind(vars, binds, &var.0, value.clone());
            true
        }
        Pattern::Lit(lit) => match (lit, value) {
            (Literal::Number(a), Value::Num(b)) => a == b,
            (Literal::Int(a), Value::Int(b)) => a == b,
            (Literal::Str(a), Value::Str(b)) => a.as_str() == &**b,
            (Literal::Bool(a), Value::Bool(b)) => a == b,
            (Literal::Nil, Value::Nil) => true,
            _ => false,
        },
        Pattern::Array { items, rest } => {
            let Value::Array(array) = value else {
                return false;
            };
            let array = array.borrow();
            let length_ok = if rest.is_some() {
                array.len() >= items.len()
            } else {
                array.len() == items.len()
            };
            if !length_ok
                || !items
                    .iter()
                    .zip(array.iter())
                    .all(|(item, element)| matches_pattern(item, element, vars, binds))
            {
                return false;
            }
            if let Some(rest) = rest {
                let remaining = gc::new_array(array[items.len()..].to_vec());
                bind(vars, binds, &rest.0, remaining);
            }
            true
        }
        Pattern::Map(entries) => {
            let Value::Map(map) = value else {
                return false;
            };
            let map = map.borrow();
            entries.iter().all(|(key, entry)| {
                map.get(key)
                    .is_some_and(|element| matches_pattern(entry, element, vars, binds))
            })
        }
        Pattern::Record { name, items } => {
            let Value::Map(map) = value else {
                return false;
            };
            let map = map.borrow();
            map.tag.as_deref() == Some(name.as_str())
                && map.len() == items.len()
                && items
                    .iter()
                    .zip(map.iter())
                    .all(|(item, (_, field))| matches_pattern(item, field, vars, binds))
        }
        Pattern::Or(alts) => alts
            .iter()
            .any(|alt| matches_pattern(alt, value, vars, binds)),
    }
}

fn bind(vars: &[Rc<str>], binds: &mut [Option<Value>], name: &str, value: Value) {
    let i = vars
        .iter()
        .position(|v| &**v == name)
        .expect("パターンの変数は vars にある");
    binds[i] = Some(value);
}
