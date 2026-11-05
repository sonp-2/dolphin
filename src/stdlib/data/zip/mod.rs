//! `std:zip`: gzip・zlib・zip・tar（design.md 5.51）
//!
//! `flate2`（deflate）・`zip`・`tar`（`zip` feature。どれも純 Rust）を使う。
//! 展開では、エントリ名のパストラバーサル（`..`・絶対パス・ドライブ）とリンクを必ず拒否する

mod deflate;
mod entry;
mod files;
mod limited;
mod options;
mod safe_path;
mod tar_archive;
#[cfg(test)]
mod tests;
mod zip_archive;

use std::io;

use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;
use crate::stdlib::error_value;
use crate::stdlib::fs::classify;

use deflate::{deflate, gunzip, gzip, inflate};
use tar_archive::{tar_create, tar_extract, tar_get, tar_read};
use zip_archive::{zip_create, zip_extract, zip_get, zip_read};

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register_module(
        "std:zip",
        &[
            ("zip.gzip", gzip),
            ("zip.gunzip", gunzip),
            ("zip.deflate", deflate),
            ("zip.inflate", inflate),
            ("zip.zip_read", zip_read),
            ("zip.zip_get", zip_get),
            ("zip.zip_extract", zip_extract),
            ("zip.zip_create", zip_create),
            ("zip.tar_read", tar_read),
            ("zip.tar_get", tar_get),
            ("zip.tar_extract", tar_extract),
            ("zip.tar_create", tar_create),
        ],
    );
}

/// 展開後の大きさの上限の既定（1 GiB）
const DEFAULT_MAX_SIZE: u64 = 1 << 30;
/// 圧縮の強さの既定
const DEFAULT_LEVEL: u32 = 6;
/// ディレクトリをたどる深さの上限
const MAX_WALK_DEPTH: usize = 128;

// ---- 失敗 ----

/// データやファイルのせいで失敗したとき（エラー値になる）。プログラムの間違いは `Err(String)`
struct Fail {
    kind: &'static str,
    message: String,
}

type R<T> = Result<T, Fail>;

fn fail(kind: &'static str, message: impl Into<String>) -> Fail {
    Fail {
        kind,
        message: message.into(),
    }
}

fn io_fail(action: &str, path: &str, e: &io::Error) -> Fail {
    let (kind, reason) = classify(e);
    fail(kind, format!("`{path}` を{action}: {reason}"))
}

/// 圧縮されたデータを読むときの失敗は、データが壊れているということ
fn corrupt(what: &str, e: &io::Error) -> Fail {
    fail("parse", format!("{what}が壊れています: {e}"))
}

fn done(result: R<Value>) -> Result<Value, String> {
    Ok(result.unwrap_or_else(|f| error_value(f.kind, f.message)))
}
