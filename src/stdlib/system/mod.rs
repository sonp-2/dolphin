//! OS との境界（ファイル・パス・標準入出力・プロセス・C の共有ライブラリ）

pub mod argparse;
#[cfg(feature = "ffi")]
pub mod ffi;
pub mod fs;
pub mod io;
pub mod os;
pub mod path;
pub mod process;
pub mod temp;
pub mod walk;
