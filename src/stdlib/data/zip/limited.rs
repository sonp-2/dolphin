//! 展開後の大きさの上限

use std::io::{self, Read};

use super::{Fail, fail};

pub(super) enum Limited {
    TooLarge(Fail),
    Io(io::Error),
}

pub(super) fn too_large(max: u64) -> Fail {
    fail(
        "too_large",
        format!("展開すると上限（{max} バイト）を超えます"),
    )
}

/// `max` バイトまで読む。超えたら `TooLarge`
pub(super) fn read_limited(reader: impl Read, max: u64) -> Result<Vec<u8>, Limited> {
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
