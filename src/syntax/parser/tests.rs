//! パーサーの単体テスト

use super::*;
use crate::syntax::lexer::tokenize;

/// AST を S 式の文字列にする
fn ast(src: &str) -> String {
    parse(&tokenize(src).unwrap()).unwrap().to_string()
}

fn error(src: &str) -> Diagnostic {
    parse(&tokenize(src).unwrap()).unwrap_err()
}

#[test]
fn defer_statement() {
    assert_eq!(
        ast("f[] (
    defer log[1]
)"),
        "(def f () ((defer (log 1))))
"
    );
    assert!(
        error(
            "defer
"
        )
        .message
        .contains("式")
    );
}

#[test]
fn precedence() {
    assert_eq!(
        ast("@x = 1 + 2 * 3 - -@y"),
        "(= @x (- (+ 1 (* 2 3)) (- @y)))\n"
    );
    assert_eq!(
        ast("@ok = !@a || @b && @c >= 1"),
        "(= @ok (|| (! @a) (&& @b (>= @c 1))))\n"
    );
}

#[test]
fn grouping_and_block() {
    // 先頭の ( はグループ化、最後の ( はブロック
    assert_eq!(
        ast("if (@a + @b) * 2 > 0 (\n    log[\"big\"]\n)"),
        "(if (> (* (+ @a @b) 2) 0) ((log \"big\")))\n"
    );
}

#[test]
fn func_def_and_call() {
    assert_eq!(
        ast("add[@a, @b] (\n    # @a + @b\n)\nlog[add[1, 2]]\nf[] (\n    #\n)"),
        "(def add (@a @b) ((# (+ @a @b))))\n(log (add 1 2))\n(def f () ((#)))\n"
    );
}

#[test]
fn else_on_next_line_and_else_if() {
    let src = "\
if @a (
    log[1]
)
else if @b (
    log[2]
)
else (
    log[3]
)";
    assert_eq!(
        ast(src),
        "(if @a ((log 1)) ((if @b ((log 2)) ((log 3)))))\n"
    );
}

#[test]
fn if_without_else_keeps_newline() {
    assert_eq!(ast("if @a (\n)\n\n@b = 1"), "(if @a ())\n(= @b 1)\n");
}

#[test]
fn arrays_and_index() {
    assert_eq!(
        ast("@n := {1, 2}\n@n{0} = @n{1}\n@e := {}"),
        "(:= @n {1 2})\n(= ({} @n 0) ({} @n 1))\n(:= @e {})\n"
    );
    // 添字は続けて書ける。代入の左辺にも書ける
    assert_eq!(
        ast("@m{\"a\"}{0} = @m{\"b\"}{1}"),
        "(= ({} ({} @m \"a\") 0) ({} ({} @m \"b\") 1))\n"
    );
}

#[test]
fn module_member_and_call() {
    assert_eq!(
        ast("@x = @u.add[1, 2] + @u.version"),
        "(= @x (+ ((. @u add) 1 2) (. @u version)))\n"
    );
    // `.名前[...]` は関数呼び出し、添字は `{ }`
    assert_eq!(
        ast("log[@u.items{0}, @a.b.f[]]"),
        "(log ({} (. @u items) 0) ((. (. @a b) f)))\n"
    );
    // 数値の小数点とは区別される
    assert_eq!(ast("@x = 1.5"), "(= @x 1.5)\n");
}

#[test]
fn module_member_errors() {
    assert_eq!(
        error("@u.").message,
        "名前が必要です（ファイルの終わりがありました）"
    );
    // `@m.key = v` は `@m{"key"} = v`（モジュールかどうかは実行時に分かる）
    assert_eq!(ast("@u.count = 1"), "(= ({} @u \"count\") 1)\n");
    assert_eq!(
        ast("@u.items{0}.x = 1"),
        "(= ({} ({} ({} @u \"items\") 0) \"x\") 1)\n"
    );
    // 関数定義は名前空間つきでは書けない
    assert!(parse(&tokenize("@u.f[@a] (\n)").unwrap()).is_err());
}

#[test]
fn maps() {
    assert_eq!(
        ast("@m := {\"a\": 1, @k: {:}}\n@m{\"a\"} = {}"),
        "(:= @m {\"a\": 1 @k: {:}})\n(= ({} @m \"a\") {})\n"
    );
    // { } の中の改行は無視されるので複数行に書ける
    assert_eq!(
        ast("@m := {\n    \"x\": 1,\n    \"y\": 2\n}"),
        "(:= @m {\"x\": 1 \"y\": 2})\n"
    );
}

#[test]
fn mixed_array_and_map() {
    let e = error("@m := {\"a\": 1, 2}");
    assert_eq!(e.message, "`:`が必要です（`}`がありました）");
    let e = error("@a := {1, 2: 3}");
    assert_eq!(e.message, "`,` か `}`が必要です（`:`がありました）");
}

#[test]
fn multiline_args() {
    assert_eq!(ast("log[\n    1,\n    2\n]"), "(log 1 2)\n");
}

#[test]
fn string_interpolation() {
    assert_eq!(
        ast(r#"log["Hi, @name! \@x"]"#),
        "(log \"Hi, @name! \\@x\")\n"
    );
}

#[test]
fn two_statements_on_one_line() {
    let e = error("while true ( @a = 1 @b = 2 )");
    assert_eq!(e.message, "改行が必要です（`@b`がありました）");
    assert_eq!(e.span, Span { line: 1, col: 21 });
}

#[test]
fn missing_block() {
    let e = error("if @a\n");
    assert_eq!(e.message, "`(`が必要です（改行がありました）");
    assert_eq!(e.span, Span { line: 1, col: 6 });
}

#[test]
fn unclosed_block() {
    let e = error("while true (\n    @a = 1\n");
    assert_eq!(e.message, "`)`が必要です（ファイルの終わりがありました）");
}

#[test]
fn invalid_assign_target() {
    let e = error("1 = 2");
    assert_eq!(e.message, "代入できるのは変数か配列の要素だけです");
}

#[test]
fn non_variable_param() {
    let e = error("f[1] (\n)");
    assert_eq!(e.message, "変数（`@x`）が必要です（`1`がありました）");
}

#[test]
fn function_values() {
    // 関数名だけ = 関数値、無名関数、値の呼び出し
    assert_eq!(
        ast("@f := add
@g := [@a, @b] ( # @a + @b )
log[@f[1], @g[1, 2], make[][3]]"),
        "(:= @f (ref add))
(:= @g (fn (@a @b) ((# (+ @a @b)))))
(log (call @f 1) (call @g 1 2) (call (make) 3))
"
    );
    // 名前つきの呼び出しと関数定義は、これまでどおり
    assert_eq!(
        ast("f[@a] (
    # @a
)"),
        "(def f (@a) ((# @a)))
"
    );
    // 無名関数は複数行に書ける
    assert_eq!(
        ast("log[[@x] (
    @y := @x
    # @y
)]"),
        "(log (fn (@x) ((:= @y @x) (# @y))))
"
    );
    // 引数は変数だけ
    let e = error("@f := [1] ( )");
    assert_eq!(e.message, "変数（`@x`）が必要です（`1`がありました）");
}

#[test]
fn for_and_nil() {
    assert_eq!(
        ast("for @x in {1, 2} (\n    log[@x]\n)\n@n := nil"),
        "(for @x {1 2} ((log @x)))\n(:= @n nil)\n"
    );
    let e = error("for 1 in @a ( )");
    assert_eq!(
        e.message,
        "変数（`@x`）かパターン（`{@k, @v}`）が必要です（`1`がありました）"
    );
    let e = error("for @x @a ( )");
    assert_eq!(e.message, "`in`が必要です（`@a`がありました）");
}

#[test]
fn for_with_pattern() {
    assert_eq!(
        ast("for {@k, @v} in @m ( )"),
        "(for (arr @k @v) @m ())
"
    );
    assert_eq!(
        ast("for {\"n\": @n, \"p\": {@a, ...@r}} in @xs ( )"),
        "(for (map (\"n\" @n) (\"p\" (arr @a (... @r)))) @xs ())
"
    );
}

#[test]
fn try_operator() {
    assert_eq!(
        ast("@t := file_read[@p]?\n@n := to_num[@t]? + 1\n@v := @f[1]?{0}"),
        "(:= @t (? (file_read @p)))\n(:= @n (+ (? (to_num @t)) 1))\n(:= @v ({} (? (call @f 1)) 0))\n"
    );
}

#[test]
fn declare() {
    assert_eq!(ast("@x := 1 + 2"), "(:= @x (+ 1 2))\n");
    let e = error("@a{0} := 1");
    assert_eq!(e.message, "宣言できるのは変数だけです");
    let e = error("f[] := 1");
    assert_eq!(e.message, "宣言できるのは変数だけです");
}

#[test]
fn chained_comparison() {
    let e = error("@x = 1 < @a < 3");
    assert!(e.message.starts_with("比較は続けて書けません"));
}

fn parse_src(src: &str) -> Result<Program, Diagnostic> {
    parse(&tokenize(src).unwrap())
}

#[test]
fn record_definition_is_a_constructor_function() {
    let program = parse_src("record Point[@x, @y]\n").unwrap();
    assert_eq!(program.to_string(), "(record Point (@x @y) ())\n");
}

#[test]
fn record_definition_errors() {
    for src in [
        "record P[]\n",
        "record P[@x = 1]\n",
        "record P[...@r]\n",
        "record [@x]\n",
    ] {
        assert!(parse_src(src).is_err(), "{src}");
    }
}

#[test]
fn match_statement() {
    assert_eq!(
        ast(
            "match @v (\n    0 => log[1]\n    @x if @x > 1 => (\n        log[@x]\n    )\n    _ => log[2]\n)\n"
        ),
        "(match @v (0 ((log 1))) (@x (if (> @x 1)) ((log @x))) (_ ((log 2))))\n"
    );
}

#[test]
fn match_expression() {
    assert_eq!(
        ast("@k := match @v ( 0 => \"a\", 1 | 2 => \"b\", _ => \"c\" )\n"),
        "(:= @k (match @v (0 \"a\") ((or 1 2) \"b\") (_ \"c\")))\n"
    );
    // 文の先頭以外では式。値を `#` で返す
    assert_eq!(
        ast("f[@v] (\n    # match @v (\n        1 => 2\n        _ => 3\n    )\n)\n"),
        "(def f (@v) ((# (match @v (1 2) (_ 3)))))\n"
    );
}

#[test]
fn match_patterns() {
    let first = |src: &str| {
        let text = ast(&format!("@x := match @v (\n    {src} => 1\n)\n"));
        text.trim_start_matches("(:= @x (match @v (")
            .split(" 1)")
            .next()
            .unwrap()
            .to_string()
    };
    assert_eq!(first("-2"), "-2");
    assert_eq!(first("-3i"), "-3i");
    assert_eq!(first("1.5"), "1.5");
    assert_eq!(first("\"a\""), "\"a\"");
    assert_eq!(first("true"), "true");
    assert_eq!(first("nil"), "nil");
    assert_eq!(first("{}"), "(arr)");
    assert_eq!(first("{:}"), "(map)");
    assert_eq!(first("{@a, ...@r}"), "(arr @a (... @r))");
    assert_eq!(
        first("{\"k\": {1, @a}, \"j\": _}"),
        "(map (\"k\" (arr 1 @a)) (\"j\" _))"
    );
    assert_eq!(first("Point[@x, 1 | 2]"), "(rec Point @x (or 1 2))");
    assert_eq!(first("{1 | 2, @x}"), "(arr (or 1 2) @x)");
}

#[test]
fn match_arm_separators() {
    assert!(parse_src("match @v (\n\n    1 => 2,\n\n    3 => 4\n    ,\n)\n").is_ok());
    assert!(parse_src("match @v ()\n").is_ok());
    // 腕の間に区切りがない
    assert!(
        error("match @v ( 1 => 2 3 => 4 )\n")
            .message
            .contains("改行か `,`")
    );
}

#[test]
fn match_syntax_errors() {
    for src in [
        "match @v (\n    1\n)\n",
        "match @v (\n    => 1\n)\n",
        "match @v\n",
        "match (\n)\n",
        "match @v (\n    1 =>\n)\n",
        // 文字列の展開は書けない
        "match @v (\n    \"a@x\" => 1\n)\n",
        // 式の match の本体は式（文は書けない）
        "@x := match @v (\n    1 => break\n)\n",
        "@x := match @v (\n    1 => @y = 2\n)\n",
        // `|` はパターンの外では使えない
        "@x := 1 | 2\n",
        "match @v (\n    Point => 1\n)\n",
        "match @v (\n    {...@r, @a} => 1\n)\n",
    ] {
        assert!(parse_src(src).is_err(), "{src}");
    }
}

#[test]
fn refutable_patterns_are_match_only() {
    for src in [
        "{1, @a} := @v\n",
        "{\"k\": nil} := @v\n",
        "{@a | @b} := @v\n",
        "for Point[@x] in @v (\n)\n",
        "for {\"s\"} in @v (\n)\n",
    ] {
        assert!(parse_src(src).is_err(), "{src}");
    }
    // `_` は分割代入と for でも書ける
    assert_eq!(
        ast("{_, @b} := @v\nfor {_, @w} in @m (\n)\n"),
        "(:= (arr _ @b) @v)\n(for (arr _ @w) @m ())\n"
    );
}

#[test]
fn match_is_a_keyword() {
    assert!(error("match := 1\n").message.contains("式"));
    // `@match` は変数
    assert_eq!(ast("@match = 1\n"), "(= @match 1)\n");
}
