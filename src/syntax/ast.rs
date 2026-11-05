//! 構文木（design.md 4 章）。すべての文と式が位置情報を持つ

use std::fmt;
use std::rc::Rc;

use super::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}

pub type Block = Vec<Stmt>;

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// 評価器が関数表に登録するので Rc で共有する
    FuncDef(Rc<FuncDef>),
    /// `else if` は、if 文 1 つだけを含む else ブロックとして表す
    If {
        cond: Expr,
        then_block: Block,
        else_block: Option<Block>,
    },
    While {
        cond: Expr,
        body: Block,
    },
    /// `for @x in expr ( … )` / `for {@k, @v} in expr ( … )`。配列・マップ・文字列の要素を
    /// 順に `target`（変数か分割代入のパターン）に入れて本体を実行する（design.md 5.54）
    For {
        target: Pattern,
        iter: Expr,
        body: Block,
    },
    /// `# expr`
    Return(Option<Expr>),
    /// `while` を抜ける
    Break,
    /// `while` の次の繰り返しへ進む
    Continue,
    /// `defer expr`。いまの関数を抜けるとき、登録の逆順に `expr` を評価する（design.md 5.53）
    Defer(Expr),
    /// `@x := expr`。いま入っているスコープに新しい変数を作る
    Declare {
        target: Var,
        value: Expr,
    },
    /// `@x = expr` / `@x{i} = expr` / `@x{i}{j} = expr`。`indices` は添字を外側から順に並べたもの
    Assign {
        target: Var,
        indices: Vec<Expr>,
        value: Expr,
    },
    /// `{@a, @b} := expr` / `{"k": @v} = expr`。`declare` なら宣言、でなければ代入（design.md 5.19）
    Destructure {
        pattern: Pattern,
        value: Expr,
        declare: bool,
    },
    /// `match 式 ( パターン => 文 … )`（文の位置。design.md 5.59）
    Match {
        scrutinee: Expr,
        arms: Vec<MatchArm<Block>>,
    },
    Expr(Expr),
}

/// `match` の腕 1 つ。`B` は本体（文の `match` ではブロック、式の `match` では式）
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm<B> {
    pub pattern: Pattern,
    /// `パターン if 条件 => …` の条件
    pub guard: Option<Expr>,
    pub body: B,
    /// 腕の先頭（パターン）の位置
    pub span: Span,
}

/// パターンのリテラル（design.md 5.59）。比べるときは型も合わせる（`1` は数値、`1i` は整数）
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Number(f64),
    Int(i64),
    Str(String),
    Bool(bool),
    Nil,
}

/// 分割代入の左辺
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Var(Var),
    /// `{@a, @b, ...@rest}`
    Array {
        items: Vec<Pattern>,
        rest: Option<Var>,
    },
    /// `{"key": パターン, ...}`。`match` の `{:}`（任意のマップ）は空の `Map`
    Map(Vec<(String, Pattern)>),
    /// `_`。何にも合い、何も束縛しない（design.md 5.59）
    Wild,
    /// `1` `"a"` `true` `nil` など。`match` だけ
    Lit(Literal),
    /// `Point[p1, p2]`。型名が同じレコードの、定義順のフィールドに当てる。`match` だけ
    Record {
        name: String,
        items: Vec<Pattern>,
    },
    /// `p1 | p2`。どれかに合う。各選択肢は同じ変数を束縛する。`match` だけ
    Or(Vec<Pattern>),
}

impl Pattern {
    /// パターンに含まれる変数を、左から順に
    pub fn vars(&self) -> Vec<&Var> {
        match self {
            Pattern::Var(var) => vec![var],
            Pattern::Array { items, rest } => items
                .iter()
                .flat_map(Pattern::vars)
                .chain(rest.iter())
                .collect(),
            Pattern::Map(entries) => entries.iter().flat_map(|(_, p)| p.vars()).collect(),
            Pattern::Wild | Pattern::Lit(_) => Vec::new(),
            Pattern::Record { items, .. } => items.iter().flat_map(Pattern::vars).collect(),
            // 各選択肢が同じ変数を束縛する（Resolver が確かめる）ので、先頭の選択肢の分
            Pattern::Or(alts) => alts.first().map_or_else(Vec::new, Pattern::vars),
        }
    }

    /// 変数の束縛だけでなく、リテラルなどの「合わないことがある」部分を含むか
    pub fn is_refutable(&self) -> bool {
        match self {
            Pattern::Var(_) | Pattern::Wild => false,
            Pattern::Array { items, .. } => items.iter().any(Pattern::is_refutable),
            Pattern::Map(entries) => entries.iter().any(|(_, p)| p.is_refutable()),
            Pattern::Lit(_) | Pattern::Record { .. } | Pattern::Or(_) => true,
        }
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Number(n) => write!(f, "{n}"),
            Literal::Int(n) => write!(f, "{n}i"),
            Literal::Str(s) => write!(f, "{s:?}"),
            Literal::Bool(b) => write!(f, "{b}"),
            Literal::Nil => write!(f, "nil"),
        }
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pattern::Var(var) => write!(f, "{var}"),
            Pattern::Array { items, rest } => {
                write!(f, "(arr")?;
                for item in items {
                    write!(f, " {item}")?;
                }
                if let Some(rest) = rest {
                    write!(f, " (... {rest})")?;
                }
                write!(f, ")")
            }
            Pattern::Map(entries) => {
                write!(f, "(map")?;
                for (key, pattern) in entries {
                    write!(f, " ({key:?} {pattern})")?;
                }
                write!(f, ")")
            }
            Pattern::Wild => write!(f, "_"),
            Pattern::Lit(lit) => write!(f, "{lit}"),
            Pattern::Record { name, items } => {
                write!(f, "(rec {name}")?;
                for item in items {
                    write!(f, " {item}")?;
                }
                write!(f, ")")
            }
            Pattern::Or(alts) => {
                write!(f, "(or")?;
                for alt in alts {
                    write!(f, " {alt}")?;
                }
                write!(f, ")")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuncDef {
    /// 無名関数は空文字列
    pub name: String,
    /// `async` 関数（design.md 5.46）。呼ぶと future を返し、`await` で止まれる
    pub is_async: bool,
    /// `record 名前[@a, @b]` が作るコンストラクタ（design.md 5.55）。本体は空で、
    /// 呼ぶと引数を `params` の名前でまとめた、型名つきのマップを返す
    pub record: bool,
    pub params: Vec<Param>,
    /// `...@rest`。残りの引数を配列で受け取る（design.md 5.19）
    pub rest: Option<String>,
    pub body: Block,
    pub span: Span,
}

/// 文字列リテラルの部分。`"a@{1 + 2}b"` は Text("a"), Expr(1 + 2), Text("b")。
/// `@name` は Expr(Var) になる
#[derive(Debug, Clone, PartialEq)]
pub enum StrSegment {
    Text(String),
    Expr(Expr),
}

/// 関数の引数 1 つ。`default` があれば、渡されなかったときにその式の値になる（design.md 5.19）
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub default: Option<Expr>,
}

impl FuncDef {
    /// 渡さなければならない引数の数（既定値のない引数の数）
    pub fn min_args(&self) -> usize {
        self.params.iter().filter(|p| p.default.is_none()).count()
    }

    /// 渡せる引数の数の上限。可変長引数があれば None
    pub fn max_args(&self) -> Option<usize> {
        self.rest.is_none().then_some(self.params.len())
    }
}

/// `@name`。名前に `@` は含まない
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Var(pub String);

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Number(f64),
    /// 整数リテラル（design.md 5.40）
    Int(i64),
    Str(Vec<StrSegment>),
    Bool(bool),
    /// `nil`
    Nil,
    Var(Var),
    /// `name[args]`
    Call {
        name: String,
        args: Vec<Expr>,
    },
    /// `name`（`[` なし）。関数を値として取り出す
    FuncRef(String),
    /// `[@a, @b] ( … )`。作られた場所のスコープを捕まえる
    Lambda(Rc<FuncDef>),
    /// `...式`。呼び出しの引数と配列リテラルの中で、配列の要素を並べる
    Spread(Box<Expr>),
    /// `if 条件 ( 式 ) else ( 式 )`。式として値を持つ条件分岐（design.md 5.19）
    If {
        cond: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },
    /// `expr?`。値がエラー値なら、いまの関数からそのエラーを返す
    Try(Box<Expr>),
    /// `await expr`。future が終わるまで待って、その値になる（design.md 5.46）
    Await(Box<Expr>),
    /// `match 式 ( パターン => 式 … )`。選ばれた腕の式の値になる（design.md 5.59）
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm<Expr>>,
    },
    /// `callee[args]`。関数値を呼ぶ（`@f[1]`）
    CallValue {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    /// `{a, b, c}`
    Array(Vec<Expr>),
    /// `{"k": v, ...}`。空は `{:}`
    Map(Vec<(Expr, Expr)>),
    /// `@u.name`。モジュールの `@` 変数を読む
    Member {
        target: Box<Expr>,
        name: String,
    },
    /// `@u.name[args]`。モジュールの関数を呼ぶ
    MemberCall {
        target: Box<Expr>,
        name: String,
        args: Vec<Expr>,
    },
    /// `target{index}`
    Index {
        target: Box<Expr>,
        index: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
}

// ---- S 式での表示（`dolphin --ast` とテストで使う） ----
//
// @x := 1 + 2     → (:= @x (+ 1 2))
// @x = 1          → (= @x 1)
// @a{0} = 1       → (= ({} @a 0) 1)
// add[@a, @b] ( … ) → (def add (@a @b) (…))
// if c ( … ) else ( … ) → (if c (…) (…))

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for stmt in &self.stmts {
            writeln!(f, "{stmt}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Stmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            StmtKind::FuncDef(def) => {
                let head = if def.record {
                    "record"
                } else if def.is_async {
                    "async-def"
                } else {
                    "def"
                };
                write!(f, "({head} {} (", def.name)?;
                write_seq(
                    f,
                    def.params
                        .iter()
                        .map(ToString::to_string)
                        .chain(def.rest.iter().map(|r| format!("(... @{r})"))),
                )?;
                write!(f, ") {})", BlockDisplay(&def.body))
            }
            StmtKind::If {
                cond,
                then_block,
                else_block,
            } => {
                write!(f, "(if {cond} {}", BlockDisplay(then_block))?;
                if let Some(else_block) = else_block {
                    write!(f, " {}", BlockDisplay(else_block))?;
                }
                write!(f, ")")
            }
            StmtKind::While { cond, body } => write!(f, "(while {cond} {})", BlockDisplay(body)),
            StmtKind::For { target, iter, body } => {
                write!(f, "(for {target} {iter} {})", BlockDisplay(body))
            }
            StmtKind::Return(Some(value)) => write!(f, "(# {value})"),
            StmtKind::Return(None) => write!(f, "(#)"),
            StmtKind::Break => write!(f, "(break)"),
            StmtKind::Continue => write!(f, "(continue)"),
            StmtKind::Defer(expr) => write!(f, "(defer {expr})"),
            StmtKind::Declare { target, value } => write!(f, "(:= {target} {value})"),
            StmtKind::Destructure {
                pattern,
                value,
                declare,
            } => {
                let op = if *declare { ":=" } else { "=" };
                write!(f, "({op} {pattern} {value})")
            }
            StmtKind::Assign {
                target,
                indices,
                value,
            } => {
                let mut place = target.to_string();
                for index in indices {
                    place = format!("({{}} {place} {index})");
                }
                write!(f, "(= {place} {value})")
            }
            StmtKind::Match { scrutinee, arms } => {
                write!(f, "(match {scrutinee}")?;
                for arm in arms {
                    write_arm(f, arm, &BlockDisplay(&arm.body))?;
                }
                write!(f, ")")
            }
            StmtKind::Expr(expr) => write!(f, "{expr}"),
        }
    }
}

/// `(pattern body)` / `(pattern (if guard) body)`
fn write_arm<B>(
    f: &mut fmt::Formatter<'_>,
    arm: &MatchArm<B>,
    body: &dyn fmt::Display,
) -> fmt::Result {
    write!(f, " ({}", arm.pattern)?;
    if let Some(guard) = &arm.guard {
        write!(f, " (if {guard})")?;
    }
    write!(f, " {body})")
}

struct BlockDisplay<'a>(&'a [Stmt]);

impl fmt::Display for BlockDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(")?;
        write_seq(f, self.0)?;
        write!(f, ")")
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ExprKind::Number(n) => write!(f, "{n}"),
            ExprKind::Int(n) => write!(f, "{n}i"),
            ExprKind::Str(parts) => {
                write!(f, "\"")?;
                for part in parts {
                    match part {
                        StrSegment::Text(text) => {
                            let escaped = text.escape_debug().to_string().replace('@', "\\@");
                            write!(f, "{escaped}")?;
                        }
                        StrSegment::Expr(Expr {
                            kind: ExprKind::Var(var),
                            ..
                        }) => write!(f, "{var}")?,
                        StrSegment::Expr(expr) => write!(f, "@{{{expr}}}")?,
                    }
                }
                write!(f, "\"")
            }
            ExprKind::Bool(b) => write!(f, "{b}"),
            ExprKind::Nil => write!(f, "nil"),
            ExprKind::Var(var) => write!(f, "{var}"),
            ExprKind::Call { name, args } => {
                write!(f, "({name}")?;
                for arg in args {
                    write!(f, " {arg}")?;
                }
                write!(f, ")")
            }
            ExprKind::FuncRef(name) => write!(f, "(ref {name})"),
            ExprKind::Try(inner) => write!(f, "(? {inner})"),
            ExprKind::Await(inner) => write!(f, "(await {inner})"),
            ExprKind::Spread(inner) => write!(f, "(... {inner})"),
            ExprKind::Match { scrutinee, arms } => {
                write!(f, "(match {scrutinee}")?;
                for arm in arms {
                    write_arm(f, arm, &arm.body)?;
                }
                write!(f, ")")
            }
            ExprKind::If {
                cond,
                then_expr,
                else_expr,
            } => write!(f, "(if {cond} {then_expr} {else_expr})"),
            ExprKind::Lambda(def) => {
                write!(f, "({} (", if def.is_async { "async-fn" } else { "fn" })?;
                write_seq(
                    f,
                    def.params
                        .iter()
                        .map(ToString::to_string)
                        .chain(def.rest.iter().map(|r| format!("(... @{r})"))),
                )?;
                write!(f, ") {})", BlockDisplay(&def.body))
            }
            ExprKind::CallValue { callee, args } => {
                write!(f, "(call {callee}")?;
                for arg in args {
                    write!(f, " {arg}")?;
                }
                write!(f, ")")
            }
            ExprKind::Array(items) => {
                write!(f, "{{")?;
                write_seq(f, items)?;
                write!(f, "}}")
            }
            ExprKind::Map(entries) if entries.is_empty() => write!(f, "{{:}}"),
            ExprKind::Map(entries) => {
                write!(f, "{{")?;
                write_seq(f, entries.iter().map(|(k, v)| format!("{k}: {v}")))?;
                write!(f, "}}")
            }
            ExprKind::Member { target, name } => write!(f, "(. {target} {name})"),
            ExprKind::MemberCall { target, name, args } => {
                write!(f, "((. {target} {name})")?;
                for arg in args {
                    write!(f, " {arg}")?;
                }
                write!(f, ")")
            }
            ExprKind::Index { target, index } => write!(f, "({{}} {target} {index})"),
            ExprKind::Unary { op, operand } => write!(f, "({op} {operand})"),
            ExprKind::Binary { op, lhs, rhs } => write!(f, "({op} {lhs} {rhs})"),
        }
    }
}

impl fmt::Display for Var {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.0)
    }
}

impl fmt::Display for UnaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
        })
    }
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Rem => "%",
            BinaryOp::Eq => "==",
            BinaryOp::NotEq => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::LtEq => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::GtEq => ">=",
            BinaryOp::And => "&&",
            BinaryOp::Or => "||",
        })
    }
}

/// 空白区切りで並べる
impl fmt::Display for Param {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.default {
            Some(default) => write!(f, "(@{} {default})", self.name),
            None => write!(f, "@{}", self.name),
        }
    }
}

fn write_seq<T: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    items: impl IntoIterator<Item = T>,
) -> fmt::Result {
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            write!(f, " ")?;
        }
        write!(f, "{item}")?;
    }
    Ok(())
}
