//! リクエストの処理（インタプリタのスレッド）。`serve` の本体と、ハンドラ・コールバックの呼び出し

use std::any::Any;
use std::collections::HashMap;
use std::net::TcpListener;
use std::rc::Rc;
use std::thread;

use super::connection::accept_loop;
use super::response::{internal_error, response_data};
use super::websocket::{upgrade_key, ws_entry};
use super::{DEFAULT_MAX_BODY, Jobs, Msg, Reply, RequestData, ResponseData, TEXT, WsEntry, WsIn};
use crate::diag::Diagnostic;
use crate::runtime::error::RuntimeError;
use crate::runtime::handle::{Handle, HandleKind};
use crate::runtime::interp::Interpreter;
use crate::runtime::map::Map;
use crate::runtime::value::Value;
use crate::stdlib::ws_proto::accept_key;
use crate::stdlib::{error_value, expect_args, func_arg, new_map, type_error};

pub fn serve(interp: &mut Interpreter<'_>, args: &[Value]) -> Result<Value, String> {
    expect_args("http.serve", args, 2)?;
    let (host, port, max_body) = options(args)?;
    let handler = func_arg("http.serve", args, 1)?.clone();

    let listener = match TcpListener::bind((host.as_str(), port)) {
        Ok(listener) => listener,
        Err(e) => {
            return Ok(error_value(
                "network",
                format!("{host}:{port} で待ち受けられません: {e}"),
            ));
        }
    };
    let jobs = Jobs(interp.events.sender());
    thread::spawn(move || accept_loop(&listener, &jobs, max_body));

    // 補助スレッドからのメッセージは、イベントループ（design.md 5.23）に `Ext` で届く
    let mut sockets: HashMap<u64, WsEntry> = HashMap::new();
    let mut next_id = 1;
    let receive = move |interp: &mut Interpreter<'_>, payload: Box<dyn Any + Send>| {
        let Ok(msg) = payload.downcast::<Msg>() else {
            return Ok(());
        };
        match *msg {
            Msg::Request(job) => {
                let reply = match handle(interp, &handler, job.req, &mut sockets, &mut next_id) {
                    Ok(reply) => reply,
                    Err(message) => {
                        // `os.exit` はサーバーごと止める
                        if interp.exit_code.is_some() {
                            return Err(message);
                        }
                        eprintln!("{message}");
                        Reply::Response(internal_error())
                    }
                };
                // 相手がもう切れていても気にしない
                let _ = job.reply.send(reply);
            }
            Msg::WsOpen { id, tx } => {
                let Some(entry) = sockets.get_mut(&id) else {
                    return Ok(());
                };
                let conn = Handle::new(HandleKind::WsServer { tx });
                entry.handle = Some(Rc::clone(&conn));
                let callback = entry.on_open.clone();
                run_callback(interp, callback, vec![Value::Handle(conn)])?;
            }
            Msg::WsMessage { id, message } => {
                let Some(entry) = sockets.get(&id) else {
                    return Ok(());
                };
                let (Some(conn), Some(callback)) = (&entry.handle, entry.on_message.clone()) else {
                    return Ok(());
                };
                let value = match message {
                    WsIn::Text(s) => Value::Str(s.into()),
                    WsIn::Binary(b) => Value::bytes(b),
                };
                let conn = Value::Handle(Rc::clone(conn));
                run_callback(interp, Some(callback), vec![conn, value])?;
            }
            Msg::WsClosed { id, code } => {
                let Some(entry) = sockets.remove(&id) else {
                    return Ok(());
                };
                if let Some(conn) = &entry.handle {
                    let args = vec![Value::Handle(Rc::clone(conn)), Value::Num(f64::from(code))];
                    let result = run_callback(interp, entry.on_close.clone(), args);
                    conn.close();
                    result?;
                }
            }
        }
        Ok(())
    };
    interp
        .events
        .set_ext(Box::new(receive))
        .map_err(|why| format!("http.serve: {why}"))?;
    let result = interp.run_event_loop();
    interp.events.clear_ext();
    result.map(|()| Value::Nil)
}

/// WebSocket のコールバックを呼ぶ。エラーは標準エラーに出して続ける。`os.exit` だけは
/// サーバーごと止める（`Err` を返す）
fn run_callback(
    interp: &mut Interpreter<'_>,
    callback: Option<Value>,
    args: Vec<Value>,
) -> Result<(), String> {
    let Some(callback) = callback else {
        return Ok(());
    };
    if let Err(message) = interp.call_value(&callback, args) {
        if interp.exit_code.is_some() {
            return Err(message);
        }
        match interp.take_native_error() {
            Some(e) => eprintln!("{}", describe(e)),
            None => eprintln!("http.serve: WebSocket のコールバックでエラー: {message}"),
        }
    }
    Ok(())
}

/// `serve[port, handler]` か `serve[options, handler]` の `(host, port, max_body)`
fn options(args: &[Value]) -> Result<(String, u16, usize), String> {
    let port_of = |n: f64| -> Result<u16, String> {
        if n.fract() == 0.0 && (0.0..=65535.0).contains(&n) {
            Ok(n as u16)
        } else {
            Err(format!("http.serve のポートは 0〜65535 の整数です（{n}）"))
        }
    };
    match &args[0] {
        Value::Num(n) => Ok(("127.0.0.1".into(), port_of(*n)?, DEFAULT_MAX_BODY)),
        Value::Map(options) => {
            let options = options.borrow();
            for (key, _) in options.iter() {
                if !["port", "host", "max_body"].contains(&&**key) {
                    return Err(format!("http.serve のオプション `{key}` は知りません"));
                }
            }
            let port = match options.get("port") {
                Some(Value::Num(n)) => port_of(*n)?,
                Some(other) => return Err(option_type("port", "数値", other)),
                None => return Err("http.serve に `port` がありません".into()),
            };
            let host = match options.get("host") {
                Some(Value::Str(s)) => s.to_string(),
                Some(other) => return Err(option_type("host", "文字列", other)),
                None => "127.0.0.1".into(),
            };
            let max_body = match options.get("max_body") {
                Some(Value::Num(n)) if *n >= 0.0 && n.fract() == 0.0 => *n as usize,
                Some(other) => return Err(option_type("max_body", "0 以上の整数", other)),
                None => DEFAULT_MAX_BODY,
            };
            Ok((host, port, max_body))
        }
        other => Err(type_error("http.serve", 0, "数値かマップ", other)),
    }
}

fn option_type(key: &str, expected: &str, got: &Value) -> String {
    format!(
        "http.serve のオプション `{key}` は{expected}です（{}が渡されました）",
        got.type_name()
    )
}

/// ハンドラを呼んで、応答のデータにする（WebSocket に切り替えるなら、そのコールバックを登録する）。
/// 失敗は標準エラーに出すメッセージ
fn handle(
    interp: &mut Interpreter<'_>,
    handler: &Value,
    req: RequestData,
    sockets: &mut HashMap<u64, WsEntry>,
    next_id: &mut u64,
) -> Result<Reply, String> {
    let upgrade = upgrade_key(&req);
    let value = interp
        .call_value(handler, vec![request_value(req)])
        .map_err(|message| match interp.take_native_error() {
            Some(e) => describe(e),
            None => format!("http.serve: ハンドラでエラー: {message}"),
        })?;
    if let Value::Map(map) = &value
        && map.borrow().contains_key("websocket")
    {
        let entry = ws_entry(&map.borrow())
            .map_err(|m| format!("http.serve: ハンドラの戻り値が正しくありません: {m}"))?;
        let key = match upgrade {
            Ok(key) => key,
            Err(why) => {
                // WebSocket の接続要求でないリクエストには、400 を返す
                eprintln!("http.serve: WebSocket にできないリクエストです（{why}）");
                return Ok(Reply::Response(ResponseData {
                    status: 400,
                    headers: vec![("content-type".into(), TEXT.into())],
                    body: b"Bad Request".to_vec(),
                }));
            }
        };
        let id = *next_id;
        *next_id += 1;
        sockets.insert(id, entry);
        return Ok(Reply::Upgrade {
            id,
            accept: accept_key(&key),
        });
    }
    response_data(&value)
        .map(Reply::Response)
        .map_err(|m| format!("http.serve: ハンドラの戻り値が正しくありません: {m}"))
}

fn describe(e: RuntimeError) -> String {
    let diag = Diagnostic::from(e);
    let mut text = format!(
        "http.serve: ハンドラでエラー: {}:{}: {}",
        diag.span.line, diag.span.col, diag.message
    );
    for note in &diag.notes {
        text.push_str(&format!("\n  {note}"));
    }
    text
}

fn request_value(req: RequestData) -> Value {
    let strings = |pairs: Vec<(String, String)>| {
        let mut map = Map::default();
        for (k, v) in pairs {
            map.insert(k.as_str().into(), Value::Str(v.into()));
        }
        new_map(map)
    };
    let mut map = Map::default();
    map.insert("method".into(), Value::Str(req.method.into()));
    map.insert("path".into(), Value::Str(req.path.into()));
    map.insert("query".into(), strings(req.query));
    map.insert("headers".into(), strings(req.headers));
    let body = match std::str::from_utf8(&req.body) {
        Ok(s) => Value::Str(s.into()),
        Err(_) => Value::Nil,
    };
    map.insert("body".into(), body);
    map.insert("raw".into(), Value::bytes(&req.body));
    map.insert("remote".into(), Value::Str(req.remote.into()));
    new_map(map)
}
