//! `std:http` の `serve`（design.md 5.16）
//!
//! 接続の受け付け、HTTP の解析、応答の書き込みは補助スレッド（接続ごとに 1 つ）で行い、
//! 解析済みのデータ（`RequestData`）だけをインタプリタのスレッドに送る。ハンドラはインタプリタの
//! スレッドで 1 つずつ呼び、応答のデータ（`ResponseData`）を補助スレッドに返す。関数値や `Rc` は
//! スレッドをまたがない（design.md 8.3）。HTTP の解析は自前で、外部クレートは使わない

mod connection;
mod request;
mod response;
mod serve;
mod websocket;

use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::runtime::events::Event;
use crate::runtime::handle::{Handle, WsOut};
use crate::runtime::value::Value;

pub use serve::serve;

const DEFAULT_MAX_BODY: usize = 10 * 1024 * 1024;
/// request line とヘッダー全体の大きさの上限
const MAX_HEAD: usize = 64 * 1024;
const MAX_CONNECTIONS: usize = 256;
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const TEXT: &str = "text/plain; charset=utf-8";

struct RequestData {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    remote: String,
}

struct ResponseData {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

struct Job {
    req: RequestData,
    reply: Sender<Reply>,
}

/// ハンドラの結果。応答を返すか、WebSocket に切り替える（design.md 5.22）
enum Reply {
    Response(ResponseData),
    Upgrade { id: u64, accept: String },
}

/// 補助スレッドから、イベントループへメッセージを送る口
#[derive(Clone)]
struct Jobs(Sender<Event>);

impl Jobs {
    fn send(&self, msg: Msg) -> Result<(), ()> {
        self.0.send(Event::Ext(Box::new(msg))).map_err(|_| ())
    }
}

/// 補助スレッドからインタプリタのスレッドへ届くもの
enum Msg {
    Request(Job),
    WsOpen {
        id: u64,
        tx: Sender<WsOut>,
    },
    WsMessage {
        id: u64,
        message: WsIn,
    },
    /// 接続が終わった。`code` は終了コード
    WsClosed {
        id: u64,
        code: u16,
    },
}

enum WsIn {
    Text(String),
    Binary(Vec<u8>),
}

/// WebSocket にした接続のコールバック（インタプリタのスレッドだけが持つ）
struct WsEntry {
    on_open: Option<Value>,
    on_message: Option<Value>,
    on_close: Option<Value>,
    handle: Option<Rc<Handle>>,
}
