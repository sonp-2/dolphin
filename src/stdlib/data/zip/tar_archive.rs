//! tar の読み書き（gzip 圧縮された tar も読める）

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;

use flate2::Compression;
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use tar::EntryType;

use super::entry::{entry_info, write_entry};
use super::files::{Item, Source, gather};
use super::limited::{Limited, read_limited, too_large};
use super::options::{Opts, options};
use super::safe_path::{safe_components, unsafe_entry};
use super::{Fail, R, corrupt, done, fail, io_fail};
use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;
use crate::stdlib::{new_array, str_arg};

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

pub(super) fn tar_read(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
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

pub(super) fn tar_get(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
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

pub(super) fn tar_extract(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let opts = options("zip.tar_extract", args, 2, &["max_size"])?;
    let path = str_arg("zip.tar_extract", args, 0)?;
    let dest = str_arg("zip.tar_extract", args, 1)?;
    done(extract_tar(path, dest, &opts))
}

pub(super) fn extract_tar(path: &str, dest: &str, opts: &Opts) -> R<Value> {
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

pub(super) fn tar_create(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
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
