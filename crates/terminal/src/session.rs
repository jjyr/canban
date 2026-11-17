//! Platform-neutral Herdr PTY live attachment pool shared across macOS and Linux viewers.
use crate::model::Model;
use canban_herdr::{
    Client,
    stream::{Attachment, DisplayFacts, Event},
};
use canban_types::Config;
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub const CONNECTING_GRACE: Duration = Duration::from_millis(180);
pub const MAX_POOLED_SESSIONS: usize = 8;

type WakeCallback = Arc<dyn Fn() + Send + Sync>;

/// Multi-viewer wakeup hub for a single live PTY session.
pub struct WakeHub {
    listeners: Mutex<Vec<(usize, WakeCallback)>>,
    pending: AtomicBool,
}

impl Default for WakeHub {
    fn default() -> Self {
        Self::new()
    }
}

impl WakeHub {
    pub fn new() -> Self {
        Self {
            listeners: Mutex::new(Vec::new()),
            pending: AtomicBool::new(false),
        }
    }

    pub fn next_listener_id() -> usize {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    }

    pub fn attach(&self, id: usize, callback: WakeCallback) {
        if id == 0 {
            return;
        }
        let mut listeners = self.listeners.lock().unwrap();
        if let Some(slot) = listeners.iter_mut().find(|(existing, _)| *existing == id) {
            slot.1 = callback;
        } else {
            listeners.push((id, callback));
        }
    }

    pub fn detach(&self, id: usize) {
        if id == 0 {
            return;
        }
        let mut listeners = self.listeners.lock().unwrap();
        listeners.retain(|(existing, _)| *existing != id);
        if listeners.is_empty() {
            self.pending.store(false, Ordering::SeqCst);
        }
    }

    pub fn has_listeners(&self) -> bool {
        !self.listeners.lock().unwrap().is_empty()
    }

    pub fn notify(&self) {
        let snapshot: Vec<WakeCallback> = self
            .listeners
            .lock()
            .unwrap()
            .iter()
            .map(|(_, cb)| cb.clone())
            .collect();
        if snapshot.is_empty() {
            return;
        }
        if snapshot.len() == 1 {
            if !self.pending.swap(true, Ordering::SeqCst) {
                (snapshot[0])();
            }
            return;
        }
        for cb in snapshot {
            cb();
        }
    }

    pub fn ack(&self) {
        self.pending.store(false, Ordering::SeqCst);
    }
}

pub struct State {
    pub model: Model,
    pub status: String,
    pub connected: bool,
    pub exited: bool,
    pub connecting_since: Option<Instant>,
    pub size_ready: bool,
    pub focused: bool,
    pub theme: Option<Value>,
}

impl State {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            model: Model::new(cols, rows),
            status: String::new(),
            connected: false,
            exited: false,
            connecting_since: None,
            size_ready: false,
            focused: false,
            theme: None,
        }
    }
}

#[derive(Debug)]
pub enum Input {
    Bytes(Vec<u8>),
    AutoBytes(Vec<u8>),
    Resize(u16, u16),
    Focus(bool),
    Facts(DisplayFacts),
}

pub struct LiveSession {
    pub key: (String, String),
    pub state: Arc<Mutex<State>>,
    pub size_cv: Arc<Condvar>,
    pub wake: Arc<WakeHub>,
    pub input: mpsc::Sender<Input>,
    pub stop: Arc<AtomicBool>,
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.size_cv.notify_all();
    }
}

pub struct SessionInit {
    pub config: Config,
    pub core: String,
    pub terminal: String,
    pub initial_cols: u16,
    pub initial_rows: u16,
    pub initial_cell: (u32, u32),
    pub size_ready: bool,
    pub focused: bool,
    pub theme: Option<Value>,
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

pub fn accepts_pty_input(state: &State, read_only: bool) -> bool {
    if read_only {
        return false;
    }
    state.connected || state.status.starts_with("Connecting")
}

pub fn effective_status(
    state: &State,
    read_only: bool,
    input_status: &str,
    is_preview: bool,
) -> String {
    if read_only {
        let reason = if !input_status.is_empty() {
            input_status
        } else if is_preview {
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

pub fn display_facts_for_model(model: &Model, theme: Option<&Value>) -> DisplayFacts {
    let parse_rgb = |v: &Value| -> Option<[u8; 3]> {
        let a = v.as_array()?;
        if a.len() != 3 {
            return None;
        }
        Some([
            u8::try_from(a[0].as_u64()?).ok()?,
            u8::try_from(a[1].as_u64()?).ok()?,
            u8::try_from(a[2].as_u64()?).ok()?,
        ])
    };
    let (def_fg, def_bg, def_cur) = model.default_colors();
    let fg = theme
        .and_then(|t| parse_rgb(&t["foreground"]))
        .or(Some(def_fg));
    let bg = theme
        .and_then(|t| parse_rgb(&t["background"]))
        .or(Some(def_bg));
    let cursor = theme
        .and_then(|t| parse_rgb(&t["cursor"]))
        .or(Some(def_cur));
    DisplayFacts {
        fg,
        bg,
        cursor,
        cell: Some(model.cell_size()),
    }
}

pub fn spawn_session(init: SessionInit) -> Arc<LiveSession> {
    let SessionInit {
        config,
        core,
        terminal,
        initial_cols,
        initial_rows,
        initial_cell,
        size_ready,
        focused,
        theme,
    } = init;
    let cols = initial_cols.max(2);
    let rows = initial_rows.max(1);
    let mut model = Model::new(cols, rows);
    let _ = model.resize_with_metrics(
        cols,
        rows,
        initial_cell.0.max(1),
        initial_cell.1.max(1),
        6,
        6,
        6,
        6,
    );
    if let Some(ref t) = theme {
        model.theme(t);
    }
    let _ = model.replies();
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
    let wake = Arc::new(WakeHub::new());
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
            let attempt = (|| -> anyhow::Result<(Attachment, (u16, u16), DisplayFacts)> {
                if !relay_fenced {
                    anyhow::ensure!(
                        client.status()?["core"]["id"] == core,
                        "Original core identity changed"
                    );
                }
                let (size, display) = {
                    let s = state.lock().unwrap();
                    (
                        (s.model.cols, s.model.height),
                        display_facts_for_model(&s.model, s.theme.as_ref()),
                    )
                };
                let mut attachment = client.attach_fenced_with_display(
                    &terminal,
                    size.0,
                    size.1,
                    Some(&core),
                    Some(&display),
                )?;
                if !relay_fenced {
                    attachment.track_view(&core, &terminal);
                }
                Ok((attachment, size, display))
            })();
            let (attach, attached_size, attached_display) = match attempt {
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
                    let prev_cell = s.model.cell_size();
                    s.model = Model::new(size.0, size.1);
                    let _ = s.model.resize_with_metrics(
                        size.0,
                        size.1,
                        prev_cell.0.max(1),
                        prev_cell.1.max(1),
                        8,
                        8,
                        6,
                        6,
                    );
                    if let Some(theme) = s.theme.clone() {
                        s.model.theme(&theme);
                    }
                    let _ = s.model.replies();
                }
                s.status = String::new();
                s.connecting_since = None;
                let mut setup_res = if s.focused {
                    attach.focus(true)
                } else {
                    Ok(())
                };
                // Layout may have resized the view or updated font/theme while attach was connecting.
                if setup_res.is_ok() && s.size_ready && size != attached_size {
                    setup_res = attach.resize(size.0, size.1);
                }
                let latest_display = display_facts_for_model(&s.model, s.theme.as_ref());
                if setup_res.is_ok() && latest_display != attached_display {
                    setup_res = attach.facts(&latest_display);
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
                            Ok(Input::AutoBytes(bytes)) => writer.automatic_input(&bytes),
                            Ok(Input::Resize(c, r)) => writer.resize(c, r),
                            Ok(Input::Focus(on)) => writer.focus(on),
                            Ok(Input::Facts(facts)) => writer.facts(&facts),
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
                        let (replies, should_wake) = {
                            let mut s = state.lock().unwrap();
                            s.model.write(&data);
                            let should_wake =
                                !s.model.is_render_held() || s.model.render_state_dirty();
                            (s.model.replies(), should_wake)
                        };
                        if should_wake {
                            wake.notify();
                        }
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
                    Ok(None) => {
                        let should_wake = {
                            let mut s = state.lock().unwrap();
                            s.model.expire_render_hold_if_due()
                                || s.model.advance_animation_if_due()
                        };
                        if should_wake {
                            wake.notify();
                        }
                    }
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

pub fn get_or_spawn_session(init: SessionInit) -> Arc<LiveSession> {
    let _span = canban_profiler::Span::new("terminal.attach");
    let mut pool = session_pool().lock().unwrap();
    pool.retain(|s| !s.stop.load(Ordering::SeqCst) && !s.state.lock().unwrap().exited);
    if let Some(idx) = pool
        .iter()
        .position(|s| s.key.0 == init.core && s.key.1 == init.terminal)
    {
        let existing = pool.remove(idx).unwrap();
        let mut facts_to_send = None;
        {
            let mut s = existing.state.lock().unwrap();
            if let Some(ref t) = init.theme
                && s.theme.as_ref() != Some(t)
            {
                s.model.theme(t);
                s.theme = Some(t.clone());
                if s.connected {
                    facts_to_send = Some(display_facts_for_model(&s.model, s.theme.as_ref()));
                }
            }
            if init.size_ready && !s.size_ready {
                s.size_ready = true;
                existing.size_cv.notify_all();
            }
        }
        if let Some(facts) = facts_to_send {
            let _ = existing.input.send(Input::Facts(facts));
        }
        pool.push_back(existing.clone());
        return existing;
    }
    let created = spawn_session(init);
    while pool.len() >= MAX_POOLED_SESSIONS {
        // Prefer evicting a background session that currently has no attached viewer.
        if let Some(idle_idx) = pool.iter().position(|s| !s.wake.has_listeners()) {
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

pub fn detach_session_listener(session: &LiveSession, listener_id: usize) {
    let _span = canban_profiler::Span::new("terminal.detach");
    session.wake.detach(listener_id);
    if !session.wake.has_listeners() {
        let was_focused = {
            let mut s = session.state.lock().unwrap();
            let was = s.focused;
            s.focused = false;
            was
        };
        if was_focused {
            let _ = session.input.send(Input::Focus(false));
        }
    }
}
