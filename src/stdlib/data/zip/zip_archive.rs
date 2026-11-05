//! zip の読み書き

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::Path;

use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::entry::{entry_info, write_entry};
use super::files::{Source, gather};
use super::limited::{Limited, read_limited, too_large};
use super::options::{Opts, options};
use super::safe_path::{safe_components, unsafe_entry};
use super::{Fail, R, corrupt, done, fail, io_fail};
use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;
use crate::stdlib::{new_array, str_arg};

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

fn open_zip(path: &str) -> R<ZipArchive<BufReader<File>>> {
    let file = File::open(path).map_err(|e| io_fail("開けません", path, &e))?;
    ZipArchive::new(BufReader::new(file)).map_err(|e| zip_fail(path, e))
}

pub(super) fn zip_read(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
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

pub(super) fn zip_get(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
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

pub(super) fn zip_extract(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let opts = options("zip.zip_extract", args, 2, &["max_size"])?;
    let path = str_arg("zip.zip_extract", args, 0)?;
    let dest = str_arg("zip.zip_extract", args, 1)?;
    done(extract_zip(path, dest, &opts))
}

pub(super) fn extract_zip(path: &str, dest: &str, opts: &Opts) -> R<Value> {
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

pub(super) fn zip_create(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
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
