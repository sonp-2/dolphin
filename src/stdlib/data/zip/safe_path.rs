//! 展開時の安全な名前の検査（パストラバーサル対策）。ここを通ったものだけを書き出す

use std::fs;
use std::path::Path;

use super::{Fail, R, fail};

/// 書庫のエントリ名を、安全な相対パスの成分に分ける。`\` も区切りとして扱う。
/// 絶対パス・ドライブ・`..`・`:` を含む成分・NUL・空の名前は `Err`。`.` と空の成分は捨てる
/// （`./` のように成分が残らないこともある）
pub(super) fn safe_components(name: &str) -> Result<Vec<String>, String> {
    if name.is_empty() {
        return Err("名前が空です".into());
    }
    if name.contains('\0') {
        return Err("名前に NUL が入っています".into());
    }
    let name = name.replace('\\', "/");
    if name.starts_with('/') {
        return Err("絶対パスです".into());
    }
    let mut parts = Vec::new();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err("`..` を含んでいます".into()),
            p if p.contains(':') => return Err("`:`（ドライブなど）を含んでいます".into()),
            p => parts.push(p.to_string()),
        }
    }
    Ok(parts)
}

pub(super) fn unsafe_entry(name: &str, why: &str) -> Fail {
    fail(
        "unsafe_path",
        format!("危険なエントリ名 `{name}`（{why}）のため展開しません"),
    )
}

/// 展開先の途中にシンボリックリンクがあれば、外へ書き出せてしまうので拒否する
pub(super) fn check_no_symlinks(dest: &Path, parts: &[String]) -> R<()> {
    let mut path = dest.to_path_buf();
    for part in parts {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(fail(
                    "unsafe_path",
                    format!(
                        "展開先の途中にシンボリックリンク `{}` があります",
                        path.display()
                    ),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_reject_traversal() {
        for bad in [
            "",
            "../a",
            "a/../b",
            "a/..",
            "/etc/passwd",
            "\\windows",
            "C:/x",
            "c:\\x",
            "a\\..\\..\\b",
            "a\0b",
            "a/b:c",
        ] {
            assert!(safe_components(bad).is_err(), "{bad:?} は拒否する");
        }
        assert_eq!(safe_components("./a//b/").unwrap(), ["a", "b"]);
        assert_eq!(safe_components("a\\b").unwrap(), ["a", "b"]);
        assert!(safe_components("./").unwrap().is_empty());
        assert_eq!(safe_components("日本語/ファイル.txt").unwrap().len(), 2);
    }
}
