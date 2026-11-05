//! 数学: abs, floor, ceil, round, sqrt, pow, min, max,
//! trunc, sign, clamp, num_fixed。`std:math`（定数・三角関数・対数。design.md 5.12）

use std::cell::Cell;
use std::time::{SystemTime, UNIX_EPOCH};

use super::deprecated::legacy;
use super::{array_arg, expect_args, int_arg, num_arg, str_arg, type_error};
use crate::runtime::interp::{Const, Interpreter};
use crate::runtime::value::Value;

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register("abs", abs);
    interp.register("floor", |_, args| unary("floor", args, f64::floor));
    interp.register("ceil", |_, args| unary("ceil", args, f64::ceil));
    // 0.5 は 0 から遠いほうへ丸める（2.5 → 3、-2.5 → -3）
    interp.register("round", |_, args| unary("round", args, f64::round));
    interp.register("sqrt", sqrt);
    interp.register("pow", pow);
    interp.register("min", |_, args| fold("min", args, f64::min, i64::min));
    interp.register("max", |_, args| fold("max", args, f64::max, i64::max));
    interp.register("random", legacy!("random", super::random::float));
    interp.register("random_int", legacy!("random_int", super::random::int));
    interp.register("trunc", |_, args| unary("trunc", args, f64::trunc));
    interp.register("sign", sign_value);
    interp.register("clamp", clamp);
    interp.register("num_fixed", num_fixed);
    interp.register("num_group", num_group);
    register_std_math(interp);
}

/// `import["std:math"]`。変数の `pi` `e` `inf` `nan` と、三角関数・対数など
fn register_std_math(interp: &mut Interpreter<'_>) {
    use std::f64::consts;
    interp.register_module_constants(
        "std:math",
        &[
            ("pi", Const::Num(consts::PI)),
            ("e", Const::Num(consts::E)),
            ("inf", Const::Num(f64::INFINITY)),
            ("nan", Const::Num(f64::NAN)),
        ],
    );
    interp.register_module(
        "std:math",
        &[
            ("math.sin", |_, args| unary("math.sin", args, f64::sin)),
            ("math.cos", |_, args| unary("math.cos", args, f64::cos)),
            ("math.tan", |_, args| unary("math.tan", args, f64::tan)),
            ("math.asin", |_, args| unary("math.asin", args, f64::asin)),
            ("math.acos", |_, args| unary("math.acos", args, f64::acos)),
            ("math.atan", |_, args| unary("math.atan", args, f64::atan)),
            ("math.atan2", |_, args| {
                binary("math.atan2", args, f64::atan2)
            }),
            ("math.exp", |_, args| unary("math.exp", args, f64::exp)),
            ("math.ln", |_, args| unary("math.ln", args, f64::ln)),
            ("math.log10", |_, args| {
                unary("math.log10", args, f64::log10)
            }),
            ("math.log2", |_, args| unary("math.log2", args, f64::log2)),
            ("math.hypot", |_, args| {
                binary("math.hypot", args, f64::hypot)
            }),
            ("math.mean", mean),
            ("math.median", median),
            ("math.stdev", stdev),
            ("math.is_nan", |_, args| {
                expect_args("math.is_nan", args, 1)?;
                Ok(Value::Bool(num_arg("math.is_nan", args, 0)?.is_nan()))
            }),
            ("math.is_finite", |_, args| {
                expect_args("math.is_finite", args, 1)?;
                Ok(Value::Bool(num_arg("math.is_finite", args, 0)?.is_finite()))
            }),
        ],
    );
}

/// 引数 2 つの数値関数の共通処理
fn binary(name: &str, args: &[Value], f: fn(f64, f64) -> f64) -> Result<Value, String> {
    expect_args(name, args, 2)?;
    Ok(Value::Num(f(
        num_arg(name, args, 0)?,
        num_arg(name, args, 1)?,
    )))
}

/// -1 / 0 / 1。NaN は NaN
fn sign(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        x
    } else {
        x.signum()
    }
}

/// `x` を `lo` 以上 `hi` 以下に収める
fn clamp(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("clamp", args, 3)?;
    let (x, lo, hi) = (
        num_arg("clamp", args, 0)?,
        num_arg("clamp", args, 1)?,
        num_arg("clamp", args, 2)?,
    );
    if lo.is_nan() || hi.is_nan() || lo > hi {
        return Err(format!(
            "clamp の範囲 {lo}〜{hi} は使えません（下限 ≦ 上限）"
        ));
    }
    Ok(Value::Num(x.clamp(lo, hi)))
}

/// 小数点以下 `digits` 桁（0〜20）の文字列にする。ちょうど半分は 0 から遠いほうへ丸める
fn num_fixed(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("num_fixed", args, 2)?;
    let n = num_arg("num_fixed", args, 0)?;
    let digits = int_arg("num_fixed", args, 1)?;
    if !n.is_finite() {
        return Err(format!("num_fixed の 1 番目の引数は有限の数値です（{n}）"));
    }
    if !(0..=20).contains(&digits) {
        return Err(format!("num_fixed の桁数は 0〜20 です（{digits}）"));
    }
    let d = digits as usize;
    // Rust の書式は、ちょうど半分を偶数側に丸める。ちょうど半分かを、さらに 25 桁見て調べ、
    // そうなら 1 ulp だけ 0 から遠ざけて、遠いほうへ丸まるようにする
    let probe = format!("{:.*}", d + 25, n.abs());
    let tail = &probe[probe.len() - 25..];
    let is_tie = tail.starts_with('5') && tail[1..].bytes().all(|b| b == b'0');
    let n = if is_tie {
        f64::from_bits(n.to_bits() + 1)
    } else {
        n
    };
    let mut out = format!("{n:.d$}");
    // 丸めて 0 になった負の数は、`-0.00` ではなく `0.00`
    if out.starts_with('-') && out[1..].bytes().all(|b| b == b'0' || b == b'.') {
        out.remove(0);
    }
    Ok(Value::Str(out.into()))
}

// ---- 統計（design.md 5.50） ----

/// 数値の配列を読む。数値以外の要素は実行時エラー
fn numbers_arg(name: &str, args: &[Value]) -> Result<Vec<f64>, String> {
    expect_args(name, args, 1)?;
    let items = array_arg(name, args, 0)?.borrow();
    items
        .iter()
        .enumerate()
        .map(|(i, item)| match item {
            Value::Num(n) => Ok(*n),
            other => Err(format!(
                "{name} の {} 番目の要素は数値です（{}が渡されました）",
                i + 1,
                other.type_name()
            )),
        })
        .collect()
}

fn mean_of(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn median_of(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    }
}

/// 標本標準偏差（n - 1 で割る）。2 個以上
fn stdev_of(values: &[f64]) -> f64 {
    let mean = mean_of(values);
    let sum: f64 = values.iter().map(|v| (v - mean) * (v - mean)).sum();
    (sum / (values.len() - 1) as f64).sqrt()
}

fn mean(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let values = numbers_arg("math.mean", args)?;
    if values.is_empty() {
        return Ok(Value::error(
            "range",
            "math.mean: 空の配列の平均は決まりません",
        ));
    }
    Ok(Value::Num(mean_of(&values)))
}

fn median(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let values = numbers_arg("math.median", args)?;
    if values.is_empty() {
        return Ok(Value::error(
            "range",
            "math.median: 空の配列の中央値は決まりません",
        ));
    }
    Ok(Value::Num(median_of(&values)))
}

fn stdev(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let values = numbers_arg("math.stdev", args)?;
    if values.len() < 2 {
        return Ok(Value::error(
            "range",
            "math.stdev: 標準偏差には要素が 2 個以上要ります",
        ));
    }
    Ok(Value::Num(stdev_of(&values)))
}

/// 整数部を 3 桁ごとに `sep` で区切る
fn group_digits(text: &str, sep: &str) -> String {
    // 指数表記（`1e21` など）はそのまま
    if text.contains(['e', 'E']) {
        return text.to_string();
    }
    let (sign, rest) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text),
    };
    let (int_part, frac) = match rest.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (rest, None),
    };
    let mut out = String::from(sign);
    let len = int_part.len();
    for (i, ch) in int_part.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push_str(sep);
        }
        out.push(ch);
    }
    if let Some(frac) = frac {
        out.push('.');
        out.push_str(frac);
    }
    out
}

/// 数値か整数を、3 桁区切りの文字列にする
fn num_group(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("num_group", args, 2)?;
    let text = match &args[0] {
        Value::Num(n) if n.is_finite() => n.to_string(),
        Value::Num(n) => return Err(format!("num_group の 1 番目の引数は有限の数値です（{n}）")),
        Value::Int(n) => n.to_string(),
        other => return Err(type_error("num_group", 0, "数値か整数", other)),
    };
    let sep = str_arg("num_group", args, 1)?;
    Ok(Value::Str(group_digits(&text, sep).into()))
}

// 整数（design.md 5.40）は整数のまま、数値は数値で返す
fn abs(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("abs", args, 1)?;
    match &args[0] {
        Value::Int(n) => n
            .checked_abs()
            .map(Value::Int)
            .ok_or_else(|| "abs の結果が整数の範囲を超えました".to_string()),
        _ => unary("abs", args, f64::abs),
    }
}

fn sign_value(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("sign", args, 1)?;
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(n.signum())),
        _ => unary("sign", args, sign),
    }
}

/// 引数 1 つの数値関数の共通処理
fn unary(name: &str, args: &[Value], f: fn(f64) -> f64) -> Result<Value, String> {
    expect_args(name, args, 1)?;
    Ok(Value::Num(f(num_arg(name, args, 0)?)))
}

/// min / max。引数は 1 個以上
/// 引数がすべて整数なら整数で返す（混在は実行時エラー。design.md 5.40）
fn fold(
    name: &str,
    args: &[Value],
    f: fn(f64, f64) -> f64,
    int_f: fn(i64, i64) -> i64,
) -> Result<Value, String> {
    if args.is_empty() {
        return Err(format!("{name} には 1 個以上の数値を渡してください"));
    }
    if let Value::Int(first) = args[0] {
        let mut acc = first;
        for (i, arg) in args.iter().enumerate().skip(1) {
            match arg {
                Value::Int(n) => acc = int_f(acc, *n),
                other => return Err(super::type_error(name, i, "整数", other)),
            }
        }
        return Ok(Value::Int(acc));
    }
    let mut acc = num_arg(name, args, 0)?;
    for i in 1..args.len() {
        acc = f(acc, num_arg(name, args, i)?);
    }
    Ok(Value::Num(acc))
}

fn sqrt(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("sqrt", args, 1)?;
    let n = num_arg("sqrt", args, 0)?;
    if n < 0.0 {
        return Err(format!("負の数 {n} の平方根は求められません"));
    }
    Ok(Value::Num(n.sqrt()))
}

fn pow(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("pow", args, 2)?;
    let base = num_arg("pow", args, 0)?;
    let exp = num_arg("pow", args, 1)?;
    Ok(Value::Num(base.powf(exp)))
}

// ---- 乱数 ----
//
// コア言語は外部クレートを使わない方針なので、xorshift64* を自前で持つ。
// 暗号用途には使えないが、ゲームやサンプルには十分

thread_local! {
    static RNG_STATE: Cell<u64> = Cell::new(seed());
}

fn seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    // 状態が 0 だと 0 しか出なくなるので、最下位ビットを立てる
    nanos | 1
}

/// 乱数列の種を決める。`None` なら現在時刻から（`std:random` の `seed`）
pub(super) fn reseed(seed_value: Option<i64>) {
    let state = match seed_value {
        None => seed(),
        Some(n) => {
            // splitmix64 で混ぜる。近い値の種でも、離れた状態になる
            let mut z = (n as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
    };
    RNG_STATE.with(|cell| cell.set(if state == 0 { 1 } else { state }));
}

pub(super) fn next_u64() -> u64 {
    RNG_STATE.with(|state| {
        let mut x = state.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        state.set(x);
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    })
}

/// 0 以上 1 未満
pub(super) fn next_f64() -> f64 {
    (next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

/// 0 以上 `n` 未満の整数（`n` > 0）。剰余の偏りが出ないよう、棄却法で選ぶ
pub(super) fn below(n: u64) -> u64 {
    let threshold = n.wrapping_neg() % n;
    loop {
        let r = next_u64();
        if r >= threshold {
            return r % n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statistics() {
        assert_eq!(mean_of(&[1.0, 2.0, 3.0, 4.0]), 2.5);
        assert_eq!(median_of(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median_of(&[4.0, 1.0, 3.0, 2.0]), 2.5);
        assert_eq!(median_of(&[7.0]), 7.0);
        // 標本標準偏差（n - 1）
        let sample = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        assert!((stdev_of(&sample) - 2.138_089_935_299_395).abs() < 1e-12);
        assert_eq!(stdev_of(&[1.0, 1.0, 1.0]), 0.0);
    }

    #[test]
    fn digit_grouping() {
        assert_eq!(group_digits("1234567", ","), "1,234,567");
        assert_eq!(group_digits("999", ","), "999");
        assert_eq!(group_digits("1000", ","), "1,000");
        assert_eq!(group_digits("-1234567.891", " "), "-1 234 567.891");
        assert_eq!(group_digits("-123", ","), "-123");
        assert_eq!(group_digits("0.5", ","), "0.5");
        assert_eq!(group_digits("1234", ""), "1234");
        assert_eq!(group_digits("1e21", ","), "1e21");
    }
}
