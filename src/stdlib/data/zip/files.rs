//! 作る側の `files`（パスの配列か、エントリ名 → 中身のマップ）

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::safe_path::safe_components;
use super::{MAX_WALK_DEPTH, R, fail, io_fail};
use crate::runtime::value::Value;
use crate::stdlib::type_error;

pub(super) enum Source {
    Dir,
    Mem(Vec<u8>),
    Disk(PathBuf),
}

pub(super) struct Item {
    pub(super) name: String,
    pub(super) source: Source,
}

/// `files`（パスの配列か、エントリ名 → 中身のマップ）を、書庫に入れるものの一覧にする。
/// 名前の間違いなどは `Err(String)`（実行時エラー）、ファイルがないなどは `Ok(Err(..))`
pub(super) fn gather(name: &str, files: &Value) -> Result<R<Vec<Item>>, String> {
    let bad_name = |n: &str, why: &str| {
        format!("{name} のエントリ名 `{n}` は使えません（{why}。相対パスだけです）")
    };
    let mut items = Vec::new();
    match files {
        Value::Array(paths) => {
            for path in paths.borrow().iter() {
                let Value::Str(path) = path else {
                    return Err(format!(
                        "{name} の `files` の配列には、パスの文字列を入れます（{}が入っています）",
                        path.type_name()
                    ));
                };
                let parts = safe_components(path).map_err(|why| bad_name(path, &why))?;
                if parts.is_empty() {
                    return Err(bad_name(path, "名前が残りません"));
                }
                if let Err(f) = walk(Path::new(&**path), &parts.join("/"), &mut items, 0) {
                    return Ok(Err(f));
                }
            }
        }
        Value::Map(map) => {
            for (key, value) in map.borrow().iter() {
                let parts = safe_components(key).map_err(|why| bad_name(key, &why))?;
                if parts.is_empty() {
                    return Err(bad_name(key, "名前が残りません"));
                }
                let data = match value {
                    Value::Str(s) => s.as_bytes().to_vec(),
                    Value::Bytes(b) => b.to_vec(),
                    other => {
                        return Err(format!(
                            "{name} の `files` のマップの値は、バイト列か文字列です（`{key}` は{}）",
                            other.type_name()
                        ));
                    }
                };
                items.push(Item {
                    name: parts.join("/"),
                    source: Source::Mem(data),
                });
            }
        }
        other => return Err(type_error(name, 1, "配列かマップ", other)),
    }
    Ok(Ok(items))
}

/// ディスクのファイルかディレクトリを、`name` という名前で `items` に足す（ディレクトリは再帰）
fn walk(path: &Path, name: &str, items: &mut Vec<Item>, depth: usize) -> R<()> {
    let shown = path.display().to_string();
    let meta = fs::symlink_metadata(path).map_err(|e| io_fail("調べられません", &shown, &e))?;
    if meta.file_type().is_symlink() {
        return Err(fail(
            "unsupported",
            format!("`{shown}` はシンボリックリンクなので入れられません"),
        ));
    }
    if meta.is_dir() {
        if depth >= MAX_WALK_DEPTH {
            return Err(fail("unsupported", format!("`{shown}` は深すぎます")));
        }
        items.push(Item {
            name: name.to_string(),
            source: Source::Dir,
        });
        let mut children: Vec<_> = fs::read_dir(path)
            .and_then(|rd| rd.collect::<io::Result<Vec<_>>>())
            .map_err(|e| io_fail("読み込めません", &shown, &e))?
            .into_iter()
            .map(|e| e.file_name())
            .collect();
        children.sort();
        for child in children {
            let child_name = format!("{name}/{}", child.to_string_lossy());
            walk(&path.join(&child), &child_name, items, depth + 1)?;
        }
    } else if meta.is_file() {
        items.push(Item {
            name: name.to_string(),
            source: Source::Disk(path.to_path_buf()),
        });
    } else {
        return Err(fail(
            "unsupported",
            format!("`{shown}` は通常のファイルでもディレクトリでもありません"),
        ));
    }
    Ok(())
}
