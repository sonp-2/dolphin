//! 展開のパストラバーサルを拒否することの検査（zip と tar の両方）

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use tar::EntryType;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use super::options::Opts;
use super::tar_archive::extract_tar;
use super::zip_archive::extract_zip;
use super::{DEFAULT_LEVEL, DEFAULT_MAX_SIZE, R};
use crate::runtime::value::Value;

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
