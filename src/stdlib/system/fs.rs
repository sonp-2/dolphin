//! ファイル: fs.read, fs.write, fs.append, fs.exists（design.md 5.5）、
//! fs.list, fs.info, fs.mkdir, fs.remove, fs.remove_all, fs.rename, fs.copy（5.13）
//!
//! パスは実行したときのカレントディレクトリからの相対パス（`import` と違い、
//! 呼び出したファイルのあるディレクトリではない）。文字コードは UTF-8。
//! 失敗したときはエラー値を返す（design.md 8.6）。種類は `not_found` `permission` `exists` `io` `parse`

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::time::UNIX_EPOCH;

use crate::runtime::interp::Interpreter;
use crate::runtime::map::Map;
use crate::runtime::value::Value;
use crate::stdlib::deprecated::legacy;
use crate::stdlib::{bytes_arg, error_value, expect_args, new_array, new_map, str_arg};

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register_module(
        "std:fs",
        &[
            ("fs.read", file_read),
            ("fs.write", file_write),
            ("fs.append", file_append),
            ("fs.exists", file_exists),
            ("fs.read_bytes", file_read_bytes),
            ("fs.write_bytes", file_write_bytes),
            ("fs.append_bytes", file_append_bytes),
            ("fs.list", file_list),
            ("fs.info", file_info),
            ("fs.mkdir", file_mkdir),
            ("fs.remove", file_remove),
            ("fs.remove_all", file_remove_all),
            ("fs.rename", file_rename),
            ("fs.copy", file_copy),
            ("fs.walk", super::walk::file_walk),
            ("fs.glob", super::walk::file_glob),
            ("fs.temp", super::temp::file_temp),
            ("fs.temp_dir", super::temp::file_temp_dir),
            ("fs.chmod", super::temp::file_chmod),
        ],
    );
    interp.register("file_read", legacy!("file_read", file_read));
    interp.register("file_write", legacy!("file_write", file_write));
    interp.register("file_append", legacy!("file_append", file_append));
    interp.register("file_exists", legacy!("file_exists", file_exists));
    interp.register(
        "file_read_bytes",
        legacy!("file_read_bytes", file_read_bytes),
    );
    interp.register(
        "file_write_bytes",
        legacy!("file_write_bytes", file_write_bytes),
    );
    interp.register(
        "file_append_bytes",
        legacy!("file_append_bytes", file_append_bytes),
    );
    interp.register("file_list", legacy!("file_list", file_list));
    interp.register("file_info", legacy!("file_info", file_info));
    interp.register("file_mkdir", legacy!("file_mkdir", file_mkdir));
    interp.register("file_remove", legacy!("file_remove", file_remove));
    interp.register(
        "file_remove_all",
        legacy!("file_remove_all", file_remove_all),
    );
    interp.register("file_rename", legacy!("file_rename", file_rename));
    interp.register("file_copy", legacy!("file_copy", file_copy));
    interp.register("file_walk", legacy!("file_walk", super::walk::file_walk));
    interp.register("file_glob", legacy!("file_glob", super::walk::file_glob));
    interp.register("file_temp", legacy!("file_temp", super::temp::file_temp));
    interp.register(
        "file_temp_dir",
        legacy!("file_temp_dir", super::temp::file_temp_dir),
    );
    interp.register("file_chmod", legacy!("file_chmod", super::temp::file_chmod));
}

/// ファイル全体を文字列で返す
pub(super) fn file_read(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("fs.read", args, 1)?;
    let path = str_arg("fs.read", args, 0)?;
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => return Ok(io_error("読み込めません", path, &e)),
    };
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Value::Str(text.into())),
        Err(_) => Ok(error_value(
            "parse",
            format!("`{path}` を読み込めません: UTF-8 として読めません"),
        )),
    }
}

/// 文字列を書き込む。ファイルがなければ作り、あれば中身を置き換える
pub(super) fn file_write(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("fs.write", args, 2)?;
    let path = str_arg("fs.write", args, 0)?;
    let text = str_arg("fs.write", args, 1)?;
    match fs::write(path, text) {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Ok(io_error("書き込めません", path, &e)),
    }
}

/// 文字列をファイルの末尾に足す。ファイルがなければ作る
pub(super) fn file_append(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("fs.append", args, 2)?;
    let path = str_arg("fs.append", args, 0)?;
    let text = str_arg("fs.append", args, 1)?;
    let result = OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()));
    match result {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Ok(io_error("書き込めません", path, &e)),
    }
}

/// ファイルかディレクトリがあるか
pub(super) fn file_exists(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("fs.exists", args, 1)?;
    let path = str_arg("fs.exists", args, 0)?;
    Ok(Value::Bool(fs::exists(path).unwrap_or(false)))
}

/// OS の言語に左右されないよう、よくある原因は固定の文言にする
pub(in crate::stdlib) fn io_error(action: &str, path: &str, e: &io::Error) -> Value {
    let (kind, reason) = classify(e);
    error_value(kind, format!("`{path}` を{action}: {reason}"))
}

/// 2 つのパスを使う操作（名前の変更・コピー）の失敗
fn io_error2(action: &str, from: &str, to: &str, e: &io::Error) -> Value {
    let (kind, reason) = classify(e);
    error_value(kind, format!("`{from}` を `{to}` に{action}: {reason}"))
}

/// エラーの種類（`kind`）と、メッセージの理由
pub(in crate::stdlib) fn classify(e: &io::Error) -> (&'static str, String) {
    match e.kind() {
        io::ErrorKind::NotFound => ("not_found", "ファイルが見つかりません".to_string()),
        io::ErrorKind::PermissionDenied => ("permission", "権限がありません".to_string()),
        io::ErrorKind::AlreadyExists => ("exists", "すでにあります".to_string()),
        io::ErrorKind::DirectoryNotEmpty => ("io", "ディレクトリが空ではありません".to_string()),
        io::ErrorKind::NotADirectory => ("io", "ディレクトリではありません".to_string()),
        io::ErrorKind::IsADirectory => ("io", "ディレクトリです".to_string()),
        _ => ("io", e.to_string()),
    }
}

/// ファイル全体をバイト列で返す
pub(super) fn file_read_bytes(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("fs.read_bytes", args, 1)?;
    let path = str_arg("fs.read_bytes", args, 0)?;
    Ok(match fs::read(path) {
        Ok(bytes) => Value::bytes(bytes),
        Err(e) => io_error("読み込めません", path, &e),
    })
}

/// バイト列を書き込む。ファイルがなければ作り、あれば中身を置き換える
pub(super) fn file_write_bytes(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("fs.write_bytes", args, 2)?;
    let path = str_arg("fs.write_bytes", args, 0)?;
    let bytes = bytes_arg("fs.write_bytes", args, 1)?;
    Ok(match fs::write(path, &bytes[..]) {
        Ok(()) => Value::Nil,
        Err(e) => io_error("書き込めません", path, &e),
    })
}

/// バイト列をファイルの末尾に足す。ファイルがなければ作る
pub(super) fn file_append_bytes(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("fs.append_bytes", args, 2)?;
    let path = str_arg("fs.append_bytes", args, 0)?;
    let bytes = bytes_arg("fs.append_bytes", args, 1)?;
    let result = OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .and_then(|mut file| file.write_all(bytes));
    Ok(match result {
        Ok(()) => Value::Nil,
        Err(e) => io_error("書き込めません", path, &e),
    })
}

/// ディレクトリの中の名前を、文字コード順に並べた配列
pub(super) fn file_list(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("fs.list", args, 1)?;
    let path = str_arg("fs.list", args, 0)?;
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) => return Ok(io_error("開けません", path, &e)),
    };
    let mut names = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => names.push(entry.file_name().to_string_lossy().into_owned()),
            Err(e) => return Ok(io_error("読み込めません", path, &e)),
        }
    }
    names.sort_unstable();
    Ok(new_array(
        names.into_iter().map(|n| Value::Str(n.into())).collect(),
    ))
}

/// `{"is_file", "is_dir", "size", "modified"}`。シンボリックリンクはたどる
pub(super) fn file_info(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("fs.info", args, 1)?;
    let path = str_arg("fs.info", args, 0)?;
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) => return Ok(io_error("調べられません", path, &e)),
    };
    // 1970 年より前なら負の数。取れない環境では nil
    let modified = match meta.modified() {
        Ok(time) => match time.duration_since(UNIX_EPOCH) {
            Ok(after) => Value::Num(after.as_secs_f64()),
            Err(before) => Value::Num(-before.duration().as_secs_f64()),
        },
        Err(_) => Value::Nil,
    };
    let mut info = Map::default();
    info.insert("is_file".into(), Value::Bool(meta.is_file()));
    info.insert("is_dir".into(), Value::Bool(meta.is_dir()));
    info.insert("size".into(), Value::Num(meta.len() as f64));
    info.insert("modified".into(), modified);
    Ok(new_map(info))
}

/// ディレクトリを作る。途中のディレクトリもまとめて作り、すでにあれば何もしない
pub(super) fn file_mkdir(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("fs.mkdir", args, 1)?;
    let path = str_arg("fs.mkdir", args, 0)?;
    match fs::create_dir_all(path) {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Ok(io_error("作れません", path, &e)),
    }
}

/// ファイルか空のディレクトリを消す
pub(super) fn file_remove(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("fs.remove", args, 1)?;
    let path = str_arg("fs.remove", args, 0)?;
    let result = fs::symlink_metadata(path).and_then(|meta| {
        if meta.is_dir() {
            fs::remove_dir(path)
        } else {
            fs::remove_file(path)
        }
    });
    match result {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Ok(io_error("消せません", path, &e)),
    }
}

/// ディレクトリを中身ごと消す。ファイルも消せる。リンクは、リンク先をたどらずにリンクだけを消す
pub(super) fn file_remove_all(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("fs.remove_all", args, 1)?;
    let path = str_arg("fs.remove_all", args, 0)?;
    let result = fs::symlink_metadata(path).and_then(|meta| {
        if meta.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        }
    });
    match result {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Ok(io_error("消せません", path, &e)),
    }
}

/// 名前を変える・移す。`to` が既にあるファイルなら置き換える
pub(super) fn file_rename(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("fs.rename", args, 2)?;
    let from = str_arg("fs.rename", args, 0)?;
    let to = str_arg("fs.rename", args, 1)?;
    match fs::rename(from, to) {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Ok(io_error2("移せません", from, to, &e)),
    }
}

/// ファイルをコピーする（ディレクトリは不可）。`to` が既にあれば置き換える
pub(super) fn file_copy(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("fs.copy", args, 2)?;
    let from = str_arg("fs.copy", args, 0)?;
    let to = str_arg("fs.copy", args, 1)?;
    match fs::copy(from, to) {
        Ok(_) => Ok(Value::Nil),
        Err(e) => Ok(io_error2("コピーできません", from, to, &e)),
    }
}
