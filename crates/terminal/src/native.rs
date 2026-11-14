use crate::{
    ffi::*,
    model::{KeyInput, Model, MouseInput, RenderSnapshot, WheelOutput},
};
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
    collections::VecDeque,
    ffi::{CString, c_void},
    rc::Rc,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CbGeometry {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u32,
    pub cell_height: u32,
    pub pad_left: u32,
    pub pad_right: u32,
    pub pad_top: u32,
    pub pad_bottom: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CbKeyEvent {
    pub action: u8,
    pub composing: u8,
    pub unidentified: u8,
    pub keycode: u16,
    pub mods: u16,
    pub consumed_mods: u16,
    pub unshifted_cp: u32,
    pub utf8_ptr: *const u8,
    pub utf8_len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CbMouseEvent {
    pub action: u8,
    pub button: u8,
    pub any_pressed: u8,
    pub alt_override: u8,
    pub mods: u16,
    pub x_px: f32,
    pub y_px: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CbSelectEvent {
    pub kind: u8,
    pub shift_extend: u8,
    pub rectangle: u8,
    pub x_px: f64,
    pub y_px: f64,
    pub time_ns: u64,
    pub repeat_interval_ns: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CbLinkSpan {
    pub found: u8,
    pub row: u16,
    pub start_col: u16,
    pub end_col: u16,
    pub uri_ptr: *const u8,
    pub uri_len: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CbCallbacks {
    pub snapshot: extern "C" fn(*mut c_void, CbGeometry, bool) -> *const RenderSnapshot,
    pub key: extern "C" fn(*mut c_void, CbKeyEvent) -> bool,
    pub text: extern "C" fn(*mut c_void, *const u8, usize),
    pub paste: extern "C" fn(*mut c_void, *const u8, usize),
    pub mouse: extern "C" fn(*mut c_void, CbMouseEvent) -> bool,
    pub wheel: extern "C" fn(*mut c_void, i32, i32, f32, f32, u16, bool),
    pub select: extern "C" fn(*mut c_void, CbSelectEvent) -> i8,
    pub copy_text: extern "C" fn(*mut c_void, *mut usize) -> *const u8,
    pub has_selection: extern "C" fn(*mut c_void) -> bool,
    pub link_at: extern "C" fn(*mut c_void, f64, f64) -> CbLinkSpan,
    pub focus: extern "C" fn(*mut c_void, bool),
    pub clear_screen: extern "C" fn(*mut c_void),
    pub accessibility_text: extern "C" fn(*mut c_void, *mut usize) -> *const u8,
    pub take_clipboard: extern "C" fn(*mut c_void, *mut usize) -> *const u8,
    pub take_bell: extern "C" fn(*mut c_void) -> bool,
}

unsafe extern "C" {
    fn cb_terminal_new(host: *mut c_void, callbacks: CbCallbacks) -> *mut NSView;
    fn cb_terminal_detach(view: *mut c_void);
    fn cb_terminal_wake(view: *mut c_void);
    fn cb_terminal_auto_focus(view: *mut c_void, enabled: bool);
    fn cb_terminal_ui_command_keys(view: *mut c_void, json: *const i8);
    fn cb_terminal_set_font(view: *mut c_void, family: *const i8, size: f64);
    fn cb_terminal_set_theme_bg(view: *mut c_void, r: u8, g: u8, b: u8);
    fn cb_terminal_sync_rebind(view: *mut c_void);
    fn cb_terminal_list_fonts_json() -> *mut i8;
    fn cb_terminal_free_cstring(ptr: *mut i8);
    #[cfg(test)]
    fn cb_terminal_test_mutable_ime_buffers(view: *mut c_void) -> bool;
}

pub fn installed_fonts() -> crate::InstalledFonts {
    use std::{ffi::CStr, sync::OnceLock};
    static CACHED: OnceLock<crate::InstalledFonts> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            let ptr = unsafe { cb_terminal_list_fonts_json() };
            if ptr.is_null() {
                return crate::InstalledFonts::default();
            }
            let json_str = unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned();
            unsafe { cb_terminal_free_cstring(ptr) };
            let Ok(val) = serde_json::from_str::<Value>(&json_str) else {
                return crate::InstalledFonts::default();
            };
            let parse_list = |key: &str| -> Vec<String> {
                val[key]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            };
            crate::InstalledFonts {
                monospaced: parse_list("monospaced"),
                others: parse_list("others"),
            }
        })
        .clone()
}

const CONNECTING_GRACE: Duration = Duration::from_millis(180);
const MAX_POOLED_SESSIONS: usize = 8;

struct WakeHandle {
    views: Mutex<Vec<usize>>,
    pending: AtomicBool,
}

impl WakeHandle {
    fn new() -> Self {
        Self {
            views: Mutex::new(Vec::new()),
            pending: AtomicBool::new(false),
        }
    }

    fn attach(&self, ptr: *mut NSView) {
        let p = ptr as usize;
        if p == 0 {
            return;
        }
        let mut views = self.views.lock().unwrap();
        if !views.contains(&p) {
            views.push(p);
        }
    }

    fn detach(&self, ptr: *mut NSView) {
        let p = ptr as usize;
        if p == 0 {
            return;
        }
        let mut views = self.views.lock().unwrap();
        views.retain(|&v| v != p);
        if views.is_empty() {
            self.pending.store(false, Ordering::SeqCst);
        }
    }

    fn notify(&self) {
        let snapshot: Vec<usize> = self.views.lock().unwrap().clone();
        if snapshot.is_empty() {
            return;
        }
        if snapshot.len() == 1 {
            if !self.pending.swap(true, Ordering::SeqCst) {
                unsafe { cb_terminal_wake(snapshot[0] as *mut c_void) };
            }
            return;
        }
        for ptr in snapshot {
            unsafe { cb_terminal_wake(ptr as *mut c_void) };
        }
    }

    fn ack(&self) {
        self.pending.store(false, Ordering::SeqCst);
    }
}

struct State {
    model: Model,
    status: String,
    connected: bool,
    exited: bool,
    connecting_since: Option<Instant>,
    size_ready: bool,
    focused: bool,
    theme: Option<Value>,
}

#[derive(Debug)]
enum Input {
    Bytes(Vec<u8>),
    Resize(u16, u16),
    Focus(bool),
}

struct LiveSession {
    key: (String, String),
    state: Arc<Mutex<State>>,
    size_cv: Arc<Condvar>,
    wake: Arc<WakeHandle>,
    input: mpsc::Sender<Input>,
    stop: Arc<AtomicBool>,
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.size_cv.notify_all();
    }
}

fn session_pool() -> &'static Mutex<VecDeque<Arc<LiveSession>>> {
    static POOL: OnceLock<Mutex<VecDeque<Arc<LiveSession>>>> = OnceLock::new();
    POOL.get_or_init(Default::default)
}

pub fn evict_session(core_id: &str, terminal: &str) {
    let mut pool = session_pool().lock().unwrap();
    pool.retain(|s| {
        let keep = !(s.key.0 == core_id && s.key.1 == terminal);
        if !keep {
            s.stop.store(true, Ordering::SeqCst);
            s.size_cv.notify_all();
        }
        keep
    });
}

struct SessionInit {
    config: Config,
    core: String,
    terminal: String,
    initial_cols: u16,
    initial_rows: u16,
    size_ready: bool,
    focused: bool,
    theme: Option<Value>,
}

fn spawn_session(init: SessionInit) -> Arc<LiveSession> {
    let SessionInit {
        config,
        core,
        terminal,
        initial_cols,
        initial_rows,
        size_ready,
        focused,
        theme,
    } = init;
    let mut model = Model::new(initial_cols.max(2), initial_rows.max(1));
    if let Some(ref t) = theme {
        model.theme(t);
    }
    let state = Arc::new(Mutex::new(State {
        model,
        status: "Connecting…".into(),
        connected: false,
        exited: false,
        connecting_since: Some(Instant::now()),
        size_ready,
        focused,
        theme,
    }));
    let size_cv = Arc::new(Condvar::new());
    let wake = Arc::new(WakeHandle::new());
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let rx = Arc::new(Mutex::new(rx));

    // Delayed wake so slow/remote connects reveal the "Connecting…" pill cleanly after grace.
    {
        let state_grace = state.clone();
        let wake_grace = wake.clone();
        let stop_grace = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(CONNECTING_GRACE);
            if stop_grace.load(Ordering::SeqCst) {
                return;
            }
            let still_connecting = {
                let s = state_grace.lock().unwrap();
                !s.connected && s.status == "Connecting…"
            };
            if still_connecting {
                wake_grace.notify();
            }
        });
    }

    let session = Arc::new(LiveSession {
        key: (core.clone(), terminal.clone()),
        state: state.clone(),
        size_cv: size_cv.clone(),
        wake: wake.clone(),
        input: tx,
        stop: stop.clone(),
    });

    std::thread::spawn(move || {
        let relay_fenced = config.relay_endpoint.is_some();
        let client = Client { config };
        {
            let mut guard = state.lock().unwrap();
            while !guard.size_ready {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                let (next, _) = size_cv
                    .wait_timeout(guard, Duration::from_millis(50))
                    .unwrap();
                guard = next;
            }
        }
        let mut first_attach = true;
        while !stop.load(Ordering::SeqCst) {
            let attempt = (|| -> anyhow::Result<(Attachment, (u16, u16))> {
                if !relay_fenced {
                    anyhow::ensure!(
                        client.status()?["core"]["id"] == core,
                        "Original core identity changed"
                    );
                }
                let size = {
                    let s = state.lock().unwrap();
                    (s.model.cols, s.model.height)
                };
                let mut attachment =
                    client.attach_fenced(&terminal, size.0, size.1, Some(&core))?;
                if !relay_fenced {
                    attachment.track_view(&core, &terminal);
                }
                Ok((attachment, size))
            })();
            let (attach, attached_size) = match attempt {
                Ok(v) => v,
                Err(e) => {
                    {
                        let mut s = state.lock().unwrap();
                        s.connected = false;
                        s.connecting_since = None;
                        s.status = format!("Disconnected · {e:#} · reconnecting");
                    }
                    wake.notify();
                    for _ in rx.lock().unwrap().try_iter() {}
                    for _ in 0..10 {
                        if stop.load(Ordering::SeqCst) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    continue;
                }
            };
            let was_first_attach = first_attach;
            if !was_first_attach {
                for _ in rx.lock().unwrap().try_iter() {}
            }
            first_attach = false;
            {
                let mut s = state.lock().unwrap();
                let size = (s.model.cols, s.model.height);
                if !was_first_attach {
                    s.model = Model::new(size.0, size.1);
                    if let Some(theme) = s.theme.clone() {
                        s.model.theme(&theme);
                    }
                }
                s.status = String::new();
                s.connecting_since = None;
                let mut setup_res = if s.focused {
                    attach.focus(true)
                } else {
                    Ok(())
                };
                // Layout may have resized the view while attach was connecting or across a reconnect.
                if setup_res.is_ok() && s.size_ready && size != attached_size {
                    setup_res = attach.resize(size.0, size.1);
                }
                if let Err(e) = setup_res {
                    s.status = format!("Attach setup failed: {e} · reconnecting");
                    drop(s);
                    wake.notify();
                    continue;
                }
                s.connected = true;
            }
            wake.notify();

            let input_stop = Arc::new(AtomicBool::new(false));
            let failed = Arc::new(AtomicBool::new(false));
            let input_worker = {
                let writer = attach.input_writer();
                let rx = rx.clone();
                let stopped = input_stop.clone();
                let failed = failed.clone();
                let state = state.clone();
                let wake = wake.clone();
                std::thread::spawn(move || {
                    while !stopped.load(Ordering::SeqCst) {
                        let message = rx.lock().unwrap().recv_timeout(Duration::from_millis(20));
                        let result = match message {
                            Ok(Input::Bytes(bytes)) => writer.input(&bytes),
                            Ok(Input::Resize(c, r)) => writer.resize(c, r),
                            Ok(Input::Focus(on)) => writer.focus(on),
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(_) => break,
                        };
                        if let Err(e) = result {
                            state.lock().unwrap().status = format!("Input disconnected: {e:#}");
                            failed.store(true, Ordering::SeqCst);
                            wake.notify();
                            break;
                        }
                    }
                })
            };
            let mut reconnect = false;
            while !stop.load(Ordering::SeqCst) && !failed.load(Ordering::SeqCst) {
                match attach.recv(Duration::from_millis(15)) {
                    Ok(Some(Event::Data(data))) => {
                        let replies = {
                            let mut s = state.lock().unwrap();
                            s.model.write(&data);
                            s.model.replies()
                        };
                        wake.notify();
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
                        let mut s = state.lock().unwrap();
                        if !s.size_ready {
                            s.model.resize(c, r);
                            drop(s);
                            wake.notify();
                        }
                    }
                    Ok(Some(Event::Closed(reason))) => {
                        {
                            let mut s = state.lock().unwrap();
                            s.connected = false;
                            s.exited = true;
                            s.connecting_since = None;
                            s.status = format!(
                                "Terminal exited · {}",
                                reason["reason"].as_str().unwrap_or("closed")
                            );
                        }
                        wake.notify();
                        input_stop.store(true, Ordering::SeqCst);
                        let _ = input_worker.join();
                        evict_session(&core, &terminal);
                        return;
                    }
                    Ok(Some(Event::Lost(error))) => {
                        state.lock().unwrap().status =
                            format!("Disconnected · {error} · reconnecting");
                        wake.notify();
                        reconnect = true;
                        break;
                    }
                    Err(e) => {
                        state.lock().unwrap().status = format!("Disconnected · {e}");
                        wake.notify();
                        reconnect = true;
                        break;
                    }
                    _ => {}
                }
            }
            input_stop.store(true, Ordering::SeqCst);
            let _ = input_worker.join();
            reconnect |= failed.load(Ordering::SeqCst);
            state.lock().unwrap().connected = false;
            wake.notify();
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

    session
}

fn get_or_spawn_session(init: SessionInit) -> Arc<LiveSession> {
    let mut pool = session_pool().lock().unwrap();
    pool.retain(|s| !s.stop.load(Ordering::SeqCst) && !s.state.lock().unwrap().exited);
    if let Some(idx) = pool
        .iter()
        .position(|s| s.key.0 == init.core && s.key.1 == init.terminal)
    {
        let existing = pool.remove(idx).unwrap();
        {
            let mut s = existing.state.lock().unwrap();
            if let Some(ref t) = init.theme
                && s.theme.as_ref() != Some(t)
            {
                s.model.theme(t);
                s.theme = Some(t.clone());
            }
            if init.size_ready && !s.size_ready {
                s.size_ready = true;
                existing.size_cv.notify_all();
            }
        }
        pool.push_back(existing.clone());
        return existing;
    }
    let created = spawn_session(init);
    while pool.len() >= MAX_POOLED_SESSIONS {
        // Prefer evicting a background session that currently has no attached NSView.
        if let Some(idle_idx) = pool
            .iter()
            .position(|s| s.wake.views.lock().unwrap().is_empty())
        {
            if let Some(evicted) = pool.remove(idle_idx) {
                evicted.stop.store(true, Ordering::SeqCst);
                evicted.size_cv.notify_all();
            }
        } else {
            break;
        }
    }
    pool.push_back(created.clone());
    created
}

struct Host {
    state: Arc<Mutex<State>>,
    size_cv: Arc<Condvar>,
    wake: Arc<WakeHandle>,
    input: Option<mpsc::Sender<Input>>,
    session: Option<Arc<LiveSession>>,
    view_ptr: *mut NSView,
    theme: Option<Value>,
    seen: u64,
    seen_status: String,
    status_scratch: String,
    copy_scratch: String,
    link_scratch: String,
    ax_scratch: String,
    clipboard_scratch: String,
    snapshot_box: RenderSnapshot,
    binding: String,
    content: Option<String>,
    read_only: bool,
    input_status: String,
    font_family: Option<String>,
    font_size: f64,
}

impl Host {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                model: Model::new(80, 24),
                status: String::new(),
                connected: false,
                exited: false,
                connecting_since: None,
                size_ready: false,
                focused: false,
                theme: None,
            })),
            size_cv: Arc::new(Condvar::new()),
            wake: Arc::new(WakeHandle::new()),
            input: None,
            session: None,
            view_ptr: std::ptr::null_mut(),
            theme: None,
            seen: u64::MAX,
            seen_status: String::new(),
            status_scratch: String::new(),
            copy_scratch: String::new(),
            link_scratch: String::new(),
            ax_scratch: String::new(),
            clipboard_scratch: String::new(),
            snapshot_box: RenderSnapshot {
                cells: std::ptr::null(),
                utf16: std::ptr::null(),
                dirty_rows: std::ptr::null(),
                status_ptr: std::ptr::null(),
                status_len: 0,
                cols: 0,
                rows: 0,
                dirty_state: 0,
                mouse_tracking: 0,
                background: [24, 24, 28],
                foreground: [224, 228, 236],
                cursor: Default::default(),
                scrollbar: Default::default(),
            },
            binding: String::new(),
            content: None,
            read_only: false,
            input_status: String::new(),
            font_family: None,
            font_size: 13.0,
        }
    }

    fn accepts_pty_input(&self, state: &State) -> bool {
        if self.read_only {
            return false;
        }
        state.connected || state.status.starts_with("Connecting")
    }

    fn send_bytes(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        if let Some(tx) = &self.input {
            let _ = tx.send(Input::Bytes(bytes));
        }
    }

    fn effective_status(&self, state: &State) -> String {
        if self.read_only {
            let reason = if !self.input_status.is_empty() {
                self.input_status.as_str()
            } else if self.content.is_some() {
                "Preview"
            } else if !state.connected && state.status.starts_with("Connecting") {
                "Connecting"
            } else {
                "Disconnected"
            };
            if state.status.is_empty()
                || reason == "Closing"
                || (reason == "Connecting" && state.status == "Connecting…")
            {
                reason.to_owned()
            } else if state.status.starts_with(reason) {
                state.status.clone()
            } else {
                format!("{reason} · {}", state.status)
            }
        } else if !state.connected
            && state.status == "Connecting…"
            && state
                .connecting_since
                .is_some_and(|t| t.elapsed() < CONNECTING_GRACE)
        {
            String::new()
        } else {
            state.status.clone()
        }
    }

    fn detach_current_session(&mut self) {
        if let Some(old_session) = self.session.take() {
            old_session.wake.detach(self.view_ptr);
            if old_session.wake.views.lock().unwrap().is_empty() {
                let was_focused = {
                    let mut s = old_session.state.lock().unwrap();
                    let was = s.focused;
                    s.focused = false;
                    was
                };
                if was_focused {
                    let _ = old_session.input.send(Input::Focus(false));
                }
            }
        }
    }

    fn bind(&mut self, value: &Value) -> bool {
        let key = value.to_string();
        if self.binding == key {
            return false;
        }
        let prev_focused = self.state.lock().unwrap().focused;
        let (prev_cols, prev_rows, prev_ready) = {
            let s = self.state.lock().unwrap();
            (s.model.cols, s.model.height, s.size_ready)
        };
        let theme = self
            .theme
            .clone()
            .or_else(|| self.state.lock().unwrap().theme.clone());
        self.detach_current_session();
        self.binding = key;
        self.content = None;

        let Ok(config) = serde_json::from_value::<Config>(value["config"].clone()) else {
            return true;
        };
        let Some(terminal) = value["terminal"].as_str().map(str::to_owned) else {
            return true;
        };
        let Some(core) = value["core_id"].as_str().map(str::to_owned) else {
            return true;
        };

        let session = get_or_spawn_session(SessionInit {
            config,
            core,
            terminal,
            initial_cols: prev_cols.max(2),
            initial_rows: prev_rows.max(1),
            size_ready: prev_ready,
            focused: prev_focused,
            theme,
        });
        if !self.view_ptr.is_null() {
            session.wake.attach(self.view_ptr);
        }
        let focus_seq = {
            let mut s = session.state.lock().unwrap();
            if s.focused != prev_focused {
                s.focused = prev_focused;
                let seq = if !self.read_only && (s.connected || s.status.starts_with("Connecting"))
                {
                    s.model.focus_sequence(prev_focused)
                } else {
                    Vec::new()
                };
                Some((prev_focused, seq))
            } else {
                None
            }
        };
        if let Some((focused, seq)) = focus_seq {
            let _ = session.input.send(Input::Focus(focused));
            if !seq.is_empty() {
                let _ = session.input.send(Input::Bytes(seq));
            }
        }
        self.state = session.state.clone();
        self.size_cv = session.size_cv.clone();
        self.wake = session.wake.clone();
        self.input = Some(session.input.clone());
        self.session = Some(session);
        self.seen = u64::MAX;
        self.seen_status.clear();
        true
    }
}

/// Complete macOS virtual keycode (`kVK_*`, `0x00..=0x7E`) mapping to `GhosttyKey`.
pub fn macos_key(code: u16) -> GhosttyKey {
    match code {
        0x00 => GHOSTTY_KEY_A,
        0x01 => GHOSTTY_KEY_S,
        0x02 => GHOSTTY_KEY_D,
        0x03 => GHOSTTY_KEY_F,
        0x04 => GHOSTTY_KEY_H,
        0x05 => GHOSTTY_KEY_G,
        0x06 => GHOSTTY_KEY_Z,
        0x07 => GHOSTTY_KEY_X,
        0x08 => GHOSTTY_KEY_C,
        0x09 => GHOSTTY_KEY_V,
        0x0A => GHOSTTY_KEY_INTL_BACKSLASH,
        0x0B => GHOSTTY_KEY_B,
        0x0C => GHOSTTY_KEY_Q,
        0x0D => GHOSTTY_KEY_W,
        0x0E => GHOSTTY_KEY_E,
        0x0F => GHOSTTY_KEY_R,
        0x10 => GHOSTTY_KEY_Y,
        0x11 => GHOSTTY_KEY_T,
        0x12 => GHOSTTY_KEY_DIGIT_1,
        0x13 => GHOSTTY_KEY_DIGIT_2,
        0x14 => GHOSTTY_KEY_DIGIT_3,
        0x15 => GHOSTTY_KEY_DIGIT_4,
        0x16 => GHOSTTY_KEY_DIGIT_6,
        0x17 => GHOSTTY_KEY_DIGIT_5,
        0x18 => GHOSTTY_KEY_EQUAL,
        0x19 => GHOSTTY_KEY_DIGIT_9,
        0x1A => GHOSTTY_KEY_DIGIT_7,
        0x1B => GHOSTTY_KEY_MINUS,
        0x1C => GHOSTTY_KEY_DIGIT_8,
        0x1D => GHOSTTY_KEY_DIGIT_0,
        0x1E => GHOSTTY_KEY_BRACKET_RIGHT,
        0x1F => GHOSTTY_KEY_O,
        0x20 => GHOSTTY_KEY_U,
        0x21 => GHOSTTY_KEY_BRACKET_LEFT,
        0x22 => GHOSTTY_KEY_I,
        0x23 => GHOSTTY_KEY_P,
        0x24 => GHOSTTY_KEY_ENTER,
        0x25 => GHOSTTY_KEY_L,
        0x26 => GHOSTTY_KEY_J,
        0x27 => GHOSTTY_KEY_QUOTE,
        0x28 => GHOSTTY_KEY_K,
        0x29 => GHOSTTY_KEY_SEMICOLON,
        0x2A => GHOSTTY_KEY_BACKSLASH,
        0x2B => GHOSTTY_KEY_COMMA,
        0x2C => GHOSTTY_KEY_SLASH,
        0x2D => GHOSTTY_KEY_N,
        0x2E => GHOSTTY_KEY_M,
        0x2F => GHOSTTY_KEY_PERIOD,
        0x30 => GHOSTTY_KEY_TAB,
        0x31 => GHOSTTY_KEY_SPACE,
        0x32 => GHOSTTY_KEY_BACKQUOTE,
        0x33 => GHOSTTY_KEY_BACKSPACE,
        0x35 => GHOSTTY_KEY_ESCAPE,
        0x36 => GHOSTTY_KEY_META_RIGHT,
        0x37 => GHOSTTY_KEY_META_LEFT,
        0x38 => GHOSTTY_KEY_SHIFT_LEFT,
        0x39 => GHOSTTY_KEY_CAPS_LOCK,
        0x3A => GHOSTTY_KEY_ALT_LEFT,
        0x3B => GHOSTTY_KEY_CONTROL_LEFT,
        0x3C => GHOSTTY_KEY_SHIFT_RIGHT,
        0x3D => GHOSTTY_KEY_ALT_RIGHT,
        0x3E => GHOSTTY_KEY_CONTROL_RIGHT,
        0x3F => GHOSTTY_KEY_FN,
        0x40 => GHOSTTY_KEY_F17,
        0x41 => GHOSTTY_KEY_NUMPAD_DECIMAL,
        0x43 => GHOSTTY_KEY_NUMPAD_MULTIPLY,
        0x45 => GHOSTTY_KEY_NUMPAD_ADD,
        0x47 => GHOSTTY_KEY_NUMPAD_CLEAR,
        0x48 => GHOSTTY_KEY_AUDIO_VOLUME_UP,
        0x49 => GHOSTTY_KEY_AUDIO_VOLUME_DOWN,
        0x4A => GHOSTTY_KEY_AUDIO_VOLUME_MUTE,
        0x4B => GHOSTTY_KEY_NUMPAD_DIVIDE,
        0x4C => GHOSTTY_KEY_NUMPAD_ENTER,
        0x4E => GHOSTTY_KEY_NUMPAD_SUBTRACT,
        0x4F => GHOSTTY_KEY_F18,
        0x50 => GHOSTTY_KEY_F19,
        0x51 => GHOSTTY_KEY_NUMPAD_EQUAL,
        0x52 => GHOSTTY_KEY_NUMPAD_0,
        0x53 => GHOSTTY_KEY_NUMPAD_1,
        0x54 => GHOSTTY_KEY_NUMPAD_2,
        0x55 => GHOSTTY_KEY_NUMPAD_3,
        0x56 => GHOSTTY_KEY_NUMPAD_4,
        0x57 => GHOSTTY_KEY_NUMPAD_5,
        0x58 => GHOSTTY_KEY_NUMPAD_6,
        0x59 => GHOSTTY_KEY_NUMPAD_7,
        0x5A => GHOSTTY_KEY_F20,
        0x5B => GHOSTTY_KEY_NUMPAD_8,
        0x5C => GHOSTTY_KEY_NUMPAD_9,
        0x5D => GHOSTTY_KEY_INTL_YEN,
        0x5E => GHOSTTY_KEY_INTL_RO,
        0x5F => GHOSTTY_KEY_NUMPAD_COMMA,
        0x60 => GHOSTTY_KEY_F5,
        0x61 => GHOSTTY_KEY_F6,
        0x62 => GHOSTTY_KEY_F7,
        0x63 => GHOSTTY_KEY_F3,
        0x64 => GHOSTTY_KEY_F8,
        0x65 => GHOSTTY_KEY_F9,
        0x66 => GHOSTTY_KEY_NON_CONVERT,
        0x67 => GHOSTTY_KEY_F11,
        0x68 => GHOSTTY_KEY_KANA_MODE,
        0x69 => GHOSTTY_KEY_F13,
        0x6A => GHOSTTY_KEY_F16,
        0x6B => GHOSTTY_KEY_F14,
        0x6D => GHOSTTY_KEY_F10,
        0x6E => GHOSTTY_KEY_CONTEXT_MENU,
        0x6F => GHOSTTY_KEY_F12,
        0x71 => GHOSTTY_KEY_F15,
        0x72 => GHOSTTY_KEY_INSERT,
        0x73 => GHOSTTY_KEY_HOME,
        0x74 => GHOSTTY_KEY_PAGE_UP,
        0x75 => GHOSTTY_KEY_DELETE,
        0x76 => GHOSTTY_KEY_F4,
        0x77 => GHOSTTY_KEY_END,
        0x78 => GHOSTTY_KEY_F2,
        0x79 => GHOSTTY_KEY_PAGE_DOWN,
        0x7A => GHOSTTY_KEY_F1,
        0x7B => GHOSTTY_KEY_ARROW_LEFT,
        0x7C => GHOSTTY_KEY_ARROW_RIGHT,
        0x7D => GHOSTTY_KEY_ARROW_DOWN,
        0x7E => GHOSTTY_KEY_ARROW_UP,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    }
}

extern "C" fn cb_snapshot(
    host_ptr: *mut c_void,
    geom: CbGeometry,
    force_full: bool,
) -> *const RenderSnapshot {
    let host = unsafe { &mut *(host_ptr as *mut Host) };
    host.wake.ack();

    let mut state = host.state.lock().unwrap();
    if geom.cols >= 2 && geom.rows >= 1 {
        let cols = geom.cols.clamp(2, 500);
        let rows = geom.rows.clamp(1, 200);
        let became_ready = !state.size_ready;
        state.size_ready = true;
        let grid_changed = state.model.resize_with_metrics(
            cols,
            rows,
            geom.cell_width.max(1),
            geom.cell_height.max(1),
            geom.pad_left,
            geom.pad_right,
            geom.pad_top,
            geom.pad_bottom,
        );
        if grid_changed && let Some(tx) = &host.input {
            let _ = tx.send(Input::Resize(cols, rows));
        }
        if became_ready {
            host.size_cv.notify_all();
        }
    }
    let replies = state.model.replies();
    let status = host.effective_status(&state);
    let status_changed = status != host.seen_status;
    let rev = state.model.revision;
    if !force_full && rev == host.seen && !status_changed {
        return std::ptr::null();
    }
    host.seen = rev;
    host.seen_status = status.clone();
    host.status_scratch = status;
    let mut snap = state
        .model
        .render_snapshot(&host.status_scratch, force_full);
    if status_changed && snap.dirty_state == 0 {
        snap.dirty_state = 1;
    }
    drop(state);
    if !replies.is_empty() {
        host.send_bytes(replies);
    }
    host.snapshot_box = snap;
    &host.snapshot_box as *const RenderSnapshot
}

extern "C" fn cb_key(host_ptr: *mut c_void, ev: CbKeyEvent) -> bool {
    let host = unsafe { &mut *(host_ptr as *mut Host) };
    let mut state = host.state.lock().unwrap();
    if !host.accepts_pty_input(&state) {
        return false;
    }
    let action = match ev.action {
        0 => GHOSTTY_KEY_ACTION_RELEASE,
        2 => GHOSTTY_KEY_ACTION_REPEAT,
        _ => GHOSTTY_KEY_ACTION_PRESS,
    };
    let utf8 = if ev.utf8_ptr.is_null() || ev.utf8_len == 0 {
        ""
    } else {
        std::str::from_utf8(unsafe { std::slice::from_raw_parts(ev.utf8_ptr, ev.utf8_len) })
            .unwrap_or("")
    };
    let key = if ev.unidentified != 0 {
        GHOSTTY_KEY_UNIDENTIFIED
    } else {
        macos_key(ev.keycode)
    };
    let encoded = state.model.encode_key(KeyInput {
        action,
        key,
        mods: ev.mods,
        consumed_mods: ev.consumed_mods,
        composing: ev.composing != 0,
        utf8,
        unshifted_codepoint: ev.unshifted_cp,
    });
    drop(state);
    if encoded.is_empty() {
        return false;
    }
    host.send_bytes(encoded);
    true
}

extern "C" fn cb_text(host_ptr: *mut c_void, ptr: *const u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    let host = unsafe { &mut *(host_ptr as *mut Host) };
    let Ok(text) = std::str::from_utf8(unsafe { std::slice::from_raw_parts(ptr, len) }) else {
        return;
    };
    let bytes = {
        let mut state = host.state.lock().unwrap();
        if !host.accepts_pty_input(&state) {
            return;
        }
        state.model.commit_text(text)
    };
    host.send_bytes(bytes);
}

extern "C" fn cb_paste(host_ptr: *mut c_void, ptr: *const u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    let host = unsafe { &mut *(host_ptr as *mut Host) };
    let Ok(text) = std::str::from_utf8(unsafe { std::slice::from_raw_parts(ptr, len) }) else {
        return;
    };
    let bytes = {
        let mut state = host.state.lock().unwrap();
        if !host.accepts_pty_input(&state) {
            return;
        }
        state.model.paste(text)
    };
    host.send_bytes(bytes);
}

extern "C" fn cb_mouse(host_ptr: *mut c_void, ev: CbMouseEvent) -> bool {
    if ev.alt_override != 0 {
        return false;
    }
    let host = unsafe { &mut *(host_ptr as *mut Host) };
    let mut state = host.state.lock().unwrap();
    if !host.accepts_pty_input(&state) || !state.model.mouse_tracking() {
        return false;
    }
    let action = match ev.action {
        1 => GHOSTTY_MOUSE_ACTION_RELEASE,
        2 => GHOSTTY_MOUSE_ACTION_MOTION,
        _ => GHOSTTY_MOUSE_ACTION_PRESS,
    };
    let button = match ev.button {
        1 => Some(GHOSTTY_MOUSE_BUTTON_LEFT),
        2 => Some(GHOSTTY_MOUSE_BUTTON_RIGHT),
        3 => Some(GHOSTTY_MOUSE_BUTTON_MIDDLE),
        _ => None,
    };
    let bytes = state.model.encode_mouse(MouseInput {
        action,
        button,
        mods: ev.mods,
        x_px: ev.x_px,
        y_px: ev.y_px,
        any_button_pressed: ev.any_pressed != 0,
    });
    drop(state);
    host.send_bytes(bytes);
    true
}

extern "C" fn cb_wheel(
    host_ptr: *mut c_void,
    delta_rows: i32,
    delta_cols: i32,
    x_px: f32,
    y_px: f32,
    mods: u16,
    alt_override: bool,
) {
    let host = unsafe { &mut *(host_ptr as *mut Host) };
    let out = {
        let mut state = host.state.lock().unwrap();
        let can_send = host.accepts_pty_input(&state);
        state.model.wheel(
            delta_rows as isize,
            delta_cols as isize,
            x_px,
            y_px,
            mods,
            alt_override || !can_send,
        )
    };
    if let WheelOutput::Pty(bytes) = out {
        host.send_bytes(bytes);
    }
}

extern "C" fn cb_select(host_ptr: *mut c_void, ev: CbSelectEvent) -> i8 {
    let host = unsafe { &mut *(host_ptr as *mut Host) };
    let mut state = host.state.lock().unwrap();
    match ev.kind {
        0 => {
            state.model.select_press(
                ev.x_px,
                ev.y_px,
                ev.time_ns,
                ev.repeat_interval_ns,
                ev.shift_extend != 0,
                ev.rectangle != 0,
            );
            0
        }
        1 => state.model.select_drag(ev.x_px, ev.y_px, ev.rectangle != 0),
        2 => {
            state.model.select_release(ev.x_px, ev.y_px);
            0
        }
        3 => state
            .model
            .select_autoscroll_tick(ev.x_px, ev.y_px, ev.rectangle != 0),
        4 => {
            state.model.select_all();
            0
        }
        5 => {
            state.model.clear_selection();
            0
        }
        _ => 0,
    }
}

extern "C" fn cb_copy_text(host_ptr: *mut c_void, out_len: *mut usize) -> *const u8 {
    let host = unsafe { &mut *(host_ptr as *mut Host) };
    let text = host.state.lock().unwrap().model.selected_text();
    match text {
        Some(s) if !s.is_empty() => {
            host.copy_scratch = s;
            if !out_len.is_null() {
                unsafe { *out_len = host.copy_scratch.len() };
            }
            host.copy_scratch.as_ptr()
        }
        _ => {
            if !out_len.is_null() {
                unsafe { *out_len = 0 };
            }
            std::ptr::null()
        }
    }
}

extern "C" fn cb_has_selection(host_ptr: *mut c_void) -> bool {
    let host = unsafe { &*(host_ptr as *const Host) };
    host.state.lock().unwrap().model.has_selection()
}

extern "C" fn cb_link_at(host_ptr: *mut c_void, x_px: f64, y_px: f64) -> CbLinkSpan {
    let host = unsafe { &mut *(host_ptr as *mut Host) };
    let found = host.state.lock().unwrap().model.link_at_px(x_px, y_px);
    match found {
        Some(link) => {
            host.link_scratch = link.uri;
            CbLinkSpan {
                found: 1,
                row: link.row,
                start_col: link.start_col,
                end_col: link.end_col,
                uri_ptr: host.link_scratch.as_ptr(),
                uri_len: host.link_scratch.len(),
            }
        }
        None => CbLinkSpan::default(),
    }
}

extern "C" fn cb_focus(host_ptr: *mut c_void, focused: bool) {
    let host = unsafe { &mut *(host_ptr as *mut Host) };
    let (changed, focus_seq) = {
        let mut state = host.state.lock().unwrap();
        let changed = state.focused != focused;
        state.focused = focused;
        let seq = if changed && host.accepts_pty_input(&state) {
            state.model.focus_sequence(focused)
        } else {
            Vec::new()
        };
        (changed, seq)
    };
    if !changed {
        return;
    }
    if let Some(tx) = &host.input {
        let _ = tx.send(Input::Focus(focused));
        if !focus_seq.is_empty() {
            let _ = tx.send(Input::Bytes(focus_seq));
        }
    }
}

extern "C" fn cb_clear_screen(host_ptr: *mut c_void) {
    let host = unsafe { &mut *(host_ptr as *mut Host) };
    let can_send = {
        let mut state = host.state.lock().unwrap();
        state.model.clear_selection();
        host.accepts_pty_input(&state)
    };
    if can_send {
        // Send Ctrl+L (form feed) to clear active shell/TUI cleanly without corrupting prompt state.
        host.send_bytes(b"\x0c".to_vec());
    }
}

extern "C" fn cb_accessibility_text(host_ptr: *mut c_void, out_len: *mut usize) -> *const u8 {
    let host = unsafe { &mut *(host_ptr as *mut Host) };
    let (mut text, status) = {
        let mut state = host.state.lock().unwrap();
        let text = state.model.viewport_text();
        let status = host.effective_status(&state);
        (text, status)
    };
    if !status.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&status);
    }
    host.ax_scratch = text;
    if !out_len.is_null() {
        unsafe { *out_len = host.ax_scratch.len() };
    }
    host.ax_scratch.as_ptr()
}

extern "C" fn cb_take_clipboard(host_ptr: *mut c_void, out_len: *mut usize) -> *const u8 {
    let host = unsafe { &mut *(host_ptr as *mut Host) };
    match host.state.lock().unwrap().model.take_clipboard() {
        Some(text) => {
            host.clipboard_scratch = text;
            if !out_len.is_null() {
                unsafe { *out_len = host.clipboard_scratch.len() };
            }
            host.clipboard_scratch.as_ptr()
        }
        None => {
            if !out_len.is_null() {
                unsafe { *out_len = 0 };
            }
            std::ptr::null()
        }
    }
}

extern "C" fn cb_take_bell(host_ptr: *mut c_void) -> bool {
    let host = unsafe { &*(host_ptr as *const Host) };
    host.state.lock().unwrap().model.take_bell()
}

fn callbacks() -> CbCallbacks {
    CbCallbacks {
        snapshot: cb_snapshot,
        key: cb_key,
        text: cb_text,
        paste: cb_paste,
        mouse: cb_mouse,
        wheel: cb_wheel,
        select: cb_select,
        copy_text: cb_copy_text,
        has_selection: cb_has_selection,
        link_at: cb_link_at,
        focus: cb_focus,
        clear_screen: cb_clear_screen,
        accessibility_text: cb_accessibility_text,
        take_clipboard: cb_take_clipboard,
        take_bell: cb_take_bell,
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
        let ptr = unsafe { cb_terminal_new(&mut *host as *mut _ as *mut c_void, callbacks()) };
        let view = unsafe { Retained::from_raw(ptr) }.expect("Native terminal allocation failed");
        let raw_view = Retained::as_ptr(&view) as *mut NSView;
        host.view_ptr = raw_view;
        host.wake.attach(raw_view);
        Self { view, host }
    }
}

impl NativeComponent for Terminal {
    fn view(&self) -> Retained<NSView> {
        self.view.clone()
    }

    fn set_prop(&mut self, name: &str, value: Option<&nsh_core::JsonValue>) {
        let view_ptr = Retained::as_ptr(&self.view) as *mut c_void;
        if name == "ui_command_keys"
            && let Some(value) = value
            && let Ok(data) = CString::new(value.to_string())
        {
            unsafe { cb_terminal_ui_command_keys(view_ptr, data.as_ptr()) };
        }
        if name == "focus" {
            unsafe {
                cb_terminal_auto_focus(view_ptr, value.is_some_and(|v| v["enabled"] == true))
            };
        }
        if name == "font" {
            let family = value
                .and_then(|v| v["family"].as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let size = value
                .and_then(|v| v["size"].as_f64())
                .unwrap_or(13.0)
                .clamp(9.0, 32.0);
            if self.host.font_family != family || (self.host.font_size - size).abs() > f64::EPSILON
            {
                self.host.font_family = family.clone();
                self.host.font_size = size;
                let c_family = family.and_then(|f| CString::new(f).ok());
                let fam_ptr = c_family.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
                unsafe { cb_terminal_set_font(view_ptr, fam_ptr, size) };
            }
        }
        if name == "theme"
            && let Some(theme) = value
        {
            self.host.theme = Some(theme.clone());
            if let Some(bg) = theme["background"].as_array()
                && bg.len() == 3
                && let (Some(r), Some(g), Some(b)) =
                    (bg[0].as_u64(), bg[1].as_u64(), bg[2].as_u64())
            {
                unsafe { cb_terminal_set_theme_bg(view_ptr, r as u8, g as u8, b as u8) };
            }
            let mut changed = false;
            {
                let mut state = self.host.state.lock().unwrap();
                if state.theme.as_ref() != Some(theme) {
                    state.model.theme(theme);
                    state.theme = Some(theme.clone());
                    changed = true;
                }
            }
            if changed {
                self.host.seen = u64::MAX;
                self.host.wake.notify();
            }
        }
        if name == "input_status" {
            let next = value.and_then(Value::as_str).unwrap_or("").to_owned();
            if self.host.input_status != next {
                self.host.input_status = next;
                self.host.wake.notify();
            }
        }
        if name == "read_only" {
            let next = value == Some(&json!(true));
            if self.host.read_only != next {
                self.host.read_only = next;
                self.host.wake.notify();
            }
        }
        if name == "binding"
            && let Some(value) = value
            && self.host.bind(value)
        {
            unsafe { cb_terminal_sync_rebind(view_ptr) };
        }
        if name == "content" && self.host.binding.is_empty() {
            let content = value.and_then(Value::as_str);
            if self.host.content.as_deref() != content {
                self.host.content = content.map(str::to_owned);
                {
                    let mut state = self.host.state.lock().unwrap();
                    state.model = Model::new(state.model.cols, state.model.height);
                    state.model.write(content.unwrap_or("").as_bytes());
                    if let Some(theme) = state.theme.clone() {
                        state.model.theme(&theme);
                    }
                }
                self.host.seen = u64::MAX;
                self.host.wake.notify();
            }
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let raw_view = Retained::as_ptr(&self.view) as *mut NSView;
        self.host.detach_current_session();
        self.host.wake.detach(raw_view);
        unsafe { cb_terminal_detach(raw_view as *mut c_void) };
    }
}

pub fn register(components: &Components) {
    components.register("canban/terminal", Rc::new(|_| Box::new(Terminal::new())));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_geom() -> CbGeometry {
        CbGeometry {
            cols: 80,
            rows: 24,
            cell_width: 8,
            cell_height: 18,
            pad_left: 6,
            pad_right: 6,
            pad_top: 6,
            pad_bottom: 6,
        }
    }

    #[test]
    fn first_connection_preserves_early_input_and_reconnection_discards_it() {
        let mut host = Host::new();
        host.state.lock().unwrap().status = "Connecting…".into();
        let (tx, rx) = mpsc::channel();
        host.input = Some(tx);
        let ptr = &mut host as *mut _ as *mut c_void;
        let text = b"early";
        cb_text(ptr, text.as_ptr(), text.len());
        assert!(matches!(rx.try_recv(), Ok(Input::Bytes(bytes)) if bytes == b"early"));
        host.state.lock().unwrap().status = "Disconnected".into();
        cb_text(ptr, text.as_ptr(), text.len());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn read_only_observation_disables_input_and_updates_status_without_rebinding() {
        let mut host = Host::new();
        host.state.lock().unwrap().connected = true;
        let (tx, rx) = mpsc::channel();
        host.input = Some(tx);
        host.read_only = true;
        host.input_status = "Closing".into();
        let ptr = &mut host as *mut Host as *mut c_void;
        let snap = unsafe { &*cb_snapshot(ptr, default_geom(), false) };
        let status = unsafe {
            std::str::from_utf8(std::slice::from_raw_parts(snap.status_ptr, snap.status_len))
                .unwrap()
        };
        assert_eq!(status, "Closing");
        let text = b"offline";
        cb_text(ptr, text.as_ptr(), text.len());
        assert!(rx.try_recv().is_err());

        for reason in ["Connecting", "Disconnected"] {
            host.input_status = reason.into();
            let snap = unsafe { &*cb_snapshot(ptr, default_geom(), false) };
            let status = unsafe {
                std::str::from_utf8(std::slice::from_raw_parts(snap.status_ptr, snap.status_len))
                    .unwrap()
            };
            assert_eq!(status, reason);
            cb_text(ptr, text.as_ptr(), text.len());
            assert!(rx.try_recv().is_err());
        }

        host.read_only = false;
        let snap = unsafe { &*cb_snapshot(ptr, default_geom(), false) };
        assert_eq!(snap.status_len, 0);
        cb_text(ptr, text.as_ptr(), text.len());
        assert!(matches!(rx.try_recv(), Ok(Input::Bytes(bytes)) if bytes == b"offline"));
    }

    #[test]
    fn macos_virtual_keycode_table_covers_letters_punctuation_and_navigation() {
        let mut model = Model::new(40, 8);
        assert_eq!(model.key(macos_key(0x78), 0, None), b"\x1bOQ");
        assert_eq!(model.key(macos_key(0x7B), 0, None), b"\x1b[D");
        assert_eq!(
            model.key(macos_key(0x08), GHOSTTY_MODS_CTRL as u16, Some("c")),
            b"\x03"
        );
        // Ctrl+Space (kVK_Space = 0x31) -> NUL (0x00)
        assert_eq!(
            model.encode_key(KeyInput {
                action: GHOSTTY_KEY_ACTION_PRESS,
                key: macos_key(0x31),
                mods: GHOSTTY_MODS_CTRL as u16,
                consumed_mods: 0,
                composing: false,
                utf8: " ",
                unshifted_codepoint: ' ' as u32,
            }),
            b"\x00"
        );
        // Option+B (kVK_ANSI_B = 0x0B) -> ESC b
        assert_eq!(
            model.encode_key(KeyInput {
                action: GHOSTTY_KEY_ACTION_PRESS,
                key: macos_key(0x0B),
                mods: GHOSTTY_MODS_ALT as u16,
                consumed_mods: 0,
                composing: false,
                utf8: "b",
                unshifted_codepoint: 'b' as u32,
            }),
            b"\x1bb"
        );
    }

    #[test]
    fn kitty_report_all_preserves_cjk_commit_and_synthetic_unicode_events() {
        let mut host = Host::new();
        host.state.lock().unwrap().connected = true;
        // Enable Kitty keyboard protocol flags = 15 (disambiguate | report_events | report_alternates | report_all).
        host.state.lock().unwrap().model.write(b"\x1b[>15u");
        let (tx, rx) = mpsc::channel();
        host.input = Some(tx);
        let ptr = &mut host as *mut Host as *mut c_void;

        // Direct UTF-8 text commit (e.g. WeType / Pinyin candidate, CJK punctuation "，", or multi-char paste/IME)
        // must send exact UTF-8 bytes rather than CSI-u keycode 'a' (97).
        let cjk = "你好，terminal！".as_bytes();
        cb_text(ptr, cjk.as_ptr(), cjk.len());
        assert!(matches!(rx.try_recv(), Ok(Input::Bytes(bytes)) if bytes == cjk));

        // Synthetic CGEvent Unicode key (keyCode = 0 with unidentified = 1) must not map to GHOSTTY_KEY_A.
        let ch = b"h";
        assert!(cb_key(
            ptr,
            CbKeyEvent {
                action: 1,
                composing: 0,
                unidentified: 1,
                keycode: 0,
                mods: 0,
                consumed_mods: 0,
                unshifted_cp: u32::from(b'h'),
                utf8_ptr: ch.as_ptr(),
                utf8_len: ch.len(),
            },
        ));
        assert!(matches!(rx.try_recv(), Ok(Input::Bytes(bytes)) if bytes == b"\x1b[104u"));

        // Physical 'a' key with CapsLock active still encodes as CSI-u 'a' with CapsLock consumed.
        let upper_a = b"A";
        assert!(cb_key(
            ptr,
            CbKeyEvent {
                action: 1,
                composing: 0,
                unidentified: 0,
                keycode: 0x00,
                mods: GHOSTTY_MODS_CAPS_LOCK as u16,
                consumed_mods: GHOSTTY_MODS_CAPS_LOCK as u16,
                unshifted_cp: u32::from(b'a'),
                utf8_ptr: upper_a.as_ptr(),
                utf8_len: upper_a.len(),
            },
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(Input::Bytes(bytes)) if bytes == b"\x1b[97;65u" || bytes == b"\x1b[97::65;65u" || bytes.starts_with(b"\x1b[97")
        ));
    }

    #[test]
    fn mutable_ime_buffers_are_snapshotted_before_imk_truncates_them() {
        let mut host = Box::new(Host::new());
        host.state.lock().unwrap().connected = true;
        host.state.lock().unwrap().model.write(b"\x1b[>15u");
        let (tx, rx) = mpsc::channel();
        host.input = Some(tx);

        let raw_view = unsafe { cb_terminal_new(&mut *host as *mut _ as *mut c_void, callbacks()) };
        assert!(!raw_view.is_null());
        let ok = unsafe { cb_terminal_test_mutable_ime_buffers(raw_view as *mut c_void) };
        unsafe {
            cb_terminal_detach(raw_view as *mut c_void);
            drop(Retained::from_raw(raw_view));
        }
        assert!(ok, "mutable IMK strings must be copied before truncation");
        assert!(matches!(
            rx.try_recv(),
            Ok(Input::Bytes(bytes)) if bytes == "你好！".as_bytes()
        ));
    }

    #[test]
    fn initial_connecting_grace_hides_status_pill_then_reveals_on_slow_connect() {
        let mut host = Host::new();
        {
            let mut s = host.state.lock().unwrap();
            s.status = "Connecting…".into();
            s.connecting_since = Some(Instant::now());
        }
        // During initial grace window, non-read-only pill is suppressed while early input still buffers.
        assert_eq!(host.effective_status(&host.state.lock().unwrap()), "");
        assert!(host.accepts_pty_input(&host.state.lock().unwrap()));

        // Read-only explicit status remains immediate even during grace.
        host.read_only = true;
        host.input_status = "Connecting".into();
        assert_eq!(
            host.effective_status(&host.state.lock().unwrap()),
            "Connecting"
        );
        host.read_only = false;

        // Once grace window elapses and still not connected, "Connecting…" is displayed.
        host.state.lock().unwrap().connecting_since =
            Some(Instant::now() - CONNECTING_GRACE - Duration::from_millis(10));
        assert_eq!(
            host.effective_status(&host.state.lock().unwrap()),
            "Connecting…"
        );
    }

    #[test]
    fn lru_session_pool_preserves_model_across_rebinds_and_evicts_cleanly() {
        let config = Config {
            hc: "/usr/bin/false".into(),
            hc_session: "unit-lru".into(),
            ..Default::default()
        };
        let mut host = Host::new();
        let bind_a = json!({"config": config, "core_id": "core-lru", "terminal": "tty-a"});
        let bind_b = json!({"config": config, "core_id": "core-lru", "terminal": "tty-b"});

        assert!(host.bind(&bind_a));
        host.state
            .lock()
            .unwrap()
            .model
            .write(b"retained-buffer-a\r\n");

        // Switch to terminal B on the same Host (tab switch), then switch back to A.
        assert!(host.bind(&bind_b));
        assert!(
            !host
                .state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("retained-buffer-a")
        );

        assert!(host.bind(&bind_a));
        assert!(
            host.state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("retained-buffer-a"),
            "Rebinding to a pooled terminal must immediately retain its parsed VT screen"
        );

        // Explicit eviction clears the pooled session.
        host.detach_current_session();
        evict_session("core-lru", "tty-a");
        evict_session("core-lru", "tty-b");
        host.binding.clear();
        assert!(host.bind(&bind_a));
        assert!(
            !host
                .state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("retained-buffer-a"),
            "Evicted terminal session must not retain stale state"
        );
        host.detach_current_session();
        evict_session("core-lru", "tty-a");
    }

    #[test]
    fn installed_fonts_discovers_system_monospaced_families() {
        let catalog = installed_fonts();
        assert!(
            catalog
                .monospaced
                .iter()
                .any(|f| f == "Menlo" || f == "Monaco" || f == "Courier New"),
            "expected system monospaced fonts in {:?}",
            catalog.monospaced
        );
    }

    #[test]
    fn unlaid_out_zero_bounds_do_not_clamp_terminal_to_two_columns_before_layout() {
        // 1. Pre-layout snapshot with zero cols/rows keeps size_ready false and preserves 80x24
        //    until real view geometry arrives.
        let mut host = Host::new();
        let (tx, rx) = mpsc::channel();
        host.input = Some(tx);
        let ptr = &mut host as *mut Host as *mut c_void;
        let zero_geom = CbGeometry {
            cols: 0,
            rows: 0,
            ..default_geom()
        };
        assert!(!cb_snapshot(ptr, zero_geom, true).is_null());
        {
            let s = host.state.lock().unwrap();
            assert!(!s.size_ready, "pre-layout 0x0 snapshot must not mark size_ready");
            assert_eq!((s.model.cols, s.model.height), (80, 24));
        }
        assert!(rx.try_recv().is_err(), "pre-layout 0x0 snapshot must not emit Resize");

        let real_geom = CbGeometry {
            cols: 96,
            rows: 38,
            ..default_geom()
        };
        assert!(!cb_snapshot(ptr, real_geom, false).is_null());
        {
            let s = host.state.lock().unwrap();
            assert!(s.size_ready);
            assert_eq!((s.model.cols, s.model.height), (96, 38));
        }
        assert!(matches!(rx.try_recv(), Ok(Input::Resize(96, 38))));

        // 2. End-to-end AppKit mount order: NSZeroRect -> set_font -> bind -> sync_rebind.
        let config = Config {
            hc: "/usr/bin/false".into(),
            hc_session: "unit-zero-bounds".into(),
            ..Default::default()
        };
        let mut mounted_host = Box::new(Host::new());
        let raw_view =
            unsafe { cb_terminal_new(&mut *mounted_host as *mut _ as *mut c_void, callbacks()) };
        assert!(!raw_view.is_null());
        let c_fam = CString::new("Menlo").unwrap();
        unsafe { cb_terminal_set_font(raw_view as *mut c_void, c_fam.as_ptr(), 13.0) };
        let bind_val = json!({"config": config, "core_id": "core-zero", "terminal": "tty-zero"});
        assert!(mounted_host.bind(&bind_val));
        unsafe { cb_terminal_sync_rebind(raw_view as *mut c_void) };
        {
            let s = mounted_host.state.lock().unwrap();
            assert!(
                !s.size_ready,
                "NSZeroRect font/rebind callbacks must not unblock attach with size_ready=true"
            );
            assert_eq!(
                (s.model.cols, s.model.height),
                (80, 24),
                "NSZeroRect callbacks must not shrink model to 2 columns"
            );
        }
        mounted_host.detach_current_session();
        evict_session("core-zero", "tty-zero");
        unsafe {
            cb_terminal_detach(raw_view as *mut c_void);
            drop(Retained::from_raw(raw_view));
        }
    }
}
