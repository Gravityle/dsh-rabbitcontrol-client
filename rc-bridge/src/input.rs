//! 宿主输入入口：把 stdin 上的事件转成 `rc_input::InputEvent`。
//!
//! 宿主（DSH Host 半边）往边车 stdin 写与帧流同构的成帧消息：
//! `[kind u8][len u32 BE][payload]`，其中 `kind = 10` 的 payload 是一段 JSON：
//!
//! ```json
//! { "events": [
//!   { "t": "mm", "x": 32767, "y": 16384 },
//!   { "t": "mb", "b": 0, "p": true },
//!   { "t": "mw", "d": -1, "h": false },
//!   { "t": "k",  "sc": 30, "p": true, "mod": 2 }
//! ] }
//! ```
//!
//! - `mm` 指针绝对移动，坐标为归一化 0..=65535；
//! - `mb` 鼠标键，`b` 0=左 1=右 2=中 3=侧1 4=侧2；
//! - `mw` 滚轮，`d` 为行数（正=向上/向右），`h` 为是否水平；
//! - `k` 键盘，`sc` 是 Set 1 扫描码（E0 前缀键写成 `0xE0xx`，如右 Ctrl = 0xE01D），
//!   `mod` 是修饰键快照位（bit0 左 Ctrl、bit1 左 Shift、bit2 左 Alt、bit3 左 GUI，
//!   高四位对应右侧）。

use std::io::{BufReader, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rc_input::{InputCapture, InputEvent, MouseButton};
use rc_types::Result;

/// 宿主 → 边车的输入消息类型号。
pub const INPUT_KIND: u8 = 10;
/// 单条输入消息的字节上限（防止坏长度头把内存吃光）。
const MAX_INPUT_PAYLOAD: usize = 64 * 1024;

/// 从 stdin 读输入事件的通道。
#[derive(Clone)]
pub struct IpcCapture {
    rx: Arc<Mutex<Receiver<InputEvent>>>,
    stopped: Arc<AtomicBool>,
}

impl IpcCapture {
    /// 起一条读线程后构造。
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let reader_stop = stopped.clone();
        std::thread::Builder::new()
            .name("rc-bridge-input".into())
            .spawn(move || read_input(BufReader::new(std::io::stdin().lock()), tx, &reader_stop))
            .expect("输入读线程创建失败");
        Self { rx: Arc::new(Mutex::new(rx)), stopped }
    }

    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
}

fn read_input(mut reader: impl Read, tx: Sender<InputEvent>, stopped: &AtomicBool) {
    let mut header = [0; 5];
    while reader.read_exact(&mut header).is_ok() {
        let len = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
        if header[0] == 3 && len == 0 {
            stopped.store(true, Ordering::Release);
            break;
        }
        if len > MAX_INPUT_PAYLOAD {
            eprintln!("rc-bridge 输入通道：长度头 {len} 超限，停止读取");
            break;
        }
        let mut payload = vec![0; len];
        if reader.read_exact(&mut payload).is_err() { break; }
        if header[0] == INPUT_KIND {
            for event in parse_events(&payload) {
                if tx.send(event).is_err() { return; }
            }
        }
    }
}

impl InputCapture for IpcCapture {
    fn next_events(&mut self, timeout: Duration) -> Result<Vec<InputEvent>> {
        let rx = self.rx.lock().expect("input channel lock");
        let Ok(first) = rx.recv_timeout(timeout) else { return Ok(Vec::new()) };
        Ok(std::iter::once(first).chain(rx.try_iter()).collect())
    }
}

pub fn session_capture(input: Option<&IpcCapture>) -> Box<dyn InputCapture> {
    match input {
        Some(input) => Box::new(input.clone()),
        None => Box::new(crate::IdleCapture),
    }
}

/**
 * 解析一段输入 JSON。
 * @param payload - UTF-8 JSON 字节。
 * @returns 解析出的事件；无法识别的条目被跳过。
 */
pub fn parse_events(payload: &[u8]) -> Vec<InputEvent> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) else {
        eprintln!("rc-bridge 输入通道：JSON 解析失败，丢弃一条消息");
        return Vec::new();
    };
    let Some(items) = value.get("events").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let kind = item.get("t").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "mm" => {
                let x = item.get("x").and_then(serde_json::Value::as_u64).unwrap_or(0);
                let y = item.get("y").and_then(serde_json::Value::as_u64).unwrap_or(0);
                out.push(InputEvent::MouseMove {
                    x: u16::try_from(x.min(65_535)).unwrap_or(0),
                    y: u16::try_from(y.min(65_535)).unwrap_or(0),
                });
            }
            "mb" => {
                let button = match item.get("b").and_then(serde_json::Value::as_u64).unwrap_or(0) {
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    3 => MouseButton::X1,
                    4 => MouseButton::X2,
                    _ => MouseButton::Left,
                };
                let pressed = item.get("p").and_then(serde_json::Value::as_bool).unwrap_or(false);
                out.push(InputEvent::MouseButton { button, pressed });
            }
            "mw" => {
                let delta = item.get("d").and_then(serde_json::Value::as_i64).unwrap_or(0);
                let horizontal = item.get("h").and_then(serde_json::Value::as_bool).unwrap_or(false);
                out.push(InputEvent::MouseWheel {
                    delta: i16::try_from(delta.clamp(i64::from(i16::MIN), i64::from(i16::MAX))).unwrap_or(0),
                    horizontal,
                });
            }
            "k" => {
                let scancode = item.get("sc").and_then(serde_json::Value::as_u64).unwrap_or(0);
                let pressed = item.get("p").and_then(serde_json::Value::as_bool).unwrap_or(false);
                let modifiers = item.get("mod").and_then(serde_json::Value::as_u64).unwrap_or(0);
                out.push(InputEvent::Key {
                    scancode: u16::try_from(scancode.min(u64::from(u16::MAX))).unwrap_or(0),
                    pressed,
                    modifiers: u8::try_from(modifiers.min(255)).unwrap_or(0),
                });
            }
            _ => {}
        }
    }
    out
}

/// 解析自检：JSON → 事件的映射必须逐字段对上。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_requires_an_explicit_empty_kind_three_frame() {
        for (bytes, expected) in [(&[][..], false), (&[3, 0, 0, 0, 1, 7], false),
                                  (&[3, 0, 0], false), (&[3, 0, 0, 0, 0], true)] {
            let stop = AtomicBool::new(false);
            read_input(bytes, mpsc::channel().0, &stop);
            assert_eq!(stop.load(Ordering::Acquire), expected);
        }
    }

    #[test]
    fn parses_all_event_kinds() {
        let json = br#"{"events":[
            {"t":"mm","x":1,"y":2},
            {"t":"mb","b":2,"p":true},
            {"t":"mw","d":-3,"h":true},
            {"t":"k","sc":57437,"p":false,"mod":130},
            {"t":"??"}
        ]}"#;
        let events = parse_events(json);
        assert_eq!(events.len(), 4);
        assert_eq!(events[0], InputEvent::MouseMove { x: 1, y: 2 });
        assert_eq!(events[1], InputEvent::MouseButton { button: MouseButton::Middle, pressed: true });
        assert_eq!(events[2], InputEvent::MouseWheel { delta: -3, horizontal: true });
        assert_eq!(events[3], InputEvent::Key { scancode: 57437, pressed: false, modifiers: 130 });
    }

    #[test]
    fn ignores_malformed_json() {
        assert!(parse_events(b"not json").is_empty());
        assert!(parse_events(b"{}").is_empty());
    }
}
