//! Playground から呼ぶ入口。ソースを実行して、出力とエラーを返す。
//! `canvas.run` を使うプログラムは、画面を JS が 1 フレームずつ進める（`frame`）

use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::{self, Write};

use dolphin::Session;
use dolphin::ui::canvas::{Cmd, Detached, Input};
use wasm_bindgen::prelude::*;

thread_local! {
    static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static RUNNING: RefCell<Option<Running>> = const { RefCell::new(None) };
}

struct Running {
    session: Session<'static>,
    screen: Detached,
}

/// `log` の出力の置き場。大きさがないので、作っても何も確保しない
struct Out;

impl Write for Out {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        OUTPUT.with(|o| o.borrow_mut().extend_from_slice(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn take_output() -> String {
    OUTPUT.with(|o| String::from_utf8_lossy(&std::mem::take(&mut *o.borrow_mut())).into_owned())
}

#[wasm_bindgen(getter_with_clone)]
pub struct RunResult {
    pub output: String,
    /// 構文・実行時エラーの文面。なければ空
    pub error: String,
    /// エラーの行（1 始まり）。なければ 0
    pub line: u32,
    /// `canvas.run` で画面が開かれたか。true なら `frame` で進める
    pub canvas: bool,
    pub width: f32,
    pub height: f32,
    pub fps: u32,
    /// `#rrggbb`
    pub background: String,
}

#[wasm_bindgen]
pub fn run(src: &str) -> RunResult {
    stop();
    take_output();
    let out: &'static mut Out = Box::leak(Box::new(Out));
    let mut session = Session::new(out);
    let result = session.run(src);
    let output = take_output();
    let mut res = RunResult {
        output,
        error: String::new(),
        line: 0,
        canvas: false,
        width: 0.0,
        height: 0.0,
        fps: 60,
        background: String::new(),
    };
    if let Err(d) = result {
        res.error = d.render("main.dol", src);
        res.line = d.span.line as u32;
        return res;
    }
    if let Some(screen) = session.take_canvas() {
        let o = &screen.options;
        let [r, g, b, _] = o.background;
        res.canvas = true;
        res.width = o.window.width;
        res.height = o.window.height;
        res.fps = o.fps;
        res.background = format!("#{r:02x}{g:02x}{b:02x}");
        RUNNING.with(|s| *s.borrow_mut() = Some(Running { session, screen }));
    }
    res
}

/// 画面を閉じる
#[wasm_bindgen]
pub fn stop() {
    RUNNING.with(|s| *s.borrow_mut() = None);
}

#[wasm_bindgen(getter_with_clone)]
pub struct FrameResult {
    /// 描画命令の JSON（配列）。このフレームで描けなかったときは空
    pub cmds: String,
    pub output: String,
    pub error: String,
    pub quit: bool,
}

fn keys(s: &str) -> HashSet<String> {
    s.split_whitespace().map(str::to_owned).collect()
}

/// 画面を 1 フレーム進める。キーは空白区切りの名前（`left` `space` など）、
/// ボタンは下位 3 ビットが 左・右・中
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn frame(
    dt: f32,
    down: &str,
    pressed: &str,
    released: &str,
    mouse_x: f32,
    mouse_y: f32,
    inside: bool,
    buttons: u8,
    buttons_pressed: u8,
    wheel: f32,
) -> FrameResult {
    let bit = |bits: u8| [bits & 1 != 0, bits & 2 != 0, bits & 4 != 0];
    let input = Input {
        down: keys(down),
        pressed: keys(pressed),
        released: keys(released),
        mouse_x,
        mouse_y,
        inside,
        buttons: bit(buttons),
        buttons_pressed: bit(buttons_pressed),
        wheel,
    };
    let stepped = RUNNING.with(|s| {
        let mut s = s.borrow_mut();
        let running = s.as_mut()?;
        Some(running.screen.step(running.session.interpreter(), dt, input))
    });
    let output = take_output();
    match stepped {
        Some(Ok(f)) => FrameResult {
            cmds: f.cmds.as_deref().map(to_json).unwrap_or_default(),
            output,
            error: f.error.unwrap_or_default(),
            quit: f.quit,
        },
        Some(Err(error)) => {
            stop();
            FrameResult { cmds: String::new(), output, error, quit: true }
        }
        None => FrameResult { cmds: String::new(), output, error: String::new(), quit: true },
    }
}

// ---- 描画命令の JSON ----

fn to_json(cmds: &[Cmd]) -> String {
    let mut s = String::from("[");
    for (i, cmd) in cmds.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        write_cmd(&mut s, cmd);
    }
    s.push(']');
    s
}

fn color(c: &[u8; 4]) -> String {
    format!("[{},{},{},{}]", c[0], c[1], c[2], c[3])
}

fn stroke(s: Option<f32>) -> String {
    s.map_or("null".into(), |w| w.to_string())
}

fn write_cmd(s: &mut String, cmd: &Cmd) {
    let _ = match cmd {
        Cmd::Clear(c) => write!(s, r#"{{"t":"clear","c":{}}}"#, color(c)),
        Cmd::Rect { x, y, w, h, color: c, stroke: st } => write!(
            s,
            r#"{{"t":"rect","x":{x},"y":{y},"w":{w},"h":{h},"c":{},"s":{}}}"#,
            color(c),
            stroke(*st)
        ),
        Cmd::Circle { x, y, r, color: c, stroke: st } => write!(
            s,
            r#"{{"t":"circle","x":{x},"y":{y},"r":{r},"c":{},"s":{}}}"#,
            color(c),
            stroke(*st)
        ),
        Cmd::Line { x1, y1, x2, y2, color: c, width } => write!(
            s,
            r#"{{"t":"line","x1":{x1},"y1":{y1},"x2":{x2},"y2":{y2},"c":{},"w":{width}}}"#,
            color(c)
        ),
        Cmd::Polygon { points, color: c, stroke: st } => {
            let pts: Vec<String> = points.iter().map(|(x, y)| format!("[{x},{y}]")).collect();
            write!(
                s,
                r#"{{"t":"polygon","p":[{}],"c":{},"s":{}}}"#,
                pts.join(","),
                color(c),
                stroke(*st)
            )
        }
        Cmd::Text { x, y, text, color: c, size } => {
            write!(s, r#"{{"t":"text","x":{x},"y":{y},"text":"#)
                .and_then(|()| write_str(s, text))
                .and_then(|()| write!(s, r#","c":{},"z":{size}}}"#, color(c)))
        }
        // 画像はファイルを読めないので、Playground では描かない
        Cmd::Image { .. } => write!(s, r#"{{"t":"none"}}"#),
    };
}

fn write_str(s: &mut String, text: &str) -> std::fmt::Result {
    s.push('"');
    for c in text.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            c if (c as u32) < 0x20 => write!(s, "\\u{:04x}", c as u32)?,
            c => s.push(c),
        }
    }
    s.push('"');
    Ok(())
}
