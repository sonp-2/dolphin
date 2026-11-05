//! 実行前の検査（design.md 5 章）
//!
//! - 関数はトップレベルにだけ定義でき、名前は重複せず、組み込み関数とも重ならない
//! - 呼び出す関数が定義されていて、引数の数が合っている（組み込み関数は個数を実行時に検査）
//! - `#` は関数の中でだけ使える
//! - `break` と `continue` は `while` の中でだけ使える
//! - 変数は、読む前に宣言されている（`@x := ...`）。代入できるのも宣言済みの変数だけ。
//!   同じスコープでの二重宣言は禁止（内側のスコープでの宣言は外側を隠すだけで、許す）
//!
//! スコープは `( )` のブロックと関数ごとに 1 つ。トップレベルの文は書かれた順に検査する。
//! 関数の中からは、そのファイルのトップレベルで宣言される変数すべてが見える
//! （関数は定義より前の行から呼べるので、宣言の位置は問わない。実行時に未宣言なら実行時エラー）

mod expr;
mod func;
mod matching;
mod scope;
mod stmt;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use crate::diag::Diagnostic;
use crate::syntax::ast::{Program, StmtKind};

type RResult = Result<(), Diagnostic>;

/// プログラムの外ですでに定義されている関数
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Callee {
    /// 組み込み関数。引数の数は実行時に検査する
    Native,
    /// 以前の入力で定義した関数（REPL）
    /// 渡せる引数の数は `min` 個以上 `max` 個以下（可変長引数なら上限なし）
    User { min: usize, max: Option<usize> },
}

/// 引数の数が合わないときのメッセージ。`label` は「`f`」のような関数の呼び名
pub fn arity_message(label: &str, min: usize, max: Option<usize>, got: usize) -> String {
    match max {
        Some(max) if min == max => format!("{label} の引数は {min} 個です（{got} 個渡されました）"),
        Some(max) => {
            format!("{label} の引数は {min} 個以上 {max} 個以下です（{got} 個渡されました）")
        }
        None => format!("{label} の引数は {min} 個以上です（{got} 個渡されました）"),
    }
}

/// `lookup` は、プログラムの外で定義済みの関数を名前から引く。
/// プログラム内で同じ名前を定義した場合はそちらが優先（REPL での再定義）。
/// `has_global` は、前の入力（REPL）ですでに宣言されているトップレベルの変数か
pub fn resolve(
    program: &Program,
    lookup: impl Fn(&str) -> Option<Callee>,
    has_global: impl Fn(&str) -> bool,
) -> RResult {
    let mut functions = HashMap::new();
    let mut records = HashMap::new();
    let mut hoisted = HashSet::new();
    for stmt in &program.stmts {
        match &stmt.kind {
            StmtKind::FuncDef(def) => {
                // 組み込み関数と同じ名前の関数は、そのファイルの中で組み込み関数を隠す。
                // `import` だけは隠せない（評価器が直接扱うため）
                if def.name == "import" {
                    return Err(Diagnostic::new(
                        def.span,
                        "`import` と同じ名前の関数は定義できません",
                    ));
                }
                if def.record {
                    records.insert(def.name.as_str(), def.params.len());
                }
                if functions
                    .insert(def.name.as_str(), (def.min_args(), def.max_args()))
                    .is_some()
                {
                    return Err(Diagnostic::new(
                        def.span,
                        format!("関数 `{}` はすでに定義されています", def.name),
                    ));
                }
            }
            StmtKind::Declare { target, .. } => {
                hoisted.insert(target.0.as_str());
            }
            StmtKind::Destructure {
                pattern,
                declare: true,
                ..
            } => {
                hoisted.extend(pattern.vars().into_iter().map(|v| v.0.as_str()));
            }
            _ => {}
        }
    }

    let resolver = Resolver {
        functions,
        records,
        hoisted,
        lookup: &lookup,
        has_global: &has_global,
    };
    let mut top = Ctx {
        scopes: vec![HashSet::new()],
        in_func: false,
        in_async: false,
    };
    for stmt in &program.stmts {
        match &stmt.kind {
            StmtKind::FuncDef(def) => resolver.func_def(def)?,
            _ => resolver.stmt(stmt, &mut top, false)?,
        }
    }
    Ok(())
}

/// いま検査している場所のスコープ
struct Ctx<'a> {
    /// 外側から順。各スコープで宣言済みの変数名
    scopes: Vec<HashSet<&'a str>>,
    in_func: bool,
    /// `async` 関数の中か（`await` を書けるか。design.md 5.46）
    in_async: bool,
}

struct Resolver<'a> {
    /// 関数名 → 引数の数
    functions: HashMap<&'a str, (usize, Option<usize>)>,
    /// このファイルで定義したレコード → フィールドの数（`match` のレコードのパターンの検査）
    records: HashMap<&'a str, usize>,
    /// トップレベルで宣言される変数すべて（関数の中から見える）
    hoisted: HashSet<&'a str>,
    lookup: &'a dyn Fn(&str) -> Option<Callee>,
    has_global: &'a dyn Fn(&str) -> bool,
}
