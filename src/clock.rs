//! 時計と待ち。ブラウザ（wasm32）では std の `Instant` と `SystemTime` が動かないので、
//! 同じ型を `web-time`（JS の時計）から使う

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::{Instant, SystemTime, UNIX_EPOCH};
#[cfg(target_arch = "wasm32")]
pub use web_time::{Instant, SystemTime, UNIX_EPOCH};

use std::time::Duration;

/// 待つ。ブラウザにはスレッドを止める手段がないので、Web Worker の中で時計を見て待つ
pub fn sleep(duration: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::sleep(duration);
    #[cfg(target_arch = "wasm32")]
    {
        let start = Instant::now();
        while start.elapsed() < duration {
            std::hint::spin_loop();
        }
    }
}
