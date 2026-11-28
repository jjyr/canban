//! Portable `nsh_base::Terminal` integration (Linux desktop, iOS, Android) backed by
//! `libghostty-vt` and the shared Herdr `LiveSession` pool.
pub use crate::view::{FocusRequest, FontSpec, LiveBinding, decode_file_uri, parse_font_value};
use crate::{
    InstalledFonts, input,
    model::{Model, WheelOutput},
    session::{
        self, Input, LiveSession, SessionInit, State, detach_session_listener, get_or_spawn_session,
    },
};
use nsh_base::{
    Terminal, TerminalApi, TerminalCursor, TerminalEmulator, TerminalEmulatorEvent,
    TerminalEmulatorFactory, TerminalLine, TerminalModes, TerminalSize, terminal,
};
use nsh_core::prelude::*;
use serde_json::Value;
#[cfg(any(feature = "herdr-local", test))]
use std::collections::BTreeSet;
use std::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    rc::Rc,
    sync::{Arc, Condvar, Mutex, OnceLock, mpsc},
    task::{Context, Poll, Waker},
};

/// Standalone `nsh_base::TerminalEmulator` backed by `libghostty-vt` (`crate::model::Model`).
pub struct GhosttyEmulator {
    model: Model,
    view_size: TerminalSize,
    pending_events: Vec<TerminalEmulatorEvent>,
    last_clipboard: Option<String>,
}

impl GhosttyEmulator {
    pub fn new(size: TerminalSize, scrollback: usize) -> Self {
        let cols = size.cols.max(1);
        let rows = size.rows.max(1);
        let mut model = Model::new(cols, rows);
        model.set_scrollback_lines(scrollback);
        Self {
            model,
            view_size: TerminalSize::new(cols, rows),
            pending_events: Vec::new(),
            last_clipboard: None,
        }
    }

    pub fn with_theme(size: TerminalSize, scrollback: usize, theme: &Value) -> Self {
        let mut emu = Self::new(size, scrollback);
        emu.set_theme(theme);
        emu
    }

    pub fn set_theme(&mut self, theme: &Value) {
        self.model.theme(theme);
        let _ = self.model.replies();
    }

    pub fn take_clipboard(&mut self) -> Option<String> {
        self.last_clipboard.take()
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    pub fn model_mut(&mut self) -> &mut Model {
        &mut self.model
    }

    fn collect_side_effects(&mut self) {
        if let Some(title) = self.model.take_title() {
            self.pending_events
                .push(TerminalEmulatorEvent::Title(title));
        }
        if self.model.take_bell() {
            self.pending_events.push(TerminalEmulatorEvent::Bell);
        }
        let replies = self.model.replies();
        if !replies.is_empty() {
            self.pending_events
                .push(TerminalEmulatorEvent::Reply(replies));
        }
        if let Some(clip) = self.model.take_clipboard() {
            if try_os().is_some() {
                drop(set_clipboard(clip.clone()));
            }
            self.last_clipboard = Some(clip);
        }
    }
}

impl TerminalEmulator for GhosttyEmulator {
    fn feed(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.model.write(bytes);
        }
        self.collect_side_effects();
    }

    fn resize(&mut self, size: TerminalSize) {
        let cols = size.cols.max(1);
        let rows = size.rows.max(1);
        self.view_size = TerminalSize::new(cols, rows);
        self.model.resize(cols, rows);
        self.collect_side_effects();
    }

    fn line_count(&self) -> usize {
        self.model.scrollback_rows() + usize::from(self.view_size.rows.max(1))
    }

    fn line(&self, index: usize) -> TerminalLine {
        self.model.terminal_line(index)
    }

    fn cursor(&self) -> Option<TerminalCursor> {
        let mut c = self.model.active_cursor()?;
        c.col = c.col.min(self.view_size.cols.saturating_sub(1));
        c.row = c.row.min(self.view_size.rows.saturating_sub(1));
        Some(c)
    }

    fn take_events(&mut self) -> Vec<TerminalEmulatorEvent> {
        std::mem::take(&mut self.pending_events)
    }

    fn modes(&self) -> TerminalModes {
        self.model.terminal_modes()
    }
}

/// Creates a `TerminalEmulatorFactory` backed by `GhosttyEmulator`.
pub fn emulator_factory(theme: Option<Value>) -> TerminalEmulatorFactory {
    TerminalEmulatorFactory::new(move |size, scrollback| match &theme {
        Some(t) => Box::new(GhosttyEmulator::with_theme(size, scrollback, t)),
        None => Box::new(GhosttyEmulator::new(size, scrollback)),
    })
}

#[cfg(any(feature = "herdr-local", test))]
fn is_non_ascii_symbol_font(family: &str) -> bool {
    let lower = family.to_ascii_lowercase();
    lower.contains("emoji")
        || lower.contains("signwriting")
        || lower.contains("dingbats")
        || lower.contains("braille")
        || lower.contains("symbols")
        || lower.contains("katex")
}

#[cfg(any(feature = "herdr-local", test))]
pub(crate) fn parse_fc_list_output(all_output: &str, ascii_mono_output: &str) -> InstalledFonts {
    let mut mono_set = BTreeSet::new();
    for line in ascii_mono_output.lines() {
        let mut parts = line.split('\t');
        let Some(raw_family) = parts.next() else {
            continue;
        };
        let spacing = parts.next().unwrap_or("").trim();
        let color = parts.next().unwrap_or("").trim();
        let family = raw_family.trim();
        if family.is_empty() || family.starts_with('.') {
            continue;
        }
        if matches!(spacing, "90" | "100" | "110")
            && !color.eq_ignore_ascii_case("true")
            && !is_non_ascii_symbol_font(family)
        {
            mono_set.insert(family.to_owned());
        }
    }

    let mut other_set = BTreeSet::new();
    for line in all_output.lines() {
        let mut parts = line.split('\t');
        let Some(raw_family) = parts.next() else {
            continue;
        };
        let spacing = parts.next().unwrap_or("").trim();
        let color = parts.next().unwrap_or("").trim();
        let family = raw_family.trim();
        if family.is_empty() || family.starts_with('.') {
            continue;
        }
        if ascii_mono_output.is_empty()
            && matches!(spacing, "90" | "100" | "110")
            && !color.eq_ignore_ascii_case("true")
            && !is_non_ascii_symbol_font(family)
        {
            mono_set.insert(family.to_owned());
        } else if !mono_set.contains(family) {
            other_set.insert(family.to_owned());
        }
    }

    let mut monospaced: Vec<String> = mono_set.into_iter().collect();
    monospaced.sort_by_key(|s| s.to_ascii_lowercase());
    let mut others: Vec<String> = other_set
        .into_iter()
        .filter(|f| !monospaced.contains(f))
        .collect();
    others.sort_by_key(|s| s.to_ascii_lowercase());
    InstalledFonts { monospaced, others }
}

static CACHED_FONTS: OnceLock<InstalledFonts> = OnceLock::new();

/// Returns cached installed font families on Linux without blocking `fc-list`.
pub fn try_installed_fonts() -> Option<InstalledFonts> {
    CACHED_FONTS.get().cloned()
}

/// Discovers installed monospaced and proportional font families on Linux via `fc-list`.
pub fn installed_fonts() -> InstalledFonts {
    #[cfg(feature = "herdr-local")]
    {
        CACHED_FONTS
            .get_or_init(|| {
                let run_fc = |args: &[&str]| -> Option<String> {
                    let out = canban_exec::command("fc-list").args(args).output().ok()?;
                    if !out.status.success() {
                        return None;
                    }
                    Some(String::from_utf8_lossy(&out.stdout).into_owned())
                };
                let Some(all_out) = run_fc(&["--format=%{family[0]}\t%{spacing}\t%{color}\n"])
                else {
                    return InstalledFonts::default();
                };
                let ascii_out = run_fc(&[
                    ":charset=41 6d",
                    "--format=%{family[0]}\t%{spacing}\t%{color}\n",
                ])
                .unwrap_or_default();
                parse_fc_list_output(&all_out, &ascii_out)
            })
            .clone()
    }
    #[cfg(not(feature = "herdr-local"))]
    {
        CACHED_FONTS.get_or_init(InstalledFonts::default).clone()
    }
}

// --- Async wakeup channel & shared session slot for `<nsh_base::Terminal />` ---

struct WakeState {
    notified: bool,
    closed: bool,
    waker: Option<Waker>,
}

struct AsyncWake {
    inner: Mutex<WakeState>,
}

impl AsyncWake {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(WakeState {
                notified: false,
                closed: false,
                waker: None,
            }),
        })
    }

    fn trigger(&self) {
        let waker = {
            let mut guard = self.inner.lock().unwrap();
            if guard.closed {
                return;
            }
            guard.notified = true;
            guard.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }

    fn close(&self) {
        let waker = {
            let mut guard = self.inner.lock().unwrap();
            guard.closed = true;
            guard.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }
}

struct NextWake(Arc<AsyncWake>);

impl Future for NextWake {
    type Output = bool;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<bool> {
        let mut guard = self.0.inner.lock().unwrap();
        if guard.closed {
            return Poll::Ready(false);
        }
        if guard.notified {
            guard.notified = false;
            return Poll::Ready(true);
        }
        guard.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

struct ControllerSlot {
    state: Arc<Mutex<State>>,
    size_cv: Arc<Condvar>,
    input: Option<mpsc::Sender<Input>>,
    session: Option<Arc<LiveSession>>,
    listener_id: usize,
    wake: Arc<AsyncWake>,
    binding: Option<LiveBinding>,
    theme: Option<Value>,
    view_size: TerminalSize,
    cell_px: (u32, u32),
    read_only: bool,
    input_status: String,
    notice_status: String,
    is_preview: bool,
    seen_rev: u64,
}

impl ControllerSlot {
    fn new(is_preview: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::new(80, 24))),
            size_cv: Arc::new(Condvar::new()),
            input: None,
            session: None,
            listener_id: session::WakeHub::next_listener_id(),
            wake: AsyncWake::new(),
            binding: None,
            theme: None,
            view_size: TerminalSize::default(),
            cell_px: (8, 18),
            read_only: is_preview,
            input_status: String::new(),
            notice_status: String::new(),
            is_preview,
            seen_rev: u64::MAX,
        }
    }

    fn detach_session(&mut self) {
        if let Some(old) = self.session.take() {
            detach_session_listener(&old, self.listener_id);
        }
        self.input = None;
    }

    fn rebind(&mut self, next: Option<LiveBinding>) -> bool {
        if self.binding == next {
            return false;
        }
        let prev_focused = self.state.lock().unwrap().focused;
        let (prev_cols, prev_rows, prev_cell, prev_ready) = {
            let s = self.state.lock().unwrap();
            (
                s.model.cols,
                s.model.height,
                s.model.cell_size(),
                s.size_ready,
            )
        };
        let theme = self
            .theme
            .clone()
            .or_else(|| self.state.lock().unwrap().theme.clone());
        self.detach_session();
        self.binding = next.clone();

        let Some(LiveBinding {
            host_id,
            config,
            core_id,
            terminal,
            transport,
            session_epoch,
        }) = next
        else {
            self.state = Arc::new(Mutex::new(State::new(prev_cols.max(2), prev_rows.max(1))));
            self.size_cv = Arc::new(Condvar::new());
            self.seen_rev = u64::MAX;
            return true;
        };

        let session = get_or_spawn_session(SessionInit {
            host_id,
            config,
            core: core_id,
            terminal,
            initial_cols: prev_cols.max(2),
            initial_rows: prev_rows.max(1),
            initial_cell: prev_cell,
            size_ready: prev_ready,
            focused: prev_focused,
            theme,
            transport,
            session_epoch,
        });

        let wake_target = self.wake.clone();
        session
            .wake
            .attach(self.listener_id, Arc::new(move || wake_target.trigger()));

        input::apply_focus(
            &session.state,
            Some(&session.input),
            self.read_only,
            prev_focused,
        );

        self.state = session.state.clone();
        self.size_cv = session.size_cv.clone();
        self.input = Some(session.input.clone());
        self.session = Some(session);
        self.seen_rev = u64::MAX;
        true
    }

    fn apply_theme(&mut self, next: &Value) -> bool {
        self.theme = Some(next.clone());
        let changed = input::apply_theme(&self.state, self.input.as_ref(), next);
        if changed {
            self.seen_rev = u64::MAX;
        }
        changed
    }

    fn apply_focus(&self, focused: bool) {
        input::apply_focus(&self.state, self.input.as_ref(), self.read_only, focused);
    }

    fn effective_status(&self) -> String {
        let s = self.state.lock().unwrap();
        session::effective_status(
            &s,
            self.read_only,
            &self.input_status,
            &self.notice_status,
            self.is_preview,
        )
    }

    fn paste_text_to_pty(&self, text: &str) {
        input::paste_text(&self.state, self.input.as_ref(), self.read_only, text);
    }

    fn send_bytes(&self, bytes: Vec<u8>) {
        input::send_bytes(self.input.as_ref(), bytes);
    }

    fn send_auto_bytes(&self, bytes: Vec<u8>) {
        input::send_auto_bytes(self.input.as_ref(), bytes);
    }
}

struct SharedSessionEmulator {
    slot: Rc<RefCell<ControllerSlot>>,
    pending_events: Vec<TerminalEmulatorEvent>,
}

impl TerminalEmulator for SharedSessionEmulator {
    fn feed(&mut self, bytes: &[u8]) {
        let mut slot = self.slot.borrow_mut();
        let (title, bell, replies, clip, rev) = {
            let mut s = slot.state.lock().unwrap();
            if slot.is_preview && !bytes.is_empty() {
                s.model.write(bytes);
            }
            let _ = s.model.expire_render_hold_if_due();
            let _ = s.model.advance_animation_if_due();
            (
                s.model.take_title(),
                s.model.take_bell(),
                s.model.replies(),
                s.model.take_clipboard(),
                s.model.revision,
            )
        };
        if let Some(t) = title {
            self.pending_events.push(TerminalEmulatorEvent::Title(t));
        }
        if bell {
            self.pending_events.push(TerminalEmulatorEvent::Bell);
        }
        if !replies.is_empty() {
            slot.send_auto_bytes(replies);
        }
        if let Some(text) = clip
            && try_os().is_some()
        {
            drop(set_clipboard(text));
        }
        slot.seen_rev = rev;
    }

    fn resize(&mut self, size: TerminalSize) {
        let mut slot = self.slot.borrow_mut();
        slot.view_size = TerminalSize::new(size.cols.max(1), size.rows.max(1));
        if size.cols < 2 || size.rows < 1 {
            return;
        }
        let (cw, ch) = slot.cell_px;
        input::apply_resize(
            &slot.state,
            &slot.size_cv,
            slot.input.as_ref(),
            size.cols,
            size.rows,
            cw,
            ch,
            (6, 6, 6, 6),
        );
    }

    fn line_count(&self) -> usize {
        let slot = self.slot.borrow();
        let sb = slot.state.lock().unwrap().model.scrollback_rows();
        sb + usize::from(slot.view_size.rows.max(1))
    }

    fn line(&self, index: usize) -> TerminalLine {
        let slot = self.slot.borrow();
        let s = slot.state.lock().unwrap();
        if s.is_attach_settling() {
            return TerminalLine::default();
        }
        s.model.terminal_line(index)
    }

    fn cursor(&self) -> Option<TerminalCursor> {
        let slot = self.slot.borrow();
        let s = slot.state.lock().unwrap();
        if s.is_attach_settling() {
            return None;
        }
        let mut cur = s.model.active_cursor()?;
        cur.col = cur.col.min(slot.view_size.cols.saturating_sub(1));
        cur.row = cur.row.min(slot.view_size.rows.saturating_sub(1));
        Some(cur)
    }

    fn take_events(&mut self) -> Vec<TerminalEmulatorEvent> {
        std::mem::take(&mut self.pending_events)
    }

    fn modes(&self) -> TerminalModes {
        let slot = self.slot.borrow();
        let s = slot.state.lock().unwrap();
        s.model.terminal_modes()
    }
}

fn parse_rgb_or(value: &Value, fallback: [u8; 3]) -> [u8; 3] {
    if let Some(a) = value.as_array()
        && a.len() == 3
        && let (Some(r), Some(g), Some(b)) = (a[0].as_u64(), a[1].as_u64(), a[2].as_u64())
    {
        return [r as u8, g as u8, b as u8];
    }
    fallback
}

fn adaptive_rgb(c: [u8; 3]) -> AdaptiveColor {
    let hex = (u32::from(c[0]) << 16) | (u32::from(c[1]) << 8) | u32::from(c[2]);
    let hsla: Hsla = rgb(hex).into();
    AdaptiveColor {
        light: hsla,
        dark: hsla,
    }
}

fn adaptive_rgba(c: [u8; 3], alpha: f32) -> AdaptiveColor {
    let hex = (u32::from(c[0]) << 24)
        | (u32::from(c[1]) << 16)
        | (u32::from(c[2]) << 8)
        | ((alpha.clamp(0.0, 1.0) * 255.0).round() as u32);
    let hsla: Hsla = rgba(hex).into();
    AdaptiveColor {
        light: hsla,
        dark: hsla,
    }
}

/// Optional mobile interaction & offline display hooks for portable terminals.
#[derive(Clone, Default)]
pub struct PortableOptions {
    pub mobile_controls: bool,
    pub on_fullscreen: Option<Rc<dyn Fn()>>,
    pub offline_notice: Option<Rc<dyn Fn() -> String>>,
}

#[allow(clippy::too_many_arguments)]
fn mount_terminal_surface(
    initial_preview: Option<String>,
    binding: impl Fn() -> Option<LiveBinding> + 'static,
    theme: impl Fn() -> Value + 'static,
    font: impl Fn() -> Value + 'static,
    focus: impl Fn() -> FocusRequest + 'static,
    read_only: impl Fn() -> bool + 'static,
    input_status: impl Fn() -> String + 'static,
    notice_status: impl Fn() -> String + 'static,
    options: PortableOptions,
) -> View {
    let is_preview = initial_preview.is_some();
    let binding = Rc::new(binding);
    let theme = Rc::new(theme);
    let font = Rc::new(font);
    let focus = Rc::new(focus);
    let read_only = Rc::new(read_only);
    let input_status = Rc::new(input_status);
    let notice_status = Rc::new(notice_status);
    let mobile_controls = options.mobile_controls;
    let on_fullscreen = options.on_fullscreen;
    let offline_notice = options.offline_notice;

    view().relative().overflow_hidden().child(dynamic(move || {
        let slot = Rc::new(RefCell::new(ControllerSlot::new(is_preview)));
        let ctrl_sticky = signal(false);
        let zoom_size = signal(Option::<f32>::None);
        if let Some(ref content) = initial_preview {
            let state_arc = slot.borrow().state.clone();
            let mut s = state_arc.lock().unwrap();
            s.model.write(content.as_bytes());
            let _ = s.model.replies();
        }

        let factory_slot = slot.clone();
        provide_context(TerminalEmulatorFactory::new(move |size, scrollback| {
            {
                let mut guard = factory_slot.borrow_mut();
                guard.view_size = size;
                if guard.is_preview {
                    let mut s = guard.state.lock().unwrap();
                    s.model.set_scrollback_lines(scrollback);
                    s.model.resize(size.cols.max(2), size.rows.max(1));
                }
            }
            Box::new(SharedSessionEmulator {
                slot: factory_slot.clone(),
                pending_events: Vec::new(),
            })
        }));

        let term_ref = component_ref::<Terminal>();
        let status_sig = signal(String::new());

        let theme_memo = computed({
            let theme = theme.clone();
            move || theme()
        });
        let font_memo = computed({
            let font = font.clone();
            move || {
                let mut spec = parse_font_value(&font());
                if let Some(zoomed) = zoom_size.get() {
                    let size = crate::mobile_bar::clamp_terminal_font_size(zoomed);
                    spec.size = size;
                    spec.line_height = (size * 1.385).round().max(size + 3.0);
                }
                spec
            }
        });
        let binding_memo = computed({
            let binding = binding.clone();
            move || binding()
        });
        let ro_memo = computed({
            let read_only = read_only.clone();
            move || read_only()
        });
        let status_override_memo = computed({
            let input_status = input_status.clone();
            move || input_status()
        });
        let notice_override_memo = computed({
            let notice_status = notice_status.clone();
            move || notice_status()
        });
        let focus_memo = computed({
            let focus = focus.clone();
            move || focus()
        });

        // Sync font cell pixel metrics into slot so PTY size reports reflect font size.
        {
            let slot_font = slot.clone();
            effect(move || {
                let spec = font_memo.get();
                let cw = (spec.size * 0.6).round().max(4.0) as u32;
                let ch = spec.line_height.round().max(8.0) as u32;
                slot_font.borrow_mut().cell_px = (cw, ch);
            });
        }

        // Sync theme palette into Model + LiveSession display facts + repaint.
        {
            let slot_theme = slot.clone();
            let term_ref_theme = term_ref;
            effect(move || {
                let next_theme = theme_memo.get();
                let changed = slot_theme.borrow_mut().apply_theme(&next_theme);
                if changed && let Some(api) = term_ref_theme.api::<TerminalApi>() {
                    api.write_bytes(&[]);
                }
            });
        }

        // Sync read_only + input_status + notice_status into status pill.
        {
            let slot_ro = slot.clone();
            effect(move || {
                let ro = ro_memo.get();
                let status_text = status_override_memo.get();
                let notice_text = notice_override_memo.get();
                let next_pill = {
                    let mut guard = slot_ro.borrow_mut();
                    guard.read_only = ro;
                    guard.input_status = status_text;
                    guard.notice_status = notice_text;
                    guard.effective_status()
                };
                if status_sig.get_untracked() != next_pill {
                    status_sig.set(next_pill);
                }
            });
        }

        // Sync live Herdr binding (pool lookup / attach / detach).
        if !is_preview {
            let slot_bind = slot.clone();
            let term_ref_bind = term_ref;
            effect(move || {
                let next = binding_memo.get();
                let rebound = slot_bind.borrow_mut().rebind(next);
                if rebound {
                    let next_pill = slot_bind.borrow().effective_status();
                    if status_sig.get_untracked() != next_pill {
                        status_sig.set(next_pill);
                    }
                    if let Some(api) = term_ref_bind.api::<TerminalApi>() {
                        api.write_bytes(&[]);
                    } else {
                        after_answer(move || {
                            if let Some(api) = term_ref_bind.api::<TerminalApi>() {
                                api.write_bytes(&[]);
                            }
                        });
                    }
                }
            });
        }

        // Sync focus request (`ticket` + `enabled`) -> `TerminalApi::focus` + Herdr `Focus`.
        {
            let slot_focus = slot.clone();
            let term_ref_focus = term_ref;
            let prev_req = Rc::new(RefCell::new(None::<FocusRequest>));
            effect(move || {
                let req = focus_memo.get();
                let prev = *prev_req.borrow();
                *prev_req.borrow_mut() = Some(req);
                slot_focus.borrow().apply_focus(req.enabled);
                let should_focus =
                    req.enabled && prev.is_none_or(|p| !p.enabled || p.ticket != req.ticket);
                if should_focus {
                    if let Some(api) = term_ref_focus.api::<TerminalApi>() {
                        api.focus();
                    } else {
                        after_answer(move || {
                            if let Some(api) = term_ref_focus.api::<TerminalApi>() {
                                api.focus();
                            }
                        });
                    }
                }
            });
        }

        // Linux terminal clipboard paste shortcut (`Ctrl+Shift+V`) when focused and writable.
        if !is_preview {
            let slot_paste = slot.clone();
            effect(move || {
                if !focus_memo.get().enabled || ro_memo.get() {
                    return;
                }
                let slot_key = slot_paste.clone();
                on_key_with(
                    "ctrl+shift+v",
                    KeyOptions {
                        when: nsh_core::keymap::When::Always,
                        repeat: false,
                        ..Default::default()
                    },
                    move |_| {
                        let slot_async = slot_key.clone();
                        spawn(async move {
                            let files = clipboard_files().await;
                            if !files.is_empty() {
                                let decoded: Vec<String> =
                                    files.iter().filter_map(|u| decode_file_uri(u)).collect();
                                let escaped = crate::shell_escape_paths(decoded);
                                if !escaped.is_empty() {
                                    slot_async.borrow().paste_text_to_pty(&escaped);
                                    return;
                                }
                            }
                            let text = clipboard_string().await;
                            if !text.is_empty() {
                                slot_async.borrow().paste_text_to_pty(&text);
                            }
                        });
                    },
                );
            });
        }

        // Async wakeup loop: wakes immediately whenever `LiveSession` receives PTY data
        // or transitions connection state, updating clipboard, status pill, and `TerminalApi`.
        {
            let wake = slot.borrow().wake.clone();
            let slot_wake = slot.clone();
            let term_ref_wake = term_ref;
            spawn(async move {
                while NextWake(wake.clone()).await {
                    let (need_write, next_pill, clip) = {
                        let mut guard = slot_wake.borrow_mut();
                        if let Some(ref sess) = guard.session {
                            sess.wake.ack();
                        }
                        let (rev, clip) = {
                            let mut s = guard.state.lock().unwrap();
                            (s.model.revision, s.model.take_clipboard())
                        };
                        let need_write = rev != guard.seen_rev;
                        guard.seen_rev = rev;
                        let pill = guard.effective_status();
                        (need_write, pill, clip)
                    };
                    if let Some(text) = clip
                        && try_os().is_some()
                    {
                        drop(set_clipboard(text));
                    }
                    if status_sig.get_untracked() != next_pill {
                        status_sig.set(next_pill);
                    }
                    if need_write && let Some(api) = term_ref_wake.api::<TerminalApi>() {
                        api.write_bytes(&[]);
                    }
                }
            });
        }

        // Clean up listener registration on unmount while leaving pooled `LiveSession` alive.
        {
            let slot_cleanup = slot.clone();
            cleanup(move || {
                let mut guard = slot_cleanup.borrow_mut();
                guard.wake.close();
                guard.detach_session();
            });
        }

        let slot_data = slot.clone();
        let slot_drop = slot.clone();
        let slot_wheel = slot.clone();
        let term_ref_drop = term_ref;

        let term_el = terminal()
            .component_ref(&term_ref)
            .scrollback(4000u32)
            .font_family(move || font_memo.get().family)
            .font_size(move || Points(font_memo.get().size))
            .line_height(move || Points(font_memo.get().line_height))
            .cursor_style(move |s| {
                let cur = parse_rgb_or(&theme_memo.get()["cursor"], [224, 228, 236]);
                s.bg(adaptive_rgba(cur, 0.45))
            })
            .accessibility_label(Some(SharedString::from("Terminal")))
            .on_data(move |data: &SharedString| {
                let guard = slot_data.borrow();
                if guard.is_preview || guard.read_only {
                    return;
                }
                let can_send = {
                    let mut s = guard.state.lock().unwrap();
                    if !session::accepts_pty_input(&s, guard.read_only) {
                        false
                    } else {
                        s.clear_attach_settle();
                        s.model.clear_selection();
                        s.model.scroll_to_bottom();
                        true
                    }
                };
                if can_send {
                    if ctrl_sticky.get_untracked()
                        && let Some(ctrl_bytes) =
                            crate::mobile_bar::apply_sticky_ctrl(data.as_str())
                    {
                        ctrl_sticky.set(false);
                        guard.send_bytes(ctrl_bytes);
                        return;
                    }
                    guard.send_bytes(data.as_str().as_bytes().to_vec());
                }
            })
            .size_full();

        let slot_bar_key = slot.clone();
        let slot_bar_paste = slot.clone();
        let ro_bar = ro_memo;
        let on_fullscreen_cb = on_fullscreen.clone();
        let offline_notice_cb = offline_notice.clone();

        let mut surface = view()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .relative()
            .overflow_hidden()
            .bg(move || adaptive_rgb(parse_rgb_or(&theme_memo.get()["background"], [24, 24, 28])))
            .p(px(6.0))
            .on_file_drop(move |ev: &FileDropEvent| {
                let paths: Vec<String> =
                    ev.uris.iter().filter_map(|u| decode_file_uri(u)).collect();
                let escaped = crate::shell_escape_paths(paths);
                if !escaped.is_empty() {
                    slot_drop.borrow().paste_text_to_pty(&escaped);
                    if let Some(api) = term_ref_drop.api::<TerminalApi>() {
                        api.focus();
                    }
                }
            })
            .on_wheel(move |ev: &WheelEvent| {
                let guard = slot_wheel.borrow();
                if guard.is_preview || guard.read_only {
                    return;
                }
                let delta_y = match ev.delta {
                    WheelDelta::Lines(pt) => -pt.y,
                    WheelDelta::Points(pt) => -(pt.y.0 / 18.0),
                };
                let rows_step = delta_y.round() as isize;
                if rows_step == 0 {
                    return;
                }
                let out = {
                    let mut s = guard.state.lock().unwrap();
                    if !session::accepts_pty_input(&s, guard.read_only) {
                        return;
                    }
                    s.clear_attach_settle();
                    // Only intercept wheel when TUI mouse tracking or alternate scroll (1007) is active;
                    // primary scrollback is scrolled natively by `nsh_base::Terminal`'s virtual_list.
                    if !s.model.mouse_tracking()
                        && s.model.active_screen() != crate::ffi::GHOSTTY_TERMINAL_SCREEN_ALTERNATE
                    {
                        return;
                    }
                    s.model
                        .wheel(rows_step, 0, ev.position.x.0, ev.position.y.0, 0, false)
                };
                if let WheelOutput::Pty(bytes) = out {
                    guard.send_bytes(bytes);
                }
            });

        if mobile_controls {
            let base_font_memo = font_memo;
            surface = surface.on_pinch(move |ev: &PinchEvent| {
                if ev.delta.0.abs() > f32::EPSILON {
                    let current = zoom_size
                        .get_untracked()
                        .unwrap_or_else(|| base_font_memo.get_untracked().size);
                    zoom_size.set(Some(crate::mobile_bar::scale_font_size(
                        current, ev.delta.0,
                    )));
                }
            });
        }

        let surface = surface.child(term_el).child(when(
            move || !status_sig.get().is_empty(),
            move || {
                view()
                    .absolute()
                    .top(px(8.0))
                    .right(px(12.0))
                    .pointer_events(PointerEvents::None)
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(adaptive_rgba([255, 255, 255], 0.14))
                    .bg(adaptive_rgba([20, 22, 28], 0.86))
                    .child(
                        text(move || SharedString::from(status_sig.get()))
                            .text_size(px(11.0))
                            .text_color(adaptive_rgb([214, 220, 230])),
                    )
            },
        ));

        if !mobile_controls {
            return surface.into_element();
        }

        let offline_banner = offline_notice_cb.map(|notice_fn| {
            let notice_text_fn = notice_fn.clone();
            when(
                move || !notice_fn().trim().is_empty(),
                move || {
                    let notice_line = notice_text_fn();
                    view()
                        .w_full()
                        .flex_shrink_0()
                        .px(px(12.0))
                        .py(px(6.0))
                        .border_b_1()
                        .border_color(adaptive_rgba([240, 175, 70], 0.35))
                        .bg(adaptive_rgba([240, 175, 70], 0.14))
                        .child(
                            text(format!("Offline · {notice_line}"))
                                .text_size(px(12.0))
                                .text_color(adaptive_rgb([245, 210, 140])),
                        )
                },
            )
        });

        let bar = crate::mobile_bar::accessory_bar(
            ctrl_sticky,
            move || ro_bar.get(),
            move |key| {
                let guard = slot_bar_key.borrow();
                if guard.is_preview || guard.read_only {
                    return;
                }
                let (can_send, bytes) = {
                    let mut s = guard.state.lock().unwrap();
                    if !session::accepts_pty_input(&s, guard.read_only) {
                        (false, Vec::new())
                    } else {
                        s.clear_attach_settle();
                        s.model.clear_selection();
                        s.model.scroll_to_bottom();
                        let modes = s.model.terminal_modes();
                        (
                            true,
                            crate::mobile_bar::encode_accessory_key(key, modes).to_vec(),
                        )
                    }
                };
                if can_send {
                    guard.send_bytes(bytes);
                }
            },
            move || {
                let slot_async = slot_bar_paste.clone();
                spawn(async move {
                    let text = clipboard_string().await;
                    if !text.is_empty() {
                        slot_async.borrow().paste_text_to_pty(&text);
                    }
                });
            },
            on_fullscreen_cb,
        );

        view()
            .size_full()
            .flex_col()
            .min_h(px(0.0))
            .pb(move || {
                if matches!(
                    try_os(),
                    Some(nsh_core::platform::Os::Ios | nsh_core::platform::Os::Android)
                ) {
                    px(keyboard_height().0.max(safe_area().bottom.0))
                } else {
                    px(0.0)
                }
            })
            .children(offline_banner)
            .child(surface)
            .child(bar)
            .into_element()
    }))
}

/// Mounts a live Herdr-backed `<nsh_base::Terminal />` with reactive theme, font, focus,
/// read-only gating, file drag-and-drop, clipboard (OSC 52 + `Ctrl+Shift+V`), and status pill overlay.
pub fn live_terminal(
    binding: impl Fn() -> Option<LiveBinding> + 'static,
    theme: impl Fn() -> Value + 'static,
    font: impl Fn() -> Value + 'static,
    focus: impl Fn() -> FocusRequest + 'static,
    read_only: impl Fn() -> bool + 'static,
    input_status: impl Fn() -> String + 'static,
    notice_status: impl Fn() -> String + 'static,
) -> View {
    live_terminal_with_options(
        binding,
        theme,
        font,
        focus,
        read_only,
        input_status,
        notice_status,
        PortableOptions::default(),
    )
}

/// Mounts a live Herdr-backed `<nsh_base::Terminal />` with configurable mobile options.
#[allow(clippy::too_many_arguments)]
pub fn live_terminal_with_options(
    binding: impl Fn() -> Option<LiveBinding> + 'static,
    theme: impl Fn() -> Value + 'static,
    font: impl Fn() -> Value + 'static,
    focus: impl Fn() -> FocusRequest + 'static,
    read_only: impl Fn() -> bool + 'static,
    input_status: impl Fn() -> String + 'static,
    notice_status: impl Fn() -> String + 'static,
    options: PortableOptions,
) -> View {
    mount_terminal_surface(
        None,
        binding,
        theme,
        font,
        focus,
        read_only,
        input_status,
        notice_status,
        options,
    )
}

/// Mounts a static read-only `<nsh_base::Terminal />` preview with custom status labels.
pub fn preview_terminal_with_status(
    content: impl Into<String>,
    theme: impl Fn() -> Value + 'static,
    font: impl Fn() -> Value + 'static,
    focus: impl Fn() -> FocusRequest + 'static,
    input_status: impl Fn() -> String + 'static,
    notice_status: impl Fn() -> String + 'static,
) -> View {
    preview_terminal_with_options(
        content,
        theme,
        font,
        focus,
        || true,
        input_status,
        notice_status,
        PortableOptions::default(),
    )
}

/// Mounts a static `<nsh_base::Terminal />` preview with configurable mobile options.
#[allow(clippy::too_many_arguments)]
pub fn preview_terminal_with_options(
    content: impl Into<String>,
    theme: impl Fn() -> Value + 'static,
    font: impl Fn() -> Value + 'static,
    focus: impl Fn() -> FocusRequest + 'static,
    read_only: impl Fn() -> bool + 'static,
    input_status: impl Fn() -> String + 'static,
    notice_status: impl Fn() -> String + 'static,
    options: PortableOptions,
) -> View {
    mount_terminal_surface(
        Some(content.into()),
        || None,
        theme,
        font,
        focus,
        read_only,
        input_status,
        notice_status,
        options,
    )
}

/// Mounts a static read-only `<nsh_base::Terminal />` preview (for Gallery / offline previews).
pub fn preview_terminal(
    content: impl Into<String>,
    theme: impl Fn() -> Value + 'static,
    font: impl Fn() -> Value + 'static,
    focus: impl Fn() -> FocusRequest + 'static,
) -> View {
    preview_terminal_with_status(
        content,
        theme,
        font,
        focus,
        || "Preview".to_owned(),
        String::new,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_base::TerminalColor;
    use serde_json::json;

    fn sample_theme() -> Value {
        json!({
            "foreground": [220, 225, 235],
            "background": [18, 20, 26],
            "cursor": [90, 200, 250],
            "ansi": [
                [10, 12, 16],
                [235, 80, 90],
                [80, 210, 130],
                [240, 200, 90],
                [90, 150, 245],
                [200, 110, 235],
                [80, 210, 220],
                [200, 205, 215],
                [90, 96, 110],
                [255, 105, 115],
                [105, 235, 155],
                [255, 220, 115],
                [115, 175, 255],
                [220, 135, 255],
                [105, 235, 245],
                [250, 252, 255]
            ]
        })
    }

    #[test]
    fn ghostty_emulator_renders_themed_rgb_runs_and_preserves_style_flags() {
        let mut emu = GhosttyEmulator::with_theme(TerminalSize::new(40, 6), 1000, &sample_theme());
        // ANSI index 1 (themed red [235,80,90]), bold+italic+underline, then 24-bit bg + dim + strike,
        // then inverse, then hidden, then CJK wide chars.
        emu.feed(
            "\x1b[1;3;4;31mErr\x1b[0m \x1b[2;9;48;2;30;40;50mdim\x1b[0m \x1b[7mInv\x1b[0m \x1b[8mHid\x1b[0m 世界"
                .as_bytes(),
        );
        assert_eq!(emu.line_count(), 6);
        let line0 = emu.line(0);
        assert_eq!(line0.text(), "Err dim Inv Hid 世界");

        let r_err = &line0.runs[0];
        assert_eq!(r_err.text, "Err");
        assert_eq!(r_err.style.fg, Some(TerminalColor::Rgb(235, 80, 90)));
        assert_eq!(r_err.style.bg, None);
        assert!(r_err.style.bold && r_err.style.italic && r_err.style.underline);

        let r_dim = line0.runs.iter().find(|r| r.text == "dim").unwrap();
        assert!(r_dim.style.dim && r_dim.style.strikethrough);
        assert_eq!(r_dim.style.bg, Some(TerminalColor::Rgb(30, 40, 50)));

        let r_inv = line0.runs.iter().find(|r| r.text == "Inv").unwrap();
        assert!(r_inv.style.inverse);
        assert_eq!(r_inv.style.fg, Some(TerminalColor::Rgb(220, 225, 235)));
        assert_eq!(r_inv.style.bg, Some(TerminalColor::Rgb(18, 20, 26)));

        let r_hid = line0.runs.iter().find(|r| r.text == "Hid").unwrap();
        assert!(r_hid.style.hidden);

        // Erased background-colored line (CSI 2 K with active SGR 44m blue palette [90,150,245])
        // must preserve background across all 40 cells even with no characters printed.
        emu.feed(b"\r\n\x1b[44m\x1b[2K\x1b[0m");
        let line1 = emu.line(1);
        assert_eq!(line1.text().len(), 40);
        assert_eq!(
            line1.runs[0].style.bg,
            Some(TerminalColor::Rgb(90, 150, 245))
        );
    }

    #[test]
    fn ghostty_emulator_filters_kitty_u10eeee_placeholders_and_emits_apc_replies() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let mut emu = GhosttyEmulator::new(TerminalSize::new(24, 6), 500);
        let rgba = STANDARD.encode([0u8, 200, 255, 255].repeat(16));
        let payload = format!(
            "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\
             \x1b_Gf=32,s=4,v=4,a=T,U=1,i=42,c=2,r=1,q=2;{rgba}\x1b\\\
             \x1b[1;1HA\x1b[4;38;2;0;0;42m\u{10EEEE}\u{0305}\u{0305}\u{10EEEE}\u{0305}\u{030D}\x1b[0mB"
        );
        emu.feed(payload.as_bytes());

        // Kitty query must produce a Reply event.
        let events = emu.take_events();
        assert!(events.iter().any(|ev| match ev {
            TerminalEmulatorEvent::Reply(bytes) =>
                String::from_utf8_lossy(bytes).contains("\x1b_Gi=31;OK\x1b\\"),
            _ => false,
        }));

        // Placeholder cells + combining diacritics must render as plain spaces with underline stripped.
        let line0 = emu.line(0);
        assert_eq!(line0.text(), "A  B");
        assert!(!line0.text().contains('\u{10EEEE}'));
        for run in &line0.runs {
            if run.text == "  " {
                assert!(
                    !run.style.underline,
                    "placeholder spaces must suppress underline"
                );
            }
        }
    }

    #[test]
    fn ghostty_emulator_tracks_scrollback_cursor_modes_title_bell_and_osc52() {
        let mut emu = GhosttyEmulator::new(TerminalSize::new(20, 4), 200);
        for i in 0..10 {
            emu.feed(format!("line-{i:02}\r\n").as_bytes());
        }
        emu.feed(b"prompt> ");
        let total = emu.line_count();
        assert!(total > 4, "expected scrollback lines, got {total}");
        assert_eq!(emu.line(0).text(), "line-00");
        assert_eq!(emu.line(total - 1).text(), "prompt>");

        let cur = emu.cursor().expect("cursor visible");
        assert_eq!((cur.col, cur.row), (8, 3));

        // Hide cursor (DEC mode 25 off) + enable DECCKM (1) + bracketed paste (2004).
        emu.feed(b"\x1b[?25l\x1b[?1h\x1b[?2004h");
        assert!(emu.cursor().is_none());
        let modes = emu.modes();
        assert!(modes.application_cursor);
        assert!(modes.bracketed_paste);

        // Title, Bell, and OSC 52 clipboard.
        emu.feed(b"\x1b]2;Canban Linux Shell\x07\x07\x1b]52;c;bGludXg=\x07");
        let events = emu.take_events();
        assert!(events.contains(&TerminalEmulatorEvent::Title("Canban Linux Shell".into())));
        assert!(events.contains(&TerminalEmulatorEvent::Bell));
        assert_eq!(emu.take_clipboard().as_deref(), Some("linux"));
    }

    #[test]
    fn file_uri_decoding_and_fc_list_parsing_handle_linux_edge_cases() {
        assert_eq!(
            decode_file_uri("file:///home/jjy/My%20Docs/%E6%B5%8B%E8%AF%95%20(1).png").as_deref(),
            Some("/home/jjy/My Docs/测试 (1).png")
        );
        assert_eq!(
            decode_file_uri("file://localhost/tmp/a%23b.txt").as_deref(),
            Some("/tmp/a#b.txt")
        );
        assert_eq!(decode_file_uri("https://canban.dev"), None);

        let all_fc = "\
JetBrains Mono\t100\tFalse
Adwaita Mono\t100\tFalse
Noto Color Emoji\t100\tTrue
Noto Sans SignWriting\t100\tFalse
Noto Sans\t\tFalse
Noto Sans CJK SC\t\tFalse
.Hidden System\t100\tFalse
";
        let ascii_fc = "\
JetBrains Mono\t100\tFalse
Adwaita Mono\t100\tFalse
";
        let parsed = parse_fc_list_output(all_fc, ascii_fc);
        assert_eq!(
            parsed.monospaced,
            vec!["Adwaita Mono".to_owned(), "JetBrains Mono".to_owned()]
        );
        assert!(parsed.others.contains(&"Noto Sans".to_owned()));
        assert!(parsed.others.contains(&"Noto Sans CJK SC".to_owned()));
        assert!(!parsed.monospaced.contains(&"Noto Color Emoji".to_owned()));
        assert!(
            !parsed
                .monospaced
                .contains(&"Noto Sans SignWriting".to_owned())
        );
    }

    #[test]
    fn shared_session_slot_rebinds_and_evicts_across_pooled_sessions() {
        let cfg = canban_types::Config {
            hc: "/usr/bin/false".into(),
            hc_session: "linux-slot-lru".into(),
            ..Default::default()
        };
        let bind_a = LiveBinding::new(cfg.clone(), "core-lx", "tty-lx-a");
        let bind_b = LiveBinding::new(cfg, "core-lx", "tty-lx-b");

        let mut slot = ControllerSlot::new(false);
        assert!(slot.rebind(Some(bind_a.clone())));
        slot.state
            .lock()
            .unwrap()
            .model
            .write(b"linux-pooled-output\r\n");

        assert!(slot.rebind(Some(bind_b)));
        assert!(
            !slot
                .state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("linux-pooled-output")
        );

        assert!(slot.rebind(Some(bind_a)));
        assert!(
            slot.state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("linux-pooled-output")
        );

        slot.detach_session();
        crate::evict_session("core-lx", "tty-lx-a");
        crate::evict_session("core-lx", "tty-lx-b");
    }

    #[cfg(all(target_os = "linux", feature = "herdr-local"))]
    #[test]
    fn linux_installed_fonts_discovers_host_families_gracefully() {
        let catalog = crate::installed_fonts();
        if canban_exec::command("fc-list")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            assert!(
                !catalog.monospaced.is_empty() || !catalog.others.is_empty(),
                "expected fc-list to discover installed fonts on Linux host"
            );
        }
    }
}
