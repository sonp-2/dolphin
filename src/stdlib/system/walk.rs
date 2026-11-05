//! `file_walk` と `file_glob`（design.md 5.49）。外部クレートを使わない
//!
//! パスは `/` 区切りで返す。順序は名前の文字コード順

use std::fs;
use std::path::{Path, PathBuf};

use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;
use crate::stdlib::fs::io_error;
use crate::stdlib::path::to_slash;
use crate::stdlib::{new_array, str_arg, type_error};

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register("file_walk", file_walk);
    interp.register("file_glob", file_glob);
}

/// `dir` と名前をつなげる。`dir` が空ならそのまま名前
fn child(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

fn paths_value(paths: Vec<String>) -> Value {
    new_array(paths.into_iter().map(|p| Value::Str(p.into())).collect())
}

// ---- file_walk ----

struct WalkOptions {
    files: bool,
    dirs: bool,
    follow_links: bool,
    max_depth: Option<usize>,
}

fn walk_options(value: &Value) -> Result<WalkOptions, String> {
    let Value::Map(options) = value else {
        return Err(type_error("file_walk", 1, "マップ", value));
    };
    let options = options.borrow();
    const KNOWN: [&str; 4] = ["files", "dirs", "follow_links", "max_depth"];
    for (key, _) in options.iter() {
        if !KNOWN.contains(&&**key) {
            return Err(format!(
                "file_walk に知らないオプション `{key}` があります（使えるのは {}）",
                KNOWN.join("、")
            ));
        }
    }
    let flag = |key: &str, default: bool| -> Result<bool, String> {
        match options.get(key) {
            None => Ok(default),
            Some(Value::Bool(b)) => Ok(*b),
            Some(other) => Err(format!(
                "file_walk の `{key}` は真偽値です（{}が渡されました）",
                other.type_name()
            )),
        }
    };
    let max_depth = match options.get("max_depth") {
        None | Some(Value::Nil) => None,
        Some(Value::Num(n)) if n.fract() == 0.0 && *n >= 0.0 && *n < 1e9 => Some(*n as usize),
        Some(Value::Num(n)) => {
            return Err(format!(
                "file_walk の `max_depth` は 0 以上の整数です（{n}）"
            ));
        }
        Some(other) => {
            return Err(format!(
                "file_walk の `max_depth` は数値か nil です（{}が渡されました）",
                other.type_name()
            ));
        }
    };
    Ok(WalkOptions {
        files: flag("files", true)?,
        dirs: flag("dirs", true)?,
        follow_links: flag("follow_links", false)?,
        max_depth,
    })
}

/// `dir` の下を再帰的にたどって、パスの配列を返す
fn file_walk(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    if !(1..=2).contains(&args.len()) {
        return Err(format!(
            "file_walk の引数は 1 個か 2 個です（{} 個渡されました）",
            args.len()
        ));
    }
    let dir = str_arg("file_walk", args, 0)?;
    let options = match args.get(1) {
        Some(value) => walk_options(value)?,
        None => WalkOptions {
            files: true,
            dirs: true,
            follow_links: false,
            max_depth: None,
        },
    };
    let shown = to_slash(Path::new(dir));
    // 先に開けるか調べる（なければ not_found）
    if let Err(e) = fs::read_dir(dir) {
        return Ok(io_error("開けません", dir, &e));
    }
    let mut ancestors = Vec::new();
    if options.follow_links
        && let Ok(real) = fs::canonicalize(dir)
    {
        ancestors.push(real);
    }
    let mut out = Vec::new();
    match walk(dir, &shown, 1, &options, &mut ancestors, &mut out) {
        Ok(()) => Ok(paths_value(out)),
        Err((path, e)) => Ok(io_error("読み込めません", &path, &e)),
    }
}

fn walk(
    fs_dir: &str,
    shown: &str,
    depth: usize,
    options: &WalkOptions,
    ancestors: &mut Vec<PathBuf>,
    out: &mut Vec<String>,
) -> Result<(), (String, std::io::Error)> {
    let fail = |e| (shown.to_string(), e);
    let mut entries = Vec::new();
    for entry in fs::read_dir(fs_dir).map_err(fail)? {
        let entry = entry.map_err(fail)?;
        let is_link = entry.file_type().map_err(fail)?.is_symlink();
        let is_dir = if is_link {
            options.follow_links && fs::metadata(entry.path()).is_ok_and(|m| m.is_dir())
        } else {
            entry.file_type().map_err(fail)?.is_dir()
        };
        entries.push((
            entry.file_name().to_string_lossy().into_owned(),
            is_dir,
            is_link,
        ));
    }
    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    for (name, is_dir, is_link) in entries {
        let shown_child = child(shown, &name);
        if !is_dir {
            if options.files {
                out.push(shown_child);
            }
            continue;
        }
        if options.dirs {
            out.push(shown_child.clone());
        }
        if options.max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        let fs_child = child(fs_dir, &name);
        // リンクをたどるときは、循環で同じディレクトリに戻らないようにする
        let real = if options.follow_links && is_link {
            match fs::canonicalize(&fs_child) {
                Ok(real) if ancestors.contains(&real) => continue,
                Ok(real) => Some(real),
                Err(_) => continue,
            }
        } else if options.follow_links {
            fs::canonicalize(&fs_child).ok()
        } else {
            None
        };
        if let Some(real) = &real {
            ancestors.push(real.clone());
        }
        let result = walk(&fs_child, &shown_child, depth + 1, options, ancestors, out);
        if real.is_some() {
            ancestors.pop();
        }
        result?;
    }
    Ok(())
}

// ---- file_glob ----

enum Seg {
    /// ワイルドカードのない名前
    Lit(String),
    /// `*` `?` `[]` を含む名前
    Pat(Vec<char>),
    /// `**`
    AnyDirs,
}

/// 区切りで分けた結果。`prefix` は絶対パスのルート（`/` や `C:/`）か空
struct Pattern {
    prefix: String,
    segs: Vec<Seg>,
    dir_only: bool,
}

fn compile(pattern: &str) -> Pattern {
    let text = if cfg!(windows) {
        pattern.replace('\\', "/")
    } else {
        pattern.to_string()
    };
    let (prefix, rest) = if let Some(rest) = text.strip_prefix('/') {
        ("/".to_string(), rest.to_string())
    } else if cfg!(windows)
        && text.len() >= 2
        && text.as_bytes()[0].is_ascii_alphabetic()
        && text.as_bytes()[1] == b':'
    {
        (
            format!("{}/", &text[..2]),
            text[2..].trim_start_matches('/').to_string(),
        )
    } else {
        (String::new(), text.clone())
    };
    let dir_only = rest.ends_with('/');
    let mut segs: Vec<Seg> = Vec::new();
    for part in rest.split('/').filter(|p| !p.is_empty()) {
        if part == "**" {
            // `**` が続いても 1 つと同じ
            if !matches!(segs.last(), Some(Seg::AnyDirs)) {
                segs.push(Seg::AnyDirs);
            }
        } else if part.contains(['*', '?', '[']) {
            segs.push(Seg::Pat(part.chars().collect()));
        } else {
            segs.push(Seg::Lit(part.to_string()));
        }
    }
    // 最後の `**` は `**/*`
    if matches!(segs.last(), Some(Seg::AnyDirs)) {
        segs.push(Seg::Pat(vec!['*']));
    }
    Pattern {
        prefix,
        segs,
        dir_only,
    }
}

/// 名前 1 つが、`*` `?` `[]` のパターンに合うか。`.` で始まる名前は、パターンも `.` で始まるときだけ
fn name_matches(pat: &[char], name: &str) -> bool {
    if name.starts_with('.') && pat.first() != Some(&'.') {
        return false;
    }
    let s: Vec<char> = name.chars().collect();
    matches(pat, &s)
}

fn matches(p: &[char], s: &[char]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        let advanced = if pi < p.len() {
            match p[pi] {
                '*' => {
                    star = Some((pi, si));
                    pi += 1;
                    continue;
                }
                '?' => {
                    pi += 1;
                    si += 1;
                    true
                }
                '[' => match class(p, pi, s[si]) {
                    Some((true, next)) => {
                        pi = next;
                        si += 1;
                        true
                    }
                    Some((false, _)) => false,
                    // 閉じていない `[` はその文字そのもの
                    None if s[si] == '[' => {
                        pi += 1;
                        si += 1;
                        true
                    }
                    None => false,
                },
                c if c == s[si] => {
                    pi += 1;
                    si += 1;
                    true
                }
                _ => false,
            }
        } else {
            false
        };
        if !advanced {
            match star {
                Some((sp, ss)) => {
                    pi = sp + 1;
                    si = ss + 1;
                    star = Some((sp, ss + 1));
                }
                None => return false,
            }
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// `p[i]` が `[` の文字クラスに `ch` が合うか。(合うか, クラスの次の位置)。閉じていなければ `None`
fn class(p: &[char], i: usize, ch: char) -> Option<(bool, usize)> {
    let mut j = i + 1;
    let negate = matches!(p.get(j), Some('!' | '^'));
    if negate {
        j += 1;
    }
    let mut found = false;
    let mut first = true;
    loop {
        let c = *p.get(j)?;
        if c == ']' && !first {
            return Some((found != negate, j + 1));
        }
        if p.get(j + 1) == Some(&'-') && p.get(j + 2).is_some_and(|&e| e != ']') {
            found |= c <= ch && ch <= p[j + 2];
            j += 3;
        } else {
            found |= c == ch;
            j += 1;
        }
        first = false;
    }
}

fn is_dir(path: &str) -> bool {
    fs::metadata(if path.is_empty() { "." } else { path }).is_ok_and(|m| m.is_dir())
}

fn glob_walk(base: &str, cur: &str, segs: &[Seg], dir_only: bool, out: &mut Vec<String>) {
    let fs_path = |cur: &str| child(base, cur);
    let list = |cur: &str| -> Vec<(String, bool)> {
        let path = fs_path(cur);
        let Ok(entries) = fs::read_dir(if path.is_empty() { "." } else { &path }) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|e| {
                // `**` はリンクの先へ入らない
                let real_dir = e.file_type().is_ok_and(|t| t.is_dir());
                (e.file_name().to_string_lossy().into_owned(), real_dir)
            })
            .collect()
    };
    let Some((first, rest)) = segs.split_first() else {
        if cur.is_empty() {
            return;
        }
        let path = fs_path(cur);
        if fs::symlink_metadata(&path).is_ok() && (!dir_only || is_dir(&path)) {
            out.push(cur.to_string());
        }
        return;
    };
    match first {
        Seg::Lit(name) => glob_walk(base, &child(cur, name), rest, dir_only, out),
        Seg::Pat(pat) => {
            for (name, _) in list(cur) {
                if name_matches(pat, &name) {
                    glob_walk(base, &child(cur, &name), rest, dir_only, out);
                }
            }
        }
        Seg::AnyDirs => {
            glob_walk(base, cur, rest, dir_only, out);
            for (name, real_dir) in list(cur) {
                if real_dir && !name.starts_with('.') {
                    glob_walk(base, &child(cur, &name), segs, dir_only, out);
                }
            }
        }
    }
}

/// パターンに合うパスの配列
fn file_glob(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    if !(1..=2).contains(&args.len()) {
        return Err(format!(
            "file_glob の引数は 1 個か 2 個です（{} 個渡されました）",
            args.len()
        ));
    }
    let (base, pattern) = if args.len() == 2 {
        (
            to_slash(Path::new(str_arg("file_glob", args, 0)?)),
            str_arg("file_glob", args, 1)?,
        )
    } else {
        (String::new(), str_arg("file_glob", args, 0)?)
    };
    let compiled = compile(pattern);
    let mut out = Vec::new();
    // 絶対パスのルートは、`base` ではなく結果のほうに含める
    let (base, start) = if compiled.prefix.is_empty() {
        (base, String::new())
    } else {
        (String::new(), compiled.prefix.clone())
    };
    glob_walk(&base, &start, &compiled.segs, compiled.dir_only, &mut out);
    out.sort_unstable();
    out.dedup();
    Ok(paths_value(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, s: &str) -> bool {
        name_matches(&p.chars().collect::<Vec<_>>(), s)
    }

    #[test]
    fn star_and_question() {
        assert!(m("*.rs", "main.rs"));
        assert!(!m("*.rs", "main.rc"));
        assert!(m("a*b*c", "aXXbYYc"));
        assert!(m("a*b*c", "abc"));
        assert!(!m("a*b*c", "acb"));
        assert!(m("?.txt", "a.txt"));
        assert!(!m("?.txt", "ab.txt"));
        assert!(m("*", "anything"));
        assert!(m("a*", "a"));
    }

    #[test]
    fn classes() {
        assert!(m("[abc].x", "b.x"));
        assert!(!m("[abc].x", "d.x"));
        assert!(m("[a-c].x", "b.x"));
        assert!(m("[!a-c].x", "z.x"));
        assert!(!m("[^a-c].x", "a.x"));
        assert!(m("[]a].x", "].x"));
        assert!(m("[*].x", "*.x"));
        assert!(!m("[*].x", "a.x"));
        // 閉じていない `[` はそのまま
        assert!(m("a[b", "a[b"));
        assert!(!m("a[b", "ab"));
    }

    #[test]
    fn hidden_names() {
        assert!(!m("*", ".git"));
        assert!(!m("?git", ".git"));
        assert!(m(".*", ".git"));
        assert!(m(".git", ".git"));
    }

    #[test]
    fn compile_pattern() {
        let p = compile("src/**/*.dol");
        assert_eq!(p.prefix, "");
        assert_eq!(p.segs.len(), 3);
        assert!(matches!(p.segs[1], Seg::AnyDirs));
        // 最後の `**` は `**/*`
        let p = compile("a/**");
        assert_eq!(p.segs.len(), 3);
        assert!(matches!(p.segs[2], Seg::Pat(_)));
        // `**` の連続は 1 つ
        assert_eq!(compile("**/**/x").segs.len(), 2);
        assert_eq!(compile("/usr/*").prefix, "/");
        assert!(compile("a/*/").dir_only);
        assert!(!compile("a/*").dir_only);
    }

    /// 一時ディレクトリに木を作って、終わったら消す
    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str) -> Tree {
            let root =
                std::env::temp_dir().join(format!("dolphin_walk_{tag}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("a/b")).unwrap();
            fs::write(root.join("a/b/f.txt"), "").unwrap();
            fs::write(root.join("a/g.txt"), "").unwrap();
            fs::write(root.join("top.txt"), "").unwrap();
            Tree(root)
        }

        fn root(&self) -> String {
            to_slash(&self.0)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn opts(max_depth: Option<usize>) -> WalkOptions {
        WalkOptions {
            files: true,
            dirs: true,
            follow_links: false,
            max_depth,
        }
    }

    fn walked(tree: &Tree, options: &WalkOptions) -> Vec<String> {
        let root = tree.root();
        let mut out = Vec::new();
        walk(&root, &root, 1, options, &mut Vec::new(), &mut out).unwrap();
        out.iter()
            .map(|p| p.strip_prefix(&format!("{root}/")).unwrap().to_string())
            .collect()
    }

    #[test]
    fn walk_orders_dirs_before_contents() {
        let tree = Tree::new("order");
        assert_eq!(
            walked(&tree, &opts(None)),
            ["a", "a/b", "a/b/f.txt", "a/g.txt", "top.txt"]
        );
        assert_eq!(walked(&tree, &opts(Some(1))), ["a", "top.txt"]);
        assert_eq!(
            walked(&tree, &opts(Some(2))),
            ["a", "a/b", "a/g.txt", "top.txt"]
        );
    }

    #[test]
    fn glob_finds_paths() {
        let tree = Tree::new("glob");
        let root = tree.root();
        let found = |pattern: &str| {
            let compiled = compile(pattern);
            let mut out = Vec::new();
            glob_walk(&root, "", &compiled.segs, compiled.dir_only, &mut out);
            out.sort_unstable();
            out
        };
        assert_eq!(found("*.txt"), ["top.txt"]);
        assert_eq!(found("**/*.txt"), ["a/b/f.txt", "a/g.txt", "top.txt"]);
        assert_eq!(found("a/**"), ["a/b", "a/b/f.txt", "a/g.txt"]);
        assert_eq!(found("*/"), ["a"]);
        assert!(found("none/*").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn walk_does_not_follow_links_by_default() {
        let tree = Tree::new("link");
        std::os::unix::fs::symlink(&tree.0, tree.0.join("a/loop")).unwrap();
        // 既定ではリンクの中へ入らない
        assert_eq!(
            walked(&tree, &opts(None)),
            ["a", "a/b", "a/b/f.txt", "a/g.txt", "a/loop", "top.txt"]
        );
        // たどっても、循環で元のディレクトリに戻らない
        let follow = WalkOptions {
            follow_links: true,
            ..opts(None)
        };
        assert_eq!(
            walked(&tree, &follow),
            ["a", "a/b", "a/b/f.txt", "a/g.txt", "a/loop", "top.txt"]
        );
    }
}
