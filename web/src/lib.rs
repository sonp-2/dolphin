//! Playground から呼ぶ入口。ソースを実行して、出力とエラーを返す

use wasm_bindgen::prelude::*;

#[wasm_bindgen(getter_with_clone)]
pub struct RunResult {
    pub output: String,
    /// 構文・実行時エラーの文面。なければ空
    pub error: String,
    /// エラーの行（1 始まり）。なければ 0
    pub line: u32,
}

#[wasm_bindgen]
pub fn run(src: &str) -> RunResult {
    let mut out: Vec<u8> = Vec::new();
    let result = dolphin::run_source(src, &mut out);
    let output = String::from_utf8_lossy(&out).into_owned();
    match result {
        Ok(()) => RunResult { output, error: String::new(), line: 0 },
        Err(d) => RunResult { output, error: d.render("main.dol", src), line: d.span.line as u32 },
    }
}
