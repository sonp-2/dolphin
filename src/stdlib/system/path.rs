//! `std:path`: join, dirname, basename, stem, ext, normalize, is_absolute, absolute, sep
//! （design.md 5.13）、relative, parts, with_ext（5.49）
//!
//! ファイルには触れず、文字列として組み立てる・分解する。入力はそのシステムの区切りを受け付け、
//! 出力は常に `/` 区切りにする（Windows も `/` を受け付けるので、スクリプトがシステムによらず
//! 同じ結果になる）

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crate::runtime::interp::{Const, Interpreter};
use crate::runtime::value::Value;
use crate::stdlib::{error_value, expect_args, new_array, str_arg};

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register_module_constants("std:path", &[("sep", Const::Str("/"))]);
    interp.register_module(
        "std:path",
        &[
            ("path.join", join),
            ("path.dirname", dirname),
            ("path.basename", basename),
            ("path.stem", stem),
            ("path.ext", ext),
            ("path.normalize", normalize),
            ("path.is_absolute", is_absolute),
            ("path.absolute", absolute),
            ("path.relative", relative),
            ("path.parts", parts),
            ("path.with_ext", with_ext),
        ],
    );
}

/// `/` 区切りの文字列にする。Windows 以外では、`\` は名前の一部なのでそのまま
pub(in crate::stdlib) fn to_slash(path: &Path) -> String {
    let s = path.to_string_lossy();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.into_owned()
    }
}

fn str_value(s: impl Into<String>) -> Value {
    Value::Str(s.into().into())
}

/// つなげる。あとの部分が絶対パスなら、それ以前を捨てる
fn join(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() {
        return Err("path.join の引数は 1 個以上です（0 個渡されました）".into());
    }
    let mut path = PathBuf::new();
    for i in 0..args.len() {
        path.push(str_arg("path.join", args, i)?);
    }
    Ok(str_value(to_slash(&path)))
}

/// ディレクトリ部分。`dirname["a"]` は `""`、ルートはルート自身
fn dirname(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("path.dirname", args, 1)?;
    let path = Path::new(str_arg("path.dirname", args, 0)?);
    let dir = match path.parent() {
        Some(parent) => to_slash(parent),
        // 親がないのは、ルートか空のパス
        None if path.has_root() => to_slash(path),
        None => String::new(),
    };
    Ok(str_value(dir))
}

/// 最後の名前。ルートは `""`、`a/..` は `".."`
fn basename(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("path.basename", args, 1)?;
    Ok(str_value(last_name(str_arg("path.basename", args, 0)?)))
}

fn last_name(path: &str) -> String {
    match Path::new(path).components().next_back() {
        Some(Component::Normal(name)) => name.to_string_lossy().into_owned(),
        Some(Component::ParentDir) => "..".to_string(),
        Some(Component::CurDir) => ".".to_string(),
        _ => String::new(),
    }
}

/// 拡張子を除いた最後の名前
fn stem(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("path.stem", args, 1)?;
    let name = last_name(str_arg("path.stem", args, 0)?);
    let stem = Path::new(&name).file_stem().map(OsStr::to_string_lossy);
    Ok(str_value(stem.as_deref().unwrap_or("")))
}

/// 拡張子（`"."` を含む）。なければ `""`
fn ext(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("path.ext", args, 1)?;
    let name = last_name(str_arg("path.ext", args, 0)?);
    let ext = Path::new(&name)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()));
    Ok(str_value(ext.unwrap_or_default()))
}

/// `.` と `..` を字面だけで整理する。空になったら `"."`
fn normalize_path(path: &Path) -> PathBuf {
    let mut parts: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match parts.last() {
                Some(Component::Normal(_)) => {
                    parts.pop();
                }
                // ルートより上へは行けない
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => parts.push(component),
            },
            other => parts.push(other),
        }
    }
    let result: PathBuf = parts.iter().collect();
    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

fn normalize(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("path.normalize", args, 1)?;
    let path = Path::new(str_arg("path.normalize", args, 0)?);
    Ok(str_value(to_slash(&normalize_path(path))))
}

/// ルートから始まるか（Windows の `/a` や `C:\a` も絶対パスとみなす）
fn is_absolute(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("path.is_absolute", args, 1)?;
    let path = Path::new(str_arg("path.is_absolute", args, 0)?);
    Ok(Value::Bool(path.has_root()))
}

/// カレントディレクトリを基準に絶対パスにして、字面だけで整理する
fn absolute(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("path.absolute", args, 1)?;
    let path = str_arg("path.absolute", args, 0)?;
    match std::path::absolute(path) {
        Ok(abs) => Ok(str_value(to_slash(&normalize_path(&abs)))),
        Err(e) => {
            let (kind, reason) = crate::stdlib::fs::classify(&e);
            Ok(error_value(
                kind,
                format!("`{path}` を絶対パスにできません: {reason}"),
            ))
        }
    }
}

/// 絶対パスにして、字面だけで整理する
fn absolutize(path: &str) -> Result<PathBuf, std::io::Error> {
    std::path::absolute(path).map(|abs| normalize_path(&abs))
}

/// `from` から `to` への相対パス。ファイルには触れない
fn relative(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("path.relative", args, 2)?;
    let from = str_arg("path.relative", args, 0)?;
    let to = str_arg("path.relative", args, 1)?;
    let (from_abs, to_abs) = match (absolutize(from), absolutize(to)) {
        (Ok(f), Ok(t)) => (f, t),
        (Err(e), _) | (_, Err(e)) => {
            let (kind, reason) = crate::stdlib::fs::classify(&e);
            return Ok(error_value(
                kind,
                format!("`{from}` と `{to}` を絶対パスにできません: {reason}"),
            ));
        }
    };
    Ok(relative_between(&from_abs, &to_abs).map_or_else(
        || {
            error_value(
                "invalid",
                format!("`{from}` から `{to}` への相対パスは作れません（ドライブが違います）"),
            )
        },
        str_value,
    ))
}

/// 絶対パスどうしの相対パス。ルート（ドライブ）が違えば `None`
fn relative_between(from: &Path, to: &Path) -> Option<String> {
    let a: Vec<Component<'_>> = from.components().collect();
    let b: Vec<Component<'_>> = to.components().collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return None;
    }
    let mut out: Vec<String> = vec!["..".to_string(); a.len() - common];
    out.extend(
        b[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    Some(if out.is_empty() {
        ".".to_string()
    } else {
        out.join("/")
    })
}

/// 部分の配列。ルート（ドライブつきも）は 1 つの部分
fn parts(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("path.parts", args, 1)?;
    let path = Path::new(str_arg("path.parts", args, 0)?);
    Ok(new_array(
        split_parts(path).into_iter().map(str_value).collect(),
    ))
}

fn split_parts(path: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut prefix: Option<String> = None;
    for component in path.components() {
        match component {
            Component::Prefix(p) => prefix = Some(to_slash(Path::new(p.as_os_str()))),
            Component::RootDir => out.push(format!("{}/", prefix.take().unwrap_or_default())),
            other => {
                out.extend(prefix.take());
                out.push(other.as_os_str().to_string_lossy().into_owned());
            }
        }
    }
    out.extend(prefix);
    out
}

/// 最後の名前の拡張子を置き換える。最後の名前がないパスはそのまま
fn with_ext(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("path.with_ext", args, 2)?;
    let p = str_arg("path.with_ext", args, 0)?;
    let ext = str_arg("path.with_ext", args, 1)?;
    if ext.contains(['/', '\\', '\0']) {
        return Err("path.with_ext の拡張子に区切り文字や NUL は使えません".into());
    }
    let ext = ext.strip_prefix('.').unwrap_or(ext);
    let path = Path::new(p);
    if !matches!(path.components().next_back(), Some(Component::Normal(_))) {
        return Ok(str_value(p));
    }
    Ok(str_value(to_slash(&path.with_extension(ext))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(s: &str) -> String {
        to_slash(&normalize_path(Path::new(s)))
    }

    #[test]
    fn normalize_cases() {
        assert_eq!(norm("a/./b//c"), "a/b/c");
        assert_eq!(norm("a/b/../c"), "a/c");
        assert_eq!(norm("a/.."), ".");
        assert_eq!(norm(""), ".");
        assert_eq!(norm("../a"), "../a");
        assert_eq!(norm("a/../.."), "..");
        assert_eq!(norm("./"), ".");
    }

    #[test]
    fn normalize_stays_below_root() {
        assert_eq!(norm("/a/../.."), "/");
        assert_eq!(norm("/.."), "/");
    }

    #[test]
    fn last_names() {
        assert_eq!(last_name("a/b/"), "b");
        assert_eq!(last_name("a"), "a");
        assert_eq!(last_name("a/.."), "..");
        assert_eq!(last_name("/"), "");
        assert_eq!(last_name(""), "");
    }
}
