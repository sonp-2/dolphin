//! 書庫のエントリ 1 つの書き出しと、一覧の項目

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

use super::limited::too_large;
use super::safe_path::check_no_symlinks;
use super::{R, corrupt, io_fail};
use crate::runtime::map::Map;
use crate::runtime::value::Value;
use crate::stdlib::new_map;

/// 1 エントリを書く。`budget` は残りに書ける大きさ
pub(super) fn write_entry(
    dest: &Path,
    parts: &[String],
    is_dir: bool,
    mut data: Option<&mut dyn Read>,
    budget: &mut u64,
    max: u64,
) -> R<()> {
    if parts.is_empty() {
        return Ok(());
    }
    check_no_symlinks(dest, parts)?;
    let mut path = dest.to_path_buf();
    path.extend(parts);
    let shown = path.display().to_string();
    if is_dir {
        return fs::create_dir_all(&path).map_err(|e| io_fail("作れません", &shown, &e));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| io_fail("作れません", &parent.display().to_string(), &e))?;
    }
    let mut file = File::create(&path).map_err(|e| io_fail("作れません", &shown, &e))?;
    if let Some(reader) = data.as_mut() {
        let written = io::copy(&mut reader.take(budget.saturating_add(1)), &mut file)
            .map_err(|e| corrupt("書庫のエントリ", &e))?;
        if written > *budget {
            return Err(too_large(max));
        }
        *budget -= written;
    }
    file.flush().map_err(|e| io_fail("書けません", &shown, &e))
}

pub(super) fn entry_info(name: &str, size: u64, compressed: Option<u64>, is_dir: bool) -> Value {
    let mut map = Map::default();
    map.insert("name".into(), Value::Str(name.into()));
    map.insert("size".into(), Value::Num(size as f64));
    if let Some(c) = compressed {
        map.insert("compressed_size".into(), Value::Num(c as f64));
    }
    map.insert("is_dir".into(), Value::Bool(is_dir));
    new_map(map)
}
