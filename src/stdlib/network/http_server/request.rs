//! リクエストの読み取り（補助スレッド）。request line・ヘッダー・本文（chunked を含む）の解析

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;

use super::response::valid_header;
use super::{MAX_HEAD, RequestData};

pub(super) enum ReadError {
    /// 次のリクエストが来る前に、相手が閉じた
    Closed,
    Io,
    /// このステータスで断って、接続を閉じる
    Reject(u16, &'static str),
}

impl From<io::Error> for ReadError {
    fn from(_: io::Error) -> Self {
        ReadError::Io
    }
}

const BAD: ReadError = ReadError::Reject(400, "Bad Request");

fn eof() -> ReadError {
    ReadError::Io
}

pub(super) struct Parsed {
    pub(super) req: RequestData,
    pub(super) keep_alive: bool,
    pub(super) head_only: bool,
}

/// 1 行（末尾の改行を除く）。`remaining` は、request line とヘッダーに残っている大きさ
fn read_line(
    reader: &mut BufReader<TcpStream>,
    remaining: &mut usize,
) -> Result<Option<Vec<u8>>, ReadError> {
    let mut buf = Vec::new();
    let n = reader
        .by_ref()
        .take(*remaining as u64 + 1)
        .read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if n > *remaining {
        return Err(ReadError::Reject(431, "Request Header Fields Too Large"));
    }
    *remaining -= n;
    if buf.pop() != Some(b'\n') {
        return Err(eof());
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(Some(buf))
}

pub(super) fn read_request(
    reader: &mut BufReader<TcpStream>,
    writer: &mut TcpStream,
    max_body: usize,
    remote: &str,
) -> Result<Parsed, ReadError> {
    let mut remaining = MAX_HEAD;
    // リクエストの前の空行は読み飛ばす
    let line = loop {
        match read_line(reader, &mut remaining)? {
            None => return Err(ReadError::Closed),
            Some(line) if line.is_empty() => continue,
            Some(line) => break line,
        }
    };
    let line = String::from_utf8(line).map_err(|_| BAD)?;
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(BAD);
    };
    let http10 = match version {
        "HTTP/1.1" => false,
        "HTTP/1.0" => true,
        v if v.starts_with("HTTP/") => {
            return Err(ReadError::Reject(505, "HTTP Version Not Supported"));
        }
        _ => return Err(BAD),
    };
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase() || b == b'-') {
        return Err(BAD);
    }

    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let Some(line) = read_line(reader, &mut remaining)? else {
            return Err(eof());
        };
        if line.is_empty() {
            break;
        }
        let line = String::from_utf8(line).map_err(|_| BAD)?;
        let (name, value) = line.split_once(':').ok_or(BAD)?;
        if valid_header(name, "").is_err() {
            return Err(BAD);
        }
        let name = name.to_ascii_lowercase();
        let value = value.trim().to_string();
        match headers.iter_mut().find(|(n, _)| *n == name) {
            Some((_, old)) => {
                old.push_str(", ");
                old.push_str(&value);
            }
            None => headers.push((name, value)),
        }
    }
    let header = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };

    let connection = header("connection").unwrap_or("").to_ascii_lowercase();
    let keep_alive = if http10 {
        connection.contains("keep-alive")
    } else {
        !connection.contains("close")
    };

    let chunked = match header("transfer-encoding") {
        None => false,
        Some(te) if te.eq_ignore_ascii_case("chunked") => true,
        Some(_) => return Err(ReadError::Reject(501, "Not Implemented")),
    };
    let length = match header("content-length") {
        None => None,
        Some(_) if chunked => return Err(BAD),
        Some(v) => {
            if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                return Err(BAD);
            }
            let n = v
                .parse::<usize>()
                .map_err(|_| ReadError::Reject(413, "Payload Too Large"))?;
            Some(n)
        }
    };
    if length.is_some_and(|n| n > max_body) {
        return Err(ReadError::Reject(413, "Payload Too Large"));
    }
    if header("expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue"))
        && (chunked || length.is_some_and(|n| n > 0))
    {
        writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let body = if chunked {
        read_chunked(reader, max_body)?
    } else {
        let mut body = vec![0; length.unwrap_or(0)];
        reader.read_exact(&mut body)?;
        body
    };

    let (path, query) = split_target(target)?;
    Ok(Parsed {
        req: RequestData {
            method: method.to_string(),
            path,
            query,
            headers,
            body,
            remote: remote.to_string(),
        },
        keep_alive,
        head_only: method == "HEAD",
    })
}

fn read_chunked(reader: &mut BufReader<TcpStream>, max_body: usize) -> Result<Vec<u8>, ReadError> {
    let mut body = Vec::new();
    loop {
        let mut remaining = MAX_HEAD;
        let Some(line) = read_line(reader, &mut remaining)? else {
            return Err(eof());
        };
        let line = String::from_utf8(line).map_err(|_| BAD)?;
        let size = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size, 16).map_err(|_| BAD)?;
        if size == 0 {
            // トレーラーは読み捨てる
            loop {
                match read_line(reader, &mut remaining)? {
                    Some(line) if !line.is_empty() => {}
                    _ => return Ok(body),
                }
            }
        }
        if body.len().saturating_add(size) > max_body {
            return Err(ReadError::Reject(413, "Payload Too Large"));
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader.read_exact(&mut body[start..])?;
        let mut crlf = [0; 2];
        reader.read_exact(&mut crlf)?;
        if &crlf != b"\r\n" {
            return Err(BAD);
        }
    }
}

type Target = (String, Vec<(String, String)>);

/// リクエストの対象を、デコード済みのパスとクエリにする。`http://host/path?q` の形も受け付ける
fn split_target(target: &str) -> Result<Target, ReadError> {
    let target = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))
        .map_or(target, |rest| rest.find('/').map_or("/", |i| &rest[i..]));
    let target = target.split('#').next().unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if !path.starts_with('/') && path != "*" {
        return Err(BAD);
    }
    let path = percent_decode(path, false).ok_or(BAD)?;
    let mut pairs: Vec<(String, String)> = Vec::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (k, v) = (
            percent_decode(k, true).ok_or(BAD)?,
            percent_decode(v, true).ok_or(BAD)?,
        );
        match pairs.iter_mut().find(|(n, _)| *n == k) {
            Some((_, old)) => *old = v,
            None => pairs.push((k, v)),
        }
    }
    Ok((path, pairs))
}

/// `%XX` を戻す。`plus` なら `+` を空白にする（クエリ）。UTF-8 にならなければ None
fn percent_decode(s: &str, plus: bool) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3)?;
                let hex = std::str::from_utf8(hex).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' if plus => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(target: &str) -> Target {
        match split_target(target) {
            Ok(t) => t,
            Err(_) => panic!("{target} を解析できません"),
        }
    }

    #[test]
    fn decodes_percent_and_plus() {
        assert_eq!(percent_decode("a%20b+c", false).unwrap(), "a b+c");
        assert_eq!(percent_decode("a%20b+c", true).unwrap(), "a b c");
        assert_eq!(percent_decode("%E3%81%82", false).unwrap(), "あ");
        assert!(percent_decode("%E3", false).is_none());
        assert!(percent_decode("%zz", false).is_none());
        assert!(percent_decode("%2", false).is_none());
    }

    #[test]
    fn splits_target() {
        let (path, query) = split("/a%20b?x=1&y=%E3%81%82&x=2&z");
        assert_eq!(path, "/a b");
        assert_eq!(
            query,
            [
                ("x".to_string(), "2".to_string()),
                ("y".to_string(), "あ".to_string()),
                ("z".to_string(), String::new())
            ]
        );
        assert_eq!(split("http://example.com/p?q=1").0, "/p");
        assert!(split_target("p").is_err());
    }
}
