//! `file_temp`、`file_temp_dir`、`file_chmod`（design.md 5.49）

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;
use crate::stdlib::fs::io_error;
use crate::stdlib::path::to_slash;
use crate::stdlib::{error_value, expect_args, num_arg, str_arg, type_error};

pub fn register(interp: &mut Interpreter<'_>) {
    interp.register("file_temp", file_temp);
    interp.register("file_temp_dir", file_temp_dir);
    interp.register("file_chmod", file_chmod);
}

/// `file_temp` `file_temp_dir` の `options`
struct TempOptions {
    prefix: String,
    suffix: String,
    dir: Option<String>,
}

fn temp_options(name: &str, args: &[Value]) -> Result<TempOptions, String> {
    let mut options = TempOptions {
        prefix: "tmp".to_string(),
        suffix: String::new(),
        dir: None,
    };
    if args.len() > 1 {
        return Err(format!(
            "{name} の引数は 0 個か 1 個です（{} 個渡されました）",
            args.len()
        ));
    }
    let Some(value) = args.first() else {
        return Ok(options);
    };
    let Value::Map(map) = value else {
        return Err(type_error(name, 0, "マップ", value));
    };
    let map = map.borrow();
    const KNOWN: [&str; 3] = ["prefix", "suffix", "dir"];
    for (key, _) in map.iter() {
        if !KNOWN.contains(&&**key) {
            return Err(format!(
                "{name} に知らないオプション `{key}` があります（使えるのは {}）",
                KNOWN.join("、")
            ));
        }
    }
    let text = |key: &str| -> Result<Option<String>, String> {
        match map.get(key) {
            None => Ok(None),
            Some(Value::Str(s)) => Ok(Some(s.to_string())),
            Some(other) => Err(format!(
                "{name} の `{key}` は文字列です（{}が渡されました）",
                other.type_name()
            )),
        }
    };
    for key in ["prefix", "suffix"] {
        if let Some(s) = text(key)? {
            if s.contains(['/', '\\', '\0']) {
                return Err(format!("{name} の `{key}` に区切り文字や NUL は使えません"));
            }
            if key == "prefix" {
                options.prefix = s;
            } else {
                options.suffix = s;
            }
        }
    }
    options.dir = text("dir")?;
    Ok(options)
}

/// 呼ぶたびに違う 16 進数 16 桁（時刻・プロセス番号・呼んだ回数・プロセスごとの乱数を混ぜる）
fn unique_hex() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    hasher.write_u32(std::process::id());
    if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
        hasher.write_u128(now.as_nanos());
    }
    format!("{:016x}", hasher.finish())
}

/// 一意な名前で作る。名前がぶつかったら別の名前でやり直す
fn make_temp(name: &str, args: &[Value], is_dir: bool) -> Result<Value, String> {
    let options = temp_options(name, args)?;
    let base = match &options.dir {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir(),
    };
    let shown = options
        .dir
        .clone()
        .unwrap_or_else(|| to_slash(&std::env::temp_dir()));
    for _ in 0..100 {
        let file_name = format!("{}{}{}", options.prefix, unique_hex(), options.suffix);
        let path = base.join(&file_name);
        let result = if is_dir {
            create_private_dir(&path)
        } else {
            create_private_file(&path)
        };
        match result {
            Ok(()) => return Ok(Value::Str(to_slash(&path).into())),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Ok(io_error("作れません", &shown, &e)),
        }
    }
    Ok(error_value(
        "exists",
        format!("`{shown}` に一意な名前を作れませんでした"),
    ))
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map(drop)
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> io::Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(drop)
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

/// 一意な名前の空のファイルを作って、パスを返す
fn file_temp(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    make_temp("file_temp", args, false)
}

/// 一意な名前の空のディレクトリを作って、パスを返す
fn file_temp_dir(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    make_temp("file_temp_dir", args, true)
}

/// 権限（モード）を変える。Unix だけ。Windows は `unsupported` のエラー値
fn file_chmod(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("file_chmod", args, 2)?;
    let path = str_arg("file_chmod", args, 0)?;
    let mode = num_arg("file_chmod", args, 1)?;
    if mode.fract() != 0.0 || !(0.0..=4095.0).contains(&mode) {
        return Err(format!(
            "file_chmod のモードは 0〜0o7777 の整数です（{mode}）"
        ));
    }
    chmod(path, mode as u32)
}

#[cfg(unix)]
fn chmod(path: &str, mode: u32) -> Result<Value, String> {
    use std::os::unix::fs::PermissionsExt;
    Ok(
        match fs::set_permissions(path, fs::Permissions::from_mode(mode)) {
            Ok(()) => Value::Nil,
            Err(e) => io_error("権限を変えられません", path, &e),
        },
    )
}

#[cfg(not(unix))]
fn chmod(path: &str, _mode: u32) -> Result<Value, String> {
    Ok(error_value(
        "unsupported",
        format!("`{path}` の権限を変えられません: この OS では file_chmod を使えません"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names_differ() {
        let names: std::collections::HashSet<String> = (0..1000).map(|_| unique_hex()).collect();
        assert_eq!(names.len(), 1000);
        assert!(names.iter().all(|n| n.len() == 16));
    }

    #[test]
    fn creates_and_removes() {
        let dir = std::env::temp_dir();
        let file = dir.join(format!("dolphin_unit_{}", unique_hex()));
        create_private_file(&file).unwrap();
        assert!(create_private_file(&file).is_err());
        fs::remove_file(&file).unwrap();
        let sub = dir.join(format!("dolphin_unit_{}", unique_hex()));
        create_private_dir(&sub).unwrap();
        fs::remove_dir(&sub).unwrap();
    }
}
