//! 配列（design.md 5.5、5.12）
//!
//! 配列をその場で書き換えるのは `arr_push` `arr_pop` `arr_insert` `arr_remove` `arr_clear` だけ。
//! ほかは新しい配列を返す。関数を受け取る関数は、呼び始めたときの要素を使う

use std::cmp::Ordering;

use super::{array_arg, expect_args, func_arg, int_arg, new_array};
use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register("arr_len", arr_len);
    interp.register("arr_push", arr_push);
    interp.register("arr_pop", arr_pop);
    interp.register("arr_map", arr_map);
    interp.register("arr_filter", arr_filter);
    interp.register("arr_each", arr_each);
    interp.register("range", range);
    interp.register("arr_slice", arr_slice);
    interp.register("arr_concat", arr_concat);
    interp.register("arr_reverse", arr_reverse);
    interp.register("arr_sort", arr_sort);
    interp.register("arr_sort_by", arr_sort_by);
    interp.register("arr_index_of", arr_index_of);
    interp.register("arr_contains", arr_contains);
    interp.register("arr_find", arr_find);
    interp.register("arr_find_index", arr_find_index);
    interp.register("arr_any", arr_any);
    interp.register("arr_all", arr_all);
    interp.register("arr_reduce", arr_reduce);
    interp.register("arr_insert", arr_insert);
    interp.register("arr_remove", arr_remove);
    interp.register("arr_clear", arr_clear);
    interp.register("arr_copy", arr_copy);
    interp.register("arr_sum", arr_sum);
    interp.register("arr_min", |i, a| min_max("arr_min", i, a, Ordering::Less));
    interp.register("arr_max", |i, a| min_max("arr_max", i, a, Ordering::Greater));
    interp.register("arr_enumerate", arr_enumerate);
    interp.register("arr_sort_key", arr_sort_key);
    interp.register("arr_take", |_, a| take_drop("arr_take", a, true));
    interp.register("arr_drop", |_, a| take_drop("arr_drop", a, false));
    interp.register("arr_first", |_, a| first_last("arr_first", a, true));
    interp.register("arr_last", |_, a| first_last("arr_last", a, false));
    interp.register("arr_partition", arr_partition);
    interp.register("arr_flat_map", arr_flat_map);
}

/// `range` が作る配列の要素数の上限
const RANGE_MAX: i64 = 10_000_000;

/// `a` 以上 `b` 未満の整数の配列
fn range(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("range", args, 2)?;
    let (a, b) = (int_arg("range", args, 0)?, int_arg("range", args, 1)?);
    if b.saturating_sub(a) > RANGE_MAX {
        return Err(format!("range の要素数が多すぎます（{RANGE_MAX} 個まで）"));
    }
    Ok(new_array((a..b).map(|n| Value::Num(n as f64)).collect()))
}

fn arr_len(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_len", args, 1)?;
    let items = array_arg("arr_len", args, 0)?;
    Ok(Value::Num(items.borrow().len() as f64))
}

/// 末尾に追加する。配列は参照渡しなので、呼び出し元の配列が変わる（design.md 7.1）
fn arr_push(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_push", args, 2)?;
    let items = array_arg("arr_push", args, 0)?;
    items.borrow_mut().push(args[1].clone());
    Ok(Value::Nil)
}

/// 末尾を取り除いて返す。空ならエラー
fn arr_pop(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_pop", args, 1)?;
    let items = array_arg("arr_pop", args, 0)?;
    items
        .borrow_mut()
        .pop()
        .ok_or_else(|| "空の配列からは取り出せません".to_string())
}

/// 各要素に `f` を適用した新しい配列。呼んでいる間に元の配列が書き換えられても、
/// 呼び始めたときの要素を使う
fn arr_map(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_map", args, 2)?;
    let items = array_arg("arr_map", args, 0)?.borrow().clone();
    let f = func_arg("arr_map", args, 1)?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(interp.call_value(f, vec![item])?);
    }
    Ok(new_array(out))
}

/// `f` が `true` を返した要素だけの新しい配列
fn arr_filter(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_filter", args, 2)?;
    let items = array_arg("arr_filter", args, 0)?.borrow().clone();
    let f = func_arg("arr_filter", args, 1)?;
    let mut out = Vec::new();
    for item in items {
        if truth("arr_filter", interp.call_value(f, vec![item.clone()])?)? {
            out.push(item);
        }
    }
    Ok(new_array(out))
}

/// 各要素に `f` を適用する。`f` の戻り値は捨てる
fn arr_each(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_each", args, 2)?;
    let items = array_arg("arr_each", args, 0)?.borrow().clone();
    let f = func_arg("arr_each", args, 1)?;
    for item in items {
        interp.call_value(f, vec![item])?;
    }
    Ok(Value::Nil)
}

/// 条件の関数の戻り値（真偽値）を取り出す
fn truth(name: &str, value: Value) -> Result<bool, String> {
    match value {
        Value::Bool(b) => Ok(b),
        other => Err(format!(
            "{name} の関数は真偽値を返す必要があります（{}でした）",
            other.type_name()
        )),
    }
}

/// `start` から `end` の手前までの新しい配列
fn arr_slice(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_slice", args, 3)?;
    let items = array_arg("arr_slice", args, 0)?.borrow();
    let start = int_arg("arr_slice", args, 1)?;
    let end = int_arg("arr_slice", args, 2)?;
    let len = items.len() as i64;
    if !(0 <= start && start <= end && end <= len) {
        return Err(format!(
            "arr_slice の範囲 {start}〜{end} は使えません（0 ≦ 開始 ≦ 終了 ≦ {len}）"
        ));
    }
    Ok(new_array(items[start as usize..end as usize].to_vec()))
}

fn arr_concat(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_concat", args, 2)?;
    let mut items = array_arg("arr_concat", args, 0)?.borrow().clone();
    items.extend(array_arg("arr_concat", args, 1)?.borrow().iter().cloned());
    Ok(new_array(items))
}

fn arr_reverse(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_reverse", args, 1)?;
    let mut items = array_arg("arr_reverse", args, 0)?.borrow().clone();
    items.reverse();
    Ok(new_array(items))
}

/// 昇順の新しい配列。要素はすべて数値か、すべて整数か、すべて文字列。安定ソート
fn arr_sort(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_sort", args, 1)?;
    let mut items = array_arg("arr_sort", args, 0)?.borrow().clone();
    let all_num = items.iter().all(|v| matches!(v, Value::Num(_)));
    let all_int = items.iter().all(|v| matches!(v, Value::Int(_)));
    let all_str = items.iter().all(|v| matches!(v, Value::Str(_)));
    if all_int {
        items.sort_by(|a, b| match (a, b) {
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            _ => Ordering::Equal,
        });
    } else if all_num {
        items.sort_by(|a, b| match (a, b) {
            (Value::Num(a), Value::Num(b)) => a.total_cmp(b),
            _ => Ordering::Equal,
        });
    } else if all_str {
        items.sort_by(|a, b| match (a, b) {
            (Value::Str(a), Value::Str(b)) => a.cmp(b),
            _ => Ordering::Equal,
        });
    } else {
        let first = items[0].type_name();
        let message = match items.iter().map(Value::type_name).find(|t| *t != first) {
            Some(other) => format!("{first}と{other}が混ざっています"),
            None => format!("{first}は並べ替えられません"),
        };
        return Err(format!(
            "arr_sort の要素は、すべて数値かすべて整数かすべて文字列である必要があります（{message}。`arr_sort_by` を使ってください）"
        ));
    }
    Ok(new_array(items))
}

/// `f[x, y]` が負なら `x` が先、正なら `y` が先、0 なら元の順。安定ソート
fn arr_sort_by(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_sort_by", args, 2)?;
    let items = array_arg("arr_sort_by", args, 0)?.borrow().clone();
    let f = func_arg("arr_sort_by", args, 1)?;
    let mut compare = |x: &Value, y: &Value| -> Result<Ordering, String> {
        match interp.call_value(f, vec![x.clone(), y.clone()])? {
            Value::Num(n) if n.is_nan() => Err("arr_sort_by の関数が NaN を返しました".into()),
            Value::Num(n) => Ok(n.partial_cmp(&0.0).unwrap_or(Ordering::Equal)),
            Value::Int(n) => Ok(n.cmp(&0)),
            other => Err(format!(
                "arr_sort_by の関数は数値か整数を返す必要があります（{}でした）",
                other.type_name()
            )),
        }
    };
    Ok(new_array(merge_sort(items, &mut compare)?))
}

/// 比較が失敗しうる安定ソート。比較関数が矛盾していても、パニックせず何かの順序を返す
/// （標準の `sort_by` は、矛盾した比較関数でパニックすることがある）
fn merge_sort(
    mut items: Vec<Value>,
    compare: &mut dyn FnMut(&Value, &Value) -> Result<Ordering, String>,
) -> Result<Vec<Value>, String> {
    if items.len() <= 1 {
        return Ok(items);
    }
    let right = items.split_off(items.len() / 2);
    let left = merge_sort(items, compare)?;
    let right = merge_sort(right, compare)?;
    let mut out = Vec::with_capacity(left.len() + right.len());
    let (mut left, mut right) = (left.into_iter().peekable(), right.into_iter().peekable());
    while let (Some(a), Some(b)) = (left.peek(), right.peek()) {
        // 等しいときは左を先にして、安定にする
        let take_right = compare(a, b)? == Ordering::Greater;
        out.extend(if take_right {
            right.next()
        } else {
            left.next()
        });
    }
    out.extend(left);
    out.extend(right);
    Ok(out)
}

/// `==` で等しい最初の位置。なければ -1
fn arr_index_of(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_index_of", args, 2)?;
    let items = array_arg("arr_index_of", args, 0)?.borrow();
    let pos = items.iter().position(|v| v.equals(&args[1]));
    Ok(Value::Num(pos.map_or(-1.0, |i| i as f64)))
}

fn arr_contains(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_contains", args, 2)?;
    let items = array_arg("arr_contains", args, 0)?.borrow();
    Ok(Value::Bool(items.iter().any(|v| v.equals(&args[1]))))
}

/// 条件の関数が `true` を返す最初の位置と要素。なければ None
fn find_first(
    name: &str,
    interp: &mut Interpreter<'_>,
    args: &[Value],
) -> Result<Option<(usize, Value)>, String> {
    expect_args(name, args, 2)?;
    let items = array_arg(name, args, 0)?.borrow().clone();
    let f = func_arg(name, args, 1)?;
    for (i, item) in items.into_iter().enumerate() {
        if truth(name, interp.call_value(f, vec![item.clone()])?)? {
            return Ok(Some((i, item)));
        }
    }
    Ok(None)
}

/// 条件の関数が `true` を返した最初の要素。なければ `nil`
fn arr_find(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    Ok(find_first("arr_find", interp, args)?.map_or(Value::Nil, |(_, item)| item))
}

fn arr_find_index(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let found = find_first("arr_find_index", interp, args)?;
    Ok(Value::Num(found.map_or(-1.0, |(i, _)| i as f64)))
}

/// どれかの要素で `f` が `true` か
fn arr_any(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    Ok(Value::Bool(find_first("arr_any", interp, args)?.is_some()))
}

/// すべての要素で `f` が `true` か（空なら `true`）
fn arr_all(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_all", args, 2)?;
    let items = array_arg("arr_all", args, 0)?.borrow().clone();
    let f = func_arg("arr_all", args, 1)?;
    for item in items {
        if !truth("arr_all", interp.call_value(f, vec![item])?)? {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

/// `f[acc, x]` を順に適用して畳み込む。空配列なら `init`
fn arr_reduce(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_reduce", args, 3)?;
    let items = array_arg("arr_reduce", args, 0)?.borrow().clone();
    let f = func_arg("arr_reduce", args, 2)?;
    let mut acc = args[1].clone();
    for item in items {
        acc = interp.call_value(f, vec![acc, item])?;
    }
    Ok(acc)
}

/// 位置 `i`（0 ≦ i ≦ 長さ）に挿入する
fn arr_insert(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_insert", args, 3)?;
    let items = array_arg("arr_insert", args, 0)?;
    let i = int_arg("arr_insert", args, 1)?;
    let len = items.borrow().len() as i64;
    if !(0 <= i && i <= len) {
        return Err(format!(
            "arr_insert の位置 {i} は使えません（0 ≦ 位置 ≦ {len}）"
        ));
    }
    items.borrow_mut().insert(i as usize, args[2].clone());
    Ok(Value::Nil)
}

/// 位置 `i` の要素を取り除いて返す
fn arr_remove(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_remove", args, 2)?;
    let items = array_arg("arr_remove", args, 0)?;
    let i = int_arg("arr_remove", args, 1)?;
    let len = items.borrow().len() as i64;
    if !(0 <= i && i < len) {
        return Err(format!(
            "arr_remove の位置 {i} は範囲外です（要素は {len} 個）"
        ));
    }
    Ok(items.borrow_mut().remove(i as usize))
}

fn arr_clear(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_clear", args, 1)?;
    array_arg("arr_clear", args, 0)?.borrow_mut().clear();
    Ok(Value::Nil)
}

/// 浅いコピー。要素の配列やマップは、元と同じものを指す
fn arr_copy(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_copy", args, 1)?;
    Ok(new_array(array_arg("arr_copy", args, 0)?.borrow().clone()))
}

/// 引数の個数が `min` 個以上 `max` 個以下か
fn expect_args_between(name: &str, args: &[Value], min: usize, max: usize) -> Result<(), String> {
    if (min..=max).contains(&args.len()) {
        Ok(())
    } else {
        Err(format!(
            "{name} の引数は {min} 個か {max} 個です（{} 個渡されました）",
            args.len()
        ))
    }
}

/// 要素と、その鍵（`f` があれば `f[x]` の結果、なければ要素そのもの）。
/// 呼び始めたときの要素を使う
fn keyed(
    name: &str,
    interp: &mut Interpreter<'_>,
    args: &[Value],
) -> Result<(Vec<Value>, Vec<Value>), String> {
    let items = array_arg(name, args, 0)?.borrow().clone();
    let keys = match args.get(1) {
        Some(_) => {
            let f = func_arg(name, args, 1)?;
            let mut keys = Vec::with_capacity(items.len());
            for item in &items {
                keys.push(interp.call_value(f, vec![item.clone()])?);
            }
            keys
        }
        None => items.clone(),
    };
    Ok((items, keys))
}

/// 数値の合計。要素（`f` があれば `f` の結果）がすべて数値なら数値、すべて整数なら整数。空は 0
fn arr_sum(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args_between("arr_sum", args, 1, 2)?;
    let (_, keys) = keyed("arr_sum", interp, args)?;
    if !keys.is_empty() && keys.iter().all(|k| matches!(k, Value::Int(_))) {
        let mut total: i64 = 0;
        for k in &keys {
            if let Value::Int(n) = k {
                total = total
                    .checked_add(*n)
                    .ok_or_else(|| "arr_sum の合計が整数の範囲を出ました".to_string())?;
            }
        }
        return Ok(Value::Int(total));
    }
    let mut total = 0.0;
    for k in &keys {
        match k {
            Value::Num(n) => total += n,
            other => {
                let first = keys[0].type_name();
                let message = if first != other.type_name() {
                    format!("{first}と{}が混ざっています", other.type_name())
                } else {
                    format!("{}は足せません", other.type_name())
                };
                return Err(format!(
                    "arr_sum の要素は、すべて数値かすべて整数である必要があります（{message}）"
                ));
            }
        }
    }
    Ok(Value::Num(total))
}

/// 鍵がすべて数値か、すべて整数か、すべて文字列かを確かめる
fn check_keys(name: &str, keys: &[Value]) -> Result<(), String> {
    let Some(first) = keys.first() else {
        return Ok(());
    };
    let ok = match first {
        Value::Num(_) => keys.iter().all(|k| matches!(k, Value::Num(_))),
        Value::Int(_) => keys.iter().all(|k| matches!(k, Value::Int(_))),
        Value::Str(_) => keys.iter().all(|k| matches!(k, Value::Str(_))),
        _ => false,
    };
    if ok {
        return Ok(());
    }
    let first_type = first.type_name();
    let message = match keys.iter().map(Value::type_name).find(|t| *t != first_type) {
        Some(other) => format!("{first_type}と{other}が混ざっています"),
        None => format!("{first_type}は比べられません"),
    };
    Err(format!(
        "{name} の比べる値は、すべて数値かすべて整数かすべて文字列である必要があります（{message}）"
    ))
}

/// `check_keys` を通った、同じ型の値の比較
fn cmp_keys(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Num(a), Value::Num(b)) => a.total_cmp(b),
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Str(a), Value::Str(b)) => a.cmp(b),
        _ => Ordering::Equal,
    }
}

/// 最小（`want` が `Less`）か最大（`Greater`）の要素。同じ値なら最初のもの。空なら `nil`
fn min_max(
    name: &str,
    interp: &mut Interpreter<'_>,
    args: &[Value],
    want: Ordering,
) -> Result<Value, String> {
    expect_args_between(name, args, 1, 2)?;
    let (items, keys) = keyed(name, interp, args)?;
    check_keys(name, &keys)?;
    let mut best = 0;
    for i in 1..keys.len() {
        if cmp_keys(&keys[i], &keys[best]) == want {
            best = i;
        }
    }
    Ok(items.into_iter().nth(best).unwrap_or(Value::Nil))
}

/// `{{0, x0}, {1, x1}, ...}`
fn arr_enumerate(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_enumerate", args, 1)?;
    let items = array_arg("arr_enumerate", args, 0)?.borrow().clone();
    Ok(new_array(
        items
            .into_iter()
            .enumerate()
            .map(|(i, x)| new_array(vec![Value::Num(i as f64), x]))
            .collect(),
    ))
}

/// `f[x]` を鍵にした昇順の新しい配列。鍵は要素ごとに 1 回だけ求める。安定ソート
fn arr_sort_key(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_sort_key", args, 2)?;
    let (items, keys) = keyed("arr_sort_key", interp, args)?;
    check_keys("arr_sort_key", &keys)?;
    let mut pairs: Vec<(Value, Value)> = keys.into_iter().zip(items).collect();
    pairs.sort_by(|a, b| cmp_keys(&a.0, &b.0));
    Ok(new_array(pairs.into_iter().map(|(_, x)| x).collect()))
}

/// 先頭から `n` 個（`take`）か、先頭の `n` 個を除いたもの。`n` が範囲外なら、
/// 0 個か全部に収める（エラーにしない）
fn take_drop(name: &str, args: &[Value], take: bool) -> Result<Value, String> {
    expect_args(name, args, 2)?;
    let items = array_arg(name, args, 0)?.borrow();
    let n = int_arg(name, args, 1)?.clamp(0, items.len() as i64) as usize;
    let part = if take { &items[..n] } else { &items[n..] };
    Ok(new_array(part.to_vec()))
}

/// 先頭か末尾の要素。空なら `default`
fn first_last(name: &str, args: &[Value], first: bool) -> Result<Value, String> {
    expect_args(name, args, 2)?;
    let items = array_arg(name, args, 0)?.borrow();
    let found = if first { items.first() } else { items.last() };
    Ok(found.cloned().unwrap_or_else(|| args[1].clone()))
}

/// `{f が true の要素, それ以外}`
fn arr_partition(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_partition", args, 2)?;
    let items = array_arg("arr_partition", args, 0)?.borrow().clone();
    let f = func_arg("arr_partition", args, 1)?;
    let (mut yes, mut no) = (Vec::new(), Vec::new());
    for item in items {
        if truth("arr_partition", interp.call_value(f, vec![item.clone()])?)? {
            yes.push(item);
        } else {
            no.push(item);
        }
    }
    Ok(new_array(vec![new_array(yes), new_array(no)]))
}

/// 各要素に `f`（配列を返す）を適用して、結果を 1 つにつなげた新しい配列
fn arr_flat_map(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("arr_flat_map", args, 2)?;
    let items = array_arg("arr_flat_map", args, 0)?.borrow().clone();
    let f = func_arg("arr_flat_map", args, 1)?;
    let mut out = Vec::new();
    for item in items {
        match interp.call_value(f, vec![item])? {
            Value::Array(part) => out.extend(part.borrow().iter().cloned()),
            other => {
                return Err(format!(
                    "arr_flat_map の関数は配列を返す必要があります（{}でした）",
                    other.type_name()
                ))
            }
        }
    }
    Ok(new_array(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nums(items: &[f64]) -> Vec<Value> {
        items.iter().map(|&n| Value::Num(n)).collect()
    }

    fn values(items: Vec<Value>) -> Vec<f64> {
        items
            .into_iter()
            .map(|v| match v {
                Value::Num(n) => n,
                _ => panic!("数値ではない"),
            })
            .collect()
    }

    #[test]
    fn merge_sort_is_stable() {
        // 整数部で比べ、小数部は元の順を表す
        let items = nums(&[2.1, 1.1, 2.2, 1.2, 0.1, 2.3]);
        let mut compare = |a: &Value, b: &Value| match (a, b) {
            (Value::Num(a), Value::Num(b)) => Ok(a.trunc().partial_cmp(&b.trunc()).unwrap()),
            _ => unreachable!(),
        };
        let sorted = merge_sort(items, &mut compare).unwrap();
        assert_eq!(values(sorted), vec![0.1, 1.1, 1.2, 2.1, 2.2, 2.3]);
    }

    #[test]
    fn merge_sort_survives_inconsistent_comparator() {
        // 矛盾した比較関数でもパニックせず、要素を失わない
        let items = nums(&[5.0, 3.0, 9.0, 1.0, 7.0, 2.0, 8.0, 4.0, 6.0, 0.0]);
        let mut flip = false;
        let mut compare = |_: &Value, _: &Value| {
            flip = !flip;
            Ok(if flip {
                Ordering::Less
            } else {
                Ordering::Greater
            })
        };
        let mut sorted = values(merge_sort(items, &mut compare).unwrap());
        sorted.sort_by(f64::total_cmp);
        assert_eq!(sorted, (0..10).map(f64::from).collect::<Vec<_>>());
    }

    #[test]
    fn merge_sort_propagates_errors() {
        let items = nums(&[3.0, 1.0, 2.0]);
        let mut compare = |_: &Value, _: &Value| Err::<Ordering, _>("失敗".to_string());
        assert_eq!(merge_sort(items, &mut compare).unwrap_err(), "失敗");
    }
}
