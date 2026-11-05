# 🐬 Dolphin

`@` で変数を宣言する、Rust 製の小さなインタプリタ言語。拡張子は `.dol`。

## このプロジェクトについて

インタプリタ言語がどう動いているのかを理解するために、インタプリタ言語を自作しています。せっかく自作するので、シンタックスはユニークにしました。

作る理由は、学習とロマンと自己満足です。実用を目的にはしていませんが、ロマンとして実際に使っても困らない水準を目標にしています。バックエンドの API やデスクトップアプリも作れる言語を目指しています。

```
@name := "Dolphin"
log["Hello, @name!"]

add[@a, @b] (
    # @a + @b
)

@i := 1
while @i <= 3 (
    log["count: @i"]
    @i = add[@i, 1]
)

@user := {"name": @name, "langs": {"Rust", "C++"}}
log[@user]
```

実行結果:

```
Hello, Dolphin!
count: 1
count: 2
count: 3
{"name": "Dolphin", "langs": {"Rust", "C++"}}
```

## なぜ `@` なのか

<!-- ここに書く -->

## 構文の早見表

| 書き方 | 意味 |
|---|---|
| `@x := 1` | 変数の宣言。いま入っているスコープ（ファイル・関数・`( )` のブロック）に作る |
| `@x = 2` | 宣言済みの変数への代入（宣言していなければ実行前エラー） |
| `name[a, b]` | 関数呼び出し |
| `name[@a, @b] ( … )` | 関数定義 |
| `[@a] ( … )` / `@f := name` / `@f[1]` | 無名関数、関数値、関数値の呼び出し。外側の変数を捕まえる（クロージャ） |
| `# value` | 関数から値を返す |
| `@x := f[…]?` | `f` が返したエラー値は、いまの関数からそのまま返す。`is_error[@r]` で見分ける |
| `@r := catch[f]` | 関数値 `f` を呼び、実行時エラー（`assert` の失敗・型違いなど）が起きたら、種類 `"runtime"` のエラー値にして返す。起きなければ `f` の戻り値（[5.53](docs/design.md)） |
| `defer 式` | いまの関数を抜けるとき（`#`・エラー・`?` のどれでも）、登録の逆順に評価する後始末（[5.53](docs/design.md)） |
| `if 条件 ( … )` / `else ( … )` / `else if` | 条件分岐 |
| `while 条件 ( … )` / `for @x in 配列・マップ・文字列 ( … )`（`for {@k, @v} in @m ( … )` のように分割代入も書ける）/ `break` / `continue` | 繰り返し |
| `match 式 ( パターン [if 条件] => 本体 … )` | パターンマッチ。文でも式でも書ける（`@k := match @v ( 0 => "零", _ => "他" )`）。リテラル・`_`・`@x`・配列 `{@h, ...@t}`・マップ `{"k": @v}`・レコード `Point[@x, @y]`・`1 | 2` を書け、合う腕がなければ実行時エラー（[5.59](docs/design.md)）。`match` は予約語 |
| `nil` | 値なし |
| `{1, 2, 3}` / `@a{0}` | 配列。添字は `{ }` |
| `{"key": value}` / `@m{"key"}` / `{:}` | マップ（`{:}` は空のマップ） |
| `"Hi, @name"` | 文字列。中の変数は展開される |
| `// コメント` | 行末までコメント |

詳しい仕様は [docs/design.md](docs/design.md)、組み込み関数の一覧は [design.md 5.5](docs/design.md#55-組み込み関数) にあります。

## できること

段階は [docs/design.md](docs/design.md) の 2.1 に対応しています。

- [x] **段階 1: コア言語**
  - 変数（`@`。`:=` で宣言、`=` で代入、`( )` ごとのスコープ）、四則演算・剰余・比較・論理演算（短絡評価）、単項 `-` `!`、`( )` によるグループ化
  - `if` / `else` / `else if`、`while`、`break` / `continue`
  - 関数（定義より前から呼べる、再帰、`#` で戻り値）
  - 配列、文字列の中の `@x` の展開、行末コメント
- [x] **段階 2: 実用化**
  - 行・列と呼び出し履歴つきのエラー表示
  - マップ（連想配列）、レコード（`record Point[@x, @y]`。型名つきのマップ。[design.md 5.55](docs/design.md)）
  - 文字列・数値変換・数学の組み込み関数
  - 対話モード（REPL）
- [x] **段階 3.5**: 言語の土台の見直し（[design.md 8 章](docs/design.md)）
  - [x] 変数を `@` に統一、`:=` で宣言、添字を `{ }` に、スコープ
  - [x] 関数値とクロージャ（`[@n] ( # @n + 1 )`、`arr_map` など）
  - [x] `nil`、`for`、`range`
  - [x] エラー処理（エラー値と `?`）
  - [x] 実行モデル（イベントループ。タイマー・非同期の結果・`http.serve` のメッセージを 1 スレッドで順に処理する）
- [x] **段階 3**: 複数ファイル、ファイル入出力
  - [x] `import`（名前空間つき。`@u := import["util.dol"]`、`@u.add[1, 2]`）
  - [x] ファイル入出力（`std:fs` の `read` `write` `append` `exists`）
- [x] **段階 4**: HTTP、JSON → バックエンド API
  - [x] 組み込みモジュール（`import["std:..."]`）と JSON（`std:json`）
  - [x] HTTP クライアント（`std:http`）
  - [x] HTTP サーバー（`http.serve`。WebSocket も）
  - [x] ルーターとミドルウェア（`std:router`。[design.md 5.52](docs/design.md)）
- [x] **段階 5**: ウィンドウ・描画・入力 → デスクトップアプリ
  - [x] `std:ui`（宣言的 UI）と `std:canvas`（ゲーム・描画）。`--features ui` で有効になる（eframe / egui）
  - [x] 音 `std:audio`（`--features audio`）
- [x] **そのほか**: 標準ライブラリ（`std:json` `std:regex` `std:sqlite` `std:worker` など）、`async` / `await`、バイトコード VM、`dolphin check` / `test` / `fmt` / `lsp`、FFI（`--features ffi`）

### ルーター（`std:router`）の例

`http.serve` に渡すハンドラを、経路ごとに分けて書けます（ミドルウェア・グループ・静的ファイル・CORS・クッキーつき）。

```
@http := import["std:http"]
@router := import["std:router"]

@r := @router.new[]
@router.use[@r, [@req, @next] (                 // 全リクエストを包むミドルウェア
    log[@req{"method"}, @req{"path"}]
    # @next[@req]
)]
@router.get[@r, "/users/:id", [@req] (
    # @router.json[200, {"id": @req{"params"}{"id"}}]
)]
@router.get[@r, "/static/*", @router.static["public"]]

@api := @router.new[]
@router.post[@api, "/echo", [@req] ( # @router.text[200, @req{"body"}] )]
@router.group[@r, "/api", @api]                 // POST /api/echo

@http.serve[8080, @router.handler[@r]]?
```

## 作ってみたもの

`examples/` に、Dolphin で書いたプログラムがあります。

| ファイル | 内容 |
|---|---|
| [dolvc.dol](examples/dolvc.dol) | 小さなバージョン管理ツール（git のミニ版）。`commit` `status` `diff` `log` `checkout`。ファイルの中身は sha256 で名前づけして保存する |
| [dolvc_demo.dol](examples/dolvc_demo.dol) | `dolvc` を一時ディレクトリで通して動かすデモ（`cargo run -- examples/dolvc_demo.dol`） |
| [life.dol](examples/life.dol) | ライフゲームのアニメーション（グライダー銃） |
| [mandelbrot.dol](examples/mandelbrot.dol) | マンデルブロ集合を文字で描く |
| [json.dol](examples/json.dol) / [http.dol](examples/http.dol) | JSON の読み書き / HTTP で JSON を取ってくる（ネットワークが必要） |
| [closure.dol](examples/closure.dol) | クロージャと `arr_map` |
| [shapes.dol](examples/shapes.dol) | レコード（`record Point[@x, @y]`）で図形を表し、面積を求める |
| [match_demo.dol](examples/match_demo.dol) | パターンマッチ（`match`）: 式の木の評価、メッセージの振り分け、レコードの分解 |
| [hello.dol](examples/hello.dol) / [count.dol](examples/count.dol) | 最小の例。変数・関数・`while`・配列の合計 |
| [todo_api.dol](examples/todo_api.dol) | メモを JSON で出し入れする小さな API サーバー（`http.serve` と `std:router`。`curl` で試せる） |
| [counter_ui.dol](examples/counter_ui.dol) | ウィンドウを開く小さなアプリ（`std:ui`。`cargo run --features ui -- examples/counter_ui.dol`） |
| [breakout.dol](examples/breakout.dol) | ブロック崩し（`std:canvas`。`cargo run --features ui -- examples/breakout.dol`） |

## コードを段階ごとに読む

機能ごとにタグを付けています。何が増えたかは [CHANGELOG.md](CHANGELOG.md) にまとめてあり、タグの間の差分を見ると、そのコードを読めます。

| タグ | 内容 | 差分（前のタグから） |
|---|---|---|
| [v0.1.0](https://github.com/sonp-2/dolphin/tree/v0.1.0) | 段階 1: コア言語 | — |
| [v0.2.0](https://github.com/sonp-2/dolphin/tree/v0.2.0) | 段階 2: エラー表示、文字列、マップ、REPL | [v0.1.0...v0.2.0](https://github.com/sonp-2/dolphin/compare/v0.1.0...v0.2.0) |
| [v0.3.0](https://github.com/sonp-2/dolphin/tree/v0.3.0) | `import`（名前空間つき） | [v0.2.0...v0.3.0](https://github.com/sonp-2/dolphin/compare/v0.2.0...v0.3.0) |
| [v0.4.0](https://github.com/sonp-2/dolphin/tree/v0.4.0) | ファイル入出力 | [v0.3.0...v0.4.0](https://github.com/sonp-2/dolphin/compare/v0.3.0...v0.4.0) |
| [v0.5.0](https://github.com/sonp-2/dolphin/tree/v0.5.0) | 文法の見直し（`$` の廃止、`:=`、添字 `{ }`、スコープ） | [v0.4.0...v0.5.0](https://github.com/sonp-2/dolphin/compare/v0.4.0...v0.5.0) |
| [v0.6.0](https://github.com/sonp-2/dolphin/tree/v0.6.0) | 関数値とクロージャ | [v0.5.0...v0.6.0](https://github.com/sonp-2/dolphin/compare/v0.5.0...v0.6.0) |
| [v0.7.0](https://github.com/sonp-2/dolphin/tree/v0.7.0) | `nil`、`for`、`range` | [v0.6.0...v0.7.0](https://github.com/sonp-2/dolphin/compare/v0.6.0...v0.7.0) |
| [v0.8.0](https://github.com/sonp-2/dolphin/tree/v0.8.0) | エラー値と `?` | [v0.7.0...v0.8.0](https://github.com/sonp-2/dolphin/compare/v0.7.0...v0.8.0) |
| [v0.9.0](https://github.com/sonp-2/dolphin/tree/v0.9.0) | 組み込みモジュールと `std:json` | [v0.8.0...v0.9.0](https://github.com/sonp-2/dolphin/compare/v0.8.0...v0.9.0) |
| [v0.10.0](https://github.com/sonp-2/dolphin/tree/v0.10.0) | HTTP クライアント `std:http` | [v0.9.0...v0.10.0](https://github.com/sonp-2/dolphin/compare/v0.9.0...v0.10.0) |
| [v0.11.0](https://github.com/sonp-2/dolphin/tree/v0.11.0) | 基本型の関数（配列・マップ・文字列・数値）、`type_of`、`std:math` | [v0.10.0...v0.11.0](https://github.com/sonp-2/dolphin/compare/v0.10.0...v0.11.0) |
| [v0.12.0](https://github.com/sonp-2/dolphin/tree/v0.12.0) | `std:time`、`std:os`、`std:path`、ファイルの残り | [v0.11.0...v0.12.0](https://github.com/sonp-2/dolphin/compare/v0.11.0...v0.12.0) |
| [v0.13.0](https://github.com/sonp-2/dolphin/tree/v0.13.0) | バイト列、`std:encoding`、`std:crypto` | [v0.12.0...v0.13.0](https://github.com/sonp-2/dolphin/compare/v0.12.0...v0.13.0) |
| [v0.14.0](https://github.com/sonp-2/dolphin/tree/v0.14.0) | 例（`dolvc`、ライフゲーム、マンデルブロ）、`os.exe` | [v0.13.0...v0.14.0](https://github.com/sonp-2/dolphin/compare/v0.13.0...v0.14.0) |
| [v0.15.0](https://github.com/sonp-2/dolphin/tree/v0.15.0) | 循環回収器、`gc_collect` | [v0.14.0...v0.15.0](https://github.com/sonp-2/dolphin/compare/v0.14.0...v0.15.0) |
| [v0.16.0](https://github.com/sonp-2/dolphin/tree/v0.16.0) | HTTP サーバー `http.serve` | [v0.15.0...v0.16.0](https://github.com/sonp-2/dolphin/compare/v0.15.0...v0.16.0) |
| [v0.17.0](https://github.com/sonp-2/dolphin/tree/v0.17.0) | `assert` `assert_eq` `dbg`、`dolphin check`、`dolphin test` | [v0.16.0...v0.17.0](https://github.com/sonp-2/dolphin/compare/v0.16.0...v0.17.0) |
| [v0.18.0](https://github.com/sonp-2/dolphin/tree/v0.18.0) | `dolphin fmt` | [v0.17.0...v0.18.0](https://github.com/sonp-2/dolphin/compare/v0.17.0...v0.18.0) |
| [v0.19.0](https://github.com/sonp-2/dolphin/tree/v0.19.0) | 数値リテラル（`0xFF` `1_000` `1e9`） | [v0.18.0...v0.19.0](https://github.com/sonp-2/dolphin/compare/v0.18.0...v0.19.0) |
| [v0.20.0](https://github.com/sonp-2/dolphin/tree/v0.20.0) | 組み込み関数を隠す定義 | [v0.19.0...v0.20.0](https://github.com/sonp-2/dolphin/compare/v0.19.0...v0.20.0) |
| [v0.21.0](https://github.com/sonp-2/dolphin/tree/v0.21.0) | `@m.key` | [v0.20.0...v0.21.0](https://github.com/sonp-2/dolphin/compare/v0.20.0...v0.21.0) |
| [v0.22.0](https://github.com/sonp-2/dolphin/tree/v0.22.0) | 条件式（`if c ( a ) else ( b )`） | [v0.21.0...v0.22.0](https://github.com/sonp-2/dolphin/compare/v0.21.0...v0.22.0) |
| [v0.23.0](https://github.com/sonp-2/dolphin/tree/v0.23.0) | 引数の既定値 | [v0.22.0...v0.23.0](https://github.com/sonp-2/dolphin/compare/v0.22.0...v0.23.0) |
| [v0.24.0](https://github.com/sonp-2/dolphin/tree/v0.24.0) | 可変長引数 `...@rest` と展開 `...式` | [v0.23.0...v0.24.0](https://github.com/sonp-2/dolphin/compare/v0.23.0...v0.24.0) |
| [v0.25.0](https://github.com/sonp-2/dolphin/tree/v0.25.0) | 文字列の式展開 `"@{a + b}"` と生の文字列 `r"…"` | [v0.24.0...v0.25.0](https://github.com/sonp-2/dolphin/compare/v0.24.0...v0.25.0) |
| [v0.26.0](https://github.com/sonp-2/dolphin/tree/v0.26.0) | 分割代入（`{@a, @b} := …`） | [v0.25.0...v0.26.0](https://github.com/sonp-2/dolphin/compare/v0.25.0...v0.26.0) |
| [v0.27.0](https://github.com/sonp-2/dolphin/tree/v0.27.0) | ビット演算 `std:bits` | [v0.26.0...v0.27.0](https://github.com/sonp-2/dolphin/compare/v0.26.0...v0.27.0) |
| [v0.28.0](https://github.com/sonp-2/dolphin/tree/v0.28.0) | `std:regex`（正規表現） | [v0.27.0...v0.28.0](https://github.com/sonp-2/dolphin/compare/v0.27.0...v0.28.0) |
| [v0.29.0](https://github.com/sonp-2/dolphin/tree/v0.29.0) | TCP `std:net` と `handle` 型 | [v0.28.0...v0.29.0](https://github.com/sonp-2/dolphin/compare/v0.28.0...v0.29.0) |
| [v0.30.0](https://github.com/sonp-2/dolphin/tree/v0.30.0) | WebSocket（`http.serve` の `websocket` と `std:ws`） | [v0.29.0...v0.30.0](https://github.com/sonp-2/dolphin/compare/v0.29.0...v0.30.0) |
| [v0.31.0](https://github.com/sonp-2/dolphin/tree/v0.31.0) | 非同期の関数（`std:async`、`http.get_async` など） | [v0.30.0...v0.31.0](https://github.com/sonp-2/dolphin/compare/v0.30.0...v0.31.0) |
| [v0.32.0](https://github.com/sonp-2/dolphin/tree/v0.32.0) | `std:ui`（ウィンドウと宣言的 UI。`--features ui`） | [v0.31.0...v0.32.0](https://github.com/sonp-2/dolphin/compare/v0.31.0...v0.32.0) |
| [v0.33.0](https://github.com/sonp-2/dolphin/tree/v0.33.0) | `std:canvas`（ゲーム・描画。`--features ui`） | [v0.32.0...v0.33.0](https://github.com/sonp-2/dolphin/compare/v0.32.0...v0.33.0) |
| [v0.34.0](https://github.com/sonp-2/dolphin/tree/v0.34.0) | 画像（`canvas.image`） | [v0.33.0...v0.34.0](https://github.com/sonp-2/dolphin/compare/v0.33.0...v0.34.0) |
| [v0.35.0](https://github.com/sonp-2/dolphin/tree/v0.35.0) | 音 `std:audio`（`--features audio`） | [v0.34.0...v0.35.0](https://github.com/sonp-2/dolphin/compare/v0.34.0...v0.35.0) |
| [v0.36.0](https://github.com/sonp-2/dolphin/tree/v0.36.0) | SQLite `std:sqlite`（`--features sqlite`） | [v0.35.0...v0.36.0](https://github.com/sonp-2/dolphin/compare/v0.35.0...v0.36.0) |
| [v0.37.0](https://github.com/sonp-2/dolphin/tree/v0.37.0) | CSV `std:csv` | [v0.36.0...v0.37.0](https://github.com/sonp-2/dolphin/compare/v0.36.0...v0.37.0) |
| [v0.38.0](https://github.com/sonp-2/dolphin/tree/v0.38.0) | 日付の計算（`std:time` の `add_months` `start_of` など） | [v0.37.0...v0.38.0](https://github.com/sonp-2/dolphin/compare/v0.37.0...v0.38.0) |
| [v0.39.0](https://github.com/sonp-2/dolphin/tree/v0.39.0) | 乱数 `std:random`（`seed` `choice` `shuffle` `sample` など） | [v0.38.0...v0.39.0](https://github.com/sonp-2/dolphin/compare/v0.38.0...v0.39.0) |
| [v0.40.0](https://github.com/sonp-2/dolphin/tree/v0.40.0) | `std:collections`（ヒープ・`counts` `unique` `group_by` `chunk` `zip` `flatten`） | [v0.39.0...v0.40.0](https://github.com/sonp-2/dolphin/compare/v0.39.0...v0.40.0) |
| [v0.41.0](https://github.com/sonp-2/dolphin/tree/v0.41.0) | 言語サーバー `dolphin lsp`（定義へ移動・エラー表示・ホバー・アウトライン・整形）と VS Code 拡張 | [v0.40.0...v0.41.0](https://github.com/sonp-2/dolphin/compare/v0.40.0...v0.41.0) |
| [v0.42.0](https://github.com/sonp-2/dolphin/tree/v0.42.0) | 言語サーバーに補完・参照の検索・リネーム | [v0.41.0...v0.42.0](https://github.com/sonp-2/dolphin/compare/v0.41.0...v0.42.0) |
| [v0.43.0](https://github.com/sonp-2/dolphin/tree/v0.43.0) | `wss://`（WebSocket の TLS） | [v0.42.0...v0.43.0](https://github.com/sonp-2/dolphin/compare/v0.42.0...v0.43.0) |
| [v0.44.0](https://github.com/sonp-2/dolphin/tree/v0.44.0) | 画像の加工 `std:image`（`info` `resize` `crop` `convert`） | [v0.43.0...v0.44.0](https://github.com/sonp-2/dolphin/compare/v0.43.0...v0.44.0) |
| [v0.45.0](https://github.com/sonp-2/dolphin/tree/v0.45.0) | WebSocket のコールバック版 `ws.connect_async` | [v0.44.0...v0.45.0](https://github.com/sonp-2/dolphin/compare/v0.44.0...v0.45.0) |
| [v0.46.0](https://github.com/sonp-2/dolphin/tree/v0.46.0) | パッケージの `import["名前"]`（`dolphin_modules/` と `DOLPHIN_PATH`） | [v0.45.0...v0.46.0](https://github.com/sonp-2/dolphin/compare/v0.45.0...v0.46.0) |
| [v0.47.0](https://github.com/sonp-2/dolphin/tree/v0.47.0) | エラー値の詳細データ（`err_new` の第 3 引数、`err_data`） | [v0.46.0...v0.47.0](https://github.com/sonp-2/dolphin/compare/v0.46.0...v0.47.0) |
| [v0.48.0](https://github.com/sonp-2/dolphin/tree/v0.48.0) | 整数 `int`（`123i`、`to_int`） | [v0.47.0...v0.48.0](https://github.com/sonp-2/dolphin/compare/v0.47.0...v0.48.0) |
| [v0.49.0](https://github.com/sonp-2/dolphin/tree/v0.49.0) | ストリーム `std:stream`（`HTTP` の `"stream": true` を含む） | [v0.48.0...v0.49.0](https://github.com/sonp-2/dolphin/compare/v0.48.0...v0.49.0) |
| [v0.50.0](https://github.com/sonp-2/dolphin/tree/v0.50.0) | プロセスの常駐連携（`os.spawn` `os.wait`） | [v0.49.0...v0.50.0](https://github.com/sonp-2/dolphin/compare/v0.49.0...v0.50.0) |
| [v0.51.0](https://github.com/sonp-2/dolphin/tree/v0.51.0) | ワーカー `std:worker`（別スレッドの別インタプリタ） | [v0.50.0...v0.51.0](https://github.com/sonp-2/dolphin/compare/v0.50.0...v0.51.0) |
| [v0.52.0](https://github.com/sonp-2/dolphin/tree/v0.52.0) | FFI `std:ffi`（C の共有ライブラリを呼ぶ。`--features ffi`） | [v0.51.0...v0.52.0](https://github.com/sonp-2/dolphin/compare/v0.51.0...v0.52.0) |
| [v0.53.0](https://github.com/sonp-2/dolphin/tree/v0.53.0) | バイトコード VM（評価器の置き換え。挙動は同じ） | [v0.52.0...v0.53.0](https://github.com/sonp-2/dolphin/compare/v0.52.0...v0.53.0) |
| [v0.54.0](https://github.com/sonp-2/dolphin/tree/v0.54.0) | `async` と `await`（止めて再開できる関数、`future`） | [v0.53.0...v0.54.0](https://github.com/sonp-2/dolphin/compare/v0.53.0...v0.54.0) |
| [v0.55.0](https://github.com/sonp-2/dolphin/tree/v0.55.0) | 標準ライブラリの拡充（PostgreSQL・YAML・TOML・zip・ログ・ファイル操作・引数解析・`str_format` など）と VM の高速化 | [v0.54.0...v0.55.0](https://github.com/sonp-2/dolphin/compare/v0.54.0...v0.55.0) |
| [v0.56.0](https://github.com/sonp-2/dolphin/tree/v0.56.0) | `catch` / `defer`、`for` のパターンとマップ・文字列、レコード（`record`）、ルーター `std:router` | [v0.55.0...v0.56.0](https://github.com/sonp-2/dolphin/compare/v0.55.0...v0.56.0) |
| [v0.57.0](https://github.com/sonp-2/dolphin/tree/v0.57.0) | パターンマッチ `match`、標準ライブラリの名前の整理（`std:fs`、旧名の非推奨）、安定性とエラー kind の方針 | [v0.56.0...v0.57.0](https://github.com/sonp-2/dolphin/compare/v0.56.0...v0.57.0) |

差分は、まず `docs/design.md`（仕様）から読み、次に `tests/cases/`（使い方の実例）、最後に `src/` を読むと流れがつかめます。手元では `git diff v0.2.0 v0.3.0 --stat` のように見られます。

エラーは該当箇所を指して表示します。

```
runtime_error.dol:5:12: エラー: 添字 5 は範囲外です（要素は 2 個）
  |
5 |     # @arr{5}
  |            ^
  = inner の中（2:7 から呼び出し）
  = outer の中（8:1 から呼び出し）
```

## 実装構成

```
ソースコード → Lexer → Parser → AST → Resolver → Interpreter
```

| 段階 | 役割 |
|---|---|
| Lexer | ソース文字列をトークン列に分割する |
| Parser | トークン列を再帰下降で解析し、AST（構文木）を作る |
| Resolver | 実行前に、未定義の関数・引数の数・宣言されていない変数などを検査する |
| Compiler / VM | AST を命令の列（バイトコード）にコンパイルして、VM で実行する |

周辺のツールも、同じ Lexer / Parser / Resolver を土台にしています。

| 部分 | 役割 |
|---|---|
| `fmt.rs` | `dolphin fmt`。字句単位で空白と字下げだけを整える |
| `lsp/` | `dolphin lsp`。言語サーバー（定義へ移動・エラー表示・ホバー・補完・リネームなど）。`editors/vscode` に VS Code 拡張がある |
| `testrun.rs` | `dolphin test`。`*_test.dol` を集めて `test_` で始まる関数を実行する |

全体像は [docs/architecture.md](docs/architecture.md) にまとめています。コア言語は外部クレートに依存しません（HTTP の TLS は、既定で有効な `http` feature の `ureq` を、ハッシュ・HMAC・乱数は `crypto` feature の `ring` と `md-5` を、正規表現は `regex` feature の `regex` を使います）。GUI（`std:ui`）は依存が多いので、`ui` feature（`cargo build --features ui`）を付けたときだけ含まれます。音（`std:audio`）も `audio` feature（`rodio` を使う。Linux では `libasound2-dev` が要ります）です。SQLite（`std:sqlite`）は `sqlite` feature（`rusqlite` で SQLite のソースを同梱する。C コンパイラが要ります）です。FFI（`std:ffi`）は `ffi` feature（`libloading` と `libffi` を使う。C コンパイラが要ります）です。

PostgreSQL（`std:postgres`）は `postgres` feature（純 Rust の `postgres` クレートを使う。TLS は `tls` feature の rustls）です。`cargo build --features postgres` で入り、例は [examples/pg_demo.dol](examples/pg_demo.dol)、仕様は [docs/design.md](docs/design.md) の 5.47 にあります。サーバーを使うテストは、環境変数 `DOLPHIN_TEST_PG_URL` があるときだけ動きます。

## ビルド・実行方法

必要なもの: [Rust](https://rustup.rs)（stable）

```sh
# スクリプトを実行
cargo run -- examples/hello.dol

# 引数をつけて実行（スクリプトの中で os.args[] で読める）
cargo run -- examples/hello.dol a b

# 引数なしで対話モード（exit で終了）
cargo run

# 途中経過を見る
cargo run -- --tokens examples/hello.dol   # トークン列
cargo run -- --ast examples/hello.dol      # 構文木（S 式）

# テスト
cargo test
```

### 検査・整形・テスト・言語サーバー

```sh
# 実行せずに、構文と実行前の検査（Resolver）だけをする。問題があれば終了コード 1
dolphin check a.dol b.dol

# *_test.dol の test_ で始まる関数を実行する。パスは省略すると、いまのフォルダ以下を探す
dolphin test
dolphin test tests/

# 整形して書き戻す。--check は書き換えずに、整形が必要なファイルを表示する（必要なら終了コード 1）。--stdout は結果を標準出力に出す（1 ファイルだけ）
dolphin fmt a.dol
dolphin fmt --check a.dol b.dol
dolphin fmt --stdout a.dol

# 言語サーバー（標準入出力で LSP を話す。ふつうはエディタが起動する）
dolphin lsp
```

`cargo run -- check a.dol` のように、`cargo run --` のあとに同じ引数を渡しても使えます。VS Code では [editors/vscode](editors/vscode) の拡張を入れると、`dolphin lsp` につながります。

### ベンチマーク

`bench/` に、実行の速さを測る小さなスクリプトがあります（`fib.dol`: 再帰、`loop.dol`: `while` のループ、`array.dol`: 配列への追加と `for`、`forx.dol`: 分割代入・マップ・文字列の `for`、`match.dol`: `match` と `if` の連なりの比較）。速さを調べるときは、リリースビルドで測ります。

```sh
cargo build --release
time target/release/dolphin bench/fib.dol
```

`cargo build --release` でビルドすると、実行ファイルは `target/release/dolphin`（Windows は `dolphin.exe`）にできます。

## 安定性

1.0 以降の約束です（詳しくは [docs/design.md](docs/design.md) の 5.56・5.57。いまは 0.x なので、この約束はまだ守る義務のない予告です）。

- **変えないもの**: 言語（構文と意味）、組み込み関数、主要な `std:*`（関数名・引数・戻り値の型）、エラー値の kind（`err_kind` の文字列。一覧は 5.57）、仕様に書いた数値上限（呼び出し 1000 段、`range` 1000 万要素、JSON の入れ子 512 など）。破壊的変更は MAJOR を上げるときだけです。
- **変えてよいもの**: エラーメッセージの日本語、内部実装、性能、`dolphin` コマンドの出力の細部。プログラムは文面で分岐せず、`is_error` と `err_kind` で分岐してください。
- **実験的**: `std:ui` `std:canvas`（GUI）、`std:audio`、`std:ffi`、`std:sqlite`、`std:postgres` は、1.x でも変わることがあります（いずれも既定で無効の feature です）。
- 実行時エラー（`assert` の失敗・型違いなど、プログラムの間違い）はエラー値にならず、`catch` で捕まえたときだけ kind `"runtime"` のエラー値になります。

## ブランチ構成

| ブランチ | 内容 |
|---|---|
| `main` | 作り直し版（Rust） |
| [`archive/cpp`](https://github.com/sonp-2/dolphin/tree/archive/cpp) | 初期実装（C++17 + SFML）。サンプルスクリプトは `my_scripts/` |
