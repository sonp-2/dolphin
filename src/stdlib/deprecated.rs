//! 非推奨の名前（旧名）の一覧と、呼ばれたときの警告（design.md 5.58）
//!
//! 旧名は元の関数を呼ぶ薄い入り口で、初めて呼ばれたときに標準エラーへ警告を 1 行出す。
//! 同じ旧名の 2 回目以降は出さない。次の版で、ここの表と旧名の登録をまとめて消す

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// （旧名, 新名の案内）。グローバルは名前そのもの、モジュールの関数は `os.exe` のようにモジュール名をつける。
/// 案内は「を使ってください」の前の部分
pub const DEPRECATED: &[(&str, &str)] = &[
    ("file_read", "std:fs の read"),
    ("file_write", "std:fs の write"),
    ("file_append", "std:fs の append"),
    ("file_exists", "std:fs の exists"),
    ("file_read_bytes", "std:fs の read_bytes"),
    ("file_write_bytes", "std:fs の write_bytes"),
    ("file_append_bytes", "std:fs の append_bytes"),
    ("file_list", "std:fs の list"),
    ("file_info", "std:fs の info"),
    ("file_mkdir", "std:fs の mkdir"),
    ("file_remove", "std:fs の remove"),
    ("file_remove_all", "std:fs の remove_all"),
    ("file_rename", "std:fs の rename"),
    ("file_copy", "std:fs の copy"),
    ("file_walk", "std:fs の walk"),
    ("file_glob", "std:fs の glob"),
    ("file_temp", "std:fs の temp"),
    ("file_temp_dir", "std:fs の temp_dir"),
    ("file_chmod", "std:fs の chmod"),
    ("random", "std:random の float"),
    ("random_int", "std:random の int"),
    ("str_to_bytes", "bytes_from_str"),
    ("os.exe", "std:os の exe_path"),
    ("ui.quit", "std:ui の stop"),
    ("canvas.quit", "std:canvas の stop"),
    ("worker.terminate", "std:worker の kill"),
];

/// 旧名か。`name` はグローバルの名前か、`os.exe` のようなモジュール名つきの名前
pub fn is_deprecated(name: &str) -> bool {
    replacement(name).is_some()
}

fn replacement(name: &str) -> Option<&'static str> {
    DEPRECATED
        .iter()
        .find(|&&(old, _)| old == name)
        .map(|&(_, to)| to)
}

/// 警告の文面
pub fn message(old: &str, to: &str) -> String {
    format!("警告: {old} は非推奨です。{to} を使ってください")
}

/// 旧名ごとに最初の 1 回だけ true（プロセス全体で数える）
fn first_time(old: &'static str) -> bool {
    static SEEN: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut seen = SEEN
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    seen.insert(old)
}

/// 旧名が呼ばれたときに、まだ警告していなければ標準エラーへ出す
pub fn warn(old: &'static str) {
    if let Some(to) = replacement(old)
        && first_time(old)
    {
        eprintln!("{}", message(old, to));
    }
}

/// 旧名の入り口: 警告を出してから、元の関数にそのまま渡す。
/// `register("file_read", legacy!("file_read", file_read))` のように使う
macro_rules! legacy {
    ($old:literal, $f:expr) => {
        |interp, args| {
            $crate::stdlib::deprecated::warn($old);
            $f(interp, args)
        }
    };
}
pub(crate) use legacy;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_names_the_replacement() {
        assert_eq!(
            message("file_read", "std:fs の read"),
            "警告: file_read は非推奨です。std:fs の read を使ってください"
        );
    }

    #[test]
    fn warns_only_the_first_time() {
        assert!(first_time("__test_old_name"));
        assert!(!first_time("__test_old_name"));
        assert!(first_time("__test_other_name"));
    }

    #[test]
    fn table_has_no_duplicates() {
        let names: HashSet<_> = DEPRECATED.iter().map(|&(old, _)| old).collect();
        assert_eq!(names.len(), DEPRECATED.len());
    }

    #[test]
    fn lookup() {
        assert!(is_deprecated("os.exe"));
        assert!(!is_deprecated("os.exit"));
    }

    /// 表の旧名も新名も、実際に登録されている（その feature を付けていないモジュールは飛ばす）
    #[test]
    fn every_entry_is_registered() {
        use crate::runtime::interp::Interpreter;
        let mut out = Vec::new();
        let mut interp = Interpreter::new(&mut out, std::path::PathBuf::from("."));
        crate::stdlib::register(&mut interp);
        let modules = interp.std_modules();
        let has_member = |module: &str, member: &str| {
            modules
                .iter()
                .find(|(name, _)| *name == module)
                .map(|(_, members)| members.contains(&member))
        };
        for &(old, to) in DEPRECATED {
            match old.split_once('.') {
                None => assert!(interp.native(old).is_some(), "{old}"),
                Some((module, member)) => {
                    let found = has_member(&format!("std:{module}"), member);
                    assert_ne!(found, Some(false), "{old}");
                }
            }
            match to.split_once(" の ") {
                None => assert!(interp.native(to).is_some(), "{to}"),
                Some((module, member)) => {
                    let found = has_member(module, member);
                    assert_ne!(found, Some(false), "{to}");
                    // 旧名が新名と同じモジュールに残る場合（os.exe → os.exe_path）以外は、モジュールがある
                    if !old.contains('.') {
                        assert_eq!(found, Some(true), "{to}");
                    }
                }
            }
        }
    }
}
