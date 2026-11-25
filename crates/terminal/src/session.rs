//! Platform-neutral, transport-agnostic PTY live attachment pool shared across macOS, Linux,
//! and future remote/mobile viewers.
use crate::model::Model;
use canban_types::Config;
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub const CONNECTING_GRACE: Duration = Duration::from_millis(180);
pub const ATTACH_RESIZE_COALESCE: Duration = Duration::from_millis(75);
pub const ATTACH_RESIZE_SYNC_CAP: Duration = Duration::from_millis(160);
const ATTACH_POST_SNAPSHOT_BURST: Duration = Duration::from_millis(2);
const DEFAULT_RECV_TIMEOUT: Duration = Duration::from_millis(15);
pub const MAX_POOLED_SESSIONS: usize = 8;

static LAST_VIEWPORT_SIZE: AtomicU32 = AtomicU32::new(0);
static LAST_CELL_METRICS_MILLI: AtomicU64 = AtomicU64::new(0);

/// Records the most recent live terminal viewport grid dimensions `(cols, rows)`.
pub fn record_viewport_size(cols: u16, rows: u16) {
    if cols >= 2 && rows >= 1 {
        let packed = (u32::from(cols) << 16) | u32::from(rows);
        LAST_VIEWPORT_SIZE.store(packed, Ordering::Relaxed);
    }
}

/// Returns the most recent live terminal viewport grid dimensions `(cols, rows)`.
pub fn last_viewport_size() -> Option<(u16, u16)> {
    let packed = LAST_VIEWPORT_SIZE.load(Ordering::Relaxed);
    if packed == 0 {
        return None;
    }
    let cols = (packed >> 16) as u16;
    let rows = (packed & 0xFFFF) as u16;
    (cols >= 2 && rows >= 1).then_some((cols, rows))
}

/// Records the most recent terminal cell dimensions in millipoints (`pt * 1000`).
pub fn record_cell_metrics_milli(cell_w_milli: u32, cell_h_milli: u32) {
    if cell_w_milli > 0 && cell_h_milli > 0 {
        let packed = (u64::from(cell_w_milli) << 32) | u64::from(cell_h_milli);
        LAST_CELL_METRICS_MILLI.store(packed, Ordering::Relaxed);
    }
}

/// Returns the most recent terminal cell dimensions in millipoints (`pt * 1000`).
pub fn last_cell_metrics_milli() -> Option<(u32, u32)> {
    let packed = LAST_CELL_METRICS_MILLI.load(Ordering::Relaxed);
    if packed == 0 {
        return None;
    }
    let w = (packed >> 32) as u32;
    let h = (packed & 0xFFFF_FFFF) as u32;
    (w > 0 && h > 0).then_some((w, h))
}

/// Display geometry and color facts reported to the remote/local PTY backend.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayFacts {
    pub fg: Option<[u8; 3]>,
    pub bg: Option<[u8; 3]>,
    pub cursor: Option<[u8; 3]>,
    pub cell: Option<(u32, u32)>,
}

impl DisplayFacts {
    fn hex(rgb: [u8; 3]) -> String {
        format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "fg": self.fg.map(Self::hex),
            "bg": self.bg.map(Self::hex),
            "cursor": self.cursor.map(Self::hex),
            "cell": self.cell.map(|(w, h)| [w.max(1), h.max(1)]),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.fg.is_none() && self.bg.is_none() && self.cursor.is_none() && self.cell.is_none()
    }
}

/// Transport-agnostic events emitted by an attached PTY stream.
#[derive(Debug)]
pub enum PtyEvent {
    Data(Vec<u8>),
    Resize(u16, u16),
    Closed(Value),
    Lost(String),
}

/// Parameters passed to [`PtyTransport::attach`].
#[derive(Clone, Debug)]
pub struct PtyAttachParams {
    pub core_id: String,
    pub terminal: String,
    pub cols: u16,
    pub rows: u16,
    pub display: DisplayFacts,
}

/// Thread-safe writer handle for an active PTY attachment.
pub trait PtyWriter: Send + Sync {
    fn write(&self, bytes: &[u8]) -> anyhow::Result<()>;

    fn write_auto(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.write(bytes)
    }

    fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()>;

    fn focus(&self, focused: bool) -> anyhow::Result<()> {
        let _ = focused;
        Ok(())
    }

    fn facts(&self, display: &DisplayFacts) -> anyhow::Result<()> {
        let _ = display;
        Ok(())
    }
}

/// Active attached PTY stream owned by a [`LiveSession`] worker thread.
pub trait PtyAttachment: Send {
    fn writer(&self) -> Arc<dyn PtyWriter>;

    fn recv(&mut self, timeout: Duration) -> anyhow::Result<Option<PtyEvent>>;

    fn consumed(&mut self, len: usize) -> anyhow::Result<()> {
        let _ = len;
        Ok(())
    }

    fn write(&mut self, bytes: &[u8], automatic: bool) -> anyhow::Result<()> {
        if automatic {
            self.writer().write_auto(bytes)
        } else {
            self.writer().write(bytes)
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.writer().resize(cols, rows)
    }

    fn focus(&mut self, focused: bool) -> anyhow::Result<()> {
        self.writer().focus(focused)
    }

    fn facts(&mut self, display: &DisplayFacts) -> anyhow::Result<()> {
        self.writer().facts(display)
    }

    fn resized_on_attach(&self) -> bool {
        false
    }

    fn abort(&mut self) {}

    fn detach(self: Box<Self>) {}
}

/// Transport-agnostic PTY attachment backend (`attach`, `write`, `resize`, `detach`).
pub trait PtyTransport: Send + Sync + 'static {
    fn attach(&self, params: &PtyAttachParams) -> anyhow::Result<Box<dyn PtyAttachment>>;

    fn write(&self, attachment: &mut dyn PtyAttachment, bytes: &[u8]) -> anyhow::Result<()> {
        attachment.write(bytes, false)
    }

    fn resize(
        &self,
        attachment: &mut dyn PtyAttachment,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<()> {
        attachment.resize(cols, rows)
    }

    fn detach(&self, attachment: Box<dyn PtyAttachment>) {
        attachment.detach();
    }
}

/// Local/relay Herdr (`hc`) PTY transport implementation.
#[cfg(feature = "herdr-local")]
pub struct HerdrLocalTransport {
    config: Config,
}

#[cfg(feature = "herdr-local")]
impl HerdrLocalTransport {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    fn to_herdr_display(display: &DisplayFacts) -> canban_herdr::stream::DisplayFacts {
        canban_herdr::stream::DisplayFacts {
            fg: display.fg,
            bg: display.bg,
            cursor: display.cursor,
            cell: display.cell,
        }
    }
}

#[cfg(feature = "herdr-local")]
struct HerdrWriter(canban_herdr::stream::InputWriter);

#[cfg(feature = "herdr-local")]
impl PtyWriter for HerdrWriter {
    fn write(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.0.input(bytes)
    }

    fn write_auto(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.0.automatic_input(bytes)
    }

    fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.0.resize(cols, rows)
    }

    fn focus(&self, focused: bool) -> anyhow::Result<()> {
        self.0.focus(focused)
    }

    fn facts(&self, display: &DisplayFacts) -> anyhow::Result<()> {
        self.0
            .facts(&HerdrLocalTransport::to_herdr_display(display))
    }
}

#[cfg(feature = "herdr-local")]
struct HerdrAttachment(canban_herdr::stream::Attachment);

#[cfg(feature = "herdr-local")]
impl PtyAttachment for HerdrAttachment {
    fn writer(&self) -> Arc<dyn PtyWriter> {
        Arc::new(HerdrWriter(self.0.input_writer()))
    }

    fn recv(&mut self, timeout: Duration) -> anyhow::Result<Option<PtyEvent>> {
        use canban_herdr::stream::Event;
        let ev = self.0.recv(timeout)?;
        Ok(ev.map(|e| match e {
            Event::Data(data) => PtyEvent::Data(data),
            Event::Resize(c, r) => PtyEvent::Resize(c, r),
            Event::Closed(reason) => PtyEvent::Closed(reason),
            Event::Lost(error) => PtyEvent::Lost(error),
        }))
    }

    fn consumed(&mut self, len: usize) -> anyhow::Result<()> {
        self.0.consumed(len)
    }

    fn write(&mut self, bytes: &[u8], automatic: bool) -> anyhow::Result<()> {
        self.0.input(bytes, automatic)
    }

    fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.0.resize(cols, rows)
    }

    fn focus(&mut self, focused: bool) -> anyhow::Result<()> {
        self.0.focus(focused)
    }

    fn facts(&mut self, display: &DisplayFacts) -> anyhow::Result<()> {
        self.0
            .facts(&HerdrLocalTransport::to_herdr_display(display))
    }

    fn resized_on_attach(&self) -> bool {
        self.0.resized_on_attach
    }

    fn abort(&mut self) {
        self.0.abort();
    }
}

#[cfg(feature = "herdr-local")]
impl PtyTransport for HerdrLocalTransport {
    fn attach(&self, params: &PtyAttachParams) -> anyhow::Result<Box<dyn PtyAttachment>> {
        let relay_fenced = self.config.relay_endpoint.is_some();
        let client = canban_herdr::Client::new(self.config.clone());
        if !relay_fenced {
            anyhow::ensure!(
                client.status()?["core"]["id"] == params.core_id,
                "Original core identity changed"
            );
        }
        let herdr_display = Self::to_herdr_display(&params.display);
        let mut attachment = client.attach_fenced_with_display(
            &params.terminal,
            params.cols,
            params.rows,
            Some(&params.core_id),
            Some(&herdr_display),
        )?;
        if !relay_fenced {
            attachment.track_view(&params.core_id, &params.terminal);
        }
        Ok(Box::new(HerdrAttachment(attachment)))
    }
}

#[cfg(not(feature = "herdr-local"))]
struct UnavailableTransport;

#[cfg(not(feature = "herdr-local"))]
impl PtyTransport for UnavailableTransport {
    fn attach(&self, _params: &PtyAttachParams) -> anyhow::Result<Box<dyn PtyAttachment>> {
        anyhow::bail!("herdr-local feature is disabled; provide a custom PtyTransport")
    }
}

fn default_transport(config: Config) -> Arc<dyn PtyTransport> {
    #[cfg(feature = "herdr-local")]
    {
        Arc::new(HerdrLocalTransport::new(config))
    }
    #[cfg(not(feature = "herdr-local"))]
    {
        let _ = config;
        Arc::new(UnavailableTransport)
    }
}

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
    pub attach_settle_until: Option<Instant>,
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
            attach_settle_until: None,
            size_ready: false,
            focused: false,
            theme: None,
        }
    }

    pub fn is_attach_settling(&self) -> bool {
        self.attach_settle_until
            .is_some_and(|deadline| Instant::now() < deadline)
    }

    pub fn clear_attach_settle(&mut self) -> bool {
        self.attach_settle_until.take().is_some()
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum TransportKey {
    Custom(usize),
    Config {
        relay_endpoint: Option<String>,
        relay_token: Option<String>,
        hc: String,
        hc_session: String,
    },
}

fn transport_key(transport: &Option<Arc<dyn PtyTransport>>, config: &Config) -> TransportKey {
    match transport {
        Some(t) => TransportKey::Custom(Arc::as_ptr(t) as *const () as usize),
        None => TransportKey::Config {
            relay_endpoint: config.relay_endpoint.clone(),
            relay_token: config.relay_token.clone(),
            hc: config.hc.clone(),
            hc_session: config.hc_session.clone(),
        },
    }
}

pub struct LiveSession {
    pub key: (String, String),
    pub host_id: String,
    transport_key: TransportKey,
    epoch: u64,
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
    pub host_id: String,
    pub config: Config,
    pub core: String,
    pub terminal: String,
    pub initial_cols: u16,
    pub initial_rows: u16,
    pub initial_cell: (u32, u32),
    pub size_ready: bool,
    pub focused: bool,
    pub theme: Option<Value>,
    pub transport: Option<Arc<dyn PtyTransport>>,
    pub session_epoch: u64,
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

/// Evicts any background (listener-free) cached sessions belonging to `host_id`
/// so subsequent views re-attach using the host's updated transport.
pub fn evict_idle_host_sessions(host_id: &str) {
    if host_id.is_empty() {
        return;
    }
    let mut pool = session_pool().lock().unwrap();
    pool.retain(|s| {
        let evict = s.host_id == host_id && !s.wake.has_listeners();
        if evict {
            s.stop.store(true, Ordering::SeqCst);
            s.size_cv.notify_all();
        }
        !evict
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
    notice_status: &str,
    is_preview: bool,
) -> String {
    if read_only {
        let reason = if !input_status.is_empty() {
            input_status
        } else if is_preview && !notice_status.is_empty() {
            notice_status
        } else if is_preview {
            "Preview"
        } else if !state.connected && state.status.starts_with("Connecting") {
            "Connecting"
        } else {
            "Disconnected"
        };
        if state.status.is_empty()
            || matches!(reason, "Closing" | "Auto-reclaiming…")
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
    } else if !state.status.is_empty() {
        state.status.clone()
    } else {
        notice_status.to_owned()
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

#[derive(Clone, Copy, Debug)]
struct AttachSettleTimings {
    coalesce: Duration,
    sync_cap: Duration,
    post_snapshot_burst: Duration,
}

impl Default for AttachSettleTimings {
    fn default() -> Self {
        Self {
            coalesce: ATTACH_RESIZE_COALESCE,
            sync_cap: ATTACH_RESIZE_SYNC_CAP,
            post_snapshot_burst: ATTACH_POST_SNAPSHOT_BURST,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttachSettlePhase {
    Inactive,
    AwaitingInitialSnapshot,
    AwaitingRedraw {
        snapshot_at: Instant,
        saw_render_hold: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SettleDataOutcome {
    should_wake: bool,
    released: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AttachSettle {
    phase: AttachSettlePhase,
    sync_deadline: Instant,
    post_snapshot_burst: Duration,
}

impl AttachSettle {
    fn start(
        now: Instant,
        needs_settle: bool,
        timings: AttachSettleTimings,
        deadline_out: &mut Option<Instant>,
    ) -> Self {
        let sync_deadline = now + timings.sync_cap;
        let phase = if needs_settle {
            *deadline_out = Some(now + timings.coalesce);
            AttachSettlePhase::AwaitingInitialSnapshot
        } else {
            *deadline_out = None;
            AttachSettlePhase::Inactive
        };
        Self {
            phase,
            sync_deadline,
            post_snapshot_burst: timings.post_snapshot_burst,
        }
    }

    fn is_active(&self) -> bool {
        self.phase != AttachSettlePhase::Inactive
    }

    fn next_recv_timeout(
        &mut self,
        now: Instant,
        deadline: Option<Instant>,
        render_held: bool,
    ) -> Duration {
        if !self.is_active() {
            return DEFAULT_RECV_TIMEOUT;
        }
        let Some(deadline) = deadline else {
            self.phase = AttachSettlePhase::Inactive;
            return DEFAULT_RECV_TIMEOUT;
        };
        let target = if render_held {
            deadline.max(self.sync_deadline)
        } else {
            deadline
        };
        let remaining = target.saturating_duration_since(now);
        if remaining.is_zero() {
            Duration::ZERO
        } else {
            remaining.clamp(Duration::from_millis(1), DEFAULT_RECV_TIMEOUT)
        }
    }

    fn on_data(
        &mut self,
        now: Instant,
        deadline: &mut Option<Instant>,
        render_held: bool,
        render_state_dirty: bool,
    ) -> SettleDataOutcome {
        let Some(current_deadline) = *deadline else {
            self.phase = AttachSettlePhase::Inactive;
            return SettleDataOutcome {
                should_wake: !render_held || render_state_dirty,
                released: false,
            };
        };
        match self.phase {
            AttachSettlePhase::Inactive => {
                *deadline = None;
                SettleDataOutcome {
                    should_wake: !render_held || render_state_dirty,
                    released: false,
                }
            }
            AttachSettlePhase::AwaitingInitialSnapshot => {
                if now >= current_deadline && !render_held {
                    *deadline = None;
                    self.phase = AttachSettlePhase::Inactive;
                    SettleDataOutcome {
                        should_wake: true,
                        released: true,
                    }
                } else {
                    self.phase = AttachSettlePhase::AwaitingRedraw {
                        snapshot_at: now,
                        saw_render_hold: render_held,
                    };
                    SettleDataOutcome {
                        should_wake: false,
                        released: false,
                    }
                }
            }
            AttachSettlePhase::AwaitingRedraw {
                snapshot_at,
                saw_render_hold,
            } => {
                let now_saw_hold = saw_render_hold || render_held;
                if render_held {
                    *deadline = Some(self.sync_deadline);
                    self.phase = AttachSettlePhase::AwaitingRedraw {
                        snapshot_at,
                        saw_render_hold: true,
                    };
                    SettleDataOutcome {
                        should_wake: false,
                        released: false,
                    }
                } else if now_saw_hold
                    || now.saturating_duration_since(snapshot_at) >= self.post_snapshot_burst
                    || now >= current_deadline
                {
                    *deadline = None;
                    self.phase = AttachSettlePhase::Inactive;
                    SettleDataOutcome {
                        should_wake: true,
                        released: true,
                    }
                } else {
                    self.phase = AttachSettlePhase::AwaitingRedraw {
                        snapshot_at,
                        saw_render_hold: now_saw_hold,
                    };
                    SettleDataOutcome {
                        should_wake: false,
                        released: false,
                    }
                }
            }
        }
    }

    fn on_tick(&mut self, now: Instant, deadline: &mut Option<Instant>, render_held: bool) -> bool {
        if !self.is_active() {
            return false;
        }
        let Some(current_deadline) = *deadline else {
            self.phase = AttachSettlePhase::Inactive;
            return false;
        };
        if now < current_deadline {
            return false;
        }
        if render_held && now < self.sync_deadline {
            *deadline = Some(self.sync_deadline);
            false
        } else {
            *deadline = None;
            self.phase = AttachSettlePhase::Inactive;
            true
        }
    }
}

pub fn spawn_session(init: SessionInit) -> Arc<LiveSession> {
    spawn_session_with_timings(init, AttachSettleTimings::default())
}

fn spawn_session_with_timings(
    init: SessionInit,
    settle_timings: AttachSettleTimings,
) -> Arc<LiveSession> {
    let SessionInit {
        host_id,
        config,
        core,
        terminal,
        initial_cols,
        initial_rows,
        initial_cell,
        size_ready,
        focused,
        theme,
        transport,
        session_epoch,
    } = init;
    let t_key = transport_key(&transport, &config);
    let transport = transport.unwrap_or_else(|| default_transport(config));
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
        attach_settle_until: None,
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
        host_id,
        transport_key: t_key,
        epoch: session_epoch,
        state: state.clone(),
        size_cv: size_cv.clone(),
        wake: wake.clone(),
        input: tx,
        stop: stop.clone(),
    });

    std::thread::spawn(move || {
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
        type AttachOutcome = (Box<dyn PtyAttachment>, (u16, u16), DisplayFacts);
        let mut first_attach = true;
        while !stop.load(Ordering::SeqCst) {
            let attempt = (|| -> anyhow::Result<AttachOutcome> {
                let (size, display) = {
                    let s = state.lock().unwrap();
                    (
                        (s.model.cols, s.model.height),
                        display_facts_for_model(&s.model, s.theme.as_ref()),
                    )
                };
                let params = PtyAttachParams {
                    core_id: core.clone(),
                    terminal: terminal.clone(),
                    cols: size.0,
                    rows: size.1,
                    display: display.clone(),
                };
                let attachment = transport.attach(&params)?;
                Ok((attachment, size, display))
            })();
            let (mut attach, attached_size, attached_display) = match attempt {
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
            let (effective_size, mut settle) = {
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
                let mut effective_size = attached_size;
                // Layout may have resized the view or updated font/theme while attach was connecting.
                if setup_res.is_ok() && s.size_ready && size != attached_size {
                    setup_res = transport.resize(attach.as_mut(), size.0, size.1);
                    effective_size = size;
                }
                let latest_display = display_facts_for_model(&s.model, s.theme.as_ref());
                if setup_res.is_ok() && latest_display != attached_display {
                    setup_res = attach.facts(&latest_display);
                }
                if let Err(e) = setup_res {
                    s.status = format!("Attach setup failed: {e} · reconnecting");
                    s.attach_settle_until = None;
                    drop(s);
                    transport.detach(attach);
                    wake.notify();
                    continue;
                }
                let needs_settle = attach.resized_on_attach() || effective_size != attached_size;
                let settle = AttachSettle::start(
                    Instant::now(),
                    needs_settle,
                    settle_timings,
                    &mut s.attach_settle_until,
                );
                s.connected = true;
                (effective_size, settle)
            };
            if !settle.is_active() {
                wake.notify();
            }

            let input_stop = Arc::new(AtomicBool::new(false));
            let failed = Arc::new(AtomicBool::new(false));
            let input_worker = {
                let writer = attach.writer();
                let rx = rx.clone();
                let stopped = input_stop.clone();
                let failed = failed.clone();
                let state = state.clone();
                let wake = wake.clone();
                std::thread::spawn(move || {
                    let mut last_sent_size = effective_size;
                    while !stopped.load(Ordering::SeqCst) {
                        let message = rx.lock().unwrap().recv_timeout(Duration::from_millis(20));
                        let result = match message {
                            Ok(Input::Bytes(bytes)) => writer.write(&bytes),
                            Ok(Input::AutoBytes(bytes)) => writer.write_auto(&bytes),
                            Ok(Input::Resize(c, r)) => {
                                if (c, r) == last_sent_size {
                                    continue;
                                }
                                last_sent_size = (c, r);
                                writer.resize(c, r)
                            }
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
            let mut pending_event: Option<anyhow::Result<Option<PtyEvent>>> = None;
            while !stop.load(Ordering::SeqCst) && !failed.load(Ordering::SeqCst) {
                let next_event = if let Some(ev) = pending_event.take() {
                    ev
                } else {
                    let recv_timeout = if settle.is_active() {
                        let s = state.lock().unwrap();
                        settle.next_recv_timeout(
                            Instant::now(),
                            s.attach_settle_until,
                            s.model.is_render_held(),
                        )
                    } else {
                        DEFAULT_RECV_TIMEOUT
                    };
                    attach.recv(recv_timeout)
                };
                match next_event {
                    Ok(Some(PtyEvent::Data(first_data))) => {
                        let mut chunks = vec![first_data];
                        while chunks.len() < 32 {
                            match attach.recv(Duration::ZERO) {
                                Ok(Some(PtyEvent::Data(more))) => chunks.push(more),
                                Ok(None) => break,
                                other => {
                                    pending_event = Some(other);
                                    break;
                                }
                            }
                        }
                        let total_len: usize = chunks.iter().map(Vec::len).sum();
                        let (replies, should_wake) = {
                            let mut s = state.lock().unwrap();
                            for chunk in &chunks {
                                s.model.write(chunk);
                            }
                            let render_held = s.model.is_render_held();
                            let render_dirty = s.model.render_state_dirty();
                            let outcome = settle.on_data(
                                Instant::now(),
                                &mut s.attach_settle_until,
                                render_held,
                                render_dirty,
                            );
                            if outcome.released {
                                s.model.revision += 1;
                            }
                            let should_wake = outcome.should_wake;
                            (s.model.replies(), should_wake)
                        };
                        if should_wake {
                            wake.notify();
                        }
                        let result = attach.consumed(total_len).and_then(|_| {
                            if replies.is_empty() {
                                Ok(())
                            } else {
                                attach.write(&replies, true)
                            }
                        });
                        if result.is_err() {
                            reconnect = true;
                            break;
                        }
                    }
                    Ok(Some(PtyEvent::Resize(c, r))) => {
                        let mut s = state.lock().unwrap();
                        if !s.size_ready {
                            s.model.resize(c, r);
                            drop(s);
                            wake.notify();
                        }
                    }
                    Ok(Some(PtyEvent::Closed(reason))) => {
                        {
                            let mut s = state.lock().unwrap();
                            s.connected = false;
                            s.exited = true;
                            s.connecting_since = None;
                            s.attach_settle_until = None;
                            s.status = format!(
                                "Terminal exited · {}",
                                reason["reason"].as_str().unwrap_or("closed")
                            );
                        }
                        wake.notify();
                        input_stop.store(true, Ordering::SeqCst);
                        attach.abort();
                        let _ = input_worker.join();
                        transport.detach(attach);
                        evict_session(&core, &terminal);
                        return;
                    }
                    Ok(Some(PtyEvent::Lost(error))) => {
                        {
                            let mut s = state.lock().unwrap();
                            s.attach_settle_until = None;
                            s.status = format!("Disconnected · {error} · reconnecting");
                        }
                        wake.notify();
                        reconnect = true;
                        break;
                    }
                    Err(e) => {
                        {
                            let mut s = state.lock().unwrap();
                            s.attach_settle_until = None;
                            s.status = format!("Disconnected · {e}");
                        }
                        wake.notify();
                        reconnect = true;
                        break;
                    }
                    Ok(None) => {
                        let should_wake = {
                            let mut s = state.lock().unwrap();
                            let held_expired = s.model.expire_render_hold_if_due();
                            let anim_due = s.model.advance_animation_if_due();
                            let render_held = s.model.is_render_held();
                            let settle_released = settle.on_tick(
                                Instant::now(),
                                &mut s.attach_settle_until,
                                render_held,
                            );
                            if settle_released {
                                s.model.revision += 1;
                            }
                            held_expired || anim_due || settle_released
                        };
                        if should_wake {
                            wake.notify();
                        }
                    }
                }
            }
            input_stop.store(true, Ordering::SeqCst);
            attach.abort();
            let _ = input_worker.join();
            reconnect |= failed.load(Ordering::SeqCst);
            {
                let mut s = state.lock().unwrap();
                s.connected = false;
                s.attach_settle_until = None;
            }
            wake.notify();
            transport.detach(attach);
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
    let next_transport_key = transport_key(&init.transport, &init.config);
    let mut pool = session_pool().lock().unwrap();
    pool.retain(|s| !s.stop.load(Ordering::SeqCst) && !s.state.lock().unwrap().exited);
    if let Some(idx) = pool
        .iter()
        .position(|s| s.key.0 == init.core && s.key.1 == init.terminal)
    {
        let existing = pool.remove(idx).unwrap();
        if existing.transport_key == next_transport_key && existing.epoch == init.session_epoch {
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
        // Transport, relay endpoint, or host epoch changed after disconnect/reconnect:
        // stop the stale session so the freshly spawned one opens on the live transport.
        existing.stop.store(true, Ordering::SeqCst);
        existing.size_cv.notify_all();
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

#[cfg(test)]
mod tests {
    use super::*;

    struct MockWriter {
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        resizes: Arc<Mutex<Vec<(u16, u16)>>>,
    }

    impl PtyWriter for MockWriter {
        fn write(&self, bytes: &[u8]) -> anyhow::Result<()> {
            self.written.lock().unwrap().push(bytes.to_vec());
            Ok(())
        }

        fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
            self.resizes.lock().unwrap().push((cols, rows));
            Ok(())
        }
    }

    struct MockAttachment {
        writer: Arc<MockWriter>,
        rx: mpsc::Receiver<PtyEvent>,
        detached: Arc<AtomicBool>,
        resized_on_attach: bool,
    }

    impl PtyAttachment for MockAttachment {
        fn writer(&self) -> Arc<dyn PtyWriter> {
            self.writer.clone()
        }

        fn recv(&mut self, timeout: Duration) -> anyhow::Result<Option<PtyEvent>> {
            match self.rx.recv_timeout(timeout) {
                Ok(ev) => Ok(Some(ev)),
                Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
                Err(_) => Ok(Some(PtyEvent::Closed(serde_json::json!({"reason": "eof"})))),
            }
        }

        fn resized_on_attach(&self) -> bool {
            self.resized_on_attach
        }

        fn detach(self: Box<Self>) {
            self.detached.store(true, Ordering::SeqCst);
        }
    }

    struct MockTransport {
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        resizes: Arc<Mutex<Vec<(u16, u16)>>>,
        detached: Arc<AtomicBool>,
        events_rx: Mutex<Option<mpsc::Receiver<PtyEvent>>>,
        resized_on_attach: bool,
    }

    impl PtyTransport for MockTransport {
        fn attach(&self, _params: &PtyAttachParams) -> anyhow::Result<Box<dyn PtyAttachment>> {
            let rx = self
                .events_rx
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| anyhow::anyhow!("already attached"))?;
            Ok(Box::new(MockAttachment {
                writer: Arc::new(MockWriter {
                    written: self.written.clone(),
                    resizes: self.resizes.clone(),
                }),
                rx,
                detached: self.detached.clone(),
                resized_on_attach: self.resized_on_attach,
            }))
        }
    }

    #[test]
    fn custom_pty_transport_drives_live_session_and_lru_pool() {
        let (ev_tx, ev_rx) = mpsc::channel();
        let written = Arc::new(Mutex::new(Vec::new()));
        let resizes = Arc::new(Mutex::new(Vec::new()));
        let detached = Arc::new(AtomicBool::new(false));
        let transport = Arc::new(MockTransport {
            written: written.clone(),
            resizes: resizes.clone(),
            detached: detached.clone(),
            events_rx: Mutex::new(Some(ev_rx)),
            resized_on_attach: false,
        });

        let session = get_or_spawn_session(SessionInit {
            host_id: String::new(),
            config: Config::default(),
            core: "mock-core".into(),
            terminal: "mock-tty".into(),
            initial_cols: 80,
            initial_rows: 24,
            initial_cell: (8, 16),
            size_ready: true,
            focused: true,
            theme: None,
            transport: Some(transport),
            session_epoch: 0,
        });

        // Wait for transport.attach() to connect.
        for _ in 0..50 {
            if session.state.lock().unwrap().connected {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(session.state.lock().unwrap().connected);

        // Send output from transport and input/resize from viewer.
        ev_tx
            .send(PtyEvent::Data(b"hello custom transport\r\n".to_vec()))
            .unwrap();
        session.input.send(Input::Bytes(b"ls\n".to_vec())).unwrap();
        session.input.send(Input::Resize(100, 30)).unwrap();

        for _ in 0..50 {
            let has_text = session
                .state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("hello custom transport");
            let has_write = !written.lock().unwrap().is_empty();
            let has_resize = !resizes.lock().unwrap().is_empty();
            if has_text && has_write && has_resize {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(
            session
                .state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("hello custom transport")
        );
        assert_eq!(written.lock().unwrap().as_slice(), &[b"ls\n".to_vec()]);
        assert_eq!(resizes.lock().unwrap().as_slice(), &[(100, 30)]);

        // Evict and verify clean detach.
        evict_session("mock-core", "mock-tty");
        for _ in 0..50 {
            if detached.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(detached.load(Ordering::SeqCst));
    }

    #[test]
    fn effective_status_surfaces_auto_reclaim_and_writable_reclaim_warning() {
        let mut state = State::new(80, 24);
        state.connected = true;

        assert_eq!(
            effective_status(
                &state,
                false,
                "",
                "Reclaiming soon · Focus or type to keep open",
                false
            ),
            "Reclaiming soon · Focus or type to keep open"
        );
        assert!(accepts_pty_input(&state, false));

        assert_eq!(
            effective_status(&state, true, "Auto-reclaiming…", "", false),
            "Auto-reclaiming…"
        );
        assert!(!accepts_pty_input(&state, true));

        assert_eq!(
            effective_status(
                &state,
                true,
                "",
                "Reclaiming soon · Focus or type to keep open",
                true
            ),
            "Reclaiming soon · Focus or type to keep open"
        );
        assert_eq!(effective_status(&state, true, "", "", true), "Preview");

        state.status = "Disconnected".into();
        assert_eq!(
            effective_status(
                &state,
                false,
                "",
                "Reclaiming soon · Focus or type to keep open",
                false
            ),
            "Disconnected"
        );
    }

    #[test]
    fn same_size_attach_wakes_immediately_without_coalesce_delay() {
        let (ev_tx, ev_rx) = mpsc::channel();
        let written = Arc::new(Mutex::new(Vec::new()));
        let resizes = Arc::new(Mutex::new(Vec::new()));
        let detached = Arc::new(AtomicBool::new(false));
        let transport = Arc::new(MockTransport {
            written,
            resizes,
            detached,
            events_rx: Mutex::new(Some(ev_rx)),
            resized_on_attach: false,
        });
        let wakes = Arc::new(AtomicUsize::new(0));
        let wakes_cb = wakes.clone();
        let session = spawn_session(SessionInit {
            host_id: String::new(),
            config: Config::default(),
            core: "c-same".into(),
            terminal: "t-same".into(),
            initial_cols: 100,
            initial_rows: 30,
            initial_cell: (8, 18),
            size_ready: true,
            focused: false,
            theme: None,
            transport: Some(transport),
            session_epoch: 0,
        });
        session.wake.attach(
            99,
            Arc::new(move || {
                wakes_cb.fetch_add(1, Ordering::SeqCst);
            }),
        );

        for _ in 0..200 {
            if session.state.lock().unwrap().connected {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(session.state.lock().unwrap().connected);
        assert!(!session.state.lock().unwrap().is_attach_settling());

        let wakes_before = wakes.load(Ordering::SeqCst);
        session.wake.ack();
        ev_tx.send(PtyEvent::Data(b"ready\r\n".to_vec())).unwrap();
        for _ in 0..200 {
            let has_ready = session
                .state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("ready");
            if has_ready && wakes.load(Ordering::SeqCst) > wakes_before {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            session
                .state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("ready")
        );
        assert!(wakes.load(Ordering::SeqCst) > wakes_before);
    }

    #[test]
    fn attach_settle_same_size_is_inactive_and_wakes_normal_data() {
        let t0 = Instant::now();
        let mut deadline = None;
        let mut settle =
            AttachSettle::start(t0, false, AttachSettleTimings::default(), &mut deadline);
        assert_eq!(deadline, None);
        assert!(!settle.is_active());
        assert_eq!(
            settle.next_recv_timeout(t0, deadline, false),
            DEFAULT_RECV_TIMEOUT
        );
        assert!(!settle.on_tick(t0 + Duration::from_millis(100), &mut deadline, false));

        assert_eq!(
            settle.on_data(t0 + Duration::from_millis(1), &mut deadline, false, false),
            SettleDataOutcome {
                should_wake: true,
                released: false,
            }
        );
        assert_eq!(
            settle.on_data(t0 + Duration::from_millis(2), &mut deadline, true, false),
            SettleDataOutcome {
                should_wake: false,
                released: false,
            }
        );
        assert_eq!(
            settle.on_data(t0 + Duration::from_millis(3), &mut deadline, true, true),
            SettleDataOutcome {
                should_wake: true,
                released: false,
            }
        );
    }

    #[test]
    fn attach_settle_coalesces_snapshot_burst_and_sync_redraw_until_release() {
        let t0 = Instant::now();
        let mut deadline = None;
        let mut settle =
            AttachSettle::start(t0, true, AttachSettleTimings::default(), &mut deadline);
        assert!(settle.is_active());
        assert_eq!(deadline, Some(t0 + ATTACH_RESIZE_COALESCE));
        assert_eq!(
            settle.next_recv_timeout(t0, deadline, false),
            DEFAULT_RECV_TIMEOUT
        );

        // Initial reflow snapshot arrives at t0 + 1ms; stay quiet while waiting for redraw/burst.
        assert_eq!(
            settle.on_data(t0 + Duration::from_millis(1), &mut deadline, false, false),
            SettleDataOutcome {
                should_wake: false,
                released: false,
            }
        );
        assert!(settle.is_active());

        // Follow-up chunk within the 2ms burst window keeps coalescing.
        assert_eq!(
            settle.on_data(t0 + Duration::from_millis(2), &mut deadline, false, false),
            SettleDataOutcome {
                should_wake: false,
                released: false,
            }
        );
        assert!(settle.is_active());

        // Synchronized redraw (?2026h -> render_held = true) extends deadline to sync_cap (160ms).
        assert_eq!(
            settle.on_data(t0 + Duration::from_millis(5), &mut deadline, true, true),
            SettleDataOutcome {
                should_wake: false,
                released: false,
            }
        );
        assert_eq!(deadline, Some(t0 + ATTACH_RESIZE_SYNC_CAP));

        // Passing the 75ms mark while render_held remains true still suppresses wake.
        assert!(!settle.on_tick(t0 + Duration::from_millis(90), &mut deadline, true));
        assert!(settle.is_active());

        // Closing synchronized redraw (?2026l -> render_held = false) releases immediately.
        assert_eq!(
            settle.on_data(t0 + Duration::from_millis(100), &mut deadline, false, true),
            SettleDataOutcome {
                should_wake: true,
                released: true,
            }
        );
        assert_eq!(deadline, None);
        assert!(!settle.is_active());
    }

    #[test]
    fn attach_settle_handles_plain_redraw_coalesce_timeout_sync_cap_and_external_clear() {
        let t0 = Instant::now();

        // 1. Plain redraw after >= 2ms post-snapshot burst releases immediately.
        let mut deadline = None;
        let mut settle =
            AttachSettle::start(t0, true, AttachSettleTimings::default(), &mut deadline);
        assert_eq!(
            settle.on_data(t0 + Duration::from_millis(1), &mut deadline, false, false),
            SettleDataOutcome {
                should_wake: false,
                released: false,
            }
        );
        assert_eq!(
            settle.on_data(t0 + Duration::from_millis(3), &mut deadline, false, false),
            SettleDataOutcome {
                should_wake: true,
                released: true,
            }
        );
        assert_eq!(deadline, None);
        assert!(!settle.is_active());

        // 2. No redraw after initial snapshot releases cleanly at 75ms without 1ms extra clamp.
        let mut deadline = None;
        let mut settle =
            AttachSettle::start(t0, true, AttachSettleTimings::default(), &mut deadline);
        assert_eq!(
            settle.on_data(t0 + Duration::from_millis(1), &mut deadline, false, false),
            SettleDataOutcome {
                should_wake: false,
                released: false,
            }
        );
        assert!(!settle.on_tick(t0 + Duration::from_millis(74), &mut deadline, false));
        assert_eq!(
            settle.next_recv_timeout(t0 + Duration::from_millis(74), deadline, false),
            Duration::from_millis(1)
        );
        assert_eq!(
            settle.next_recv_timeout(t0 + Duration::from_millis(75), deadline, false),
            Duration::ZERO
        );
        assert!(settle.on_tick(t0 + Duration::from_millis(75), &mut deadline, false));
        assert_eq!(deadline, None);
        assert!(!settle.is_active());

        // 3. Unclosed ?2026h hold still releases at the 160ms sync cap.
        let mut deadline = None;
        let mut settle =
            AttachSettle::start(t0, true, AttachSettleTimings::default(), &mut deadline);
        assert_eq!(
            settle.on_data(t0 + Duration::from_millis(1), &mut deadline, true, false),
            SettleDataOutcome {
                should_wake: false,
                released: false,
            }
        );
        assert!(!settle.on_tick(t0 + Duration::from_millis(159), &mut deadline, true));
        assert!(settle.on_tick(t0 + Duration::from_millis(160), &mut deadline, true));
        assert_eq!(deadline, None);
        assert!(!settle.is_active());

        // 4. External UI clear (deadline = None) deactivates settle without extra release revision.
        let mut deadline = None;
        let mut settle =
            AttachSettle::start(t0, true, AttachSettleTimings::default(), &mut deadline);
        deadline = None;
        assert_eq!(
            settle.next_recv_timeout(t0 + Duration::from_millis(1), deadline, false),
            DEFAULT_RECV_TIMEOUT
        );
        assert!(!settle.is_active());
        assert!(!settle.on_tick(t0 + Duration::from_millis(2), &mut deadline, false));
        assert_eq!(
            settle.on_data(t0 + Duration::from_millis(3), &mut deadline, false, false),
            SettleDataOutcome {
                should_wake: true,
                released: false,
            }
        );
    }

    #[test]
    fn resized_attach_coalesces_initial_snapshot_and_sync_redraw_until_2026l() {
        let (ev_tx, ev_rx) = mpsc::channel();
        let written = Arc::new(Mutex::new(Vec::new()));
        let resizes = Arc::new(Mutex::new(Vec::new()));
        let detached = Arc::new(AtomicBool::new(false));
        let transport = Arc::new(MockTransport {
            written,
            resizes,
            detached,
            events_rx: Mutex::new(Some(ev_rx)),
            resized_on_attach: true,
        });
        let wakes = Arc::new(AtomicUsize::new(0));
        let wakes_cb = wakes.clone();
        let session = spawn_session_with_timings(
            SessionInit {
                host_id: String::new(),
                config: Config::default(),
                core: "c-sync".into(),
                terminal: "t-sync".into(),
                initial_cols: 100,
                initial_rows: 30,
                initial_cell: (8, 18),
                size_ready: true,
                focused: false,
                theme: None,
                transport: Some(transport),
                session_epoch: 0,
            },
            AttachSettleTimings {
                coalesce: Duration::from_secs(10),
                sync_cap: Duration::from_secs(20),
                post_snapshot_burst: Duration::ZERO,
            },
        );
        session.wake.attach(
            101,
            Arc::new(move || {
                wakes_cb.fetch_add(1, Ordering::SeqCst);
            }),
        );

        for _ in 0..200 {
            if session.state.lock().unwrap().connected {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(session.state.lock().unwrap().connected);
        assert!(session.state.lock().unwrap().is_attach_settling());
        assert_eq!(wakes.load(Ordering::SeqCst), 0);

        // 1. Garbled initial reflow snapshot arrives; must remain settling and silent.
        ev_tx
            .send(PtyEvent::Data(b"GARBLED_INIT".to_vec()))
            .unwrap();
        for _ in 0..200 {
            if session
                .state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("GARBLED_INIT")
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            session
                .state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("GARBLED_INIT")
        );
        assert!(session.state.lock().unwrap().is_attach_settling());
        assert_eq!(wakes.load(Ordering::SeqCst), 0);

        // 2. Partial synchronized redraw (?2026h without ?2026l yet) must keep settling.
        ev_tx
            .send(PtyEvent::Data(
                b"\x1b[?2026h\x1b[2J\x1b[HCLEAN_PART".to_vec(),
            ))
            .unwrap();
        for _ in 0..200 {
            if session.state.lock().unwrap().model.is_render_held() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(session.state.lock().unwrap().model.is_render_held());
        assert!(session.state.lock().unwrap().is_attach_settling());
        assert_eq!(wakes.load(Ordering::SeqCst), 0);

        // 3. Synchronized redraw finishes (?2026l) -> wakes immediately with clean screen.
        ev_tx
            .send(PtyEvent::Data(b"_DONE\x1b[?2026l".to_vec()))
            .unwrap();
        for _ in 0..200 {
            let released = session.state.lock().unwrap().attach_settle_until.is_none();
            if released && wakes.load(Ordering::SeqCst) >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(session.state.lock().unwrap().attach_settle_until.is_none());
        assert!(!session.state.lock().unwrap().is_attach_settling());
        assert!(wakes.load(Ordering::SeqCst) >= 1);
        assert!(
            session
                .state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("CLEAN_PART_DONE")
        );
    }

    #[test]
    fn resized_attach_without_winch_redraw_releases_after_coalesce_timeout() {
        let (ev_tx, ev_rx) = mpsc::channel();
        let written = Arc::new(Mutex::new(Vec::new()));
        let resizes = Arc::new(Mutex::new(Vec::new()));
        let detached = Arc::new(AtomicBool::new(false));
        let transport = Arc::new(MockTransport {
            written,
            resizes,
            detached,
            events_rx: Mutex::new(Some(ev_rx)),
            resized_on_attach: true,
        });
        let wakes = Arc::new(AtomicUsize::new(0));
        let wakes_cb = wakes.clone();
        let session = spawn_session_with_timings(
            SessionInit {
                host_id: String::new(),
                config: Config::default(),
                core: "c-plain".into(),
                terminal: "t-plain".into(),
                initial_cols: 100,
                initial_rows: 30,
                initial_cell: (8, 18),
                size_ready: true,
                focused: false,
                theme: None,
                transport: Some(transport),
                session_epoch: 0,
            },
            AttachSettleTimings {
                coalesce: Duration::from_secs(10),
                sync_cap: Duration::from_secs(20),
                post_snapshot_burst: Duration::ZERO,
            },
        );
        session.wake.attach(
            102,
            Arc::new(move || {
                wakes_cb.fetch_add(1, Ordering::SeqCst);
            }),
        );

        for _ in 0..200 {
            if session.state.lock().unwrap().connected {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(session.state.lock().unwrap().connected);
        ev_tx
            .send(PtyEvent::Data(b"PLAIN_SHELL>".to_vec()))
            .unwrap();
        for _ in 0..200 {
            if session
                .state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("PLAIN_SHELL>")
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            session
                .state
                .lock()
                .unwrap()
                .model
                .viewport_text()
                .contains("PLAIN_SHELL>")
        );
        assert!(session.state.lock().unwrap().is_attach_settling());
        assert_eq!(wakes.load(Ordering::SeqCst), 0);

        let rev_before = {
            let mut s = session.state.lock().unwrap();
            let rev = s.model.revision;
            s.attach_settle_until = Some(
                Instant::now()
                    .checked_sub(Duration::from_millis(1))
                    .unwrap_or_else(Instant::now),
            );
            rev
        };
        for _ in 0..200 {
            let released = session.state.lock().unwrap().attach_settle_until.is_none();
            if released && wakes.load(Ordering::SeqCst) >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(session.state.lock().unwrap().attach_settle_until.is_none());
        assert!(!session.state.lock().unwrap().is_attach_settling());
        assert!(wakes.load(Ordering::SeqCst) >= 1);
        assert!(session.state.lock().unwrap().model.revision > rev_before);
    }

    #[test]
    fn duplicate_pre_attach_resize_is_suppressed_while_preserving_early_bytes() {
        let (_ev_tx, ev_rx) = mpsc::channel();
        let written = Arc::new(Mutex::new(Vec::new()));
        let resizes = Arc::new(Mutex::new(Vec::new()));
        let detached = Arc::new(AtomicBool::new(false));
        let transport = Arc::new(MockTransport {
            written: written.clone(),
            resizes: resizes.clone(),
            detached,
            events_rx: Mutex::new(Some(ev_rx)),
            resized_on_attach: false,
        });
        let session = spawn_session(SessionInit {
            host_id: String::new(),
            config: Config::default(),
            core: "c-dup".into(),
            terminal: "t-dup".into(),
            initial_cols: 100,
            initial_rows: 30,
            initial_cell: (8, 18),
            size_ready: true,
            focused: false,
            theme: None,
            transport: Some(transport),
            session_epoch: 0,
        });

        for _ in 0..50 {
            if session.state.lock().unwrap().connected {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        // Duplicate resize matching attached_size (100, 30) is suppressed; new resize (110, 32) goes through.
        session.input.send(Input::Bytes(b"hi".to_vec())).unwrap();
        session.input.send(Input::Resize(100, 30)).unwrap();
        session.input.send(Input::Resize(110, 32)).unwrap();

        for _ in 0..50 {
            if !written.lock().unwrap().is_empty() && !resizes.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(written.lock().unwrap().as_slice(), &[b"hi".to_vec()]);
        assert_eq!(resizes.lock().unwrap().as_slice(), &[(110, 32)]);
    }

    #[test]
    fn pool_reuses_same_transport_and_evicts_on_transport_relay_or_epoch_change() {
        let make_transport = || {
            let (_tx, rx) = mpsc::channel();
            Arc::new(MockTransport {
                written: Arc::new(Mutex::new(Vec::new())),
                resizes: Arc::new(Mutex::new(Vec::new())),
                detached: Arc::new(AtomicBool::new(false)),
                events_rx: Mutex::new(Some(rx)),
                resized_on_attach: false,
            }) as Arc<dyn PtyTransport>
        };

        let core = "core-rebind-pool";
        let tty = "tty-rebind-pool";
        evict_session(core, tty);

        let tr1 = make_transport();
        let s1 = get_or_spawn_session(SessionInit {
            host_id: "devpc".into(),
            config: Config::default(),
            core: core.into(),
            terminal: tty.into(),
            initial_cols: 80,
            initial_rows: 24,
            initial_cell: (8, 16),
            size_ready: true,
            focused: false,
            theme: None,
            transport: Some(tr1.clone()),
            session_epoch: 0,
        });

        // Same transport pointer + same epoch -> reuses identical session.
        let s1_again = get_or_spawn_session(SessionInit {
            host_id: "devpc".into(),
            config: Config::default(),
            core: core.into(),
            terminal: tty.into(),
            initial_cols: 80,
            initial_rows: 24,
            initial_cell: (8, 16),
            size_ready: true,
            focused: false,
            theme: None,
            transport: Some(tr1),
            session_epoch: 0,
        });
        assert!(Arc::ptr_eq(&s1, &s1_again));

        // Different transport pointer (e.g. reconnected RemotePtyTransport) -> replaces + stops s1.
        let tr2 = make_transport();
        let s2 = get_or_spawn_session(SessionInit {
            host_id: "devpc".into(),
            config: Config::default(),
            core: core.into(),
            terminal: tty.into(),
            initial_cols: 80,
            initial_rows: 24,
            initial_cell: (8, 16),
            size_ready: true,
            focused: false,
            theme: None,
            transport: Some(tr2.clone()),
            session_epoch: 0,
        });
        assert!(!Arc::ptr_eq(&s1, &s2));
        assert!(s1.stop.load(Ordering::SeqCst));

        // Bumping session_epoch (e.g. SSH tunnel reconnect with same socket path) -> replaces s2.
        let s3 = get_or_spawn_session(SessionInit {
            host_id: "devpc".into(),
            config: Config::default(),
            core: core.into(),
            terminal: tty.into(),
            initial_cols: 80,
            initial_rows: 24,
            initial_cell: (8, 16),
            size_ready: true,
            focused: false,
            theme: None,
            transport: Some(tr2),
            session_epoch: 1,
        });
        assert!(!Arc::ptr_eq(&s2, &s3));
        assert!(s2.stop.load(Ordering::SeqCst));

        // evict_idle_host_sessions skips sessions with an active listener and evicts idle ones.
        s3.wake.attach(777, Arc::new(|| {}));
        let idle_core = "core-idle-evict";
        let idle_tty = "tty-idle-evict";
        evict_session(idle_core, idle_tty);
        let s_idle = get_or_spawn_session(SessionInit {
            host_id: "devpc".into(),
            config: Config::default(),
            core: idle_core.into(),
            terminal: idle_tty.into(),
            initial_cols: 80,
            initial_rows: 24,
            initial_cell: (8, 16),
            size_ready: true,
            focused: false,
            theme: None,
            transport: Some(make_transport()),
            session_epoch: 0,
        });
        evict_idle_host_sessions("devpc");
        assert!(s_idle.stop.load(Ordering::SeqCst));
        assert!(!s3.stop.load(Ordering::SeqCst));
        s3.wake.detach(777);
        evict_session(core, tty);

        // Changing Config.relay_endpoint without custom transport also replaces stale session.
        let relay_core = "core-relay-swap";
        let relay_tty = "tty-relay-swap";
        evict_session(relay_core, relay_tty);
        let cfg1 = Config {
            hc: "/usr/bin/false".into(),
            relay_endpoint: Some("mux://old".into()),
            ..Default::default()
        };
        let cfg2 = Config {
            hc: "/usr/bin/false".into(),
            relay_endpoint: Some("mux://new".into()),
            ..Default::default()
        };
        let sr1 = get_or_spawn_session(SessionInit {
            host_id: "devpc".into(),
            config: cfg1,
            core: relay_core.into(),
            terminal: relay_tty.into(),
            initial_cols: 80,
            initial_rows: 24,
            initial_cell: (8, 16),
            size_ready: false,
            focused: false,
            theme: None,
            transport: None,
            session_epoch: 0,
        });
        let sr2 = get_or_spawn_session(SessionInit {
            host_id: "devpc".into(),
            config: cfg2,
            core: relay_core.into(),
            terminal: relay_tty.into(),
            initial_cols: 80,
            initial_rows: 24,
            initial_cell: (8, 16),
            size_ready: false,
            focused: false,
            theme: None,
            transport: None,
            session_epoch: 0,
        });
        assert!(!Arc::ptr_eq(&sr1, &sr2));
        assert!(sr1.stop.load(Ordering::SeqCst));
        evict_session(relay_core, relay_tty);
    }
}
