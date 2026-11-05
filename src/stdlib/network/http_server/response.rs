//! レスポンスの組み立て。ハンドラの戻り値を `ResponseData` にし、HTTP/1.1 の応答として書き出す

use std::io::{self, Write};
use std::net::TcpStream;

use super::{ResponseData, TEXT};
use crate::runtime::value::Value;
use crate::stdlib::json::to_json_string;

pub(super) fn internal_error() -> ResponseData {
    ResponseData {
        status: 500,
        headers: vec![("content-type".into(), TEXT.into())],
        body: b"Internal Server Error".to_vec(),
    }
}

/// ハンドラの戻り値（文字列かマップ）を応答のデータにする
pub(super) fn response_data(value: &Value) -> Result<ResponseData, String> {
    let map = match value {
        Value::Str(s) => {
            return Ok(ResponseData {
                status: 200,
                headers: vec![("content-type".into(), TEXT.into())],
                body: s.as_bytes().to_vec(),
            });
        }
        Value::Map(map) => map.borrow(),
        other => {
            return Err(format!(
                "文字列かマップを返してください（{}でした）",
                other.type_name()
            ));
        }
    };
    for (key, _) in map.iter() {
        if !["status", "headers", "body", "json"].contains(&&**key) {
            return Err(format!("知らないキー `{key}` があります"));
        }
    }
    let status = match map.get("status") {
        None => 200,
        Some(Value::Num(n)) if n.fract() == 0.0 && (200.0..=599.0).contains(n) => *n as u16,
        Some(other) => {
            return Err(format!("`status` は 200〜599 の整数です（{other}）"));
        }
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    match map.get("headers") {
        None => {}
        Some(Value::Map(h)) => {
            for (name, value) in h.borrow().iter() {
                let Value::Str(value) = value else {
                    return Err(format!(
                        "`headers` の値は文字列です（`{name}` は{}）",
                        value.type_name()
                    ));
                };
                valid_header(name, value)?;
                headers.push((name.to_ascii_lowercase(), value.to_string()));
            }
        }
        Some(other) => {
            return Err(format!(
                "`headers` はマップです（{}でした）",
                other.type_name()
            ));
        }
    }
    // サーバーが決めるヘッダー
    headers.retain(|(n, _)| !["content-length", "transfer-encoding", "connection"].contains(&&**n));

    let (body, default_type) = match (map.get("body"), map.get("json")) {
        (Some(_), Some(_)) => return Err("`body` と `json` は同時に使えません".into()),
        (Some(Value::Str(s)), None) => (s.as_bytes().to_vec(), Some(TEXT)),
        (Some(Value::Bytes(b)), None) => (b.to_vec(), Some("application/octet-stream")),
        (Some(other), None) => {
            return Err(format!(
                "`body` は文字列かバイト列です（{}でした）",
                other.type_name()
            ));
        }
        (None, Some(json)) => (
            to_json_string("http.serve", json, None)?.into_bytes(),
            Some("application/json"),
        ),
        (None, None) => (Vec::new(), None),
    };
    if let Some(default_type) = default_type
        && !headers.iter().any(|(n, _)| n == "content-type")
    {
        headers.push(("content-type".into(), default_type.into()));
    }
    Ok(ResponseData {
        status,
        headers,
        body,
    })
}

pub(super) fn valid_header(name: &str, value: &str) -> Result<(), String> {
    let name_ok = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b));
    let value_ok = value
        .bytes()
        .all(|b| b == b'\t' || (b >= 0x20 && b != 0x7f));
    if name_ok && value_ok {
        Ok(())
    } else {
        Err(format!("ヘッダー `{name}` に使えない文字があります"))
    }
}

pub(super) fn write_response(
    writer: &mut TcpStream,
    response: &ResponseData,
    head_only: bool,
    keep_alive: bool,
) -> io::Result<()> {
    let mut out = Vec::with_capacity(response.body.len() + 256);
    write!(
        out,
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    )?;
    for (name, value) in &response.headers {
        write!(out, "{name}: {value}\r\n")?;
    }
    write!(out, "content-length: {}\r\n", response.body.len())?;
    let connection = if keep_alive { "keep-alive" } else { "close" };
    write!(out, "connection: {connection}\r\n\r\n")?;
    if !head_only && !matches!(response.status, 204 | 304) {
        out.extend_from_slice(&response.body);
    }
    writer.write_all(&out)?;
    writer.flush()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "",
    }
}
