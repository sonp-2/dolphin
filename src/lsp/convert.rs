//! LSP の JSON の組み立て、位置（UTF-16）・URI の変換

use std::path::{Path, PathBuf};

use super::json::Json;
use crate::diag::Diagnostic;

pub(super) fn publish(uri: &str, diagnostics: Vec<Json>) -> Json {
    Json::obj([
        ("jsonrpc", Json::str("2.0")),
        ("method", Json::str("textDocument/publishDiagnostics")),
        (
            "params",
            Json::obj([
                ("uri", Json::str(uri)),
                ("diagnostics", Json::Arr(diagnostics)),
            ]),
        ),
    ])
}

pub(super) fn diagnostic_json(src: &str, diag: &Diagnostic) -> Json {
    // import したファイルで起きたエラーは、このファイルの位置を持たないので先頭に出す
    let (line, col, len) = if diag.origin.is_some() {
        (1, 1, 0)
    } else {
        let text = src.lines().nth(diag.span.line - 1).unwrap_or("");
        (diag.span.line, diag.span.col, word_len(text, diag.span.col))
    };
    let mut message = diag.message.clone();
    for note in &diag.notes {
        message += &format!("\n{note}");
    }
    Json::obj([
        ("range", range_json(src, line, col, len.max(1))),
        ("severity", Json::num(1)),
        ("source", Json::str("dolphin")),
        ("message", Json::Str(message)),
    ])
}

/// `col`（1 始まり）から始まる単語（`@名前` や名前）の文字数。単語でなければ 1
pub(super) fn word_len(line: &str, col: usize) -> usize {
    let rest: Vec<char> = line.chars().skip(col - 1).collect();
    let skip = usize::from(rest.first() == Some(&'@'));
    let n = rest[skip..]
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
        .count();
    if n == 0 { 1 } else { n + skip }
}

pub(super) fn position(line0: usize, units: usize) -> Json {
    Json::obj([("line", Json::num(line0)), ("character", Json::num(units))])
}

/// 1 始まりの行・文字から `len` 文字の範囲
pub(super) fn range_json(src: &str, line: usize, col: usize, len: usize) -> Json {
    let text = src.lines().nth(line.saturating_sub(1)).unwrap_or("");
    let units = |upto: usize| text.chars().take(upto).map(char::len_utf16).sum::<usize>();
    Json::obj([
        ("start", position(line - 1, units(col - 1))),
        ("end", position(line - 1, units(col - 1 + len))),
    ])
}

/// LSP の UTF-16 の位置を、1 始まりの文字の列にする
pub(super) fn utf16_to_col(line: &str, units: usize) -> usize {
    let mut used = 0;
    for (i, c) in line.chars().enumerate() {
        if used >= units {
            return i + 1;
        }
        used += c.len_utf16();
    }
    line.chars().count() + 1
}

// ---- URI ----

pub(super) fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::new();
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            let hex = std::str::from_utf8(&raw[i + 1..i + 3]).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    // Windows: `/c:/dir/file` → `c:/dir/file`
    let b = path.as_bytes();
    let path = if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        path[1..].to_string()
    } else {
        path
    };
    Some(PathBuf::from(path))
}

pub(super) fn path_to_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    let mut uri = String::from("file://");
    if !text.starts_with('/') {
        uri.push('/');
    }
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~' | b':') {
            uri.push(b as char);
        } else {
            uri += &format!("%{b:02X}");
        }
    }
    uri
}
