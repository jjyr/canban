//! Bridge between `turmoil::net` simulated TCP sockets and `canban-transport` / `canban-client`,
//! with deterministic byte-stream fault injection (`FaultyStream`) and TCP connection tracking.
use anyhow::{Context, Result, bail};
use canban_app_server::AppServer;
use canban_client::{ConnectFuture, ConnectedTransport, HostConnector};
use canban_transport::{ClientTransport, NoiseKeypair, NoiseRngFactory, TcpAuthMode};
use canban_types::HostTransport;
use std::{
    collections::BTreeMap,
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    task::{Context as TaskContext, Poll, Waker},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    time::Sleep,
};

/// Deterministic byte-stream fault profile applied by [`FaultyStream`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamFaultConfig {
    /// Maximum bytes returned per `poll_read` (e.g. `Some(1)` stresses 1-byte framing splits).
    pub max_read_chunk: Option<usize>,
    /// Maximum bytes written per `poll_write`.
    pub max_write_chunk: Option<usize>,
    /// Truncate reads with a clean `0`-byte EOF once this many bytes have been read.
    pub eof_after_read_bytes: Option<u64>,
    /// Return `io::ErrorKind::ConnectionReset` once this many bytes have been read.
    pub rst_after_read_bytes: Option<u64>,
    /// Return `io::ErrorKind::ConnectionReset` once this many bytes have been written.
    pub rst_after_write_bytes: Option<u64>,
    /// Half-open read blackhole: parks `poll_read` until cleared (legacy alias of `blackhole_reads`).
    pub stall_reads: bool,
    /// Half-open write blackhole: parks `poll_write` until cleared (legacy alias of `blackhole_writes`).
    pub stall_writes: bool,
    /// Explicit half-open read blackhole: stops inbound bytes without closing or returning EOF/RST.
    pub blackhole_reads: bool,
    /// Explicit half-open write blackhole: stops outbound bytes without closing or returning RST.
    pub blackhole_writes: bool,
    /// Bidirectional half-open blackhole: stops both reads and writes without closing the stream.
    pub half_open_blackhole: bool,
    /// Optional bounded virtual-time read blackhole duration in milliseconds (e.g. 35_000..=60_000ms
    /// to exceed keepalive/heartbeat timeouts without EOF/RST). Clearing/updating the policy wakes
    /// parked readers immediately.
    pub stall_read_ms: Option<u64>,
    /// Optional bounded virtual-time write blackhole duration in milliseconds (e.g. 35_000..=60_000ms).
    /// Clearing/updating the policy wakes parked writers immediately.
    pub stall_write_ms: Option<u64>,
    /// Extra deterministic virtual latency injected before each read operation.
    pub extra_read_latency_ms: u64,
    /// Extra deterministic virtual latency injected before each write operation.
    pub extra_write_latency_ms: u64,
    /// Flip all bits (`^= 0xFF`) of the read byte at this exact offset (per policy generation)
    /// to simulate wire tampering / AEAD authentication failure.
    pub corrupt_read_byte_at: Option<u64>,
    /// Flip all bits (`^= 0xFF`) of the written byte at this exact offset (per policy generation)
    /// to simulate wire tampering / AEAD authentication failure.
    pub corrupt_write_byte_at: Option<u64>,
}

impl StreamFaultConfig {
    /// Indefinite bidirectional half-open blackhole until cleared/healed on [`StreamFaultPolicy`].
    pub fn half_open_blackhole() -> Self {
        Self {
            blackhole_reads: true,
            blackhole_writes: true,
            half_open_blackhole: true,
            ..Self::default()
        }
    }

    /// Bounded bidirectional half-open blackhole lasting `duration_ms` virtual milliseconds
    /// (e.g. 35_000..=60_000ms to exceed keepalive/heartbeat deadlines without EOF/RST),
    /// waking immediately if cleared earlier.
    pub fn half_open_blackhole_for_ms(duration_ms: u64) -> Self {
        Self {
            blackhole_reads: true,
            blackhole_writes: true,
            half_open_blackhole: true,
            stall_read_ms: Some(duration_ms),
            stall_write_ms: Some(duration_ms),
            ..Self::default()
        }
    }

    /// Bounded read-only half-open blackhole lasting `duration_ms` virtual milliseconds.
    pub fn blackhole_reads_for_ms(duration_ms: u64) -> Self {
        Self {
            blackhole_reads: true,
            stall_read_ms: Some(duration_ms),
            ..Self::default()
        }
    }

    /// Bounded write-only half-open blackhole lasting `duration_ms` virtual milliseconds.
    pub fn blackhole_writes_for_ms(duration_ms: u64) -> Self {
        Self {
            blackhole_writes: true,
            stall_write_ms: Some(duration_ms),
            ..Self::default()
        }
    }

    /// Returns `true` when indefinite read blackholing is enabled.
    pub fn reads_blackholed(&self) -> bool {
        self.stall_reads || self.blackhole_reads || self.half_open_blackhole
    }

    /// Returns `true` when indefinite write blackholing is enabled.
    pub fn writes_blackholed(&self) -> bool {
        self.stall_writes || self.blackhole_writes || self.half_open_blackhole
    }

    /// Returns `true` when any half-open blackhole or timed stall is active.
    pub fn is_half_open_blackhole(&self) -> bool {
        self.reads_blackholed()
            || self.writes_blackholed()
            || self.stall_read_ms.is_some_and(|ms| ms > 0)
            || self.stall_write_ms.is_some_and(|ms| ms > 0)
    }
}

/// Shared, live-mutable controller for [`FaultyStream`] instances across one or more hosts.
#[derive(Clone, Debug, Default)]
pub struct StreamFaultPolicy {
    inner: Arc<PolicyInner>,
}

#[derive(Debug, Default)]
struct PolicyState {
    default_cfg: StreamFaultConfig,
    host_cfgs: BTreeMap<String, StreamFaultConfig>,
    wire_taps: BTreeMap<String, Arc<Mutex<Vec<u8>>>>,
    generation: u64,
    wakers: BTreeMap<u64, Waker>,
}

impl PolicyState {
    fn effective_config(&self, host_id: Option<&str>) -> StreamFaultConfig {
        if let Some(hid) = host_id
            && let Some(cfg) = self.host_cfgs.get(hid)
        {
            return cfg.clone();
        }
        self.default_cfg.clone()
    }
}

#[derive(Debug)]
struct PolicyInner {
    state: Mutex<PolicyState>,
    next_waker_id: AtomicU64,
}

impl Default for PolicyInner {
    fn default() -> Self {
        Self {
            state: Mutex::new(PolicyState::default()),
            next_waker_id: AtomicU64::new(1),
        }
    }
}

impl StreamFaultPolicy {
    pub fn new(default_cfg: StreamFaultConfig) -> Self {
        Self {
            inner: Arc::new(PolicyInner {
                state: Mutex::new(PolicyState {
                    default_cfg,
                    host_cfgs: BTreeMap::new(),
                    wire_taps: BTreeMap::new(),
                    generation: 0,
                    wakers: BTreeMap::new(),
                }),
                next_waker_id: AtomicU64::new(1),
            }),
        }
    }

    fn alloc_waker_id(&self) -> u64 {
        self.inner.next_waker_id.fetch_add(1, Ordering::Relaxed)
    }

    fn unregister_waker(&self, waker_id: u64) {
        if let Ok(mut st) = self.inner.state.lock() {
            st.wakers.remove(&waker_id);
        }
    }

    fn update_and_wake<F>(&self, mutate: F)
    where
        F: FnOnce(&mut PolicyState),
    {
        let wakers: Vec<Waker> = {
            let mut st = self.inner.state.lock().unwrap();
            mutate(&mut st);
            st.generation = st.generation.wrapping_add(1);
            std::mem::take(&mut st.wakers).into_values().collect()
        };
        for waker in wakers {
            waker.wake();
        }
    }

    /// Monotonic counter incremented whenever the fault policy is updated or cleared.
    pub fn generation(&self) -> u64 {
        self.inner.state.lock().unwrap().generation
    }

    pub fn set_default(&self, cfg: StreamFaultConfig) {
        self.update_and_wake(|st| {
            st.default_cfg = cfg;
        });
    }

    pub fn set_for_host(&self, host_id: impl Into<String>, cfg: StreamFaultConfig) {
        let host_id = host_id.into();
        self.update_and_wake(|st| {
            st.host_cfgs.insert(host_id, cfg);
        });
    }

    /// Arms a bidirectional half-open blackhole on `host_id` (indefinite if `duration_ms` is `None`,
    /// or bounded to `duration_ms` virtual milliseconds if `Some(ms)`).
    pub fn arm_half_open_blackhole(&self, host_id: impl Into<String>, duration_ms: Option<u64>) {
        let cfg = match duration_ms {
            Some(ms) => StreamFaultConfig::half_open_blackhole_for_ms(ms),
            None => StreamFaultConfig::half_open_blackhole(),
        };
        self.set_for_host(host_id, cfg);
    }

    pub fn clear_for_host(&self, host_id: &str) {
        self.update_and_wake(|st| {
            st.host_cfgs.remove(host_id);
        });
    }

    /// Clears all faults for `host_id` and wakes any parked read/write wakers.
    pub fn heal_host(&self, host_id: &str) {
        self.clear_for_host(host_id);
    }

    pub fn clear_all(&self) {
        self.update_and_wake(|st| {
            st.default_cfg = StreamFaultConfig::default();
            st.host_cfgs.clear();
        });
    }

    pub fn effective_config(&self, host_id: Option<&str>) -> StreamFaultConfig {
        self.inner.state.lock().unwrap().effective_config(host_id)
    }

    /// Attaches a raw byte recorder capturing every byte read/written on [`FaultyStream`]s for `host_id`.
    pub fn set_wire_tap(&self, host_id: impl Into<String>, sink: Option<Arc<Mutex<Vec<u8>>>>) {
        let host_id = host_id.into();
        let mut st = self.inner.state.lock().unwrap();
        if let Some(buf) = sink {
            st.wire_taps.insert(host_id, buf);
        } else {
            st.wire_taps.remove(&host_id);
        }
    }

    fn wire_tap_for(&self, host_id: Option<&str>) -> Option<Arc<Mutex<Vec<u8>>>> {
        let hid = host_id?;
        self.inner.state.lock().unwrap().wire_taps.get(hid).cloned()
    }

    /// Number of currently parked stream read/write wakers awaiting policy heal/change.
    pub fn parked_waker_count(&self) -> usize {
        self.inner.state.lock().unwrap().wakers.len()
    }
}

/// Deterministic fault-injecting wrapper around any `AsyncRead + AsyncWrite + Unpin` byte stream
/// (including `turmoil::net::TcpStream`).
pub struct FaultyStream<S> {
    inner: S,
    host_id: Option<String>,
    policy: StreamFaultPolicy,
    policy_gen: u64,
    bytes_read: u64,
    bytes_written: u64,
    read_waker_id: u64,
    write_waker_id: u64,
    read_sleep: Option<Pin<Box<Sleep>>>,
    write_sleep: Option<Pin<Box<Sleep>>>,
    read_stall_sleep: Option<Pin<Box<Sleep>>>,
    write_stall_sleep: Option<Pin<Box<Sleep>>>,
    read_stall_completed: bool,
    write_stall_completed: bool,
    read_blackhole_recorded: bool,
    write_blackhole_recorded: bool,
}

impl<S> Drop for FaultyStream<S> {
    fn drop(&mut self) {
        self.policy.unregister_waker(self.read_waker_id);
        self.policy.unregister_waker(self.write_waker_id);
    }
}

impl<S> FaultyStream<S> {
    pub fn new(inner: S, policy: StreamFaultPolicy, host_id: Option<String>) -> Self {
        let policy_gen = policy.generation();
        let read_waker_id = policy.alloc_waker_id();
        let write_waker_id = policy.alloc_waker_id();
        Self {
            inner,
            host_id,
            policy,
            policy_gen,
            bytes_read: 0,
            bytes_written: 0,
            read_waker_id,
            write_waker_id,
            read_sleep: None,
            write_sleep: None,
            read_stall_sleep: None,
            write_stall_sleep: None,
            read_stall_completed: false,
            write_stall_completed: false,
            read_blackhole_recorded: false,
            write_blackhole_recorded: false,
        }
    }

    pub fn with_config(inner: S, cfg: StreamFaultConfig) -> Self {
        Self::new(inner, StreamFaultPolicy::new(cfg), None)
    }

    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    pub fn inner(&self) -> &S {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Resets per-arm read/write byte budgets and stall timers whenever [`StreamFaultPolicy`]
    /// changes generation, so injecting e.g. `rst_after_write_bytes: Some(32)` mid-session fires
    /// after 32 new bytes and clearing a blackhole immediately cancels any pending stall timer.
    fn sync_policy_gen_with(&mut self, current_gen: u64) {
        if current_gen != self.policy_gen {
            self.policy_gen = current_gen;
            self.bytes_read = 0;
            self.bytes_written = 0;
            self.read_sleep = None;
            self.write_sleep = None;
            self.read_stall_sleep = None;
            self.write_stall_sleep = None;
            self.read_stall_completed = false;
            self.write_stall_completed = false;
            self.read_blackhole_recorded = false;
            self.write_blackhole_recorded = false;
        }
    }

    /// Registers `waker_id` iff the policy generation still matches `expected_gen`.
    /// Returns `false` if the policy already changed (caller should re-evaluate immediately).
    fn register_waker_if_gen(
        &self,
        waker_id: u64,
        expected_gen: u64,
        cx: &TaskContext<'_>,
    ) -> bool {
        let mut st = self.policy.inner.state.lock().unwrap();
        if st.generation != expected_gen {
            return false;
        }
        st.wakers.insert(waker_id, cx.waker().clone());
        true
    }

    /// Enforces half-open read blackhole (indefinite or timed `stall_read_ms`) without closing the
    /// stream or returning EOF/RST. Returns `Poll::Ready((cfg, gen))` once reads are unblocked.
    fn poll_read_blackhole(&mut self, cx: &mut TaskContext<'_>) -> Poll<(StreamFaultConfig, u64)> {
        loop {
            let (cfg, current_gen) = {
                let policy_inner = Arc::clone(&self.policy.inner);
                let mut st = policy_inner.state.lock().unwrap();
                let current_gen = st.generation;
                self.sync_policy_gen_with(current_gen);
                let cfg = st.effective_config(self.host_id.as_deref());
                let timed_ms = cfg.stall_read_ms.filter(|&ms| ms > 0);
                if timed_ms.is_some() && !self.read_stall_completed {
                    st.wakers.insert(self.read_waker_id, cx.waker().clone());
                    (cfg, current_gen)
                } else if timed_ms.is_none() && cfg.reads_blackholed() {
                    st.wakers.insert(self.read_waker_id, cx.waker().clone());
                    if !self.read_blackhole_recorded {
                        self.read_blackhole_recorded = true;
                        crate::trace::record_coverage_hit("net.fault.read_blackhole");
                    }
                    return Poll::Pending;
                } else {
                    st.wakers.remove(&self.read_waker_id);
                    self.read_stall_sleep = None;
                    return Poll::Ready((cfg, current_gen));
                }
            };

            let ms = cfg.stall_read_ms.unwrap_or(0);
            if !self.read_blackhole_recorded {
                self.read_blackhole_recorded = true;
                crate::trace::record_coverage_hit("net.fault.read_blackhole");
            }
            if self.read_stall_sleep.is_none() {
                self.read_stall_sleep =
                    Some(Box::pin(tokio::time::sleep(Duration::from_millis(ms))));
            }
            match self.read_stall_sleep.as_mut().unwrap().as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(()) => {
                    self.read_stall_sleep = None;
                    self.read_stall_completed = true;
                    self.policy.unregister_waker(self.read_waker_id);
                    if self.policy.generation() != current_gen {
                        continue;
                    }
                    return Poll::Ready((cfg, current_gen));
                }
            }
        }
    }

    /// Enforces half-open write blackhole (indefinite or timed `stall_write_ms`) without closing the
    /// stream or returning RST. Returns `Poll::Ready((cfg, gen))` once writes are unblocked.
    fn poll_write_blackhole(&mut self, cx: &mut TaskContext<'_>) -> Poll<(StreamFaultConfig, u64)> {
        loop {
            let (cfg, current_gen) = {
                let policy_inner = Arc::clone(&self.policy.inner);
                let mut st = policy_inner.state.lock().unwrap();
                let current_gen = st.generation;
                self.sync_policy_gen_with(current_gen);
                let cfg = st.effective_config(self.host_id.as_deref());
                let timed_ms = cfg.stall_write_ms.filter(|&ms| ms > 0);
                if timed_ms.is_some() && !self.write_stall_completed {
                    st.wakers.insert(self.write_waker_id, cx.waker().clone());
                    (cfg, current_gen)
                } else if timed_ms.is_none() && cfg.writes_blackholed() {
                    st.wakers.insert(self.write_waker_id, cx.waker().clone());
                    if !self.write_blackhole_recorded {
                        self.write_blackhole_recorded = true;
                        crate::trace::record_coverage_hit("net.fault.write_blackhole");
                    }
                    return Poll::Pending;
                } else {
                    st.wakers.remove(&self.write_waker_id);
                    self.write_stall_sleep = None;
                    return Poll::Ready((cfg, current_gen));
                }
            };

            let ms = cfg.stall_write_ms.unwrap_or(0);
            if !self.write_blackhole_recorded {
                self.write_blackhole_recorded = true;
                crate::trace::record_coverage_hit("net.fault.write_blackhole");
            }
            if self.write_stall_sleep.is_none() {
                self.write_stall_sleep =
                    Some(Box::pin(tokio::time::sleep(Duration::from_millis(ms))));
            }
            match self.write_stall_sleep.as_mut().unwrap().as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(()) => {
                    self.write_stall_sleep = None;
                    self.write_stall_completed = true;
                    self.policy.unregister_waker(self.write_waker_id);
                    if self.policy.generation() != current_gen {
                        continue;
                    }
                    return Poll::Ready((cfg, current_gen));
                }
            }
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for FaultyStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();

        // 1. Half-open read blackhole / timed stall + 2. Extra virtual read latency.
        let cfg = loop {
            let (cfg, current_gen) = match this.poll_read_blackhole(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(pair) => pair,
            };

            if cfg.extra_read_latency_ms > 0 {
                if this.read_sleep.is_none() {
                    this.read_sleep = Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                        cfg.extra_read_latency_ms,
                    ))));
                }
                match this.read_sleep.as_mut().unwrap().as_mut().poll(cx) {
                    Poll::Pending => {
                        if this.register_waker_if_gen(this.read_waker_id, current_gen, cx) {
                            return Poll::Pending;
                        }
                        continue;
                    }
                    Poll::Ready(()) => {
                        this.read_sleep = None;
                        this.policy.unregister_waker(this.read_waker_id);
                    }
                }
            }
            break cfg;
        };

        // 3. RST / EOF truncation limits.
        if let Some(limit) = cfg.rst_after_read_bytes
            && this.bytes_read >= limit
        {
            crate::trace::record_coverage_hit("net.fault.read_rst");
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "simulated TCP RST during read",
            )));
        }
        if let Some(limit) = cfg.eof_after_read_bytes
            && this.bytes_read >= limit
        {
            crate::trace::record_coverage_hit("net.fault.read_eof");
            return Poll::Ready(Ok(()));
        }

        // 4. Compute effective read cap (chunking + remaining bytes before EOF/RST).
        let mut cap = buf.remaining();
        if let Some(chunk) = cfg.max_read_chunk {
            cap = cap.min(chunk.max(1));
        }
        if let Some(limit) = cfg.eof_after_read_bytes {
            let rem = limit.saturating_sub(this.bytes_read) as usize;
            cap = cap.min(rem);
        }
        if let Some(limit) = cfg.rst_after_read_bytes {
            let rem = limit.saturating_sub(this.bytes_read) as usize;
            cap = cap.min(rem);
        }

        if cap == 0 || buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }

        let before_total = this.bytes_read;
        let read_res = if cap < buf.remaining() {
            let mut sub = ReadBuf::new(buf.initialize_unfilled_to(cap));
            match Pin::new(&mut this.inner).poll_read(cx, &mut sub) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(())) => {
                    let n = sub.filled().len();
                    if let Some(tap) = this.policy.wire_tap_for(this.host_id.as_deref()) {
                        tap.lock().unwrap().extend_from_slice(sub.filled());
                    }
                    if let Some(target) = cfg.corrupt_read_byte_at
                        && target >= before_total
                        && target < before_total.saturating_add(n as u64)
                    {
                        let idx = (target - before_total) as usize;
                        sub.filled_mut()[idx] ^= 0xFF;
                        crate::trace::record_coverage_hit("net.fault.corrupt_read");
                    }
                    buf.advance(n);
                    this.bytes_read = this.bytes_read.saturating_add(n as u64);
                    Ok(())
                }
                Poll::Ready(Err(e)) => Err(e),
            }
        } else {
            let before = buf.filled().len();
            match Pin::new(&mut this.inner).poll_read(cx, buf) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(())) => {
                    let n = buf.filled().len().saturating_sub(before);
                    if let Some(tap) = this.policy.wire_tap_for(this.host_id.as_deref()) {
                        tap.lock()
                            .unwrap()
                            .extend_from_slice(&buf.filled()[before..before + n]);
                    }
                    if let Some(target) = cfg.corrupt_read_byte_at
                        && target >= before_total
                        && target < before_total.saturating_add(n as u64)
                    {
                        let idx = before + (target - before_total) as usize;
                        buf.filled_mut()[idx] ^= 0xFF;
                        crate::trace::record_coverage_hit("net.fault.corrupt_read");
                    }
                    this.bytes_read = this.bytes_read.saturating_add(n as u64);
                    Ok(())
                }
                Poll::Ready(Err(e)) => Err(e),
            }
        };
        Poll::Ready(read_res)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for FaultyStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();

        // 1. Half-open write blackhole / timed stall + 2. Extra virtual write latency.
        let cfg = loop {
            let (cfg, current_gen) = match this.poll_write_blackhole(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(pair) => pair,
            };

            if cfg.extra_write_latency_ms > 0 {
                if this.write_sleep.is_none() {
                    this.write_sleep = Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                        cfg.extra_write_latency_ms,
                    ))));
                }
                match this.write_sleep.as_mut().unwrap().as_mut().poll(cx) {
                    Poll::Pending => {
                        if this.register_waker_if_gen(this.write_waker_id, current_gen, cx) {
                            return Poll::Pending;
                        }
                        continue;
                    }
                    Poll::Ready(()) => {
                        this.write_sleep = None;
                        this.policy.unregister_waker(this.write_waker_id);
                    }
                }
            }
            break cfg;
        };

        // 3. RST limit on write.
        if let Some(limit) = cfg.rst_after_write_bytes
            && this.bytes_written >= limit
        {
            crate::trace::record_coverage_hit("net.fault.write_rst");
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "simulated TCP RST during write",
            )));
        }

        let mut max_len = buf.len();
        if let Some(chunk) = cfg.max_write_chunk {
            max_len = max_len.min(chunk.max(1));
        }
        if let Some(limit) = cfg.rst_after_write_bytes {
            let rem = limit.saturating_sub(this.bytes_written) as usize;
            max_len = max_len.min(rem.max(1));
        }

        let before_total = this.bytes_written;
        let maybe_corrupt = cfg.corrupt_write_byte_at.filter(|&target| {
            target >= before_total && target < before_total.saturating_add(max_len as u64)
        });
        let write_res = if let Some(target) = maybe_corrupt {
            let mut tampered = buf[..max_len].to_vec();
            let idx = (target - before_total) as usize;
            tampered[idx] ^= 0xFF;
            let res = Pin::new(&mut this.inner).poll_write(cx, &tampered);
            if let Poll::Ready(Ok(n)) = &res {
                if let Some(tap) = this.policy.wire_tap_for(this.host_id.as_deref()) {
                    tap.lock().unwrap().extend_from_slice(&tampered[..*n]);
                }
                if *n > idx {
                    crate::trace::record_coverage_hit("net.fault.corrupt_write");
                }
            }
            res
        } else {
            let res = Pin::new(&mut this.inner).poll_write(cx, &buf[..max_len]);
            if let Poll::Ready(Ok(n)) = &res
                && let Some(tap) = this.policy.wire_tap_for(this.host_id.as_deref())
            {
                tap.lock().unwrap().extend_from_slice(&buf[..*n]);
            }
            res
        };
        match write_res {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(n)) => {
                this.bytes_written = this.bytes_written.saturating_add(n as u64);
                Poll::Ready(Ok(n))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.poll_write_blackhole(cx).is_pending() {
            return Poll::Pending;
        }
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// RAII guard mirroring production `ActiveTcpConnGuard` in `crates/transport/src/server.rs`
/// so simulated TCP connections increment/decrement `AppServer`'s live TCP connection counter.
pub struct ActiveTcpConnGuard {
    counter: Arc<AtomicUsize>,
}

impl ActiveTcpConnGuard {
    pub fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self { counter }
    }
}

impl Drop for ActiveTcpConnGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Connects a Noise-encrypted `yamux/1` + JSON-RPC [`ClientTransport`] over `turmoil::net::TcpStream`,
/// optionally wrapped in [`FaultyStream`] when `policy` is provided.
///
/// Both remote host connectors and local framed desktop transports (e.g. `SimLocalHostDriver`)
/// can reuse this helper to route all JSON-RPC calls over real Noise-encrypted `turmoil::net` streams.
#[allow(clippy::too_many_arguments)]
pub async fn connect_turmoil_client_transport(
    addr: &str,
    client_keypair: Arc<NoiseKeypair>,
    server_pubkey: Option<String>,
    auth: TcpAuthMode,
    device_name: impl Into<String>,
    noise_rng: Arc<dyn NoiseRngFactory>,
    policy: Option<StreamFaultPolicy>,
    host_id: Option<String>,
) -> Result<Arc<ClientTransport>> {
    let normalized = canban_types::normalize_daemon_addr(addr);
    let raw_stream = tokio::time::timeout(
        Duration::from_secs(10),
        turmoil::net::TcpStream::connect(normalized.as_str()),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("Timed out connecting to {normalized}"),
        )
    })?
    .with_context(|| format!("Canban is not reachable at {normalized}"))?;
    let device_name = device_name.into();
    let client = match policy {
        Some(pol) => {
            let wrapped = FaultyStream::new(raw_stream, pol, host_id);
            ClientTransport::connect_stream_async(
                wrapped,
                normalized,
                server_pubkey,
                auth,
                device_name,
                client_keypair.as_ref(),
                noise_rng.as_ref(),
            )
            .await?
        }
        None => {
            ClientTransport::connect_stream_async(
                raw_stream,
                normalized,
                server_pubkey,
                auth,
                device_name,
                client_keypair.as_ref(),
                noise_rng.as_ref(),
            )
            .await?
        }
    };
    Ok(Arc::new(client))
}

fn resolve_tcp_auth(tailscale_auth: bool, pairing_code: Option<&str>) -> TcpAuthMode {
    if let Some(code) = pairing_code.filter(|c| !c.trim().is_empty()) {
        TcpAuthMode::PairingCode(code.trim().to_owned())
    } else if tailscale_auth {
        TcpAuthMode::Tailscale
    } else {
        TcpAuthMode::Key
    }
}

/// [`HostConnector`] establishing real Noise + `yamux/1` + JSON-RPC sessions over `turmoil::net::TcpStream`.
#[derive(Clone)]
pub struct TurmoilConnector {
    client_keypair: Arc<NoiseKeypair>,
    noise_rng: Arc<dyn NoiseRngFactory>,
}

impl TurmoilConnector {
    pub fn new(client_keypair: Arc<NoiseKeypair>, noise_rng: Arc<dyn NoiseRngFactory>) -> Self {
        Self {
            client_keypair,
            noise_rng,
        }
    }
}

impl HostConnector for TurmoilConnector {
    fn supports(&self, transport: &HostTransport) -> bool {
        matches!(transport, HostTransport::Tcp { .. })
    }

    fn connect<'a>(
        &'a self,
        _host_id: &'a str,
        transport: &'a HostTransport,
        device_name: &'a str,
        pairing_code: Option<&'a str>,
    ) -> ConnectFuture<'a> {
        let transport_owned = transport.clone();
        let device = device_name.to_owned();
        let pairing_code_owned = pairing_code.map(str::to_owned);
        let client_keypair = Arc::clone(&self.client_keypair);
        let noise_rng = Arc::clone(&self.noise_rng);
        Box::pin(async move {
            let HostTransport::Tcp {
                addr,
                server_pubkey,
                tailscale_auth,
            } = transport_owned
            else {
                bail!("TurmoilConnector only supports TCP hosts");
            };
            let auth = resolve_tcp_auth(tailscale_auth, pairing_code_owned.as_deref());
            let client = connect_turmoil_client_transport(
                &addr,
                client_keypair,
                server_pubkey,
                auth,
                device,
                noise_rng,
                None,
                None,
            )
            .await?;
            Ok(ConnectedTransport::new(client))
        })
    }
}

/// [`HostConnector`] establishing real Noise + `yamux/1` + JSON-RPC sessions wrapped in [`FaultyStream`].
#[derive(Clone)]
pub struct FaultyTurmoilConnector {
    policy: StreamFaultPolicy,
    client_keypair: Arc<NoiseKeypair>,
    noise_rng: Arc<dyn NoiseRngFactory>,
}

impl FaultyTurmoilConnector {
    pub fn new(
        policy: StreamFaultPolicy,
        client_keypair: Arc<NoiseKeypair>,
        noise_rng: Arc<dyn NoiseRngFactory>,
    ) -> Self {
        Self {
            policy,
            client_keypair,
            noise_rng,
        }
    }

    pub fn policy(&self) -> &StreamFaultPolicy {
        &self.policy
    }
}

impl HostConnector for FaultyTurmoilConnector {
    fn supports(&self, transport: &HostTransport) -> bool {
        matches!(transport, HostTransport::Tcp { .. })
    }

    fn connect<'a>(
        &'a self,
        host_id: &'a str,
        transport: &'a HostTransport,
        device_name: &'a str,
        pairing_code: Option<&'a str>,
    ) -> ConnectFuture<'a> {
        let transport_owned = transport.clone();
        let device = device_name.to_owned();
        let pairing_code_owned = pairing_code.map(str::to_owned);
        let host = host_id.to_owned();
        let policy = self.policy.clone();
        let client_keypair = Arc::clone(&self.client_keypair);
        let noise_rng = Arc::clone(&self.noise_rng);
        Box::pin(async move {
            let HostTransport::Tcp {
                addr,
                server_pubkey,
                tailscale_auth,
            } = transport_owned
            else {
                bail!("FaultyTurmoilConnector only supports TCP hosts");
            };
            let auth = resolve_tcp_auth(tailscale_auth, pairing_code_owned.as_deref());
            let client = connect_turmoil_client_transport(
                &addr,
                client_keypair,
                server_pubkey,
                auth,
                device,
                noise_rng,
                Some(policy),
                Some(host),
            )
            .await?;
            Ok(ConnectedTransport::new(client))
        })
    }
}

/// Runs the simulated TCP accept loop for an [`AppServer`] inside a `turmoil` host,
/// tracking active TCP connections via [`ActiveTcpConnGuard`] and surviving transient accept errors.
pub async fn serve_turmoil_listener(
    server: Arc<AppServer>,
    listener: turmoil::net::TcpListener,
) -> Result<()> {
    serve_turmoil_listener_with_faults(server, listener, None, None).await
}

/// Runs the simulated TCP accept loop with optional server-side [`FaultyStream`] injection.
pub async fn serve_turmoil_listener_with_faults(
    server: Arc<AppServer>,
    listener: turmoil::net::TcpListener,
    policy: Option<StreamFaultPolicy>,
    host_id: Option<String>,
) -> Result<()> {
    let mut stop_rx = server.stop_receiver();
    let active_counter = server.transport_config().active_tcp_connections.clone();
    loop {
        if *stop_rx.borrow() || server.stopped() {
            break;
        }
        tokio::select! {
            biased;
            _ = stop_rx.changed() => break,
            res = listener.accept() => {
                match res {
                    Ok((stream, peer_addr)) => {
                        let srv = Arc::clone(&server);
                        let guard = ActiveTcpConnGuard::new(Arc::clone(&active_counter));
                        let policy = policy.clone();
                        let host_id = host_id.clone();
                        tokio::spawn(async move {
                            let _guard = guard;
                            if let Some(pol) = policy {
                                let wrapped = FaultyStream::new(stream, pol, host_id);
                                let _ = srv.accept_authenticated_stream(wrapped, Some(peer_addr)).await;
                            } else {
                                let _ = srv.accept_authenticated_stream(stream, Some(peer_addr)).await;
                            }
                        });
                    }
                    Err(_) => {
                        if *stop_rx.borrow() || server.stopped() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn faulty_stream_chunking_eof_and_rst() {
        // 1. 1-byte read chunking + EOF truncation after 5 bytes
        let (mut writer, reader) = tokio::io::duplex(64);
        let mut faulty = FaultyStream::with_config(
            reader,
            StreamFaultConfig {
                max_read_chunk: Some(1),
                eof_after_read_bytes: Some(5),
                ..StreamFaultConfig::default()
            },
        );
        writer.write_all(b"hello world").await.unwrap();
        let mut buf = [0u8; 16];
        let n1 = faulty.read(&mut buf).await.unwrap();
        assert_eq!(n1, 1);
        assert_eq!(&buf[..1], b"h");

        let mut rest = Vec::new();
        faulty.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, b"ello");
        assert_eq!(faulty.bytes_read(), 5);

        // 2. Mid-stream RST on read
        let (mut w2, r2) = tokio::io::duplex(64);
        let mut faulty_rst = FaultyStream::with_config(
            r2,
            StreamFaultConfig {
                rst_after_read_bytes: Some(3),
                ..StreamFaultConfig::default()
            },
        );
        w2.write_all(b"abcdef").await.unwrap();
        let mut b2 = [0u8; 8];
        let n = faulty_rst.read(&mut b2).await.unwrap();
        assert_eq!(&b2[..n], b"abc");
        let err = faulty_rst.read(&mut b2).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);

        // 3. Mid-stream RST on write
        let (w3, _r3) = tokio::io::duplex(64);
        let mut faulty_w = FaultyStream::with_config(
            w3,
            StreamFaultConfig {
                rst_after_write_bytes: Some(2),
                ..StreamFaultConfig::default()
            },
        );
        let wn = faulty_w.write(b"abcd").await.unwrap();
        assert_eq!(wn, 2);
        let werr = faulty_w.write(b"cd").await.unwrap_err();
        assert_eq!(werr.kind(), io::ErrorKind::ConnectionReset);
    }

    #[tokio::test]
    async fn faulty_stream_half_open_stall_unstalls_and_guard_tracks_connections() {
        let policy = StreamFaultPolicy::new(StreamFaultConfig {
            stall_writes: true,
            ..StreamFaultConfig::default()
        });
        let (w, mut r) = tokio::io::duplex(64);
        let mut faulty = FaultyStream::new(w, policy.clone(), Some("remote-a".into()));

        let write_task = tokio::spawn(async move {
            faulty.write_all(b"ping").await.unwrap();
            faulty.bytes_written()
        });

        // Still stalled until policy clears stall_writes
        tokio::task::yield_now().await;
        assert!(!write_task.is_finished());

        policy.clear_all();
        let written = write_task.await.unwrap();
        assert_eq!(written, 4);

        let mut got = [0u8; 4];
        r.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"ping");

        // ActiveTcpConnGuard increments on construction and decrements on drop.
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let _g1 = ActiveTcpConnGuard::new(Arc::clone(&counter));
            let _g2 = ActiveTcpConnGuard::new(Arc::clone(&counter));
            assert_eq!(counter.load(Ordering::SeqCst), 2);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn faulty_stream_policy_update_resets_byte_budgets() {
        let policy = StreamFaultPolicy::new(StreamFaultConfig::default());
        let (w, mut r) = tokio::io::duplex(256);
        let mut faulty = FaultyStream::new(w, policy.clone(), Some("remote-a".into()));

        // Warm the stream with 64 bytes under a clean policy (simulating prior session traffic).
        let warmup = vec![b'A'; 64];
        faulty.write_all(&warmup).await.unwrap();
        assert_eq!(faulty.bytes_written(), 64);
        let mut drain = vec![0u8; 64];
        r.read_exact(&mut drain).await.unwrap();

        // Arm a 16-byte write RST mid-session: the generation bump must reset the write budget so
        // 16 new bytes are written mid-flight before ConnectionReset triggers (NOT byte 0!).
        policy.set_for_host(
            "remote-a",
            StreamFaultConfig {
                rst_after_write_bytes: Some(16),
                ..StreamFaultConfig::default()
            },
        );
        let payload = vec![b'B'; 32];
        let wn = faulty.write(&payload).await.unwrap();
        assert_eq!(
            wn, 16,
            "mid-session policy update must allow 16 fresh bytes before RST"
        );
        assert_eq!(faulty.bytes_written(), 16);
        let werr = faulty.write(&payload[wn..]).await.unwrap_err();
        assert_eq!(werr.kind(), io::ErrorKind::ConnectionReset);
    }

    #[test]
    fn half_open_blackhole_blocks_past_keepalive_without_eof_and_heals_cleanly() {
        let mut sim = turmoil::Builder::new()
            .simulation_duration(Duration::from_secs(300))
            .tick_duration(Duration::from_millis(50))
            .build();

        let policy = StreamFaultPolicy::default();
        let read_policy = policy.clone();
        let write_policy = policy.clone();
        let ctrl_policy = policy.clone();

        sim.client("blackhole-test", async move {
            // 1. Timed bidirectional half-open blackhole (45s > 30s keepalive) blocks reads and
            //    writes past the keepalive window WITHOUT returning EOF/RST, then resumes once
            //    the virtual stall expires.
            ctrl_policy.arm_half_open_blackhole("remote-a", Some(45_000));

            let (mut peer_w, client_r) = tokio::io::duplex(64);
            let (client_w, mut peer_r) = tokio::io::duplex(64);
            peer_w.write_all(b"alive").await.unwrap();

            let mut faulty_r = FaultyStream::new(client_r, read_policy.clone(), Some("remote-a".into()));
            let mut faulty_w = FaultyStream::new(client_w, write_policy.clone(), Some("remote-a".into()));

            let mut rbuf = [0u8; 5];
            let read_keepalive =
                tokio::time::timeout(Duration::from_millis(35_000), faulty_r.read(&mut rbuf)).await;
            assert!(
                read_keepalive.is_err(),
                "timed half-open read blackhole must stay pending past 35s keepalive without EOF/RST"
            );

            let write_keepalive =
                tokio::time::timeout(Duration::from_millis(35_000), faulty_w.write(b"ping")).await;
            assert!(
                write_keepalive.is_err(),
                "timed half-open write blackhole must stay pending past 35s keepalive without RST"
            );

            // Advance past remaining 10s of the 45s read blackhole: bytes flow cleanly without EOF.
            let n = tokio::time::timeout(Duration::from_millis(15_000), faulty_r.read(&mut rbuf))
                .await
                .expect("timed read blackhole should unblock after 45s virtual time")
                .unwrap();
            assert_eq!(&rbuf[..n], b"alive");

            let wn = tokio::time::timeout(Duration::from_millis(15_000), faulty_w.write(b"ping"))
                .await
                .expect("timed write blackhole should unblock after 45s virtual time")
                .unwrap();
            assert_eq!(wn, 4);
            let mut got = [0u8; 4];
            peer_r.read_exact(&mut got).await.unwrap();
            assert_eq!(&got, b"ping");

            // 2. Indefinite half-open blackhole holds parked read & write tasks past 60s virtual
            //    time and wakes BOTH immediately when healed via `heal_host`.
            ctrl_policy.arm_half_open_blackhole("remote-a", None);
            peer_w.write_all(b"healed").await.unwrap();

            let read_task = tokio::spawn(async move {
                let mut out = [0u8; 6];
                faulty_r.read_exact(&mut out).await.unwrap();
                out
            });
            let write_task = tokio::spawn(async move {
                faulty_w.write_all(b"pong").await.unwrap();
                faulty_w.flush().await.unwrap();
            });

            tokio::time::sleep(Duration::from_secs(60)).await;
            assert!(
                !read_task.is_finished(),
                "indefinite blackholed reader must stay parked across 60s keepalive window"
            );
            assert!(
                !write_task.is_finished(),
                "indefinite blackholed writer must stay parked across 60s keepalive window"
            );
            assert!(
                ctrl_policy.parked_waker_count() >= 2,
                "expected both read and write wakers registered in policy"
            );

            ctrl_policy.heal_host("remote-a");

            let read_out = tokio::time::timeout(Duration::from_millis(500), read_task)
                .await
                .expect("healing policy must wake parked reader immediately")
                .unwrap();
            assert_eq!(&read_out, b"healed");

            tokio::time::timeout(Duration::from_millis(500), write_task)
                .await
                .expect("healing policy must wake parked writer immediately")
                .unwrap();
            let mut got2 = [0u8; 4];
            peer_r.read_exact(&mut got2).await.unwrap();
            assert_eq!(&got2, b"pong");
            assert_eq!(ctrl_policy.parked_waker_count(), 0);
            Ok(())
        });

        sim.run().expect("turmoil blackhole test failed");
    }

    #[test]
    fn clearing_long_timed_blackhole_wakes_parked_streams_immediately_and_drop_unregisters() {
        let mut sim = turmoil::Builder::new()
            .simulation_duration(Duration::from_secs(120))
            .tick_duration(Duration::from_millis(50))
            .build();

        let policy = StreamFaultPolicy::default();
        let stream_policy = policy.clone();

        sim.client("early-heal-test", async move {
            // Arm a 60s timed blackhole, park both reader and writer, then heal at t=5s.
            policy.set_for_host(
                "remote-b",
                StreamFaultConfig {
                    stall_read_ms: Some(60_000),
                    stall_write_ms: Some(60_000),
                    ..StreamFaultConfig::default()
                },
            );
            let (mut peer_w, client_r) = tokio::io::duplex(64);
            let (client_w, mut peer_r) = tokio::io::duplex(64);
            peer_w.write_all(b"fast").await.unwrap();

            let mut faulty_r =
                FaultyStream::new(client_r, stream_policy.clone(), Some("remote-b".into()));
            let mut faulty_w =
                FaultyStream::new(client_w, stream_policy.clone(), Some("remote-b".into()));

            let r_task = tokio::spawn(async move {
                let mut b = [0u8; 4];
                faulty_r.read_exact(&mut b).await.unwrap();
                b
            });
            let w_task = tokio::spawn(async move {
                faulty_w.write_all(b"wake").await.unwrap();
            });

            tokio::time::sleep(Duration::from_secs(5)).await;
            assert!(!r_task.is_finished());
            assert!(!w_task.is_finished());

            let before_heal = tokio::time::Instant::now();
            policy.clear_for_host("remote-b");

            let r_bytes = tokio::time::timeout(Duration::from_millis(500), r_task)
                .await
                .expect("clearing 60s timed blackhole must wake reader immediately")
                .unwrap();
            tokio::time::timeout(Duration::from_millis(500), w_task)
                .await
                .expect("clearing 60s timed blackhole must wake writer immediately")
                .unwrap();
            assert!(
                before_heal.elapsed() < Duration::from_secs(1),
                "woke after {:?} instead of immediately upon clear",
                before_heal.elapsed()
            );
            assert_eq!(&r_bytes, b"fast");
            let mut wb = [0u8; 4];
            peer_r.read_exact(&mut wb).await.unwrap();
            assert_eq!(&wb, b"wake");

            // Dropping a parked stream cleanly removes its waker from the policy.
            policy.arm_half_open_blackhole("remote-b", None);
            let (_pw2, cr2) = tokio::io::duplex(64);
            let mut parked = FaultyStream::new(cr2, stream_policy.clone(), Some("remote-b".into()));
            let mut tmp = [0u8; 4];
            assert!(
                tokio::time::timeout(Duration::from_millis(200), parked.read(&mut tmp))
                    .await
                    .is_err()
            );
            assert_eq!(policy.parked_waker_count(), 1);
            drop(parked);
            assert_eq!(policy.parked_waker_count(), 0);
            Ok(())
        });

        sim.run().expect("turmoil early-heal test failed");
    }
}
