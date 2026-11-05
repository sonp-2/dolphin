//! `std:zip`: gzip・zlib・zip・tar（design.md 5.51）
//!
//! `flate2`（deflate）・`zip`・`tar`（`zip` feature。どれも純 Rust）を使う。
//! 展開では、エントリ名のパストラバーサル（`..`・絶対パス・ドライブ）とリンクを必ず拒否する

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::{MultiGzDecoder, ZlibDecoder};
use flate2::write::{GzEncoder, ZlibEncoder};
use tar::EntryType;
use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::runtime::interp::Interpreter;
use crate::runtime::map::Map;
use crate::runtime::value::Value;
use crate::stdlib::fs::classify;
use crate::stdlib::{bytes_arg, data_arg, error_value, new_array, new_map, str_arg, type_error};

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

fn zip_fail(path: &str, e: ZipError) -> Fail {
    match e {
        ZipError::Io(e) => io_fail("読み書きできません", path, &e),
        ZipError::FileNotFound => fail("not_found", "エントリがありません"),
        ZipError::UnsupportedArchive(why) => fail(
            "unsupported",
            format!("`{path}` は扱えない zip です: {why}"),
        ),
        ZipError::CompressionMethodNotSupported(m) => fail(
            "unsupported",
            format!("`{path}` は対応しない圧縮方式（{m}）を使っています"),
        ),
        ZipError::InvalidPassword => fail(
            "unsupported",
            format!("`{path}` には暗号化されたエントリがあります（パスワードには対応しません）"),
        ),
        e => fail("parse", format!("`{path}` は壊れた zip です: {e}")),
    }
}

fn done(result: R<Value>) -> Result<Value, String> {
    Ok(result.unwrap_or_else(|f| error_value(f.kind, f.message)))
}

// ---- オプション ----

struct Opts {
    level: u32,
    gzip: bool,
    max_size: u64,
}

/// 第 `n` 引数までが必須で、その次にオプションのマップを 1 つ取れる。`allowed` 以外のキーは間違い
fn options(name: &str, args: &[Value], n: usize, allowed: &[&str]) -> Result<Opts, String> {
    if args.len() != n && args.len() != n + 1 {
        return Err(format!(
            "{name} の引数は {n} 個か {} 個です（{} 個渡されました）",
            n + 1,
            args.len()
        ));
    }
    let mut opts = Opts {
        level: DEFAULT_LEVEL,
        gzip: false,
        max_size: DEFAULT_MAX_SIZE,
    };
    let Some(options) = args.get(n) else {
        return Ok(opts);
    };
    let Value::Map(options) = options else {
        return Err(type_error(name, n, "マップ", options));
    };
    for (key, value) in options.borrow().iter() {
        if !allowed.contains(&&**key) {
            return Err(format!("{name} のオプション `{key}` は知りません"));
        }
        match (&**key, value) {
            ("level", Value::Num(x)) if x.fract() == 0.0 && (0.0..=9.0).contains(x) => {
                opts.level = *x as u32;
            }
            ("level", Value::Num(x)) => {
                return Err(format!("{name} の `level` は 0〜9 の整数です（{x}）"));
            }
            ("gzip", Value::Bool(b)) => opts.gzip = *b,
            ("max_size", Value::Num(x)) if x.is_finite() && *x >= 0.0 => {
                opts.max_size = *x as u64;
            }
            ("max_size", Value::Int(x)) if *x >= 0 => opts.max_size = *x as u64,
            ("max_size", Value::Num(_) | Value::Int(_)) => {
                return Err(format!("{name} の `max_size` は 0 以上の数です"));
            }
            (_, other) => {
                let expected = match &**key {
                    "gzip" => "真偽値",
                    _ => "数値",
                };
                return Err(format!(
                    "{name} のオプション `{key}` は{expected}です（{}が渡されました）",
                    other.type_name()
                ));
            }
        }
    }
    Ok(opts)
}

// ---- gzip / zlib ----

fn gzip(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let opts = options("zip.gzip", args, 1, &["level"])?;
    let data = data_arg("zip.gzip", args, 0)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(opts.level));
    let out = encoder
        .write_all(&data)
        .and_then(|()| encoder.finish())
        .map_err(|e| format!("zip.gzip: {e}"))?;
    Ok(Value::bytes(out))
}

fn deflate(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let opts = options("zip.deflate", args, 1, &["level"])?;
    let data = data_arg("zip.deflate", args, 0)?;
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(opts.level));
    let out = encoder
        .write_all(&data)
        .and_then(|()| encoder.finish())
        .map_err(|e| format!("zip.deflate: {e}"))?;
    Ok(Value::bytes(out))
}

fn gunzip(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let opts = options("zip.gunzip", args, 1, &["max_size"])?;
    let bytes = bytes_arg("zip.gunzip", args, 0)?;
    done(
        read_limited(MultiGzDecoder::new(&bytes[..]), opts.max_size)
            .map(Value::bytes)
            .map_err(|e| match e {
                Limited::TooLarge(f) => f,
                Limited::Io(e) => corrupt("gzip のデータ", &e),
            }),
    )
}

fn inflate(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let opts = options("zip.inflate", args, 1, &["max_size"])?;
    let bytes = bytes_arg("zip.inflate", args, 0)?;
    done(
        read_limited(ZlibDecoder::new(&bytes[..]), opts.max_size)
            .map(Value::bytes)
            .map_err(|e| match e {
                Limited::TooLarge(f) => f,
                Limited::Io(e) => corrupt("zlib のデータ", &e),
            }),
    )
}

enum Limited {
    TooLarge(Fail),
    Io(io::Error),
}

fn too_large(max: u64) -> Fail {
    fail(
        "too_large",
        format!("展開すると上限（{max} バイト）を超えます"),
    )
}

/// `max` バイトまで読む。超えたら `TooLarge`
fn read_limited(reader: impl Read, max: u64) -> Result<Vec<u8>, Limited> {
    let mut out = Vec::new();
    reader
        .take(max.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(Limited::Io)?;
    if out.len() as u64 > max {
        return Err(Limited::TooLarge(too_large(max)));
    }
    Ok(out)
}

// ---- エントリ名の検査 ----

/// 書庫のエントリ名を、安全な相対パスの成分に分ける。`\` も区切りとして扱う。
/// 絶対パス・ドライブ・`..`・`:` を含む成分・NUL・空の名前は `Err`。`.` と空の成分は捨てる
/// （`./` のように成分が残らないこともある）
fn safe_components(name: &str) -> Result<Vec<String>, String> {
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

fn unsafe_entry(name: &str, why: &str) -> Fail {
    fail(
        "unsafe_path",
        format!("危険なエントリ名 `{name}`（{why}）のため展開しません"),
    )
}

/// 展開先の途中にシンボリックリンクがあれば、外へ書き出せてしまうので拒否する
fn check_no_symlinks(dest: &Path, parts: &[String]) -> R<()> {
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

/// 1 エントリを書く。`budget` は残りに書ける大きさ
fn write_entry(
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

fn entry_info(name: &str, size: u64, compressed: Option<u64>, is_dir: bool) -> Value {
    let mut map = Map::default();
    map.insert("name".into(), Value::Str(name.into()));
    map.insert("size".into(), Value::Num(size as f64));
    if let Some(c) = compressed {
        map.insert("compressed_size".into(), Value::Num(c as f64));
    }
    map.insert("is_dir".into(), Value::Bool(is_dir));
    new_map(map)
}

// ---- zip ----

fn open_zip(path: &str) -> R<ZipArchive<BufReader<File>>> {
    let file = File::open(path).map_err(|e| io_fail("開けません", path, &e))?;
    ZipArchive::new(BufReader::new(file)).map_err(|e| zip_fail(path, e))
}

fn zip_read(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    options("zip.zip_read", args, 1, &[])?;
    let path = str_arg("zip.zip_read", args, 0)?;
    done((|| {
        let mut archive = open_zip(path)?;
        let mut items = Vec::new();
        for i in 0..archive.len() {
            let entry = archive.by_index_raw(i).map_err(|e| zip_fail(path, e))?;
            let name = entry.name().map_err(|e| zip_fail(path, e))?.into_owned();
            items.push(entry_info(
                &name,
                entry.size(),
                Some(entry.compressed_size()),
                entry.is_dir(),
            ));
        }
        Ok(new_array(items))
    })())
}

fn zip_get(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let opts = options("zip.zip_get", args, 2, &["max_size"])?;
    let path = str_arg("zip.zip_get", args, 0)?;
    let name = str_arg("zip.zip_get", args, 1)?;
    done((|| {
        let mut archive = open_zip(path)?;
        let entry = match archive.by_name(name) {
            Ok(entry) => entry,
            Err(ZipError::FileNotFound) => {
                return Err(fail("not_found", format!("エントリ `{name}` がありません")));
            }
            Err(e) => return Err(zip_fail(path, e)),
        };
        if entry.is_dir() {
            return Err(fail("io", format!("`{name}` はディレクトリです")));
        }
        read_limited(entry, opts.max_size)
            .map(Value::bytes)
            .map_err(|e| match e {
                Limited::TooLarge(f) => f,
                Limited::Io(e) => corrupt("zip のエントリ", &e),
            })
    })())
}

fn zip_extract(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let opts = options("zip.zip_extract", args, 2, &["max_size"])?;
    let path = str_arg("zip.zip_extract", args, 0)?;
    let dest = str_arg("zip.zip_extract", args, 1)?;
    done(extract_zip(path, dest, &opts))
}

fn extract_zip(path: &str, dest: &str, opts: &Opts) -> R<Value> {
    (|| {
        let mut archive = open_zip(path)?;
        // 先に全部を検査して、危険なものがあれば何も書かない
        let mut total: u64 = 0;
        for i in 0..archive.len() {
            let entry = archive.by_index_raw(i).map_err(|e| zip_fail(path, e))?;
            let name = entry.name().map_err(|e| zip_fail(path, e))?.into_owned();
            safe_components(&name).map_err(|why| unsafe_entry(&name, &why))?;
            if entry.is_symlink() {
                return Err(unsafe_entry(&name, "シンボリックリンク"));
            }
            total = total.saturating_add(entry.size());
            if total > opts.max_size {
                return Err(too_large(opts.max_size));
            }
        }
        fs::create_dir_all(dest).map_err(|e| io_fail("作れません", dest, &e))?;
        let dest = Path::new(dest);
        let mut budget = opts.max_size;
        let mut written = Vec::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| zip_fail(path, e))?;
            let name = entry.name().map_err(|e| zip_fail(path, e))?.into_owned();
            let parts = safe_components(&name).map_err(|why| unsafe_entry(&name, &why))?;
            let is_dir = entry.is_dir();
            write_entry(
                dest,
                &parts,
                is_dir,
                Some(&mut entry),
                &mut budget,
                opts.max_size,
            )?;
            if !is_dir && !parts.is_empty() {
                written.push(Value::Str(parts.join("/").into()));
            }
        }
        Ok(new_array(written))
    })()
}

fn zip_create(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let opts = options("zip.zip_create", args, 2, &["level"])?;
    let path = str_arg("zip.zip_create", args, 0)?;
    let items = match gather("zip.zip_create", &args[1])? {
        Ok(items) => items,
        Err(f) => return done(Err(f)),
    };
    done((|| {
        let file = File::create(path).map_err(|e| io_fail("作れません", path, &e))?;
        let mut writer = ZipWriter::new(BufWriter::new(file));
        let file_options = SimpleFileOptions::default()
            .compression_method(if opts.level == 0 {
                CompressionMethod::Stored
            } else {
                CompressionMethod::Deflated
            })
            .compression_level((opts.level > 0).then_some(i64::from(opts.level)))
            .unix_permissions(0o644);
        let dir_options = SimpleFileOptions::default().unix_permissions(0o755);
        for item in &items {
            match &item.source {
                Source::Dir => writer
                    .add_directory(item.name.as_str(), dir_options)
                    .map_err(|e| zip_fail(path, e))?,
                Source::Mem(data) => {
                    writer
                        .start_file(item.name.as_str(), file_options)
                        .map_err(|e| zip_fail(path, e))?;
                    writer
                        .write_all(data)
                        .map_err(|e| io_fail("書けません", path, &e))?;
                }
                Source::Disk(src) => {
                    let shown = src.display().to_string();
                    let mut input =
                        File::open(src).map_err(|e| io_fail("開けません", &shown, &e))?;
                    writer
                        .start_file(item.name.as_str(), file_options)
                        .map_err(|e| zip_fail(path, e))?;
                    io::copy(&mut input, &mut writer)
                        .map_err(|e| io_fail("書けません", path, &e))?;
                }
            }
        }
        let mut out = writer.finish().map_err(|e| zip_fail(path, e))?;
        out.flush().map_err(|e| io_fail("書けません", path, &e))?;
        Ok(Value::Nil)
    })())
}

// ---- 作る側の `files` ----

enum Source {
    Dir,
    Mem(Vec<u8>),
    Disk(PathBuf),
}

struct Item {
    name: String,
    source: Source,
}

/// `files`（パスの配列か、エントリ名 → 中身のマップ）を、書庫に入れるものの一覧にする。
/// 名前の間違いなどは `Err(String)`（実行時エラー）、ファイルがないなどは `Ok(Err(..))`
fn gather(name: &str, files: &Value) -> Result<R<Vec<Item>>, String> {
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

// ---- tar ----

/// tar を開く。先頭が gzip なら自動で展開する
fn open_tar(path: &str) -> R<tar::Archive<Box<dyn Read>>> {
    let file = File::open(path).map_err(|e| io_fail("開けません", path, &e))?;
    let mut reader = BufReader::new(file);
    let gz = reader
        .fill_buf()
        .map_err(|e| io_fail("読み込めません", path, &e))?
        .starts_with(&[0x1f, 0x8b]);
    let reader: Box<dyn Read> = if gz {
        Box::new(MultiGzDecoder::new(reader))
    } else {
        Box::new(reader)
    };
    Ok(tar::Archive::new(reader))
}

fn tar_name(entry: &tar::Entry<'_, Box<dyn Read>>) -> String {
    String::from_utf8_lossy(&entry.path_bytes()).into_owned()
}

fn tar_stream(e: &io::Error) -> Fail {
    corrupt("tar のデータ", e)
}

fn tar_read(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    options("zip.tar_read", args, 1, &[])?;
    let path = str_arg("zip.tar_read", args, 0)?;
    done((|| {
        let mut archive = open_tar(path)?;
        let mut items = Vec::new();
        for entry in archive.entries().map_err(|e| tar_stream(&e))? {
            let entry = entry.map_err(|e| tar_stream(&e))?;
            let is_dir = entry.header().entry_type() == EntryType::Directory;
            items.push(entry_info(&tar_name(&entry), entry.size(), None, is_dir));
        }
        Ok(new_array(items))
    })())
}

fn tar_get(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let opts = options("zip.tar_get", args, 2, &["max_size"])?;
    let path = str_arg("zip.tar_get", args, 0)?;
    let name = str_arg("zip.tar_get", args, 1)?;
    done((|| {
        let mut archive = open_tar(path)?;
        for entry in archive.entries().map_err(|e| tar_stream(&e))? {
            let entry = entry.map_err(|e| tar_stream(&e))?;
            if tar_name(&entry) != name {
                continue;
            }
            if entry.header().entry_type() == EntryType::Directory {
                return Err(fail("io", format!("`{name}` はディレクトリです")));
            }
            return read_limited(entry, opts.max_size)
                .map(Value::bytes)
                .map_err(|e| match e {
                    Limited::TooLarge(f) => f,
                    Limited::Io(e) => tar_stream(&e),
                });
        }
        Err(fail("not_found", format!("エントリ `{name}` がありません")))
    })())
}

/// 展開できるエントリか調べて、ディレクトリかどうかを返す
fn tar_kind(name: &str, entry: &tar::Entry<'_, Box<dyn Read>>) -> R<bool> {
    match entry.header().entry_type() {
        EntryType::Regular | EntryType::Continuous => Ok(false),
        EntryType::Directory => Ok(true),
        EntryType::Symlink | EntryType::Link => Err(unsafe_entry(name, "リンク")),
        other => Err(fail(
            "unsupported",
            format!("エントリ `{name}` は扱えない種類です（{other:?}）"),
        )),
    }
}

fn tar_extract(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let opts = options("zip.tar_extract", args, 2, &["max_size"])?;
    let path = str_arg("zip.tar_extract", args, 0)?;
    let dest = str_arg("zip.tar_extract", args, 1)?;
    done(extract_tar(path, dest, &opts))
}

fn extract_tar(path: &str, dest: &str, opts: &Opts) -> R<Value> {
    (|| {
        // 先に全部を検査して、危険なものがあれば何も書かない
        let mut total: u64 = 0;
        let mut archive = open_tar(path)?;
        for entry in archive.entries().map_err(|e| tar_stream(&e))? {
            let entry = entry.map_err(|e| tar_stream(&e))?;
            let name = tar_name(&entry);
            let parts = safe_components(&name).map_err(|why| unsafe_entry(&name, &why))?;
            let is_dir = tar_kind(&name, &entry)?;
            if !is_dir && parts.is_empty() {
                return Err(unsafe_entry(&name, "名前が残りません"));
            }
            total = total.saturating_add(entry.size());
            if total > opts.max_size {
                return Err(too_large(opts.max_size));
            }
        }
        fs::create_dir_all(dest).map_err(|e| io_fail("作れません", dest, &e))?;
        let dest = Path::new(dest);
        let mut budget = opts.max_size;
        let mut written = Vec::new();
        let mut archive = open_tar(path)?;
        for entry in archive.entries().map_err(|e| tar_stream(&e))? {
            let mut entry = entry.map_err(|e| tar_stream(&e))?;
            let name = tar_name(&entry);
            let parts = safe_components(&name).map_err(|why| unsafe_entry(&name, &why))?;
            let is_dir = tar_kind(&name, &entry)?;
            write_entry(
                dest,
                &parts,
                is_dir,
                Some(&mut entry),
                &mut budget,
                opts.max_size,
            )?;
            if !is_dir {
                written.push(Value::Str(parts.join("/").into()));
            }
        }
        Ok(new_array(written))
    })()
}

fn tar_create(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let opts = options("zip.tar_create", args, 2, &["gzip", "level"])?;
    let path = str_arg("zip.tar_create", args, 0)?;
    let items = match gather("zip.tar_create", &args[1])? {
        Ok(items) => items,
        Err(f) => return done(Err(f)),
    };
    done((|| {
        let file = File::create(path).map_err(|e| io_fail("作れません", path, &e))?;
        let out = BufWriter::new(file);
        let write_err = |e: io::Error| io_fail("書けません", path, &e);
        if opts.gzip {
            let encoder = GzEncoder::new(out, Compression::new(opts.level));
            let encoder = write_tar(encoder, &items, path)?;
            let mut out = encoder.finish().map_err(write_err)?;
            out.flush().map_err(write_err)?;
        } else {
            let mut out = write_tar(out, &items, path)?;
            out.flush().map_err(write_err)?;
        }
        Ok(Value::Nil)
    })())
}

fn write_tar<W: Write>(out: W, items: &[Item], path: &str) -> R<W> {
    let write_err = |e: io::Error| io_fail("書けません", path, &e);
    let mut builder = tar::Builder::new(out);
    for item in items {
        let mut header = tar::Header::new_gnu();
        header.set_mtime(0);
        match &item.source {
            Source::Dir => {
                header.set_entry_type(EntryType::Directory);
                header.set_mode(0o755);
                header.set_size(0);
                builder
                    .append_data(&mut header, format!("{}/", item.name), io::empty())
                    .map_err(write_err)?;
            }
            Source::Mem(data) => {
                header.set_entry_type(EntryType::Regular);
                header.set_mode(0o644);
                header.set_size(data.len() as u64);
                builder
                    .append_data(&mut header, &item.name, &data[..])
                    .map_err(write_err)?;
            }
            Source::Disk(src) => {
                let shown = src.display().to_string();
                let input = File::open(src).map_err(|e| io_fail("開けません", &shown, &e))?;
                let len = input
                    .metadata()
                    .map_err(|e| io_fail("調べられません", &shown, &e))?
                    .len();
                header.set_entry_type(EntryType::Regular);
                header.set_mode(0o644);
                header.set_size(len);
                builder
                    .append_data(&mut header, &item.name, input.take(len))
                    .map_err(write_err)?;
            }
        }
    }
    builder.into_inner().map_err(write_err)
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

    #[test]
    fn gzip_and_zlib_round_trip() {
        let data = b"hello hello hello hello".repeat(50);
        let mut e = GzEncoder::new(Vec::new(), Compression::new(6));
        e.write_all(&data).unwrap();
        let packed = e.finish().unwrap();
        assert!(packed.len() < data.len());
        let back = read_limited(MultiGzDecoder::new(&packed[..]), 1 << 20)
            .ok()
            .unwrap();
        assert_eq!(back, data);
        assert!(matches!(
            read_limited(MultiGzDecoder::new(&packed[..]), 10),
            Err(Limited::TooLarge(_))
        ));
        assert!(matches!(
            read_limited(MultiGzDecoder::new(&b"not gzip"[..]), 10),
            Err(Limited::Io(_))
        ));
    }

    // ---- 展開のパストラバーサル ----

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dolphin_zip_{}_{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn opts() -> Opts {
        Opts {
            level: DEFAULT_LEVEL,
            gzip: false,
            max_size: DEFAULT_MAX_SIZE,
        }
    }

    fn kind_of(r: R<Value>) -> &'static str {
        match r {
            Ok(_) => "ok",
            Err(f) => f.kind,
        }
    }

    fn zip_with(path: &Path, names: &[&str]) {
        let mut w = ZipWriter::new(File::create(path).unwrap());
        for name in names {
            w.start_file(*name, SimpleFileOptions::default()).unwrap();
            w.write_all(b"data").unwrap();
        }
        w.finish().unwrap();
    }

    fn tar_with(path: &Path, entries: &[(&str, EntryType, &str)]) {
        let mut b = tar::Builder::new(File::create(path).unwrap());
        for (name, ty, link) in entries {
            let mut h = tar::Header::new_gnu();
            // `set_path` は `..` を断るので、名前の欄に直接書く
            h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
            h.set_entry_type(*ty);
            h.set_mode(0o644);
            h.set_size(if *ty == EntryType::Regular { 4 } else { 0 });
            if !link.is_empty() {
                h.set_link_name(link).unwrap();
            }
            h.set_cksum();
            let data: &[u8] = if *ty == EntryType::Regular {
                b"data"
            } else {
                b""
            };
            b.append(&h, data).unwrap();
        }
        b.finish().unwrap();
    }

    #[test]
    fn zip_extract_rejects_traversal() {
        let bad_names = [
            "../evil.txt",
            "a/../../evil.txt",
            "/abs.txt",
            "C:/win.txt",
            "..\\evil.txt",
            "a\\..\\..\\evil.txt",
            "x/y:stream",
        ];
        for (i, bad) in bad_names.iter().enumerate() {
            let dir = scratch(&format!("zip_bad{i}"));
            let archive = dir.join("t.zip");
            // 良いエントリが先にあっても、何も書かれない
            zip_with(&archive, &["good.txt", bad]);
            let dest = dir.join("out");
            let r = extract_zip(archive.to_str().unwrap(), dest.to_str().unwrap(), &opts());
            assert_eq!(kind_of(r), "unsafe_path", "{bad}");
            assert!(!dest.exists(), "{bad}: 展開先ができてしまった");
            assert!(!dir.join("evil.txt").exists());
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn zip_extract_rejects_symlink_and_accepts_safe() {
        let dir = scratch("zip_link");
        let archive = dir.join("t.zip");
        let mut w = ZipWriter::new(File::create(&archive).unwrap());
        w.add_symlink("link", "/etc/passwd", SimpleFileOptions::default())
            .unwrap();
        w.finish().unwrap();
        let dest = dir.join("out");
        let r = extract_zip(archive.to_str().unwrap(), dest.to_str().unwrap(), &opts());
        assert_eq!(kind_of(r), "unsafe_path");
        assert!(!dest.exists());

        zip_with(&archive, &["./ok.txt", "d/e.txt", "d\\f.txt"]);
        let r = extract_zip(archive.to_str().unwrap(), dest.to_str().unwrap(), &opts());
        assert_eq!(kind_of(r), "ok");
        assert_eq!(fs::read(dest.join("ok.txt")).unwrap(), b"data");
        assert!(dest.join("d").join("e.txt").exists());
        assert!(dest.join("d").join("f.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tar_extract_rejects_traversal_and_links() {
        let cases: [(&str, EntryType, &str); 6] = [
            ("../evil.txt", EntryType::Regular, ""),
            ("a/../../evil.txt", EntryType::Regular, ""),
            ("/abs.txt", EntryType::Regular, ""),
            ("C:/win.txt", EntryType::Regular, ""),
            ("link", EntryType::Symlink, "/etc/passwd"),
            ("hard", EntryType::Link, "good.txt"),
        ];
        for (i, (name, ty, link)) in cases.iter().enumerate() {
            let dir = scratch(&format!("tar_bad{i}"));
            let archive = dir.join("t.tar");
            tar_with(
                &archive,
                &[("good.txt", EntryType::Regular, ""), (name, *ty, link)],
            );
            let dest = dir.join("out");
            let r = extract_tar(archive.to_str().unwrap(), dest.to_str().unwrap(), &opts());
            assert_eq!(kind_of(r), "unsafe_path", "{name}");
            assert!(!dest.exists(), "{name}: 展開先ができてしまった");
            assert!(!dir.join("evil.txt").exists());
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn tar_extract_accepts_safe_and_dot_dir() {
        let dir = scratch("tar_ok");
        let archive = dir.join("t.tar");
        tar_with(
            &archive,
            &[
                ("./", EntryType::Directory, ""),
                ("./sub/", EntryType::Directory, ""),
                ("./sub/a.txt", EntryType::Regular, ""),
            ],
        );
        let dest = dir.join("out");
        let r = extract_tar(archive.to_str().unwrap(), dest.to_str().unwrap(), &opts());
        assert_eq!(kind_of(r), "ok");
        assert_eq!(fs::read(dest.join("sub").join("a.txt")).unwrap(), b"data");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_stops_at_symlinked_directory_in_dest() {
        // 展開先の中に前から別の場所へのリンクがあっても、そこへは書かない。
        // リンクを作れない環境（権限がない Windows など）では何もしない
        let dir = scratch("dest_link");
        let outside = dir.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let dest = dir.join("out");
        fs::create_dir_all(&dest).unwrap();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, dest.join("d")).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&outside, dest.join("d")).is_ok();
        #[cfg(not(any(unix, windows)))]
        let linked = false;
        if linked {
            let archive = dir.join("t.zip");
            zip_with(&archive, &["d/pwned.txt"]);
            let r = extract_zip(archive.to_str().unwrap(), dest.to_str().unwrap(), &opts());
            assert_eq!(kind_of(r), "unsafe_path");
            assert!(!outside.join("pwned.txt").exists());
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
