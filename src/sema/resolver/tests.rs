//! 検査の単体テスト

use super::*;
use crate::syntax::{lexer::tokenize, parser::parse};

fn check(src: &str) -> Result<(), String> {
    let program = parse(&tokenize(src).unwrap()).unwrap();
    resolve(
        &program,
        |name| (name == "log").then_some(Callee::Native),
        |name| name == "prev",
    )
    .map_err(|e| e.message)
}

#[test]
fn valid_program() {
    let src = "\
log[add[1, 2]]
add[@a, @b] (
    @sum := 0
    if @a > 0 (
        @sum = @a + @b
    )
    # @sum
)";
    assert_eq!(check(src), Ok(()));
}

#[test]
fn defer_is_checked_like_a_closure() {
    assert_eq!(
        check(
            "f[] (
    @a := 1
    defer log[@a]
)"
        ),
        Ok(())
    );
    assert_eq!(
        check(
            "f[] (
    defer log[@zzz]
)"
        ),
        Err("未定義の変数 `@zzz`".into())
    );
    assert!(
        check(
            "async f[] (
    defer log[await g[]]
)
async g[] ( # 1 )"
        )
        .unwrap_err()
        .contains("await")
    );
}

#[test]
fn undefined_function() {
    assert_eq!(check("foo[1]"), Err("未定義の関数 `foo`".into()));
}

#[test]
fn wrong_arity() {
    assert_eq!(
        check("f[@a] (\n)\nf[1, 2]"),
        Err("`f` の引数は 1 個です（2 個渡されました）".into())
    );
}

#[test]
fn duplicate_function() {
    assert_eq!(
        check("f[] (\n)\nf[] (\n)"),
        Err("関数 `f` はすでに定義されています".into())
    );
}

#[test]
fn shadowing_native() {
    assert_eq!(check("log[@x] (\n)\nlog[1]"), Ok(()));
    assert_eq!(
        check("import[@x] (\n)"),
        Err("`import` と同じ名前の関数は定義できません".into())
    );
}

#[test]
fn nested_function() {
    assert_eq!(
        check("if true (\n    f[] (\n    )\n)"),
        Err("関数はトップレベルでのみ定義できます".into())
    );
}

#[test]
fn undeclared_variable() {
    assert_eq!(check("log[@x]"), Err("未定義の変数 `@x`".into()));
    assert_eq!(
        check("f[@a] (\n    # @b\n)"),
        Err("未定義の変数 `@b`".into())
    );
    // 文字列の中の変数も検査する
    assert_eq!(check("log[\"@x\"]"), Err("未定義の変数 `@x`".into()));
    assert_eq!(
        check("f[@a] (\n    log[\"@a @b\"]\n)"),
        Err("未定義の変数 `@b`".into())
    );
}

#[test]
fn assign_needs_declaration() {
    assert_eq!(
        check("@x = 1"),
        Err("宣言されていない変数 `@x` に代入しています（`@x := ...` で宣言してください）".into())
    );
    assert_eq!(check("@x := 1\n@x = 2"), Ok(()));
    // 添字つきの代入は、変数を読む
    assert_eq!(check("@a{0} = 1"), Err("未定義の変数 `@a`".into()));
}

#[test]
fn declare_before_use_at_top_level() {
    // トップレベルの文は書かれた順に検査する
    assert_eq!(check("log[@x]\n@x := 1"), Err("未定義の変数 `@x`".into()));
    // 自分自身の右辺は、宣言する前の変数を指す
    assert_eq!(check("@x := @x + 1"), Err("未定義の変数 `@x`".into()));
}

#[test]
fn functions_see_top_level_variables() {
    // 関数はどこに書いても、トップレベルの変数が見える
    assert_eq!(check("inc[] (\n    @n = @n + 1\n)\n@n := 0\ninc[]"), Ok(()));
    // ブロックの中で宣言した変数は、関数からは見えない
    assert_eq!(
        check("if true (\n    @n := 0\n)\nf[] (\n    # @n\n)"),
        Err("未定義の変数 `@n`".into())
    );
}

#[test]
fn variables_from_previous_input() {
    assert_eq!(check("log[@prev]\n@prev = 1"), Ok(()));
}

#[test]
fn block_scope() {
    // ブロックの中で宣言した変数は、ブロックの外では使えない
    assert_eq!(
        check("if true (\n    @x := 1\n)\nlog[@x]"),
        Err("未定義の変数 `@x`".into())
    );
    // 外側の変数は内側から読み書きできる
    assert_eq!(
        check("@x := 1\nwhile true (\n    @x = @x + 1\n    break\n)"),
        Ok(())
    );
}

#[test]
fn redeclare() {
    assert_eq!(
        check("@x := 1\n@x := 2"),
        Err("変数 `@x` はすでに宣言されています".into())
    );
    // 内側のスコープでは、外側の変数を隠せる
    assert_eq!(check("@x := 1\nif true (\n    @x := 2\n)"), Ok(()));
    // 引数と同じスコープなので、宣言し直せない
    assert_eq!(
        check("f[@a] (\n    @a := 1\n)"),
        Err("変数 `@a` はすでに宣言されています".into())
    );
}

#[test]
fn break_outside_loop() {
    assert_eq!(
        check("break"),
        Err("`break` は while か for の中でのみ使えます".into())
    );
    assert_eq!(
        check("if true (\n    continue\n)"),
        Err("`continue` は while か for の中でのみ使えます".into())
    );
    // 関数は while の外にあるので、関数の中の break は外側のループを抜けられない
    assert_eq!(
        check("f[] (\n    break\n)"),
        Err("`break` は while か for の中でのみ使えます".into())
    );
    assert_eq!(
        check("while true (\n    if true (\n        break\n    )\n)"),
        Ok(())
    );
}

#[test]
fn return_outside_function() {
    assert_eq!(check("# 1"), Err("`#` は関数の中でのみ使えます".into()));
}

#[test]
fn function_values() {
    assert_eq!(
        check(
            "f[] (
)
@g := f
log[@g[]]"
        ),
        Ok(())
    );
    assert_eq!(check("@g := log"), Ok(()));
    assert_eq!(check("@g := nothing"), Err("未定義の関数 `nothing`".into()));
}

#[test]
fn lambda_captures_enclosing_scopes() {
    // 外側の関数の変数が見える
    assert_eq!(
        check(
            "f[@a] (
    # [@b] ( # @a + @b )
)"
        ),
        Ok(())
    );
    // 無名関数の変数は外から見えない
    assert_eq!(
        check(
            "@f := [@b] ( @c := 1 )
log[@c]"
        ),
        Err("未定義の変数 `@c`".into())
    );
    // 引数と同じスコープ
    assert_eq!(
        check("@f := [@b] ( @b := 1 )"),
        Err("変数 `@b` はすでに宣言されています".into())
    );
    assert_eq!(
        check("@f := [@b, @b] ( )"),
        Err("引数 `@b` が重複しています".into())
    );
    // 無名関数の中の # は使える。break は外側の while に効かない
    assert_eq!(check("@f := [] ( # 1 )"), Ok(()));
    assert_eq!(
        check(
            "while true (
    @f := [] ( break )
)"
        ),
        Err("`break` は while か for の中でのみ使えます".into())
    );
    // 無名関数の中から、あとで宣言されるトップレベルの変数も見える
    assert_eq!(
        check(
            "@f := [] ( # @later )
@later := 1"
        ),
        Ok(())
    );
}

#[test]
fn for_scope() {
    assert_eq!(check("for @x in {1, 2} (\n    log[@x]\n)"), Ok(()));
    // ループ変数は、ループの外では使えない
    assert_eq!(
        check("for @x in {1} (\n)\nlog[@x]"),
        Err("未定義の変数 `@x`".into())
    );
    // 回す式は、ループ変数より外側（自分自身は見えない）
    assert_eq!(check("for @x in @x (\n)"), Err("未定義の変数 `@x`".into()));
    // 本体で同じ名前を宣言し直せない。break / continue は使える
    assert_eq!(
        check("for @x in {1} (\n    @x := 2\n)"),
        Err("変数 `@x` はすでに宣言されています".into())
    );
    assert_eq!(check("for @x in {1} (\n    break\n)"), Ok(()));
}

#[test]
fn duplicate_param() {
    assert_eq!(
        check("f[@a, @a] (\n)"),
        Err("引数 `@a` が重複しています".into())
    );
}

#[test]
fn match_scopes_and_bindings() {
    // パターンの変数は、ガードと本体で見える。腕の外では見えない
    assert_eq!(
        check("match {1} (\n    {@a} if @a > 0 => log[@a]\n    _ => log[1]\n)"),
        Ok(())
    );
    assert_eq!(
        check("match {1} (\n    {@a} => log[1]\n)\nlog[@a]"),
        Err("未定義の変数 `@a`".into())
    );
    // ほかの腕の変数は見えない
    assert_eq!(
        check("match {1} (\n    {@a} => log[1]\n    _ => log[@a]\n)"),
        Err("未定義の変数 `@a`".into())
    );
    // 対象の式は、腕の変数の外側
    assert_eq!(
        check("match @a (\n    @a => log[@a]\n)"),
        Err("未定義の変数 `@a`".into())
    );
    // 本体のブロックは、パターンの変数と同じスコープ
    assert_eq!(
        check("match 1 (\n    @a => (\n        @a := 2\n    )\n)"),
        Err("変数 `@a` はすでに宣言されています".into())
    );
    assert_eq!(
        check("match 1 (\n    @a => (\n        @b := @a\n        log[@b]\n    )\n)"),
        Ok(())
    );
    // 外側の同名の変数は隠れるだけ
    assert_eq!(
        check("@a := 1\nmatch 2 (\n    @a => log[@a]\n)\nlog[@a]"),
        Ok(())
    );
}

#[test]
fn match_expression_scopes() {
    assert_eq!(
        check("@r := match {1} (\n    {@a} if @a > 0 => @a + 1\n    _ => 0\n)\nlog[@r]"),
        Ok(())
    );
    assert_eq!(
        check("@r := match 1 (\n    @a => @b\n)"),
        Err("未定義の変数 `@b`".into())
    );
    assert_eq!(
        check("@r := match 1 (\n    @a => 1\n)\nlog[@a]"),
        Err("未定義の変数 `@a`".into())
    );
}

#[test]
fn match_pattern_errors() {
    assert_eq!(
        check("match 1 (\n    {@a, @a} => 1\n)"),
        Err("変数 `@a` はすでに宣言されています".into())
    );
    assert_eq!(
        check("match 1 (\n    {@a} | {@b} => 1\n)"),
        Err("or パターンの選択肢で束縛する変数が違います".into())
    );
    assert_eq!(
        check("match 1 (\n    1 | @a => 1\n)"),
        Err("or パターンの選択肢で束縛する変数が違います".into())
    );
    // 同じ変数を束縛する or は書ける（順序は問わない）
    assert_eq!(
        check("match {1, 2} (\n    {@a, @b} | {@b, @a} => log[@a, @b]\n)"),
        Ok(())
    );
    assert_eq!(check("match 1 (\n    1 | 2 | _ => log[1]\n)"), Ok(()));
}

#[test]
fn match_record_arity() {
    let def = "record Point[@x, @y]\n";
    assert_eq!(
        check(&format!("{def}match 1 (\n    Point[@a, @b] => log[@a]\n)")),
        Ok(())
    );
    assert_eq!(
        check(&format!("{def}match 1 (\n    Point[@a] => log[@a]\n)")),
        Err("レコード `Point` のフィールドは 2 個です（パターンは 1 個）".into())
    );
    // 定義がない名前は、実行時に合わないだけ
    assert_eq!(check("match 1 (\n    Other[@a] => log[@a]\n)"), Ok(()));
}

#[test]
fn match_with_break_return_and_guards() {
    assert_eq!(
        check("while true (\n    match 1 (\n        1 => break\n        _ => continue\n    )\n)"),
        Ok(())
    );
    assert_eq!(
        check("match 1 (\n    1 => break\n)"),
        Err("`break` は while か for の中でのみ使えます".into())
    );
    assert_eq!(
        check("f[@v] (\n    match @v (\n        @x if @x > 1 => # @x\n        _ => # 0\n    )\n)"),
        Ok(())
    );
    assert_eq!(
        check("match 1 (\n    1 => # 2\n)"),
        Err("`#` は関数の中でのみ使えます".into())
    );
    // ガードの中の未定義の変数
    assert_eq!(
        check("match 1 (\n    @a if @z => 1\n)"),
        Err("未定義の変数 `@z`".into())
    );
}
