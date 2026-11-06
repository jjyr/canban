use crate::{ffi::*, model::Model};
use canban_herdr::{
    Client,
    stream::{Attachment, Event},
};
use canban_types::Config;
use nsh_platform_macos::{
    Components, NativeComponent,
    objc2::{MainThreadMarker, rc::Retained},
    objc2_app_kit::NSView,
};
use serde_json::{Value, json};
use std::{
    ffi::{CStr, CString, c_void},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
unsafe extern "C" {
    fn cb_terminal_new(
        host: *mut c_void,
        frame: extern "C" fn(*mut c_void, u16, u16) -> *const i8,
        input: extern "C" fn(*mut c_void, *const i8),
    ) -> *mut NSView;
    fn cb_terminal_detach(view: *mut c_void);
}
struct State {
    model: Model,
    status: String,
}
enum Input {
    Bytes(Vec<u8>),
    Resize(u16, u16),
    Focus(bool),
}
struct Host {
    state: Arc<Mutex<State>>,
    input: Option<mpsc::Sender<Input>>,
    stop: Arc<AtomicBool>,
    json: CString,
    seen: u64,
    seen_status: String,
    binding: String,
    read_only: bool,
}
impl Host {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                model: Model::new(80, 24),
                status: String::new(),
            })),
            input: None,
            stop: Arc::new(AtomicBool::new(false)),
            json: CString::default(),
            seen: u64::MAX,
            seen_status: String::new(),
            binding: String::new(),
            read_only: false,
        }
    }
    fn bind(&mut self, value: &Value) {
        let key = value.to_string();
        if self.binding == key {
            return;
        }
        self.binding = key;
        self.stop.store(true, Ordering::SeqCst);
        self.stop = Arc::new(AtomicBool::new(false));
        self.state = Arc::new(Mutex::new(State {
            model: Model::new(80, 24),
            status: "Connecting…".into(),
        }));
        self.seen = u64::MAX;
        let Ok(config) = serde_json::from_value::<Config>(value["config"].clone()) else {
            return;
        };
        let Some(terminal) = value["terminal"].as_str().map(str::to_owned) else {
            return;
        };
        let Some(core) = value["core_id"].as_str().map(str::to_owned) else {
            return;
        };
        let state = self.state.clone();
        let stop = self.stop.clone();
        let (tx, rx) = mpsc::channel();
        self.input = Some(tx);
        std::thread::spawn(move || {
            let client = Client { config };
            while !stop.load(Ordering::SeqCst) {
                let attempt = (|| -> anyhow::Result<Attachment> {
                    anyhow::ensure!(
                        client.status()?["core"]["id"] == core,
                        "Original core identity changed"
                    );
                    let s = state.lock().unwrap();
                    let size = (s.model.cols, s.model.height);
                    drop(s);
                    client.attach(&terminal, size.0, size.1)
                })();
                let attach = match attempt {
                    Ok(v) => v,
                    Err(e) => {
                        state.lock().unwrap().status = format!("Could not attach: {e:#}");
                        break;
                    }
                };
                state.lock().unwrap().status = String::new();
                let mut reconnect = false;
                while !stop.load(Ordering::SeqCst) {
                    for input in rx.try_iter() {
                        let result = match input {
                            Input::Bytes(data) => attach.input(&data, false),
                            Input::Resize(c, r) => attach.resize(c, r),
                            Input::Focus(on) => attach.focus(on),
                        };
                        if let Err(e) = result {
                            state.lock().unwrap().status = e.to_string();
                            reconnect = true;
                            break;
                        }
                    }
                    if reconnect {
                        break;
                    }
                    match attach.recv(Duration::from_millis(15)) {
                        Ok(Some(Event::Data(data))) => {
                            let mut s = state.lock().unwrap();
                            s.model.write(&data);
                            let replies = s.model.replies();
                            drop(s);
                            let result = attach.consumed(data.len()).and_then(|_| {
                                if replies.is_empty() {
                                    Ok(())
                                } else {
                                    attach.input(&replies, true)
                                }
                            });
                            if result.is_err() {
                                reconnect = true;
                                break;
                            }
                        }
                        Ok(Some(Event::Resize(c, r))) => {
                            state.lock().unwrap().model.resize(c, r);
                        }
                        Ok(Some(Event::Closed(reason))) => {
                            state.lock().unwrap().status = format!(
                                "Terminal exited · {}",
                                reason["reason"].as_str().unwrap_or("closed")
                            );
                            return;
                        }
                        Ok(Some(Event::Lost(error))) => {
                            state.lock().unwrap().status =
                                format!("Connection lost · {error} · reconnecting");
                            reconnect = true;
                            break;
                        }
                        Err(e) => {
                            state.lock().unwrap().status = format!("Disconnected · {e}");
                            reconnect = true;
                            break;
                        }
                        _ => {}
                    }
                }
                drop(attach);
                if !reconnect {
                    break;
                }
                for _ in 0..10 {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        });
    }
}
extern "C" fn frame(host: *mut c_void, cols: u16, rows: u16) -> *const i8 {
    let host = unsafe { &mut *(host as *mut Host) };
    let mut state = host.state.lock().unwrap();
    let cols = cols.clamp(2, 500);
    let rows = rows.clamp(1, 200);
    if (state.model.cols, state.model.height) != (cols, rows) {
        state.model.resize(cols, rows);
        if let Some(tx) = &host.input {
            let _ = tx.send(Input::Resize(cols, rows));
        }
    }
    if state.model.revision == host.seen && state.status == host.seen_status {
        return std::ptr::null();
    }
    host.seen = state.model.revision;
    host.seen_status = state.status.clone();
    let f = state.model.frame();
    let rows:Vec<_>=f.rows.iter().map(|row|row.iter().map(|c|json!({"text":c.text,"fg":c.fg,"bg":c.bg,"bold":c.bold,"underline":c.underline,"tail":c.tail})).collect::<Vec<_>>()).collect();
    host.json=CString::new(json!({"rows":rows,"cursor":f.cursor.map(|(x,y)|vec![x,y]),"background":f.background,"status":state.status}).to_string()).unwrap();
    host.json.as_ptr()
}
fn key(code: u64, text: &str) -> GhosttyKey {
    match code {
        36 => GHOSTTY_KEY_ENTER,
        48 => GHOSTTY_KEY_TAB,
        51 => GHOSTTY_KEY_BACKSPACE,
        53 => GHOSTTY_KEY_ESCAPE,
        122 => GHOSTTY_KEY_F1,
        120 => GHOSTTY_KEY_F2,
        99 => GHOSTTY_KEY_F3,
        118 => GHOSTTY_KEY_F4,
        96 => GHOSTTY_KEY_F5,
        97 => GHOSTTY_KEY_F6,
        98 => GHOSTTY_KEY_F7,
        100 => GHOSTTY_KEY_F8,
        101 => GHOSTTY_KEY_F9,
        109 => GHOSTTY_KEY_F10,
        103 => GHOSTTY_KEY_F11,
        111 => GHOSTTY_KEY_F12,
        115 => GHOSTTY_KEY_HOME,
        116 => GHOSTTY_KEY_PAGE_UP,
        117 => GHOSTTY_KEY_DELETE,
        119 => GHOSTTY_KEY_END,
        121 => GHOSTTY_KEY_PAGE_DOWN,
        123 => GHOSTTY_KEY_ARROW_LEFT,
        124 => GHOSTTY_KEY_ARROW_RIGHT,
        125 => GHOSTTY_KEY_ARROW_DOWN,
        126 => GHOSTTY_KEY_ARROW_UP,
        _ => {
            if let Some(c) = text.chars().next().filter(|c| c.is_ascii_alphabetic()) {
                GHOSTTY_KEY_A + (c.to_ascii_lowercase() as i32 - 'a' as i32)
            } else {
                GHOSTTY_KEY_UNIDENTIFIED
            }
        }
    }
}
extern "C" fn input(host: *mut c_void, data: *const i8) {
    let host = unsafe { &mut *(host as *mut Host) };
    let Ok(v) = serde_json::from_slice::<Value>(unsafe { CStr::from_ptr(data) }.to_bytes()) else {
        return;
    };
    if host.read_only && matches!(v["kind"].as_str(), Some("text" | "key" | "paste")) {
        return;
    }
    let mut state = host.state.lock().unwrap();
    let text = v["text"].as_str().unwrap_or("");
    let command = match v["kind"].as_str() {
        Some("text") => Input::Bytes(state.model.key(GHOSTTY_KEY_UNIDENTIFIED, 0, Some(text))),
        Some("paste") => Input::Bytes(state.model.paste(text)),
        Some("key") => {
            let flags = v["mods"].as_u64().unwrap_or(0);
            let mut mods = 0u16;
            if flags & (1 << 17) != 0 {
                mods |= GHOSTTY_MODS_SHIFT as u16;
            }
            if flags & (1 << 18) != 0 {
                mods |= GHOSTTY_MODS_CTRL as u16;
            }
            if flags & (1 << 19) != 0 {
                mods |= GHOSTTY_MODS_ALT as u16;
            }
            Input::Bytes(state.model.key(
                key(v["code"].as_u64().unwrap_or(0), text),
                mods,
                if text.is_empty() { None } else { Some(text) },
            ))
        }
        Some("scroll") => {
            state
                .model
                .scroll(v["delta"].as_i64().unwrap_or(0) as isize);
            return;
        }
        Some("focus") => Input::Focus(v["value"] == true),
        _ => return,
    };
    drop(state);
    if let Some(tx) = &host.input {
        let _ = tx.send(command);
    }
}
struct Terminal {
    view: Retained<NSView>,
    host: Box<Host>,
}
impl Terminal {
    fn new() -> Self {
        MainThreadMarker::new().expect("Native terminal must be created on the UI thread");
        let mut host = Box::new(Host::new());
        let ptr = unsafe { cb_terminal_new(&mut *host as *mut _ as *mut c_void, frame, input) };
        Self {
            view: unsafe { Retained::from_raw(ptr) }.expect("Native terminal allocation failed"),
            host,
        }
    }
}
impl NativeComponent for Terminal {
    fn view(&self) -> Retained<NSView> {
        self.view.clone()
    }
    fn set_prop(&mut self, name: &str, value: Option<&nsh_core::JsonValue>) {
        if name == "read_only" {
            self.host.read_only = value == Some(&json!(true));
        }
        if name == "binding"
            && let Some(value) = value
        {
            self.host.bind(value);
        }
        if name == "fixture" && value == Some(&json!(true)) {
            self.host.state.lock().unwrap().model.write("\x1b[1;36mCanban native terminal\x1b[0m\x1b[?25l\r\nUnicode: 世界 · café\r\nSelection, scroll and paste use the production component.\r\n".as_bytes());
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe { cb_terminal_detach(Retained::as_ptr(&self.view) as *mut c_void) };
        self.host.stop.store(true, Ordering::SeqCst);
    }
}
pub fn register(components: &Components) {
    components.register("canban/terminal", Rc::new(|_| Box::new(Terminal::new())));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn function_and_navigation_keys_use_vt_encoding() {
        let mut model = Model::new(40, 8);
        assert_eq!(model.key(key(120, ""), 0, None), b"\x1bOQ");
        assert_eq!(model.key(key(123, ""), 0, None), b"\x1b[D");
        assert_eq!(
            model.key(key(8, "c"), GHOSTTY_MODS_CTRL as u16, Some("c")),
            b"\x03"
        );
    }
}
