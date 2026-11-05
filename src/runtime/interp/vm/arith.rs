//! 二項演算

use super::*;

/// 整数どうし・数値どうしの、エラーにならない二項演算だけを素早く計算する。
/// それ以外（エラーになる場合を含む）は `None` を返し、`binary_values` に任せる
#[inline]
pub(super) fn fast_binary(op: BinaryOp, l: &Value, r: &Value) -> Option<Value> {
    use BinaryOp as B;

    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Some(match op {
            B::Add => Value::Int(a.checked_add(*b)?),
            B::Sub => Value::Int(a.checked_sub(*b)?),
            B::Mul => Value::Int(a.checked_mul(*b)?),
            B::Eq => Value::Bool(a == b),
            B::NotEq => Value::Bool(a != b),
            B::Lt | B::LtEq | B::Gt | B::GtEq => Value::Bool(compare(op, Some(a.cmp(b)))),
            _ => return None,
        }),
        (Value::Num(a), Value::Num(b)) => Some(match op {
            B::Add => Value::Num(a + b),
            B::Sub => Value::Num(a - b),
            B::Mul => Value::Num(a * b),
            B::Lt | B::LtEq | B::Gt | B::GtEq => Value::Bool(compare(op, a.partial_cmp(b))),
            _ => return None,
        }),
        _ => None,
    }
}

/// 二項演算（`&&` と `||` は短絡するので、ここには来ない）
pub(super) fn binary_values(op: BinaryOp, l: &Value, r: &Value, span: Span) -> RResult<Value> {
    use BinaryOp as B;

    let value = match (op, l, r) {
        (B::Eq, _, _) => Value::Bool(l.equals(r)),
        (B::NotEq, _, _) => Value::Bool(!l.equals(r)),
        (B::Add, Value::Str(a), Value::Str(b)) => Value::Str(format!("{a}{b}").into()),
        (B::Div | B::Rem, Value::Num(_), Value::Num(b)) if *b == 0.0 => {
            return Err(RuntimeError::new(span, "0 で割ることはできません"));
        }
        (B::Div | B::Rem, Value::Int(_), Value::Int(0)) => {
            return Err(RuntimeError::new(span, "0 で割ることはできません"));
        }
        (_, Value::Int(a), Value::Int(b)) => match op {
            B::Add | B::Sub | B::Mul | B::Div | B::Rem => {
                let result = match op {
                    B::Add => a.checked_add(*b),
                    B::Sub => a.checked_sub(*b),
                    B::Mul => a.checked_mul(*b),
                    B::Div => a.checked_div(*b),
                    _ => a.checked_rem(*b),
                };
                Value::Int(result.ok_or_else(|| int_overflow(span))?)
            }
            _ => Value::Bool(compare(op, Some(a.cmp(b)))),
        },
        (_, Value::Num(a), Value::Num(b)) => match op {
            B::Add => Value::Num(a + b),
            B::Sub => Value::Num(a - b),
            B::Mul => Value::Num(a * b),
            B::Div => Value::Num(a / b),
            B::Rem => Value::Num(a % b),
            _ => Value::Bool(compare(op, a.partial_cmp(b))),
        },
        (B::Lt | B::LtEq | B::Gt | B::GtEq, Value::Str(a), Value::Str(b)) => {
            Value::Bool(compare(op, Some(a.cmp(b))))
        }
        (_, Value::Int(_), Value::Num(_)) | (_, Value::Num(_), Value::Int(_)) => {
            return Err(RuntimeError::new(
                span,
                format!(
                    "整数と数値に `{op}` は使えません（`to_int` か `to_num` でそろえてください）"
                ),
            ));
        }
        _ => {
            return Err(RuntimeError::new(
                span,
                format!("{}と{}に `{op}` は使えません", l.type_name(), r.type_name()),
            ));
        }
    };
    Ok(value)
}
