//! LSP のメッセージの読み書き（`Content-Length` つきのフレーム）

use std::io::{self, BufRead, Write};

use super::json::Json;

pub(super) fn read_message(input: &mut dyn BufRead) -> io::Result<Option<String>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let Some(length) = length else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Content-Length がありません",
        ));
    };
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    String::from_utf8(body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub(super) fn write_message(output: &mut dyn Write, message: &Json) -> io::Result<()> {
    let body = message.to_string();
    write!(output, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    output.flush()
}
