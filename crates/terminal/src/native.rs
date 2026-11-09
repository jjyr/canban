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
    fn cb_terminal_auto_focus(view: *mut c_void, enabled: bool);
    fn cb_terminal_ui_command_keys(view: *mut c_void, json: *const i8);
}
struct State {
    model: Model,
    status: String,
    connected: bool,
    size_ready: bool,
    focused: bool,
    theme: Option<Value>,
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
    content: Option<String>,
    read_only: bool,
    input_status: String,
}
impl Host {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                model: Model::new(80, 24),
                status: String::new(),
                connected: false,
                size_ready: false,
                focused: false,
                theme: None,
            })),
            input: None,
            stop: Arc::new(AtomicBool::new(false)),
            json: CString::default(),
            seen: u64::MAX,
            seen_status: String::new(),
            binding: String::new(),
            content: None,
            read_only: false,
            input_status: String::new(),
        }
    }
    fn bind(&mut self, value: &Value) {
        let key = value.to_string();
        if self.binding == key {
            return;
        }
        self.binding = key;
        self.content = None;
        self.stop.store(true, Ordering::SeqCst);
        self.stop = Arc::new(AtomicBool::new(false));
        let theme = self.state.lock().unwrap().theme.clone();
        self.state = Arc::new(Mutex::new(State {
            model: Model::new(80, 24),
            status: "Connecting…".into(),
            connected: false,
            size_ready: false,
            focused: false,
            theme,
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
            // Layout arrives after set_prop. Attach at the real native size so
            // Herdr's initial history snapshot is not hard-wrapped at 80 cols.
            while !state.lock().unwrap().size_ready {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            while !stop.load(Ordering::SeqCst) {
                let attempt = (|| -> anyhow::Result<Attachment> {
                    anyhow::ensure!(
                        client.status()?["core"]["id"] == core,
                        "Original core identity changed"
                    );
                    let s = state.lock().unwrap();
                    let size = (s.model.cols, s.model.height);
                    drop(s);
                    {
                        let mut attachment = client.attach(&terminal, size.0, size.1)?;
                        attachment.track_view(&core, &terminal);
                        Ok(attachment)
                    }
                })();
                let attach = match attempt {
                    Ok(v) => v,
                    Err(e) => {
                        {
                            let mut s = state.lock().unwrap();
                            s.connected = false;
                            s.status = format!("Disconnected · {e:#} · reconnecting");
                        }
                        for _ in rx.try_iter() {} // Never replay input across a lost connection.
                        for _ in 0..10 {
                            if stop.load(Ordering::SeqCst) {
                                return;
                            }
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        continue;
                    }
                };
                for _ in rx.try_iter() {}
                {
                    let mut s = state.lock().unwrap();
                    let size = (s.model.cols, s.model.height);
                    s.model = Model::new(size.0, size.1);
                    if let Some(theme) = s.theme.clone() {
                        s.model.theme(&theme);
                    }
                    s.status = String::new();
                    // Size/focus may have changed while the attach handshake
                    // was in flight. Dropping stale keystrokes must not lose
                    // the latest geometry or focus state.
                    if let Err(e) = attach
                        .resize(size.0, size.1)
                        .and_then(|_| attach.focus(s.focused))
                    {
                        s.status = format!("Attach setup failed: {e} · reconnecting");
                        continue;
                    }
                    s.connected = true;
                }
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
                            let mut s = state.lock().unwrap();
                            s.connected = false;
                            s.status = format!(
                                "Terminal exited · {}",
                                reason["reason"].as_str().unwrap_or("closed")
                            );
                            return;
                        }
                        Ok(Some(Event::Lost(error))) => {
                            state.lock().unwrap().status =
                                format!("Disconnected · {error} · reconnecting");
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
                state.lock().unwrap().connected = false;
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
    state.size_ready = true;
    let cols = cols.clamp(2, 500);
    let rows = rows.clamp(1, 200);
    if (state.model.cols, state.model.height) != (cols, rows) {
        state.model.resize(cols, rows);
        if let Some(tx) = &host.input {
            let _ = tx.send(Input::Resize(cols, rows));
        }
    }
    let status = if host.read_only {
        let reason = if !host.input_status.is_empty() {
            host.input_status.as_str()
        } else if host.content.is_some() {
            "Preview"
        } else if !state.connected && state.status.starts_with("Connecting") {
            "Connecting"
        } else {
            "Disconnected"
        };
        if state.status.is_empty() || reason == "Closing" {
            reason.to_owned()
        } else if state.status.starts_with(reason) {
            state.status.clone()
        } else {
            format!("{reason} · {}", state.status)
        }
    } else {
        state.status.clone()
    };

    if state.model.revision == host.seen && status == host.seen_status {
        return std::ptr::null();
    }
    host.seen = state.model.revision;
    host.seen_status = status.clone();
    let f = state.model.frame();
    let rows:Vec<_>=f.rows.iter().map(|row|row.iter().map(|c|json!({"text":c.text,"fg":c.fg,"bg":c.bg,"bold":c.bold,"underline":c.underline,"tail":c.tail})).collect::<Vec<_>>()).collect();
    host.json=CString::new(json!({"rows":rows,"cursor":f.cursor.map(|(x,y)|vec![x,y]),"background":f.background,"cursor_color":f.cursor_color,"status":status}).to_string()).unwrap();
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
                GHOSTTY_KEY_A + (c.to_ascii_lowercase() as u32 - 'a' as u32)
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
    if !state.connected && matches!(v["kind"].as_str(), Some("text" | "key" | "paste")) {
        return;
    }
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
        Some("focus") => {
            state.focused = v["value"] == true;
            Input::Focus(state.focused)
        }
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
        if name == "ui_command_keys"
            && let Some(value) = value
        {
            let data = CString::new(value.to_string()).unwrap();
            unsafe {
                cb_terminal_ui_command_keys(
                    Retained::as_ptr(&self.view) as *mut c_void,
                    data.as_ptr(),
                )
            };
        }
        if name == "focus" {
            unsafe {
                cb_terminal_auto_focus(
                    Retained::as_ptr(&self.view) as *mut c_void,
                    value.is_some_and(|v| v["enabled"] == true),
                )
            };
        }
        if name == "theme"
            && let Some(theme) = value
        {
            let mut state = self.host.state.lock().unwrap();
            if state.theme.as_ref() != Some(theme) {
                state.model.theme(theme);
                state.theme = Some(theme.clone());
            }
        }
        if name == "input_status" {
            self.host.input_status = value.and_then(Value::as_str).unwrap_or("").to_owned();
        }
        if name == "read_only" {
            self.host.read_only = value == Some(&json!(true));
        }
        if name == "binding"
            && let Some(value) = value
        {
            self.host.bind(value);
        }
        // Static VT content uses the same renderer without a Core binding.
        // Replacing content is idempotent; preview data belongs to the caller.
        if name == "content" && self.host.binding.is_empty() {
            let content = value.and_then(Value::as_str);
            if self.host.content.as_deref() != content {
                self.host.content = content.map(str::to_owned);
                let mut state = self.host.state.lock().unwrap();
                state.model = Model::new(state.model.cols, state.model.height);
                state.model.write(content.unwrap_or("").as_bytes());
                if let Some(theme) = state.theme.clone() {
                    state.model.theme(&theme);
                }
                self.host.seen = u64::MAX;
            }
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
    fn read_only_observation_disables_input_and_updates_status_without_rebinding() {
        let mut host = Host::new();
        host.state.lock().unwrap().connected = true;
        let (tx, rx) = mpsc::channel();
        host.input = Some(tx);
        host.read_only = true;
        host.input_status = "Closing".into();
        let ptr = &mut host as *mut Host as *mut c_void;
        let value: Value =
            serde_json::from_slice(unsafe { CStr::from_ptr(frame(ptr, 80, 24)) }.to_bytes())
                .unwrap();
        assert_eq!(value["status"], "Closing");
        let text = CString::new(json!({"kind":"text","text":"offline"}).to_string()).unwrap();
        input(ptr, text.as_ptr());
        assert!(rx.try_recv().is_err());
        for reason in ["Connecting", "Disconnected"] {
            host.input_status = reason.into();
            let value: Value =
                serde_json::from_slice(unsafe { CStr::from_ptr(frame(ptr, 80, 24)) }.to_bytes())
                    .unwrap();
            assert_eq!(value["status"], reason);
            input(ptr, text.as_ptr());
            assert!(rx.try_recv().is_err());
        }
        host.read_only = false;
        let value: Value =
            serde_json::from_slice(unsafe { CStr::from_ptr(frame(ptr, 80, 24)) }.to_bytes())
                .unwrap();
        assert_eq!(value["status"], "");
        input(ptr, text.as_ptr());
        assert!(matches!(rx.try_recv(),Ok(Input::Bytes(bytes)) if !bytes.is_empty()));
    }
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
