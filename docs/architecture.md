# Dolphin 処理系の構成

処理系（Rust）の全体像をまとめる。言語仕様は [design.md](design.md)。

状態の書き方: ✅ 実装済み / 🚧 ひな形のみ / ⬜ 未着手

---

## 1. 全体の流れ

```
                ┌──────────────── dolphin（CLI） ────────────────┐
                │  run <file> / repl / check / test / fmt / lsp │
                └───────────────────────┬────────────────────────┘
                                        ▼
 .dol ─▶ Lexer ─▶ Parser ─▶  AST  ─▶ Resolver ─▶ Interpreter ─▶ 実行結果
        トークン   再帰下降          スコープ検査     バイトコードにコンパイルして VM で実行
                                    関数の事前登録
          │          │                 │               │
          └──────────┴──── Diagnostics ┴───────────────┘
                     行・列つきのエラー表示（ソース行に ^^^）
                                                       │ ネイティブ関数呼び出し
                                                       ▼
        ┌──────────────────── 標準ライブラリ ────────────────────┐
        │ system: io fs path os process ffi                      │
        │ network: net http（クライアント・サーバー） ws          │
        │ data: json csv bytes encoding crypto regex sqlite      │
        │ media: audio image   ほか: string array map math time … │
        │ ui / canvas（GUI。feature = "ui"）は src/ui/           │
        └────────────────────────────────────────────────────────┘
```

| 層 | 入力 → 出力 | 役割 | 状態 |
|---|---|---|---|
| Lexer | `&str` → `Vec<Token>` | 字句に分割する。`[ ]` `{ }` の中の改行を捨てる | ✅ |
| Parser | `&[Token]` → `Program` | design.md 4 章の EBNF を再帰下降で解析する | ✅ |
| Resolver | `&Program` → `()` | 関数の重複・未定義の関数・引数の数・関数の外での `#`・未宣言の変数への読み書き・二重宣言を実行前に検査する | ✅ |
| Interpreter | `&Program` → 出力 | AST をバイトコードにコンパイルして VM で実行する（`runtime/compile.rs`・`runtime/bytecode.rs`・`runtime/interp/vm.rs`）。呼び出しの深さは 1000 段まで | ✅ |
| Diagnostics | 各層のエラー → 文字列 | すべての層のエラーを同じ形で表示する | ✅ |
| CLI のツール | ソース → 検査・整形・テスト結果・LSP の応答 | `dolphin check`（実行せず検査）、`test`（`testrun.rs`）、`fmt`（`fmt.rs`）、`lsp`（`lsp/`） | ✅ |
| 標準ライブラリ | `&[Value]` → `Value` | ネイティブ関数を Interpreter に登録する。`stdlib/`（system・network・data・media と基本型の関数）と、GUI の `ui/` | ✅ |

Resolver を独立させる理由:

- 「宣言していない変数に代入した」などの誤りを、実行前にまとめて見つけられる
- 評価器を（実際に v0.53.0 でバイトコード VM に差し替えたように）置き換えても、前段を書き直さずに済む

---

## 2. CPython との対応

| 役割 | CPython | Dolphin |
|---|---|---|
| 字句解析 | `Parser/tokenizer.c` | `syntax/lexer.rs` |
| 構文解析 | PEG パーサ（`Grammar/python.gram`） | `syntax/parser/`（再帰下降） |
| 構文木 | `ast` モジュール | `syntax/ast.rs` |
| 名前解決 | `Python/symtable.c` | `sema/resolver/` |
| 実行 | `compile.c` → バイトコード → `ceval.c` | `runtime/compile.rs` → `runtime/interp/vm.rs`（バイトコード VM） |
| 値 | `PyObject` | `runtime/value.rs` の `Value` |
| 組み込み関数 | `builtins`、C 拡張 | `stdlib/`（`NativeFn` を登録） |

---

## 3. ディレクトリ構成

```
dolphin/
├─ Cargo.toml
├─ src/
│  ├─ main.rs            CLI の入口（引数の解析、実行、--tokens / --ast、check / test / fmt / lsp）
│  ├─ repl.rs            対話モード（feature = "repl"、rustyline を使う）
│  ├─ lib.rs             公開 API（run_source、Session）。CLI・テスト・将来の wasm 版から使う
│  ├─ fmt.rs             `dolphin fmt`（字句単位の整形。空白と字下げだけを決め直す）
│  ├─ lsp/               `dolphin lsp`（言語サーバー。mod.rs: 各要求の処理、transport.rs: JSON-RPC の読み書き、convert.rs: JSON の組み立てと位置・URI の変換、analysis.rs: 名前の宣言と使用の解析、completion.rs: 補完、json.rs: 小さな JSON）
│  ├─ ui/             std:ui と std:canvas（canvas.rs / canvas_gui.rs: フレームごとの描画。mod.rs: コールバックの番号・ui.run、tree.rs: 部品の値から Node の木を作る、widgets.rs: 部品を作る関数、host.rs: メインスレッドのホスト、gui.rs: egui の描画（feature = "ui"））
│  ├─ testrun.rs        `dolphin test`（*_test.dol を集めて test_ 関数を実行）
│  ├─ diag.rs            Diagnostic（エラー表示）
│  ├─ syntax/
│  │  ├─ span.rs         位置情報（行・列）
│  │  ├─ lexer.rs        字句解析
│  │  ├─ ast.rs          Program / Stmt / Expr
│  │  └─ parser/         構文解析（stmt・expr・pattern・matching・func・record・lookahead・string）
│  ├─ sema/
│  │  └─ resolver/       実行前の検査（func・stmt・expr・scope）
│  ├─ runtime/
│  │  ├─ value.rs        値の型
│  │  ├─ map.rs          順序つきマップ
│  │  ├─ gc.rs           循環回収器（配列・マップ・スコープ・関数値の登録簿と試行削除）
│  │  ├─ handle.rs       ハンドル（TCP の接続・待ち受けなど、閉じる必要のある資源）
│  │  ├─ data.rs         Data（スレッドをまたげる値。非同期の結果を運ぶ）
│  │  ├─ events.rs       イベントループ（タイマー・非同期の結果・http.serve のメッセージ）
│  │  ├─ module.rs       モジュール（import したファイル 1 つ分の `@` 変数と関数）
│  │  ├─ env.rs          Env（スコープ 1 つ分の変数表。外側への参照を持つ）
│  │  ├─ error.rs        RuntimeError（呼び出し履歴つき）
│  │  ├─ bytecode.rs     命令（Op）と、コンパイルした 1 つのまとまり（Chunk）
│  │  ├─ compile.rs      AST → バイトコード
│  │  ├─ interp.rs       Interpreter（モジュール・組み込み関数の呼び出し・分割代入）
│  │  ├─ interp/import.rs  import（ファイルと組み込みモジュールの読み込み）
│  │  └─ interp/vm.rs    VM（枠と値のスタックをヒープに持つ。Dolphin の関数呼び出しで Rust の再帰を使わない）と、`async` 関数の流れ（止めて再開する。future の終わりをイベントループが拾う）
│  └─ stdlib/            `stdlib::http` のように 1 段で呼べるよう、mod.rs で再公開している
│     ├─ mod.rs          全モジュールの登録、引数検査の共通処理
│     ├─ string.rs       str_*、to_str、to_num
│     ├─ array.rs        arr_len, arr_push, arr_pop
│     ├─ map.rs          map_len, map_keys, map_has, map_remove
│     ├─ collections.rs  std:collections
│     ├─ bits.rs         std:bits（ビット演算）
│     ├─ math.rs         abs, floor, …（乱数の本体は random.rs。xorshift64* を自前で実装）
│     ├─ random.rs       std:random
│     ├─ time.rs         std:time（暦の計算は自前）
│     ├─ types.rs        type_of
│     ├─ error.rs        is_error, err_kind, err_message, err_new
│     ├─ debug.rs        assert, assert_eq, dbg
│     ├─ gc.rs           gc_collect
│     ├─ async_.rs       std:async（タイマー、コールバック型の非同期の関数の共通処理）
│     ├─ stream.rs       std:stream（ファイル・標準入出力・HTTP の本文・メモリ・TCP を共通に読み書きする）
│     ├─ worker.rs       std:worker（別スレッドの別インタプリタとメッセージでやり取りする。runtime/worker.rs に口の型）
│     ├─ system/         OS との境界
│     │  ├─ io.rs        log, input
│     │  ├─ fs.rs        std:fs（fs.read, fs.write, …。旧名 file_* は deprecated.rs の警告つき）
│     │  ├─ path.rs      std:path
│     │  ├─ os.rs        std:os（args / env / exit / run など）
│     │  ├─ process.rs   os.spawn / wait / poll / kill（常駐する子プロセスとの連携）
│     │  └─ ffi.rs       std:ffi（C の共有ライブラリを呼ぶ。ffi feature。libloading と libffi）
│     ├─ network/        通信
│     │  ├─ net.rs       std:net（TCP。ブロッキング、標準ライブラリだけ）
│     │  ├─ http.rs      std:http のクライアント（feature = "http"。ureq + rustls）
│     │  ├─ http_server/  std:http の serve（std::net だけ。HTTP の解析は自前）
│     │  ├─ ws.rs        std:ws（クライアント。サーバー側の接続の send / close も受ける）
│     │  └─ ws_proto.rs  WebSocket のフレーム・メッセージ・ハンドシェイク（RFC 6455。SHA-1 も自前）
│     ├─ data/           データの形式と変換
│     │  ├─ json.rs      std:json（parse / stringify / pretty。自前の実装、外部クレートなし）
│     │  ├─ csv.rs       std:csv
│     │  ├─ bytes.rs     バイト列（bytes_* ）
│     │  ├─ encoding.rs  std:encoding（base64 / hex / URL。自前）
│     │  ├─ crypto.rs    std:crypto（feature = "crypto"。ring + md-5）
│     │  ├─ regex.rs     std:regex（feature = "regex"。regex クレート）
│     │  └─ sqlite.rs    std:sqlite（feature = "sqlite"）
│     └─ media/          音と画像
│        ├─ audio.rs     std:audio（feature = "audio"。rodio。再生は補助スレッド）
│        └─ image_.rs    std:image（feature = "image"）
├─ tests/
│  ├─ golden.rs          cases/*.dol を実行し、*.out と比較する
│  ├─ cases/             .dol と期待出力 .out
│  ├─ examples.rs        examples/ のスクリプトが動くこと（ネットワークを使うものは構文だけ）
│  ├─ fmt_files.rs       リポジトリの .dol を整形して、字句が変わらず冪等なこと
│  ├─ cli.rs             実行ファイルを起動して確かめる（os.args・os.exit・os.run）
│  ├─ http.rs / http_server/ / ws.rs   HTTP クライアント・サーバー、WebSocket
│  ├─ async_io.rs / await.rs / stream.rs   非同期の関数、async / await、ストリーム
│  └─ process.rs / worker.rs / ffi.rs      os.spawn、ワーカー、FFI
├─ examples/             サンプルスクリプト（breakout・counter_ui・todo_api・dolvc など）
├─ bench/                実行の速さを測るスクリプト（fib・loop・array）
├─ editors/vscode/       VS Code 拡張（シンタックスハイライトと dolphin lsp への接続）
├─ .github/workflows/    CI（fmt / clippy / test）
└─ docs/
```

- 最初は crate を 1 つにして、モジュールで分ける
- HTTP や GUI の重い依存は cargo の feature で切り替え、使わないときはビルドに含めない
- ビルドが遅くなったら workspace（`crates/dolphin-syntax` など）に分ける。モジュール境界をそのまま crate 境界にできるよう、`syntax` は `runtime` に依存しない

### 依存の向き

```
main.rs ─▶ lib.rs ─┬─▶ syntax ◀─┬─ sema
                   │            │
                   ├─▶ runtime ─┘ （AST を読む）
                   ├─▶ stdlib ──▶ runtime
                   └─▶ diag ◀──── 全層
```

---

## 4. 主な型

### 4.1 AST（`syntax/ast.rs`）

```rust
struct Stmt { kind: StmtKind, span: Span }
enum StmtKind { FuncDef(Rc<FuncDef>), If { .. }, While { .. }, For { .. }, Return(..), Break, Continue, Declare { .. }, Assign { .. }, Destructure { .. }, Expr(..) }

struct Expr { kind: ExprKind, span: Span }
enum ExprKind { Number, Int, Str, Bool, Nil, Var, Call, FuncRef, Lambda, Spread, If, Try, Await, CallValue, Array, Map, Member, MemberCall, Index, Unary, Binary }
```

- すべての文と式が `Span` を持つ。実行時エラーにも行・列を出すため
- 文字列リテラルは、Lexer の段階で文字の部分と変数の部分（`StrPart`）に分けてある

### 4.2 値（`runtime/value.rs`）

```rust
enum Value {
    Nil,                             // 値なし
    Bool(bool),
    Num(f64),
    Int(i64),                        // 64 ビット符号付き整数。数値とは別の型（design.md 5.40）
    Str(Rc<str>),
    Array(Rc<RefCell<Vec<Value>>>),
    Map(Rc<RefCell<Map>>),           // runtime/map.rs
    Module(Rc<Module>),              // runtime/module.rs。import の結果
    Func(Rc<Closure>),               // 関数値。定義と、作られた場所の Env の組
    Native(NativeRef),               // 関数値になった組み込み関数
    Error(Rc<ErrorValue>),           // 外の世界の失敗（種類とメッセージ）。`?` で伝える
    Bytes(Rc<[u8]>),                 // バイト列。変更できない
    Handle(Rc<Handle>),              // 閉じる必要のある資源（TCP など）と future（runtime/handle.rs）
}
```

- 配列とマップは**参照渡し**（design.md 7.1）。`Value` を clone しても `Rc` が増えるだけで、同じものを指す。書き換えは `RefCell::borrow_mut` で行う
- `Map` は自作の順序つきマップ。`Vec<(キー, 値)>` に追加順で持ち、`HashMap<キー, 位置>` を索引にする。コア言語は外部クレートを使わない方針なので、`indexmap` は使わない

### 4.3 組み込み関数（`runtime/interp.rs`）

```rust
type NativeFn = fn(&mut Interpreter<'_>, &[Value]) -> Result<Value, String>;
```

- エラーはメッセージだけ返し、呼び出し位置は Interpreter が付ける
- Resolver は、プログラムの外で定義済みの関数（組み込み関数と、REPL の前の入力で定義した関数）を `Interpreter::callee` で問い合わせる。そのため `Session` は、標準ライブラリを登録してから Resolver を呼ぶ
- `&mut Interpreter` を受け取るので、`log` は `interp.out` に書ける。テストでは `out` に `Vec<u8>` を渡して出力を取り出す

### 4.4 Session と REPL（`lib.rs`、`repl.rs`）

```rust
let mut session = Session::new(&mut stdout);
session.eval("@x = 1")?;          // 変数と関数は次の eval でも残る
session.eval("@x + 1")?;          // 最後の文が式なら Some(値) を返す
```

- `run_source` は `Session` を 1 回だけ使うもの。ファイル実行と REPL で処理は同じ
- REPL は、`(` や `"` が閉じていないエラー（`Diagnostic::unexpected_eof`）なら、エラーを出さずに続きの行を読む
- 行編集（矢印キー・履歴）の rustyline は、CLI だけが使う optional な依存。lib は外部クレートに依存しない

### 4.5 エラー

| 型 | 発生する層 | 中身 |
|---|---|---|
| `Diagnostic` | 全層の共通の形 | メッセージ、位置、補足 |
| `RuntimeError` | Interpreter | メッセージ、位置、呼び出し履歴。`Diagnostic` に変換して表示する |
| エラー値（`Value::Error`） | 標準ライブラリ（`stdlib::error_value`） | `kind`・`message`・任意の `data`。実行時エラーとは別で、`catch` が捕まえた `RuntimeError` だけが kind `"runtime"` のエラー値になる |

エラー値の `kind` は、1.0 以降の互換性の約束の対象で、一覧は design.md 5.57 に 1 か所でまとめている。`tests/error_kinds.rs` が、`src/` が返す kind と 5.57 の表を照合する（kind を足すときは、先に表を直す）。メッセージの文面は約束しない（5.56）。

```
main.dol:2:6: エラー: 文字列が閉じられていません
  |
2 | @b = "open
  |      ^
```

---

## 5. 段階ごとに増えるもの

段階は design.md 2.1 と同じ。

| 段階 | 構文・意味 | ランタイム・標準ライブラリ | ツール・品質 |
|---|---|---|---|
| 1 コア | Parser、AST、Resolver | Value、Interpreter、io / string / array | ゴールデンテスト、CI |
| 2 実用化 | マップ ✅、文字列操作 ✅ | string の拡充 ✅、map ✅、math ✅ | REPL ✅ |
| 3 複数ファイル | `import` ✅（`.` による名前空間） | モジュールの読み込み ✅、fs ✅、json ✅（自前） | `dolphin check` ✅ |
| 4 バックエンド | — | http ✅（クライアントは `ureq`、サーバーは `std::net` の上に自前。tokio も axum も使わない） | サンプルの API ✅（`examples/todo_api.dol`） |
| 5 デスクトップ | `gameloop` は作らず、`ui` / `canvas` が毎フレーム呼ぶ関数を受け取る（design.md 7.2） | `std:ui` / `std:canvas` ✅（`eframe`（egui）。`ui` feature）、`std:audio` ✅ | サンプルのゲーム ✅（`examples/breakout.dol`） |
| 6 任意 | `async` / `await` ✅ | バイトコード VM ✅（v0.53.0） | ベンチマーク ✅（`bench/` の fib など） |
| おまけ | — | — | VS Code のシンタックスハイライトと言語サーバー ✅（`editors/vscode`）、`dolphin test` ✅、`dolphin fmt` ✅、wasm で動くブラウザ版 Playground ⬜ |

---

## 6. 先に分かっている難所

| 箇所 | 問題 | 方針案 |
|---|---|---|
| 段階 4 HTTP | サーバーは複数スレッドで動くが、インタプリタは `Rc` を使っていて別スレッドに渡せない（`Send` でない） | 実際にそうした: サーバー（`std::net` の上に自前）は別スレッドで動かし、リクエストはチャネルでインタプリタのスレッドに送る。インタプリタは 1 スレッドで順番に処理する（JavaScript のイベントループと同じ型。`runtime/events.rs`） |
| 段階 5 GUI | eframe がメインスレッドのループを握る | メインスレッドは GUI のホスト（`ui/host.rs`）に使い、スクリプトはインタプリタ専用のスレッドで動かす。毎フレーム呼ぶ関数は `ui` / `canvas` に登録する（design.md 7.2） |
| メモリ | `Rc` は循環参照を解放できない（配列が自分自身を含む場合や、関数値が自分を作ったスコープの変数に入る場合など） | 当面は許容する。問題になったら GC を自作する。表示と `==` は循環しても止まるようにしてある |
| スタック | 旧評価器（tree-walking）は Dolphin の呼び出し 1 段で Rust の再帰が何段も深くなり、Windows の既定スタック（1 MB）ではすぐあふれた | 呼び出しの深さを 1000 段に制限し、CLI は 256 MB のスタックを持つスレッドで動かす。v0.53.0 の VM は Dolphin の関数呼び出しで再帰しないが、組み込み関数が呼ぶ関数値の入れ子には再帰が残るので、制限とスレッドはそのまま |
| 性能 | tree-walking は遅い | v0.53.0 でバイトコード VM にした（`await` の土台が目的。関数のローカル変数はスロット（番号）に解決して値のスタックに置く。無名関数・分割代入を含む関数とファイルの本体は名前で引く Env のまま）。`bench/` で測れる。変数を作らないブロックはスコープを作らない |

---

## 7. 開発の流れ

| コマンド | 内容 |
|---|---|
| `cargo run -- examples/count.dol` | スクリプトを実行する |
| `cargo run -- --tokens examples/count.dol` | トークン列を表示する |
| `cargo test` | 単体テストと統合テスト（`tests/`）を実行する |
| `cargo run -- --ast examples/count.dol` | AST を S 式で表示する |
| `cargo run -- check examples/count.dol` | 実行せずに、構文と Resolver の検査だけをする |
| `cargo run -- test [パス...]` | `*_test.dol` の `test_` で始まる関数を実行する（パスなしは `.`） |
| `cargo run -- fmt [--check \| --stdout] file.dol` | 整形する。`--check` は書き換えず、必要なら終了コード 1 |
| `cargo run -- lsp` | 言語サーバーを標準入出力で起動する（ふつうはエディタが起動する） |
| `cargo build --release` のあと `target/release/dolphin bench/fib.dol` | `bench/` で実行の速さを測る |
| `cargo fmt` / `cargo clippy` | 整形と静的解析。CI でも同じものを実行する |

テストの置き場所:

- 各層の単体テストは、その層のファイルの `#[cfg(test)] mod tests` に書く
- ネットワーク・非同期・プロセスなど、実行ファイルや外部との接点は `tests/*.rs` の統合テストで確かめる
- Dolphin で書いたテストは `*_test.dol` に `test_` で始まる関数として書き、`dolphin test` で実行する
- 言語として正しく動くかは `tests/cases/` に `.dol` と期待出力 `.out` を並べて確かめる。エラーになるケースも、エラー表示を `.out` に書いてテストする
