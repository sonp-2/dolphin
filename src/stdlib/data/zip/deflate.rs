//! gzip と zlib（deflate）。バイト列を丸ごと圧縮・展開する

use std::io::Write;

use flate2::Compression;
use flate2::read::{MultiGzDecoder, ZlibDecoder};
use flate2::write::{GzEncoder, ZlibEncoder};

use super::limited::{Limited, read_limited};
use super::options::options;
use super::{corrupt, done};
use crate::runtime::interp::Interpreter;
use crate::runtime::value::Value;
use crate::stdlib::{bytes_arg, data_arg};

pub(super) fn gzip(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let opts = options("zip.gzip", args, 1, &["level"])?;
    let data = data_arg("zip.gzip", args, 0)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(opts.level));
    let out = encoder
        .write_all(&data)
        .and_then(|()| encoder.finish())
        .map_err(|e| format!("zip.gzip: {e}"))?;
    Ok(Value::bytes(out))
}

pub(super) fn deflate(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    let opts = options("zip.deflate", args, 1, &["level"])?;
    let data = data_arg("zip.deflate", args, 0)?;
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(opts.level));
    let out = encoder
        .write_all(&data)
        .and_then(|()| encoder.finish())
        .map_err(|e| format!("zip.deflate: {e}"))?;
    Ok(Value::bytes(out))
}

pub(super) fn gunzip(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
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

pub(super) fn inflate(_: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
