//! ネットワーク（TCP・HTTP・WebSocket）。`stdlib::http` のように、上の階層から直接呼べるよう
//! `stdlib/mod.rs` で再公開している

#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "http")]
pub mod http_server;
pub mod net;
pub mod ws;
pub mod ws_proto;
