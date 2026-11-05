//! 接続の管理（補助スレッド）。接続を受け付けて、接続ごとにリクエストを読み、応答を返す

use std::io::{self, BufReader};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::request::{ReadError, read_request};
use super::response::write_response;
use super::websocket::websocket;
use super::{IDLE_TIMEOUT, Job, Jobs, MAX_CONNECTIONS, Msg, Reply, ResponseData, TEXT};

pub(super) fn accept_loop(listener: &TcpListener, jobs: &Jobs, max_body: usize) {
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            thread::sleep(Duration::from_millis(10));
            continue;
        };
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let jobs = jobs.clone();
        let active = Arc::clone(&active);
        thread::spawn(move || {
            let _ = connection(stream, &jobs, max_body);
            active.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

fn connection(stream: TcpStream, jobs: &Jobs, max_body: usize) -> io::Result<()> {
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_write_timeout(Some(IDLE_TIMEOUT))?;
    let remote = stream
        .peer_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default();
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    loop {
        let parsed = match read_request(&mut reader, &mut writer, max_body, &remote) {
            Ok(parsed) => parsed,
            Err(ReadError::Closed | ReadError::Io) => return Ok(()),
            Err(ReadError::Reject(status, text)) => {
                let response = ResponseData {
                    status,
                    headers: vec![("content-type".into(), TEXT.into())],
                    body: text.as_bytes().to_vec(),
                };
                return write_response(&mut writer, &response, false, false);
            }
        };
        let (reply, answer) = mpsc::channel();
        let job = Job {
            req: parsed.req,
            reply,
        };
        if jobs.send(Msg::Request(job)).is_err() {
            return Ok(());
        }
        let response = match answer.recv() {
            Ok(Reply::Response(response)) => response,
            Ok(Reply::Upgrade { id, accept }) => {
                return websocket(reader, &mut writer, jobs, id, &accept);
            }
            Err(_) => return Ok(()),
        };
        write_response(&mut writer, &response, parsed.head_only, parsed.keep_alive)?;
        if !parsed.keep_alive {
            return Ok(());
        }
    }
}
