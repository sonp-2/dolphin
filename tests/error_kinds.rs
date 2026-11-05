//! エラー値の kind の網羅を守る（design.md 5.57）
//!
//! - 実装（src/ のテスト以外）が返す kind は、5.57 の表に載っていること
//! - 表の kind は、実装のどこかに文字列として残っていること（消した kind を表に残さない）
//!
//! kind を新しく足す・消すときは、先に design.md の 5.57 の表を直す。
//! 実装の走査は文字列の形で見つけるので、kind を返す書き方を変えたら、ここも直す。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// design.md 5.57 の表の kind
fn documented_kinds() -> BTreeSet<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = fs::read_to_string(root.join("docs/design.md")).unwrap();
    let start = text.find("### 5.57 ").expect("5.57 の節がありません");
    let end = text[start..]
        .find("#### 揺れ")
        .map(|i| start + i)
        .expect("5.57 に「揺れ」の見出しがありません");
    let mut kinds = BTreeSet::new();
    for line in text[start..end].lines() {
        // `| \`kind\` | …` の行
        if let Some(rest) = line.strip_prefix("| `")
            && let Some(name) = rest.split('`').next()
            && is_kind(name)
        {
            kinds.insert(name.to_string());
        }
    }
    kinds
}

fn is_kind(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            && path.file_name().is_some_and(|n| n != "tests.rs")
        {
            out.push(path);
        }
    }
}

/// `prefix` の直後（空白を飛ばして）にある `"ident"` を集める。`tail` が空でなければ、
/// 閉じた引用符のあとがそれで始まるものだけ
fn quoted_after(text: &str, prefix: &str, tail: &[&str], out: &mut BTreeSet<String>) {
    let mut from = 0;
    while let Some(i) = text[from..].find(prefix) {
        let mut p = from + i + prefix.len();
        from = p;
        let bytes = text.as_bytes();
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        if bytes.get(p) != Some(&b'"') {
            continue;
        }
        let Some(len) = text[p + 1..].find('"') else {
            continue;
        };
        let name = &text[p + 1..p + 1 + len];
        let after = &text[p + 2 + len..];
        if is_kind(name) && (tail.is_empty() || tail.iter().any(|t| after.starts_with(t))) {
            out.insert(name.to_string());
        }
    }
}

/// 実装が返す kind（文字列の形で見つけたもの）と、実装に現れる引用符つきの文字列すべて
fn implemented_kinds() -> (BTreeSet<String>, String) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    let mut found = BTreeSet::new();
    let mut all_source = String::new();
    for path in files {
        let full = fs::read_to_string(&path).unwrap();
        // インラインの `mod tests` 以降は見ない
        let text = full.split("#[cfg(test)]").next().unwrap();
        all_source.push_str(text);
        for prefix in [
            "error_value(",
            "Value::error(",
            "Value::error_with(",
            "fail(",
        ] {
            quoted_after(text, prefix, &[], &mut found);
        }
        // kind を変数で渡すファイルは、その変数への代入（`=> "x"`、`("x", メッセージ)`）も見る
        let indirect = text.contains("error_value(kind")
            || text.contains("error_value(f.")
            || text.contains("let (kind,")
            || text.contains("let kind =")
            || text.contains("Err((kind,");
        if indirect {
            quoted_after(text, "=>", &[",", "\n"], &mut found);
            quoted_after(
                text,
                "=> (",
                &[", format!(", ", \"", ", message"],
                &mut found,
            );
            quoted_after(text, "(", &[", format!(", ", \"", ", message"], &mut found);
        }
    }
    // 呼び出しの形ではないが、kind として拾ってしまうものは使わない
    (found, all_source)
}

#[test]
fn implemented_kinds_are_documented() {
    let documented = documented_kinds();
    assert!(documented.len() >= 20, "表の読み取りに失敗: {documented:?}");
    let (found, _) = implemented_kinds();
    let missing: Vec<_> = found.difference(&documented).collect();
    assert!(
        missing.is_empty(),
        "design.md 5.57 の表にない kind を実装が返しています: {missing:?}"
    );
}

#[test]
fn documented_kinds_exist_in_source() {
    let (_, source) = implemented_kinds();
    let stale: Vec<_> = documented_kinds()
        .into_iter()
        .filter(|k| !source.contains(&format!("\"{k}\"")))
        .collect();
    assert!(
        stale.is_empty(),
        "5.57 の表にあるが、実装にない kind: {stale:?}"
    );
}

/// 実装から見つけた kind が、少なくとも主なものを含んでいること（走査が空振りしていないか）
#[test]
fn scan_finds_known_kinds() {
    let (found, _) = implemented_kinds();
    for k in [
        "parse",
        "not_found",
        "permission",
        "io",
        "network",
        "timeout",
        "sql",
        "busy",
        "unsafe_path",
    ] {
        assert!(
            found.contains(k),
            "走査が {k} を見つけられません: {found:?}"
        );
    }
}
