//! WebSocket（design.md 5.22）。接続要求の判定、コールバックの取り出し、補助スレッド側のフレームの読み書き

use std::io::{self, BufReader, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use super::{Jobs, Msg, RequestData, WsEntry, WsIn};
use crate::runtime::handle::WsOut;
use crate::runtime::map::Map;
use crate::runtime::value::Value;
use crate::stdlib::ws_proto::{
    self, Message, OP_BINARY, OP_CLOSE, OP_PONG, OP_TEXT, close_payload,
};

/// WebSocket の接続要求なら、`Sec-WebSocket-Key`。違えば理由
pub(super) fn upgrade_key(req: &RequestData) -> Result<String, &'static str> {
    let header = |name: &str| {
        req.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_ascii_lowercase())
    };
    if req.method != "GET" {
        return Err("GET ではありません");
    }
    if !header("upgrade").is_some_and(|v| v.split(',').any(|t| t.trim() == "websocket")) {
        return Err("Upgrade: websocket がありません");
    }
    if !header("connection").is_some_and(|v| v.split(',').any(|t| t.trim() == "upgrade")) {
        return Err("Connection: Upgrade がありません");
    }
    if header("sec-websocket-version").as_deref() != Some("13") {
        return Err("Sec-WebSocket-Version が 13 ではありません");
    }
    match req.headers.iter().find(|(n, _)| n == "sec-websocket-key") {
        Some((_, key)) if key.trim().len() == 24 => Ok(key.trim().to_string()),
        _ => Err("Sec-WebSocket-Key が正しくありません"),
    }
}

/// `{"websocket": {"on_open": f, "on_message": f, "on_close": f}}` のコールバックを取り出す
pub(super) fn ws_entry(map: &Map) -> Result<WsEntry, String> {
    if map.len() != 1 {
        return Err("`websocket` は `status` `headers` `body` `json` と同時に使えません".into());
    }
    let Some(Value::Map(callbacks)) = map.get("websocket") else {
        return Err("`websocket` はマップです".into());
    };
    let callbacks = callbacks.borrow();
    let mut entry = WsEntry {
        on_open: None,
        on_message: None,
        on_close: None,
        handle: None,
    };
    for (key, value) in callbacks.iter() {
        if !matches!(value, Value::Func(_) | Value::Native(_)) {
            return Err(format!(
                "`websocket` の `{key}` は関数です（{}でした）",
                value.type_name()
            ));
        }
        let slot = match &**key {
            "on_open" => &mut entry.on_open,
            "on_message" => &mut entry.on_message,
            "on_close" => &mut entry.on_close,
            _ => return Err(format!("`websocket` の知らないキー `{key}` があります")),
        };
        *slot = Some(value.clone());
    }
    Ok(entry)
}

/// 101 を返して、以降は WebSocket のフレームを読む。書き込みは別のスレッドが受け持つ
pub(super) fn websocket(
    mut reader: BufReader<TcpStream>,
    writer: &mut TcpStream,
    jobs: &Jobs,
    id: u64,
    accept: &str,
) -> io::Result<()> {
    write!(
        writer,
        "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-accept: {accept}\r\n\r\n"
    )?;
    writer.flush()?;
    // WebSocket の間は、時間切れなし
    reader.get_ref().set_read_timeout(None)?;
    writer.set_write_timeout(None)?;

    let (tx, rx) = mpsc::channel::<WsOut>();
    let socket = writer.try_clone()?;
    let writer_thread = thread::spawn(move || ws_writer(socket, &rx));
    if jobs.send(Msg::WsOpen { id, tx: tx.clone() }).is_err() {
        return Ok(());
    }
    let code = loop {
        let pong = |payload: &[u8]| {
            tx.send(WsOut::Pong(payload.to_vec()))
                .map_err(|_| io::Error::other("接続が閉じています"))
        };
        let message = match ws_proto::read_message(&mut reader, true, pong) {
            Ok(message) => message,
            Err(e) => {
                let code = e.close_code();
                // 通信の失敗のときは、相手に伝えられない
                if !matches!(e, ws_proto::ProtoError::Io(_)) {
                    let _ = tx.send(WsOut::Close(code, String::new()));
                }
                break code;
            }
        };
        let message = match message {
            Message::Text(s) => WsIn::Text(s),
            Message::Binary(b) => WsIn::Binary(b),
            Message::Close(close) => {
                let _ = tx.send(WsOut::Close(1000, String::new()));
                break close.map_or(1005, |(code, _)| code);
            }
        };
        if jobs.send(Msg::WsMessage { id, message }).is_err() {
            return Ok(());
        }
    };
    let _ = jobs.send(Msg::WsClosed { id, code });
    drop(tx);
    let _ = writer_thread.join();
    Ok(())
}

/// 送信専用のスレッド。close を送ったあとは、相手が閉じるのを少し待ってから切る
fn ws_writer(mut stream: TcpStream, rx: &Receiver<WsOut>) {
    while let Ok(out) = rx.recv() {
        let result = match &out {
            WsOut::Text(s) => ws_proto::write_frame(&mut stream, OP_TEXT, s.as_bytes(), None),
            WsOut::Binary(b) => ws_proto::write_frame(&mut stream, OP_BINARY, b, None),
            WsOut::Pong(p) => ws_proto::write_frame(&mut stream, OP_PONG, p, None),
            WsOut::Close(code, reason) => {
                let _ = ws_proto::write_frame(
                    &mut stream,
                    OP_CLOSE,
                    &close_payload(*code, reason),
                    None,
                );
                // 送り手がすべていなくなる（読み込み側が終わる）まで、5 秒だけ待つ
                while !matches!(
                    rx.recv_timeout(Duration::from_secs(5)),
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected)
                ) {}
                break;
            }
        };
        if result.is_err() {
            break;
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}
