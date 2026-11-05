//! `std:postgres` を実際のサーバーで確かめる（design.md 5.47）。
//! 環境変数 `DOLPHIN_TEST_PG_URL`（`postgres://user:password@host:port/dbname`）があるときだけ動かし、
//! なければ飛ばす。サーバーがなくても通るテストは、tests/cases/postgres_*.dol と src/stdlib/data/postgres.rs にある。
//! 実行: `DOLPHIN_TEST_PG_URL=postgres://postgres:pw@localhost/postgres cargo test --features postgres --test postgres`

#![cfg(feature = "postgres")]

use std::fs;
use std::path::PathBuf;

fn test_url() -> Option<String> {
    std::env::var("DOLPHIN_TEST_PG_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

#[test]
fn server_script() {
    if test_url().is_none() {
        eprintln!(
            "skip: DOLPHIN_TEST_PG_URL がないので、PostgreSQL のサーバーを使うテストは飛ばします"
        );
        return;
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/pg");
    let src = fs::read_to_string(dir.join("server.dol")).unwrap();
    let mut out = Vec::new();
    let result = dolphin::run_source_in(&src, dir.clone(), &mut out);
    let text = String::from_utf8(out).unwrap().replace("\r\n", "\n");
    if let Err(diag) = result {
        panic!("{}\n出力:\n{text}", diag.render("server.dol", &src));
    }
    let failed: Vec<&str> = text.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(
        failed.is_empty(),
        "失敗した確認:\n{}\n出力:\n{text}",
        failed.join("\n")
    );
    assert_eq!(text.lines().last(), Some("done"), "出力:\n{text}");
    let ok = text.lines().filter(|l| l.starts_with("ok")).count();
    assert!(ok >= 50, "確認が少なすぎる（{ok} 個）:\n{text}");
}
