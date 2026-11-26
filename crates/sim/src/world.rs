//! Core `turmoil`-backed Deterministic Simulation World (`SimWorld`).
//!
//! Runs 1..N real [`AppServer`] nodes (each with real SQLite, Workflow Engine, Scheduler,
//! Notifications, GitHub sync, and Herdr observation in `inline_workers` mode) and 1..M real
//! [`canban_client::MultiHostClient`] nodes (`"ui-client"` plus optional extra clients) inside
//! a single-threaded `turmoil::Sim` stepped deterministically via `sim.step()`.
use crate::{
    clock::TurmoilClock,
    exec::SimExec,
    github::{SimGitHub, SimGitHubHost},
    herdr::SimHerdr,
    net::{FaultyTurmoilConnector, StreamFaultPolicy, serve_turmoil_listener_with_faults},
    trace::{CoverageTracker, EventTrace},
};
use anyhow::{Context, Result, anyhow, bail};
use canban_app_server::{AppServer, ServerRuntimeEnv, hosts::DesktopGlobalEnvProvider};
pub use canban_app_server::{FsDurability, ScriptExecutionMode};
use canban_client::{
    ClientCapabilities, ClientEvent, ConnectFuture, HostConnector, InlineHostsFileStore,
    LocalHostDriver, MultiHostClient, MultiHostClientOptions, RemoteClientConfig,
};
pub use canban_storage::{StorageFaultInjector, StorageFaultKind};
use canban_transport::{
    ClientTransport, MAX_FRAME_BYTES, TailscalePeerIdentity, TailscalePeerVerifier,
    read_bounded_line, write_json_line_async,
};
use canban_types::{
    Config, FailpointEvent, Failpoints, GlobalEnvSyncStatus, HostSummary, HostTransport,
    RuntimeState, SavedHost, SchedulerSettings, Snapshot, SyncAction, SyncItemState, SyncItemView,
};
use rand::{SeedableRng, rngs::StdRng};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    future::Future,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

pub const SIM_PORT: u16 = 7680;
/// Simulated local framed JSON-RPC port serving [`AppServer::serve_local_stream`] over `turmoil::net`
/// when [`SimWorldConfig::framed_local_transport`] is enabled.
pub const SIM_LOCAL_FRAMED_PORT: u16 = SIM_PORT + 1;
pub const PRIMARY_CLIENT: &str = "ui-client";
const MAX_BUFFERED_CLIENT_EVENTS: usize = 2_048;

fn make_observed_failpoints() -> Arc<Failpoints> {
    let fp = Failpoints::shared();
    fp.set_observer(Some(Arc::new(|name, ev| match ev {
        FailpointEvent::Armed => crate::trace::record_fault_armed(name),
        FailpointEvent::Fired => crate::trace::record_fault_fired(name),
        FailpointEvent::Reached => {}
    })));
    fp
}

struct CountingHostConnector<C> {
    inner: C,
    dial_counts: Arc<Mutex<BTreeMap<String, u64>>>,
}

impl<C: HostConnector> HostConnector for CountingHostConnector<C> {
    fn supports(&self, transport: &HostTransport) -> bool {
        self.inner.supports(transport)
    }

    fn connect<'a>(
        &'a self,
        host_id: &'a str,
        transport: &'a HostTransport,
        device_name: &'a str,
    ) -> ConnectFuture<'a> {
        {
            let mut counts = self.dial_counts.lock().unwrap();
            *counts.entry(host_id.to_owned()).or_insert(0) += 1;
        }
        self.inner.connect(host_id, transport, device_name)
    }
}

/// Controllable deterministic [`TailscalePeerVerifier`] for DST (`transport_auth`).
#[derive(Clone, Default)]
pub struct SimTailscaleVerifier {
    inner: Arc<Mutex<SimTailscaleVerifierState>>,
}

#[derive(Default)]
struct SimTailscaleVerifierState {
    enabled: bool,
    default_identity: Option<TailscalePeerIdentity>,
    peer_overrides: BTreeMap<IpAddr, Option<TailscalePeerIdentity>>,
    attempts: Vec<(SocketAddr, bool)>,
}

impl std::fmt::Debug for SimTailscaleVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let st = self.inner.lock().unwrap();
        f.debug_struct("SimTailscaleVerifier")
            .field("enabled", &st.enabled)
            .field("default_identity", &st.default_identity)
            .field("overrides", &st.peer_overrides.len())
            .field("attempts", &st.attempts.len())
            .finish()
    }
}

impl SimTailscaleVerifier {
    pub fn accepting(
        node_id: impl Into<String>,
        device_name: impl Into<String>,
        login_name: impl Into<String>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SimTailscaleVerifierState {
                enabled: true,
                default_identity: Some(TailscalePeerIdentity {
                    node_id: node_id.into(),
                    device_name: device_name.into(),
                    login_name: login_name.into(),
                }),
                peer_overrides: BTreeMap::new(),
                attempts: Vec::new(),
            })),
        }
    }

    pub fn rejecting() -> Self {
        Self::default()
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.inner.lock().unwrap().enabled = enabled;
    }

    pub fn set_default_identity(&self, identity: Option<TailscalePeerIdentity>) {
        self.inner.lock().unwrap().default_identity = identity;
    }

    pub fn set_peer_identity(&self, ip: IpAddr, identity: Option<TailscalePeerIdentity>) {
        self.inner
            .lock()
            .unwrap()
            .peer_overrides
            .insert(ip, identity);
    }

    pub fn clear_peer_overrides(&self) {
        self.inner.lock().unwrap().peer_overrides.clear();
    }

    pub fn attempts(&self) -> Vec<(SocketAddr, bool)> {
        self.inner.lock().unwrap().attempts.clone()
    }
}

impl TailscalePeerVerifier for SimTailscaleVerifier {
    fn verify_peer(&self, peer_addr: SocketAddr) -> Option<TailscalePeerIdentity> {
        let mut st = self.inner.lock().unwrap();
        let res = if !st.enabled {
            None
        } else if let Some(ov) = st.peer_overrides.get(&peer_addr.ip()) {
            ov.clone()
        } else {
            st.default_identity.clone()
        };
        st.attempts.push((peer_addr, res.is_some()));
        res
    }
}

/// Derives a deterministic, platform-independent 64-bit seed for a simulated host and boot
/// generation (FNV-1a + SplitMix64 mixing; no randomized hasher).
///
/// Boot generation `0` is initial startup; each [`SimWorld::bounce_server`] increments the host's
/// boot generation so post-bounce `DeterministicIdGen` sequences never collide with pre-bounce IDs
/// while remaining 100% reproducible for the same world seed.
pub(crate) fn derive_host_seed(world_seed: u64, host_name: &str, boot_generation: u64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ world_seed;
    for &b in host_name.as_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= 0xff ^ boot_generation.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h = h.wrapping_add(0x9e37_79b9_7f4a_7c15);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

/// Configuration for constructing a [`SimWorld`].
#[derive(Clone, Debug)]
pub struct SimWorldConfig {
    pub seed: u64,
    pub scenario: String,
    /// Whether `"ui-client"` includes a Desktop `"local"` primary host + `DesktopGlobalEnvProvider`.
    pub desktop_local: bool,
    /// Names of remote `AppServer` hosts (e.g. `["remote-a", "remote-b"]`).
    pub remote_hosts: Vec<String>,
    /// Optional per-host wall-clock skew in milliseconds (useful for Global Env LWW tests).
    pub host_clock_skew_ms: BTreeMap<String, i64>,
    /// Network latency bounds in virtual milliseconds.
    pub min_latency_ms: u64,
    pub max_latency_ms: u64,
    /// Initial Turmoil link packet-failure rate (`0.0..=1.0`).
    pub fail_rate: f64,
    /// Simulation tick duration in virtual milliseconds.
    pub tick_ms: u64,
    /// Filesystem fsync durability mode for servers and clients (defaults to `NoSync` in DST).
    pub fs_durability: FsDurability,
    /// Whether server SQLite databases run with `PRAGMA synchronous = OFF` (defaults to `true`).
    pub sqlite_synchronous_off: bool,
    /// Whether background OS file watchers are disabled inside DST (defaults to `true`).
    pub disable_os_watchers: bool,
    /// Whether Turmoil randomizes host step order deterministically from seed (defaults to `true`).
    pub enable_random_order: bool,
    /// Optional shared [`StreamFaultPolicy`] injected into remote TCP streams (defaults to a fresh clean policy).
    pub stream_fault_policy: Option<StreamFaultPolicy>,
    /// Optional secondary simulated client host names (e.g. `["ui-client-2"]`) for multi-client races.
    pub extra_clients: Vec<String>,
    /// Optional per-host [`SimTailscaleVerifier`] injected into `ServerRuntimeEnv::tailscale_verifier`.
    pub tailscale_verifiers: BTreeMap<String, SimTailscaleVerifier>,
    /// Optional per-host API version override injected into `ServerRuntimeEnv::desktop_api_override`.
    pub desktop_api_overrides: BTreeMap<String, u32>,
    /// Default script execution mode injected into `ServerRuntimeEnv::script_execution_mode`.
    pub script_execution_mode: ScriptExecutionMode,
    /// Optional per-host override for `ServerRuntimeEnv::script_execution_mode`.
    pub host_script_execution_modes: BTreeMap<String, ScriptExecutionMode>,
    /// When `true`, binds a local framed `turmoil::net::TcpListener` serving
    /// `AppServer::serve_local_stream` and routes `SimLocalHostDriver` RPCs over framed JSON-RPC
    /// instead of direct in-process `Arc<AppServer>` calls (default `false` to preserve legacy digests).
    pub framed_local_transport: bool,
}

impl SimWorldConfig {
    pub fn desktop_default(seed: u64, scenario: impl Into<String>) -> Self {
        Self {
            seed,
            scenario: scenario.into(),
            desktop_local: true,
            remote_hosts: vec!["remote-a".into(), "remote-b".into()],
            host_clock_skew_ms: BTreeMap::new(),
            min_latency_ms: 2,
            max_latency_ms: 15,
            fail_rate: 0.0,
            tick_ms: 20,
            fs_durability: FsDurability::NoSync,
            sqlite_synchronous_off: true,
            disable_os_watchers: true,
            enable_random_order: true,
            stream_fault_policy: None,
            extra_clients: Vec::new(),
            tailscale_verifiers: BTreeMap::new(),
            desktop_api_overrides: BTreeMap::new(),
            script_execution_mode: ScriptExecutionMode::Auto,
            host_script_execution_modes: BTreeMap::new(),
            framed_local_transport: false,
        }
    }

    pub fn mobile_default(seed: u64, scenario: impl Into<String>) -> Self {
        Self {
            seed,
            scenario: scenario.into(),
            desktop_local: false,
            remote_hosts: vec!["remote-a".into(), "remote-b".into()],
            host_clock_skew_ms: BTreeMap::new(),
            min_latency_ms: 2,
            max_latency_ms: 15,
            fail_rate: 0.0,
            tick_ms: 20,
            fs_durability: FsDurability::NoSync,
            sqlite_synchronous_off: true,
            disable_os_watchers: true,
            enable_random_order: true,
            stream_fault_policy: None,
            extra_clients: Vec::new(),
            tailscale_verifiers: BTreeMap::new(),
            desktop_api_overrides: BTreeMap::new(),
            script_execution_mode: ScriptExecutionMode::Auto,
            host_script_execution_modes: BTreeMap::new(),
            framed_local_transport: false,
        }
    }
}

async fn connect_local_transport(local_tok: String) -> Result<Arc<ClientTransport>> {
    let addr = format!("local-server:{SIM_PORT}");
    let stream = turmoil::net::TcpStream::connect(addr.as_str())
        .await
        .map_err(|e| anyhow!("connect local-server: {e}"))?;
    let transport =
        ClientTransport::connect_stream_async(stream, addr, local_tok, "Sim Desktop".into())
            .await
            .map_err(|e| anyhow!("local mux: {e:#}"))?;
    Ok(Arc::new(transport))
}

fn decode_framed_jsonrpc_reply(line: &str) -> Result<Value> {
    let reply: Value = serde_json::from_str(line).context("decode framed local JSON-RPC reply")?;
    if let Some(err) = reply.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("local framed RPC error");
        bail!("{msg}");
    }
    Ok(reply.get("result").cloned().unwrap_or(Value::Null))
}

struct FramedLocalConn {
    reader: tokio::io::BufReader<tokio::io::ReadHalf<turmoil::net::TcpStream>>,
    writer: tokio::io::WriteHalf<turmoil::net::TcpStream>,
    next_id: u64,
}

impl FramedLocalConn {
    async fn connect() -> Result<Self> {
        let addr = format!("local-server:{SIM_LOCAL_FRAMED_PORT}");
        let stream = turmoil::net::TcpStream::connect(addr.as_str())
            .await
            .map_err(|e| anyhow!("connect framed local-server ({addr}): {e}"))?;
        let (read_half, writer) = tokio::io::split(stream);
        Ok(Self {
            reader: tokio::io::BufReader::new(read_half),
            writer,
            next_id: 0,
        })
    }

    async fn rpc(&mut self, method: &str, params: &Value) -> Result<Value> {
        self.next_id = self.next_id.wrapping_add(1);
        let req = json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params,
        });
        write_json_line_async(&mut self.writer, &req).await?;
        let line_opt = read_bounded_line(&mut self.reader, MAX_FRAME_BYTES).await?;
        let Some(line) = line_opt else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("Framed local AppServer closed stream during {method}"),
            )
            .into());
        };
        decode_framed_jsonrpc_reply(&line)
    }
}

#[derive(Clone, Default)]
struct FramedLocalCache {
    server_id: Option<String>,
    revision: Value,
    snapshot: Snapshot,
    runtime_state: RuntimeState,
    config: Config,
}

#[derive(Default)]
struct FramedLocalState {
    pool: Mutex<Vec<(u64, FramedLocalConn)>>,
    cache: Mutex<FramedLocalCache>,
}

struct SimLocalHostDriver {
    home: PathBuf,
    server_slot: Arc<Mutex<Option<Arc<AppServer>>>>,
    server_boot_gen: Arc<AtomicU64>,
    token_slot: Arc<Mutex<Option<String>>>,
    transport_state: Arc<Mutex<(u64, Arc<ClientTransport>)>>,
    reconnecting: Arc<AtomicBool>,
    framed: Option<Arc<FramedLocalState>>,
}

impl SimLocalHostDriver {
    fn is_framed(&self) -> bool {
        self.framed.is_some()
    }

    fn framed_cached_revision(&self) -> Option<Value> {
        self.framed
            .as_ref()
            .map(|f| f.cache.lock().unwrap().revision.clone())
    }

    async fn refresh_transport(&self) -> Result<()> {
        let target_gen = self.server_boot_gen.load(Ordering::SeqCst);
        let tok = self
            .token_slot
            .lock()
            .unwrap()
            .clone()
            .context("Simulated local-server token unavailable")?;
        let fresh = connect_local_transport(tok).await?;
        *self.transport_state.lock().unwrap() = (target_gen, fresh);
        Ok(())
    }

    async fn call_framed_async(&self, method: &str, mut params: Value) -> Result<Value> {
        let framed = self
            .framed
            .as_ref()
            .context("framed_local_transport is disabled on SimLocalHostDriver")?;
        if let Some(obj) = params.as_object_mut() {
            obj.remove("host_id");
        }
        let target_gen = self.server_boot_gen.load(Ordering::SeqCst);
        let popped = {
            let mut pool = framed.pool.lock().unwrap();
            pool.retain(|(g, _)| *g == target_gen);
            pool.pop()
        };
        let (mut conn, reused) = match popped {
            Some((_, conn)) => (conn, true),
            None => (FramedLocalConn::connect().await?, false),
        };
        crate::trace::record_coverage_hit("transport.local_framed_rpc");
        let reply = match conn.rpc(method, &params).await {
            Ok(v) => v,
            Err(err)
                if reused
                    && err.downcast_ref::<std::io::Error>().is_some()
                    && self.server_boot_gen.load(Ordering::SeqCst) == target_gen =>
            {
                // Idle keep-alive connections are closed by `serve_local_stream` after 60s;
                // mirror `ClientTransport::call` by transparently retrying once on a fresh stream
                // when the server generation has not bounced.
                conn = FramedLocalConn::connect().await?;
                conn.rpc(method, &params).await?
            }
            Err(err) => return Err(err),
        };
        {
            let mut cache = framed.cache.lock().unwrap();
            match method {
                "config.agents.save" | "agent.setup" => {
                    if let Ok(cfg) = serde_json::from_value::<Config>(reply.clone()) {
                        cache.config = cfg;
                    }
                }
                "scheduler.configure" => {
                    if let Ok(settings) = serde_json::from_value::<SchedulerSettings>(reply.clone())
                    {
                        cache.config.scheduler = settings;
                    }
                }
                _ => {}
            }
        }
        if self.server_boot_gen.load(Ordering::SeqCst) == target_gen {
            let mut pool = framed.pool.lock().unwrap();
            if pool.len() < 4 {
                pool.push((target_gen, conn));
            }
        }
        Ok(reply)
    }

    async fn refresh_framed_cache_async(&self) -> Result<()> {
        let Some(framed) = self.framed.as_ref() else {
            return Ok(());
        };
        let need_server_id = framed.cache.lock().unwrap().server_id.is_none();
        let ping_sid = if need_server_id {
            self.call_framed_async("app.ping", json!({}))
                .await?
                .get("server_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else {
            None
        };
        let mut state = self.call_framed_async("app.state", json!({})).await?;
        let revision = state["revision"].take();
        let config: Config = serde_json::from_value(state["config"].take())?;
        let snapshot: Snapshot = serde_json::from_value(state["snapshot"].take())?;
        let mut rt_val = self.call_framed_async("runtime.state", json!({})).await?;
        let runtime_state: RuntimeState = serde_json::from_value(rt_val["runtime"].take())?;
        let mut cache = framed.cache.lock().unwrap();
        if let Some(sid) = ping_sid {
            cache.server_id = Some(sid);
        }
        cache.revision = revision;
        cache.config = config;
        cache.snapshot = snapshot;
        cache.runtime_state = runtime_state;
        Ok(())
    }

    async fn refresh_framed_runtime_only_async(&self) -> Result<()> {
        let Some(framed) = self.framed.as_ref() else {
            return Ok(());
        };
        let mut rt_val = self.call_framed_async("runtime.state", json!({})).await?;
        let runtime_state: RuntimeState = serde_json::from_value(rt_val["runtime"].take())?;
        framed.cache.lock().unwrap().runtime_state = runtime_state;
        Ok(())
    }

    async fn reconnect_framed_async(&self) -> Result<()> {
        if let Some(framed) = self.framed.as_ref() {
            framed.pool.lock().unwrap().clear();
            self.refresh_framed_cache_async().await?;
        }
        Ok(())
    }

    /// Synchronous framed JSON-RPC fallback through `AppServer::serve_local_stream` when
    /// `LocalHostDriver::call` is invoked synchronously inside `MultiHostClient` while
    /// `framed_local_transport == true`.
    fn call_framed_sync(&self, method: &str, mut params: Value) -> Result<Value> {
        let srv = self
            .server_slot
            .lock()
            .unwrap()
            .clone()
            .context("Simulated local AppServer is crashed")?;
        if let Some(obj) = params.as_object_mut() {
            obj.remove("host_id");
        }
        let method_owned = method.to_owned();
        crate::trace::record_coverage_hit("transport.local_framed_rpc");
        let reply = std::thread::scope(|s| -> Result<Value> {
            let handle = s.spawn(move || -> Result<Value> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .context("build scoped framed local runtime")?;
                rt.block_on(async move {
                    let (client_io, server_io) = tokio::io::duplex(256 * 1024);
                    let srv_task = tokio::spawn(Arc::clone(&srv).serve_local_stream(server_io));
                    let (read_half, mut writer) = tokio::io::split(client_io);
                    let mut reader = tokio::io::BufReader::new(read_half);
                    let req = json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": method_owned,
                        "params": params,
                    });
                    write_json_line_async(&mut writer, &req).await?;
                    let line = read_bounded_line(&mut reader, MAX_FRAME_BYTES)
                        .await?
                        .ok_or_else(|| {
                            std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "Framed local stream closed unexpectedly",
                            )
                        })?;
                    drop(writer);
                    drop(reader);
                    let _ = srv_task.await;
                    decode_framed_jsonrpc_reply(&line)
                })
            });
            handle
                .join()
                .map_err(|_| anyhow!("framed local sync thread panicked"))?
        })?;
        if let Some(framed) = self.framed.as_ref() {
            let mut cache = framed.cache.lock().unwrap();
            match method {
                "config.agents.save" | "agent.setup" => {
                    if let Ok(cfg) = serde_json::from_value::<Config>(reply.clone()) {
                        cache.config = cfg;
                    }
                }
                "scheduler.configure" => {
                    if let Ok(settings) = serde_json::from_value::<SchedulerSettings>(reply.clone())
                    {
                        cache.config.scheduler = settings;
                    }
                }
                _ => {}
            }
        }
        Ok(reply)
    }
}

async fn run_framed_local_watcher(
    weak_client: std::sync::Weak<MultiHostClient>,
    driver: Arc<SimLocalHostDriver>,
) {
    let mut last_cursor = Value::Null;
    loop {
        if weak_client.strong_count() == 0 {
            return;
        }
        let addr = format!("local-server:{SIM_LOCAL_FRAMED_PORT}");
        let Ok(stream) = turmoil::net::TcpStream::connect(addr.as_str()).await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let (read_half, mut writer) = tokio::io::split(stream);
        let mut reader = tokio::io::BufReader::new(read_half);
        let sub_req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "app.subscribe",
            "params": {
                "after": last_cursor,
                "topics": ["snapshot", "herdr"],
                "heartbeat_ms": 15_000,
            }
        });
        if write_json_line_async(&mut writer, &sub_req).await.is_err() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        let Ok(Some(hs_line)) = read_bounded_line(&mut reader, MAX_FRAME_BYTES).await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        if let Ok(hs) = decode_framed_jsonrpc_reply(&hs_line) {
            let changed = hs["changed"].as_bool().unwrap_or(false);
            let refreshed_ok = if changed {
                driver.refresh_framed_cache_async().await.is_ok()
            } else {
                true
            };
            if refreshed_ok && !hs["revision"].is_null() {
                last_cursor = hs["revision"].clone();
            }
        }
        loop {
            if weak_client.strong_count() == 0 {
                return;
            }
            match tokio::time::timeout(
                Duration::from_secs(45),
                read_bounded_line(&mut reader, MAX_FRAME_BYTES),
            )
            .await
            {
                Ok(Ok(Some(first_line))) => {
                    // Drain any already-buffered push lines in one batch so multi-event mutations
                    // trigger a single framed `app.state` refresh instead of one per event.
                    let mut latest_cursor = Value::Null;
                    let mut saw_herdr = false;
                    let mut process_line = |line: &str| {
                        if let Ok(msg) = serde_json::from_str::<Value>(line)
                            && msg.get("method").and_then(Value::as_str) == Some("event")
                        {
                            let ev_cursor = &msg["params"]["revision"];
                            if !ev_cursor.is_null() {
                                latest_cursor = ev_cursor.clone();
                            }
                            if msg["params"]["topic"].as_str() == Some("herdr") {
                                saw_herdr = true;
                            }
                        }
                    };
                    process_line(&first_line);
                    while !reader.buffer().is_empty() {
                        match read_bounded_line(&mut reader, MAX_FRAME_BYTES).await {
                            Ok(Some(extra)) => process_line(&extra),
                            _ => break,
                        }
                    }
                    let cursor_changed = !latest_cursor.is_null() && latest_cursor != last_cursor;
                    if cursor_changed {
                        if driver.refresh_framed_cache_async().await.is_ok() {
                            last_cursor = latest_cursor;
                        }
                    } else if saw_herdr {
                        let _ = driver.refresh_framed_runtime_only_async().await;
                    }
                }
                Ok(Ok(None)) | Ok(Err(_)) | Err(_) => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    break;
                }
            }
        }
    }
}

impl LocalHostDriver for SimLocalHostDriver {
    fn home(&self) -> &Path {
        &self.home
    }

    fn endpoint_label(&self) -> String {
        if self.framed.is_some() {
            format!("turmoil://local-server:{SIM_LOCAL_FRAMED_PORT}")
        } else {
            "turmoil://local-server".into()
        }
    }

    fn server_id(&self) -> Option<String> {
        if let Some(framed) = self.framed.as_ref() {
            if self.server_slot.lock().unwrap().is_none() {
                return None;
            }
            return framed.cache.lock().unwrap().server_id.clone();
        }
        let srv = self.server_slot.lock().unwrap().clone()?;
        srv.call("app.ping", json!({}))
            .ok()?
            .get("server_id")?
            .as_str()
            .map(str::to_owned)
    }

    fn server_notice(&self) -> Option<String> {
        None
    }

    fn api_version(&self) -> u32 {
        canban_app_server::API_VERSION
    }

    fn version(&self) -> String {
        "sim".into()
    }

    fn platform(&self) -> String {
        "sim".into()
    }

    fn transport(&self) -> Arc<ClientTransport> {
        let (connected_gen, current) = {
            let guard = self.transport_state.lock().unwrap();
            (guard.0, Arc::clone(&guard.1))
        };
        let cur_gen = self.server_boot_gen.load(Ordering::SeqCst);
        if connected_gen != cur_gen
            && self.server_slot.lock().unwrap().is_some()
            && let Ok(rt) = tokio::runtime::Handle::try_current()
            && !self.reconnecting.swap(true, Ordering::SeqCst)
        {
            let server_boot_gen = Arc::clone(&self.server_boot_gen);
            let token_slot = Arc::clone(&self.token_slot);
            let transport_state = Arc::clone(&self.transport_state);
            let reconnecting = Arc::clone(&self.reconnecting);
            rt.spawn(async move {
                struct ResetFlag(Arc<AtomicBool>);
                impl Drop for ResetFlag {
                    fn drop(&mut self) {
                        self.0.store(false, Ordering::SeqCst);
                    }
                }
                let _reset = ResetFlag(reconnecting);
                let target_gen = server_boot_gen.load(Ordering::SeqCst);
                let Some(tok) = token_slot.lock().unwrap().clone() else {
                    return;
                };
                if let Ok(fresh) = connect_local_transport(tok).await {
                    *transport_state.lock().unwrap() = (target_gen, fresh);
                }
            });
        }
        current
    }

    fn connect_push_transport<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<ClientTransport>>> + Send + 'a>> {
        Box::pin(async move {
            let (connected_gen, current) = {
                let guard = self.transport_state.lock().unwrap();
                (guard.0, Arc::clone(&guard.1))
            };
            let cur_gen = self.server_boot_gen.load(Ordering::SeqCst);
            if connected_gen != cur_gen {
                if self.server_slot.lock().unwrap().is_none() {
                    bail!("Simulated local AppServer is crashed");
                }
                self.refresh_transport().await?;
                return Ok(Arc::clone(&self.transport_state.lock().unwrap().1));
            }
            Ok(current)
        })
    }

    fn call(&self, method: &str, params: Value) -> Result<Value> {
        if self.framed.is_some() {
            return self.call_framed_sync(method, params);
        }
        let srv = self
            .server_slot
            .lock()
            .unwrap()
            .clone()
            .context("Simulated local AppServer is crashed")?;
        srv.call(method, params)
    }

    fn snapshot(&self) -> Result<Snapshot> {
        if let Some(framed) = self.framed.as_ref() {
            if self.server_slot.lock().unwrap().is_none() {
                bail!("Simulated local AppServer is crashed");
            }
            return Ok(framed.cache.lock().unwrap().snapshot.clone());
        }
        let srv = self
            .server_slot
            .lock()
            .unwrap()
            .clone()
            .context("Simulated local AppServer is crashed")?;
        srv.snapshot()
    }

    fn runtime_state(&self) -> Result<RuntimeState> {
        if let Some(framed) = self.framed.as_ref() {
            if self.server_slot.lock().unwrap().is_none() {
                bail!("Simulated local AppServer is crashed");
            }
            return Ok(framed.cache.lock().unwrap().runtime_state.clone());
        }
        let srv = self
            .server_slot
            .lock()
            .unwrap()
            .clone()
            .context("Simulated local AppServer is crashed")?;
        srv.runtime_state()
    }

    fn config(&self) -> Config {
        if let Some(framed) = self.framed.as_ref() {
            return framed.cache.lock().unwrap().config.clone();
        }
        self.server_slot
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.config())
            .unwrap_or_default()
    }
}

type ClientTaskFn =
    Box<dyn FnOnce(Arc<MultiHostClient>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

struct ClientCommand {
    run: ClientTaskFn,
}

/// Handle to an in-flight non-blocking RPC spawned via [`SimWorld::client_spawn`].
pub struct ClientCallHandle {
    pub client_id: String,
    pub method: String,
    rx: oneshot::Receiver<Result<Value>>,
}

impl ClientCallHandle {
    /// Non-blocking poll of the spawned RPC result.
    pub fn try_recv(&mut self) -> Option<Result<Value>> {
        match self.rx.try_recv() {
            Ok(res) => Some(res),
            Err(oneshot::error::TryRecvError::Empty) => None,
            Err(oneshot::error::TryRecvError::Closed) => Some(Err(anyhow!(
                "RPC '{}' on client '{}' cancelled (client crashed or dropped)",
                self.method,
                self.client_id
            ))),
        }
    }
}

/// Shared handles for a simulated [`AppServer`] host.
pub struct SimServerHandle {
    pub name: String,
    pub home: PathBuf,
    pub herdr: Arc<SimHerdr>,
    pub github: Arc<SimGitHubHost>,
    pub exec: Arc<SimExec>,
    pub clock: Arc<TurmoilClock>,
    pub storage_faults: Arc<StorageFaultInjector>,
    pub failpoints: Arc<Failpoints>,
    pub tailscale_verifier: Arc<Mutex<Option<SimTailscaleVerifier>>>,
    pub desktop_api_override: Arc<Mutex<Option<u32>>>,
    pub server_slot: Arc<Mutex<Option<Arc<AppServer>>>>,
    pub abrupt_crash: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    pub token: Arc<Mutex<Option<String>>>,
    pub crashed: Arc<AtomicBool>,
    pub ever_bounced: Arc<AtomicBool>,
    pub boot_generation: Arc<AtomicU64>,
}

impl SimServerHandle {
    pub fn server(&self) -> Option<Arc<AppServer>> {
        self.server_slot.lock().unwrap().clone()
    }

    pub fn token(&self) -> Option<String> {
        self.token.lock().unwrap().clone()
    }
}

/// Shared handles for a simulated [`MultiHostClient`] host (primary `"ui-client"` or secondary).
pub struct SimClientHandle {
    pub name: String,
    pub data_dir: PathBuf,
    pub failpoints: Arc<Failpoints>,
    pub dial_counts: Arc<Mutex<BTreeMap<String, u64>>>,
    pub client_slot: Arc<Mutex<Option<Arc<MultiHostClient>>>>,
    pub rt_handle: Arc<Mutex<Option<tokio::runtime::Handle>>>,
    cmd_tx: Arc<Mutex<Option<mpsc::UnboundedSender<ClientCommand>>>>,
    local_driver: Arc<Mutex<Option<Arc<SimLocalHostDriver>>>>,
    events: Arc<Mutex<VecDeque<ClientEvent>>>,
    total_events: Arc<AtomicU64>,
    pub crashed: Arc<AtomicBool>,
    pub boot_generation: Arc<AtomicU64>,
}

impl SimClientHandle {
    pub fn client(&self) -> Option<Arc<MultiHostClient>> {
        self.client_slot.lock().unwrap().clone()
    }

    fn with_runtime<T>(&self, f: impl FnOnce(&MultiHostClient) -> T) -> Result<T> {
        let client = self
            .client_slot
            .lock()
            .unwrap()
            .clone()
            .with_context(|| format!("Simulated client '{}' is not running", self.name))?;
        let rt = self.rt_handle.lock().unwrap().clone();
        let _guard = rt.as_ref().map(tokio::runtime::Handle::enter);
        Ok(f(&client))
    }

    pub fn total_events(&self) -> u64 {
        self.total_events.load(Ordering::SeqCst)
    }
}

/// Deterministic simulation world orchestrating `turmoil::Sim`, server nodes, and `MultiHostClient`s.
pub struct SimWorld {
    _tempdir: TempDir,
    pub config: SimWorldConfig,
    pub rng: StdRng,
    pub trace: EventTrace,
    pub github: Arc<SimGitHub>,
    pub client_data_dir: PathBuf,
    pub servers: BTreeMap<String, SimServerHandle>,
    pub clients: BTreeMap<String, SimClientHandle>,
    stream_fault_policy: StreamFaultPolicy,
    coverage: Mutex<CoverageTracker>,
    sim: turmoil::Sim<'static>,
    shared_elapsed_ms: Arc<AtomicU64>,
    partitioned_pairs: BTreeSet<(String, String)>,
    held_pairs: BTreeSet<(String, String)>,
    link_fail_pairs: BTreeSet<(String, String)>,
    faulted_hosts: BTreeSet<String>,
    global_net_faulted: bool,
    suppress_herdr_heartbeats: bool,
    last_herdr_heartbeat_ms: u64,
    step_count: u64,
    fault_count: u64,
}

impl SimWorld {
    pub fn new(config: SimWorldConfig) -> Result<Self> {
        let tempdir = tempfile::Builder::new()
            .prefix(&format!("canban-sim-{}-", config.seed))
            .tempdir()
            .context("create sim tempdir")?;
        let root = tempdir.path().to_path_buf();
        let client_data_dir = root.join(PRIMARY_CLIENT);
        std::fs::create_dir_all(&client_data_dir)?;

        let mut builder = turmoil::Builder::new();
        builder
            .rng_seed(config.seed ^ 0x9e37_79b9_7f4a_7c15)
            .tick_duration(Duration::from_millis(config.tick_ms.max(1)))
            .simulation_duration(Duration::from_secs(24 * 60 * 60))
            .min_message_latency(Duration::from_millis(config.min_latency_ms))
            .max_message_latency(Duration::from_millis(
                config.max_latency_ms.max(config.min_latency_ms),
            ))
            .fail_rate(config.fail_rate);
        if config.enable_random_order {
            builder.enable_random_order();
        }

        let mut sim = builder.build();
        let shared_elapsed_ms = Arc::new(AtomicU64::new(0));
        let github_clock = Arc::new(TurmoilClock::new(Arc::clone(&shared_elapsed_ms)));
        let github = SimGitHub::with_clock(github_clock);
        let stream_fault_policy = config.stream_fault_policy.clone().unwrap_or_default();

        let mut servers = BTreeMap::new();
        let mut host_names = Vec::new();
        if config.desktop_local {
            host_names.push("local-server".to_owned());
        }
        host_names.extend(config.remote_hosts.clone());

        for host_name in &host_names {
            let home = root.join(format!("srv-{host_name}"));
            let config_dir = home.join("config");
            std::fs::create_dir_all(&config_dir)?;
            let cfg_path = config_dir.join("agents.json");
            if !cfg_path.exists() {
                let mut sim_cfg = Config::default();
                for prof in &mut sim_cfg.profiles {
                    prof.preset = None;
                    prof.argv = Some(vec!["/bin/echo".into(), "{prompt}".into()]);
                    prof.resume_argv = Some(vec![
                        "/bin/echo".into(),
                        "--resume".into(),
                        "{session_id}".into(),
                        "{prompt}".into(),
                    ]);
                    prof.exit = Some(canban_types::ExitAction::Input {
                        text: "/exit\r".into(),
                    });
                }
                std::fs::write(&cfg_path, serde_json::to_vec_pretty(&sim_cfg)?)?;
            }

            let skew = config
                .host_clock_skew_ms
                .get(host_name)
                .copied()
                .unwrap_or(0);
            let clock = Arc::new(TurmoilClock::with_skew(
                Arc::clone(&shared_elapsed_ms),
                skew,
            ));
            let herdr = SimHerdr::new(format!("core-{host_name}"), clock.clone());
            let host_github = github.for_host(host_name.clone());
            let exec = SimExec::new(host_name.clone());
            let storage_faults = Arc::new(StorageFaultInjector::new());
            let failpoints = make_observed_failpoints();
            let tailscale_verifier_slot: Arc<Mutex<Option<SimTailscaleVerifier>>> = Arc::new(
                Mutex::new(config.tailscale_verifiers.get(host_name).cloned()),
            );
            let desktop_api_override_slot: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(
                config.desktop_api_overrides.get(host_name).copied(),
            ));
            let server_slot: Arc<Mutex<Option<Arc<AppServer>>>> = Arc::new(Mutex::new(None));
            let abrupt_slot: Arc<Mutex<Option<Arc<AtomicBool>>>> = Arc::new(Mutex::new(None));
            let token_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
            let crashed = Arc::new(AtomicBool::new(false));
            let ever_bounced = Arc::new(AtomicBool::new(false));
            let boot_generation = Arc::new(AtomicU64::new(0));

            let h_home = home.clone();
            let h_clock = clock.clone();
            let h_herdr = herdr.clone();
            let h_github = host_github.clone();
            let h_exec = exec.clone();
            let h_storage_faults = Arc::clone(&storage_faults);
            let h_failpoints = Arc::clone(&failpoints);
            let h_ts_verifier = Arc::clone(&tailscale_verifier_slot);
            let h_api_override = Arc::clone(&desktop_api_override_slot);
            let h_slot = Arc::clone(&server_slot);
            let h_abrupt = Arc::clone(&abrupt_slot);
            let h_token = Arc::clone(&token_slot);
            let h_boot_gen = Arc::clone(&boot_generation);
            let h_name = host_name.clone();
            let h_seed = config.seed;
            let h_durability = config.fs_durability;
            let h_sqlite_sync_off = config.sqlite_synchronous_off;
            let h_disable_watchers = config.disable_os_watchers;
            let h_framed_local = config.framed_local_transport && host_name == "local-server";
            let h_script_mode = config
                .host_script_execution_modes
                .get(host_name)
                .copied()
                .unwrap_or(config.script_execution_mode);
            let h_stream_policy = if host_name == "local-server" {
                None
            } else {
                Some(stream_fault_policy.clone())
            };

            sim.host(host_name.as_str(), move || {
                let home = h_home.clone();
                let clock = h_clock.clone();
                let herdr = h_herdr.clone();
                let host_github = h_github.clone();
                let exec = h_exec.clone();
                let storage_faults = Arc::clone(&h_storage_faults);
                let failpoints = Arc::clone(&h_failpoints);
                let ts_verifier_slot = Arc::clone(&h_ts_verifier);
                let api_override_slot = Arc::clone(&h_api_override);
                let slot = Arc::clone(&h_slot);
                let abrupt_slot = Arc::clone(&h_abrupt);
                let token_slot = Arc::clone(&h_token);
                let boot_gen = h_boot_gen.fetch_add(1, Ordering::SeqCst);
                let host_label = h_name.clone();
                let id_gen = canban_types::deterministic_id_gen(derive_host_seed(
                    h_seed,
                    &host_label,
                    boot_gen,
                ));
                let stream_policy = h_stream_policy.clone();
                async move {
                    let tailscale_verifier: Option<Arc<dyn TailscalePeerVerifier>> =
                        ts_verifier_slot
                            .lock()
                            .unwrap()
                            .clone()
                            .map(|v| Arc::new(v) as Arc<dyn TailscalePeerVerifier>);
                    let desktop_api_override = *api_override_slot.lock().unwrap();
                    let env = ServerRuntimeEnv {
                        clock,
                        id_gen,
                        herdr_backend: Some(herdr),
                        github_transport: Some(host_github),
                        inline_workers: true,
                        exec_runner: exec,
                        fs_durability: h_durability,
                        sqlite_synchronous_off: h_sqlite_sync_off,
                        disable_os_watchers: h_disable_watchers,
                        tailscale_verifier,
                        desktop_api_override,
                        script_execution_mode: h_script_mode,
                        storage_faults: Some(storage_faults),
                        failpoints: Some(failpoints),
                    };
                    let server = AppServer::open_with_env(home, PathBuf::from("/bin/echo"), env)
                        .map_err(|e| format!("open_with_env({host_label}): {e:#}"))?;
                    // Ensure at least one valid device token exists across restarts.
                    {
                        let mut tok = token_slot.lock().unwrap();
                        if tok.is_none() {
                            let (_, issued) = server
                                .issue_device_token(&format!("sim-client-{host_label}"))
                                .map_err(|e| format!("issue_device_token({host_label}): {e:#}"))?;
                            *tok = Some(issued);
                        }
                    }
                    let running = server
                        .start_in_runtime(tokio::runtime::Handle::current())
                        .map_err(|e| format!("start_in_runtime({host_label}): {e:#}"))?;
                    *abrupt_slot.lock().unwrap() = Some(running.abrupt_crash_flag());
                    let listener = turmoil::net::TcpListener::bind(("0.0.0.0", SIM_PORT))
                        .await
                        .map_err(|e| format!("bind({host_label}): {e:#}"))?;
                    let local_framed_task = if h_framed_local {
                        let framed_listener =
                            turmoil::net::TcpListener::bind(("0.0.0.0", SIM_LOCAL_FRAMED_PORT))
                                .await
                                .map_err(|e| format!("bind_framed({host_label}): {e:#}"))?;
                        let framed_srv = Arc::clone(&server);
                        let mut framed_stop = server.stop_receiver();
                        Some(tokio::spawn(async move {
                            loop {
                                if *framed_stop.borrow() || framed_srv.stopped() {
                                    break;
                                }
                                tokio::select! {
                                    biased;
                                    _ = framed_stop.changed() => break,
                                    res = framed_listener.accept() => {
                                        match res {
                                            Ok((stream, _peer)) => {
                                                let srv = Arc::clone(&framed_srv);
                                                tokio::spawn(async move {
                                                    let _ = srv.serve_local_stream(stream).await;
                                                });
                                            }
                                            Err(_) => {
                                                if *framed_stop.borrow() || framed_srv.stopped() {
                                                    break;
                                                }
                                                tokio::time::sleep(Duration::from_millis(5)).await;
                                            }
                                        }
                                    }
                                }
                            }
                        }))
                    } else {
                        None
                    };
                    *slot.lock().unwrap() = Some(Arc::clone(&server));
                    let res = serve_turmoil_listener_with_faults(
                        Arc::clone(&server),
                        listener,
                        stream_policy,
                        Some(host_label.clone()),
                    )
                    .await;
                    if let Some(task) = local_framed_task {
                        task.abort();
                    }
                    *slot.lock().unwrap() = None;
                    *abrupt_slot.lock().unwrap() = None;
                    drop(running);
                    res.map_err(|e| format!("serve({host_label}): {e:#}").into())
                }
            });

            servers.insert(
                host_name.clone(),
                SimServerHandle {
                    name: host_name.clone(),
                    home,
                    herdr,
                    github: host_github,
                    exec,
                    clock,
                    storage_faults,
                    failpoints,
                    tailscale_verifier: tailscale_verifier_slot,
                    desktop_api_override: desktop_api_override_slot,
                    server_slot,
                    abrupt_crash: abrupt_slot,
                    token: token_slot,
                    crashed,
                    ever_bounced,
                    boot_generation,
                },
            );
        }

        // Wait until all server hosts have booted and bound their simulated TCP listeners.
        for _ in 0..400 {
            sim.step().map_err(|e| anyhow!("turmoil boot step: {e}"))?;
            shared_elapsed_ms.store(sim.elapsed().as_millis() as u64, Ordering::SeqCst);
            let all_ready = servers
                .values()
                .all(|s| s.server().is_some() && s.token().is_some());
            if all_ready {
                break;
            }
        }
        for (name, h) in &servers {
            if h.server().is_none() || h.token().is_none() {
                bail!("Simulated server '{name}' failed to boot within 400 steps");
            }
        }

        let rng = StdRng::seed_from_u64(config.seed);
        let trace = EventTrace::new(config.seed, config.scenario.clone());
        let desktop_local = config.desktop_local;
        let extra_clients = config.extra_clients.clone();
        let mut world = Self {
            _tempdir: tempdir,
            config,
            rng,
            trace,
            github,
            client_data_dir: client_data_dir.clone(),
            servers,
            clients: BTreeMap::new(),
            stream_fault_policy,
            coverage: Mutex::new(CoverageTracker::new()),
            sim,
            shared_elapsed_ms,
            partitioned_pairs: BTreeSet::new(),
            held_pairs: BTreeSet::new(),
            link_fail_pairs: BTreeSet::new(),
            faulted_hosts: BTreeSet::new(),
            global_net_faulted: false,
            suppress_herdr_heartbeats: false,
            last_herdr_heartbeat_ms: 0,
            step_count: 0,
            fault_count: 0,
        };

        world.register_client_host(PRIMARY_CLIENT, client_data_dir, desktop_local)?;
        for extra in extra_clients {
            if extra != PRIMARY_CLIENT {
                world.add_client(&extra, false)?;
            }
        }

        world.advance(Duration::from_millis(200))?;
        Ok(world)
    }

    fn register_client_host(
        &mut self,
        client_name: &str,
        data_dir: PathBuf,
        desktop_local: bool,
    ) -> Result<()> {
        if self.clients.contains_key(client_name) {
            bail!("Simulated client '{client_name}' already exists");
        }
        std::fs::create_dir_all(&data_dir)?;

        let client_slot: Arc<Mutex<Option<Arc<MultiHostClient>>>> = Arc::new(Mutex::new(None));
        let rt_handle: Arc<Mutex<Option<tokio::runtime::Handle>>> = Arc::new(Mutex::new(None));
        let cmd_tx_slot: Arc<Mutex<Option<mpsc::UnboundedSender<ClientCommand>>>> =
            Arc::new(Mutex::new(None));
        let local_driver_slot: Arc<Mutex<Option<Arc<SimLocalHostDriver>>>> =
            Arc::new(Mutex::new(None));
        let client_events: Arc<Mutex<VecDeque<ClientEvent>>> =
            Arc::new(Mutex::new(VecDeque::new()));
        let total_events = Arc::new(AtomicU64::new(0));
        let crashed = Arc::new(AtomicBool::new(false));
        let boot_generation = Arc::new(AtomicU64::new(0));
        let client_failpoints = make_observed_failpoints();
        let dial_counts: Arc<Mutex<BTreeMap<String, u64>>> = Arc::new(Mutex::new(BTreeMap::new()));

        let ui_data_dir = data_dir.clone();
        let ui_clock = Arc::new(TurmoilClock::new(Arc::clone(&self.shared_elapsed_ms)));
        let ui_client_slot = Arc::clone(&client_slot);
        let ui_rt_handle = Arc::clone(&rt_handle);
        let ui_cmd_tx = Arc::clone(&cmd_tx_slot);
        let ui_local_driver = Arc::clone(&local_driver_slot);
        let ui_events = Arc::clone(&client_events);
        let ui_total_events = Arc::clone(&total_events);
        let ui_boot_gen = Arc::clone(&boot_generation);
        let ui_failpoints = Arc::clone(&client_failpoints);
        let ui_dial_counts = Arc::clone(&dial_counts);
        let ui_seed = self.config.seed;
        let ui_durability = self.config.fs_durability;
        let ui_framed_local = self.config.framed_local_transport;
        let ui_policy = self.stream_fault_policy.clone();
        let ui_name = client_name.to_owned();

        let local_info = if desktop_local {
            self.servers.get("local-server").map(|s| {
                (
                    s.home.clone(),
                    Arc::clone(&s.server_slot),
                    Arc::clone(&s.boot_generation),
                    Arc::clone(&s.token),
                )
            })
        } else {
            None
        };
        let initial_remotes: Vec<(String, String)> = self
            .config
            .remote_hosts
            .iter()
            .filter_map(|name| {
                self.servers
                    .get(name)
                    .and_then(|s| s.token())
                    .map(|tok| (name.clone(), tok))
            })
            .collect();

        self.sim.host(client_name, move || {
            let data_dir = ui_data_dir.clone();
            let clock = ui_clock.clone();
            let client_slot = Arc::clone(&ui_client_slot);
            let rt_slot = Arc::clone(&ui_rt_handle);
            let cmd_tx_holder = Arc::clone(&ui_cmd_tx);
            let local_driver_holder = Arc::clone(&ui_local_driver);
            let events_sink = Arc::clone(&ui_events);
            let total_events_ctr = Arc::clone(&ui_total_events);
            let failpoints = Arc::clone(&ui_failpoints);
            let dial_counts = Arc::clone(&ui_dial_counts);
            let boot_gen = ui_boot_gen.fetch_add(1, Ordering::SeqCst);
            let client_id_gen =
                canban_types::deterministic_id_gen(derive_host_seed(ui_seed, &ui_name, boot_gen));
            let env_id_gen = canban_types::deterministic_id_gen(derive_host_seed(
                ui_seed,
                &format!("{ui_name}:global-env"),
                boot_gen,
            ));
            let local_info = local_info.clone();
            let remotes = initial_remotes.clone();
            let policy = ui_policy.clone();
            let device_label = format!("Sim Client ({ui_name})");
            async move {
                let (tx, mut rx) = mpsc::unbounded_channel::<ClientCommand>();
                #[allow(clippy::type_complexity)]
                let (concrete_local_driver, local_driver, global_env_provider, capabilities): (
                    Option<Arc<SimLocalHostDriver>>,
                    Option<Arc<dyn LocalHostDriver>>,
                    Option<Arc<dyn canban_client::GlobalEnvSyncProvider>>,
                    ClientCapabilities,
                ) = if desktop_local {
                    let (local_home, srv_slot, srv_boot_gen, tok_slot) =
                        local_info.expect("local server info");
                    let connected_gen = srv_boot_gen.load(Ordering::SeqCst);
                    let local_tok = tok_slot.lock().unwrap().clone().expect("local token");
                    let transport = connect_local_transport(local_tok)
                        .await
                        .map_err(|e| format!("{e:#}"))?;
                    let framed = ui_framed_local.then(|| Arc::new(FramedLocalState::default()));
                    let concrete_driver = Arc::new(SimLocalHostDriver {
                        home: local_home.clone(),
                        server_slot: srv_slot,
                        server_boot_gen: srv_boot_gen,
                        token_slot: tok_slot,
                        transport_state: Arc::new(Mutex::new((connected_gen, transport))),
                        reconnecting: Arc::new(AtomicBool::new(false)),
                        framed,
                    });
                    if ui_framed_local {
                        concrete_driver
                            .refresh_framed_cache_async()
                            .await
                            .map_err(|e| format!("initial framed local sync: {e:#}"))?;
                    }
                    *local_driver_holder.lock().unwrap() = Some(Arc::clone(&concrete_driver));
                    let driver: Arc<dyn LocalHostDriver> =
                        Arc::clone(&concrete_driver) as Arc<dyn LocalHostDriver>;
                    let env_provider: Arc<dyn canban_client::GlobalEnvSyncProvider> =
                        Arc::new(DesktopGlobalEnvProvider::new(local_home).with_id_gen(env_id_gen));
                    (
                        Some(concrete_driver),
                        Some(driver),
                        Some(env_provider),
                        ClientCapabilities::desktop(),
                    )
                } else {
                    (None, None, None, ClientCapabilities::mobile())
                };

                let mut rc_cfg = RemoteClientConfig::new(data_dir.clone());
                rc_cfg.device_name = device_label;
                let host_store = Arc::new(InlineHostsFileStore::new_with_durability(
                    data_dir.join("hosts.json"),
                    ui_durability,
                ));
                let rt_cur = tokio::runtime::Handle::current();
                let connector = Arc::new(CountingHostConnector {
                    inner: FaultyTurmoilConnector::new(policy),
                    dial_counts,
                });
                let client = MultiHostClient::from_options_with_runtime_id_gen_and_failpoints(
                    MultiHostClientOptions {
                        config: rc_cfg,
                        host_store,
                        connector,
                        capabilities,
                        local_host: local_driver,
                        global_env_provider,
                        min_api_version: Some(canban_app_server::API_VERSION),
                        clock,
                        fs_durability: ui_durability,
                    },
                    rt_cur.clone(),
                    client_id_gen,
                    Some(failpoints),
                )
                .map_err(|e| format!("MultiHostClient init: {e:#}"))?;

                if ui_framed_local && let Some(driver) = concrete_local_driver {
                    let weak_client = Arc::downgrade(&client);
                    tokio::spawn(run_framed_local_watcher(weak_client, driver));
                }

                // On first boot, register initial remote hosts. On subsequent bounces after
                // `crash_client()`, `hosts.json` + `.outbox.json` already exist and were loaded
                // by `MultiHostClient::from_options_with_runtime` (preserving `InFlight -> Queued` recovery!).
                let existing_ids: BTreeSet<String> =
                    client.host_summaries().into_iter().map(|s| s.id).collect();
                for (name, token) in remotes {
                    if existing_ids.contains(&name) {
                        continue;
                    }
                    let saved = SavedHost {
                        id: name.clone(),
                        name: name.clone(),
                        transport: HostTransport::Tcp {
                            addr: format!("{name}:{SIM_PORT}"),
                            token,
                        },
                        ssh_target: None,
                        server_id: None,
                        enabled: true,
                        added_at: 0,
                    };
                    client
                        .add_host_async(saved)
                        .await
                        .map_err(|e| format!("add_host({name}): {e:#}"))?;
                }

                let mut ev_rx = client.subscribe();
                let ev_sink = Arc::clone(&events_sink);
                let ev_total = Arc::clone(&total_events_ctr);
                tokio::spawn(async move {
                    while let Some(ev) = ev_rx.recv().await {
                        ev_total.fetch_add(1, Ordering::SeqCst);
                        let mut guard = ev_sink.lock().unwrap();
                        if guard.len() >= MAX_BUFFERED_CLIENT_EVENTS {
                            guard.pop_front();
                        }
                        guard.push_back(ev);
                    }
                });

                *rt_slot.lock().unwrap() = Some(rt_cur);
                *cmd_tx_holder.lock().unwrap() = Some(tx);
                *client_slot.lock().unwrap() = Some(Arc::clone(&client));

                while let Some(cmd) = rx.recv().await {
                    let c = Arc::clone(&client);
                    let fut = (cmd.run)(c);
                    tokio::spawn(fut);
                }
                *client_slot.lock().unwrap() = None;
                *cmd_tx_holder.lock().unwrap() = None;
                *local_driver_holder.lock().unwrap() = None;
                *rt_slot.lock().unwrap() = None;
                Ok(())
            }
        });

        self.clients.insert(
            client_name.to_owned(),
            SimClientHandle {
                name: client_name.to_owned(),
                data_dir,
                failpoints: client_failpoints,
                dial_counts,
                client_slot,
                rt_handle,
                cmd_tx: cmd_tx_slot,
                local_driver: local_driver_slot,
                events: client_events,
                total_events,
                crashed,
                boot_generation,
            },
        );

        for _ in 0..600 {
            self.step_once()?;
            if self.clients[client_name].client().is_some() {
                return Ok(());
            }
        }
        bail!("Simulated client '{client_name}' failed to initialize within 600 steps");
    }

    /// Dynamically spawns an additional [`MultiHostClient`] host (e.g. for multi-client race tests).
    pub fn add_client(&mut self, name: &str, desktop_local: bool) -> Result<()> {
        let dir = self._tempdir.path().join(name);
        self.register_client_host(name, dir, desktop_local)?;
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            name,
            "client.add",
            format!("desktop_local={desktop_local}"),
        );
        self.hit("world.multi_client");
        Ok(())
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.shared_elapsed_ms.load(Ordering::SeqCst)
    }

    pub fn now_ms(&self) -> u64 {
        crate::clock::SIM_EPOCH_BASE_MS.saturating_add(self.elapsed_ms())
    }

    pub fn step_count(&self) -> u64 {
        self.step_count
    }

    pub fn fault_count(&self) -> u64 {
        self.fault_count
    }

    /// Records a property-coverage (`sometimes`) hit for this world (flushed on drop).
    pub fn hit(&self, property: &str) {
        self.coverage.lock().unwrap().hit(property);
    }

    /// Returns a snapshot of property-coverage counts recorded on this world so far.
    pub fn coverage_counts(&self) -> BTreeMap<String, u64> {
        self.coverage.lock().unwrap().counts().clone()
    }

    /// Flushes this world's recorded coverage counts into the thread-local coverage collector.
    pub fn flush_coverage(&self) {
        let mut cov = self.coverage.lock().unwrap();
        if !cov.is_empty() {
            let drained = std::mem::take(&mut *cov);
            crate::trace::merge_into_thread_coverage(&drained);
        }
    }

    /// Live-mutable byte-stream fault policy wired into remote TCP streams.
    pub fn stream_fault_policy(&self) -> &StreamFaultPolicy {
        &self.stream_fault_policy
    }

    /// Creates and returns a writable temporary repository directory inside the simulation tempdir.
    pub fn project_dir(&self, name: &str) -> Result<String> {
        let dir = self._tempdir.path().join("repos").join(name);
        std::fs::create_dir_all(&dir)?;
        Ok(dir.to_string_lossy().into_owned())
    }

    /// Returns an isolated absolute file path inside the simulation tempdir (creating parent dirs).
    pub fn temp_file_path(&self, rel: &str) -> Result<PathBuf> {
        let path = self._tempdir.path().join("scratch").join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(path)
    }

    /// Returns the configured [`SimTailscaleVerifier`] handle for `host`, if any.
    pub fn tailscale_verifier(&self, host: &str) -> Option<SimTailscaleVerifier> {
        self.servers
            .get(host)
            .and_then(|s| s.tailscale_verifier.lock().unwrap().clone())
    }

    /// Installs or clears a live [`SimTailscaleVerifier`] on `host` (persisting across bounces).
    pub fn set_tailscale_verifier(
        &mut self,
        host: &str,
        verifier: Option<SimTailscaleVerifier>,
    ) -> Result<()> {
        let handle = self
            .servers
            .get(host)
            .with_context(|| format!("Unknown server host '{host}'"))?;
        *handle.tailscale_verifier.lock().unwrap() = verifier.clone();
        if let Some(srv) = handle.server() {
            srv.set_tailscale_verifier(
                verifier.map(|v| Arc::new(v) as Arc<dyn TailscalePeerVerifier>),
            );
        }
        Ok(())
    }

    /// Overrides the advertised API version on `host` (for `ApiMismatch` testing; persists across bounces).
    pub fn set_desktop_api_override(&mut self, host: &str, api: Option<u32>) -> Result<()> {
        let handle = self
            .servers
            .get(host)
            .with_context(|| format!("Unknown server host '{host}'"))?;
        *handle.desktop_api_override.lock().unwrap() = api;
        if let Some(srv) = handle.server() {
            srv.set_desktop_api_override(api);
        }
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            host,
            "server.api",
            format!("{api:?}"),
        );
        Ok(())
    }

    /// Controls whether `SimWorld` automatically emits virtual-time Herdr control-plane heartbeats
    /// every 15s (default `false` = heartbeats enabled). Set `true` in heartbeat-timeout tests.
    pub fn set_suppress_herdr_heartbeats(&mut self, suppress: bool) {
        self.suppress_herdr_heartbeats = suppress;
    }

    pub fn step_once(&mut self) -> Result<()> {
        self.sim
            .step()
            .map_err(|e| anyhow!("turmoil step error: {e}"))?;
        self.step_count += 1;
        let elapsed_ms = self.sim.elapsed().as_millis() as u64;
        self.shared_elapsed_ms.store(elapsed_ms, Ordering::SeqCst);
        if !self.suppress_herdr_heartbeats
            && elapsed_ms.saturating_sub(self.last_herdr_heartbeat_ms) >= 15_000
        {
            self.last_herdr_heartbeat_ms = elapsed_ms;
            for h in self.servers.values() {
                if !h.crashed.load(Ordering::SeqCst) && h.server().is_some() {
                    let _ = h.herdr.maybe_emit_heartbeat(14_000);
                }
            }
        }
        Ok(())
    }

    pub fn advance(&mut self, duration: Duration) -> Result<()> {
        let target = self.sim.elapsed().saturating_add(duration);
        while self.sim.elapsed() < target {
            self.step_once()?;
        }
        Ok(())
    }

    /// Steps virtual time deterministically until `pred` returns `true` or `timeout_ms` elapses.
    pub fn wait_until<F>(&mut self, timeout_ms: u64, mut pred: F) -> Result<()>
    where
        F: FnMut(&Self) -> Result<bool>,
    {
        let deadline = self
            .sim
            .elapsed()
            .saturating_add(Duration::from_millis(timeout_ms));
        loop {
            if pred(self)? {
                return Ok(());
            }
            if self.sim.elapsed() >= deadline {
                bail!(
                    "{}",
                    self.trace.format_failure_with_state(
                        &format!(
                            "SimWorld::wait_until timed out after {timeout_ms}ms virtual time"
                        ),
                        Some(&self.state_summary()),
                    )
                );
            }
            self.step_once()?;
        }
    }

    fn resolve_server_name<'a>(&'a self, name: &'a str) -> &'a str {
        if name == "local"
            && !self.servers.contains_key("local")
            && self.servers.contains_key("local-server")
        {
            "local-server"
        } else {
            name
        }
    }

    pub fn server(&self, name: &str) -> Option<Arc<AppServer>> {
        let key = self.resolve_server_name(name);
        self.servers.get(key).and_then(|s| s.server())
    }

    pub fn herdr(&self, name: &str) -> Option<Arc<SimHerdr>> {
        let key = self.resolve_server_name(name);
        self.servers.get(key).map(|s| Arc::clone(&s.herdr))
    }

    pub fn github_host(&self, name: &str) -> Option<Arc<SimGitHubHost>> {
        let key = self.resolve_server_name(name);
        self.servers.get(key).map(|s| Arc::clone(&s.github))
    }

    pub fn exec_runner(&self, name: &str) -> Option<Arc<SimExec>> {
        let key = self.resolve_server_name(name);
        self.servers.get(key).map(|s| Arc::clone(&s.exec))
    }

    /// Returns the per-host [`StorageFaultInjector`] attached to `host_name` (persists across
    /// `bounce_server` on the same host and is cleared by `heal_and_quiesce`).
    pub fn storage_faults(&self, host_name: &str) -> Option<&Arc<StorageFaultInjector>> {
        let key = self.resolve_server_name(host_name);
        self.servers.get(key).map(|s| &s.storage_faults)
    }

    /// Returns the per-host [`Failpoints`] registry attached to `host` (persists across
    /// `bounce_server` on the same host, mirrors arm/fire events into coverage telemetry,
    /// and is cleared by `heal_and_quiesce`). Panics if `host` is not a registered server host.
    pub fn failpoints(&self, host: &str) -> Arc<Failpoints> {
        self.try_failpoints(host)
            .unwrap_or_else(|| panic!("Unknown simulated server host '{host}'"))
    }

    /// Non-panicking lookup for the per-host [`Failpoints`] registry attached to `host`.
    pub fn try_failpoints(&self, host: &str) -> Option<Arc<Failpoints>> {
        let key = self.resolve_server_name(host);
        self.servers.get(key).map(|s| Arc::clone(&s.failpoints))
    }

    /// Returns the per-client [`Failpoints`] registry attached to `client` (persists across
    /// `bounce_client`, mirrors arm/fire events into coverage telemetry, and is cleared by
    /// `heal_and_quiesce`). Panics if `client` is not a registered simulated client.
    pub fn client_failpoints(&self, client: &str) -> Arc<Failpoints> {
        self.try_client_failpoints(client)
            .unwrap_or_else(|| panic!("Unknown simulated client host '{client}'"))
    }

    /// Non-panicking lookup for the per-client [`Failpoints`] registry attached to `client`.
    pub fn try_client_failpoints(&self, client: &str) -> Option<Arc<Failpoints>> {
        self.clients.get(client).map(|c| Arc::clone(&c.failpoints))
    }

    /// Returns the total number of remote connector dial attempts initiated by `client_id`
    /// toward `host_id` across the lifetime of this simulation world.
    pub fn client_dial_count(&self, client_id: &str, host_id: &str) -> u64 {
        self.clients
            .get(client_id)
            .and_then(|c| c.dial_counts.lock().unwrap().get(host_id).copied())
            .unwrap_or(0)
    }

    /// Traced direct RPC against a server host (`AppServer::call`), recording the actor in `trace`.
    pub fn server_call(&mut self, host: &str, method: &str, params: Value) -> Result<Value> {
        let key = self.resolve_server_name(host).to_owned();
        let srv = self
            .server(&key)
            .with_context(|| format!("Server host '{host}' is not running"))?;
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            &key,
            "server.rpc",
            method,
        );
        srv.call(method, params)
    }

    pub fn client(&self) -> Arc<MultiHostClient> {
        self.client_named(PRIMARY_CLIENT)
            .expect("ui-client initialized")
    }

    pub fn client_named(&self, client_id: &str) -> Option<Arc<MultiHostClient>> {
        self.clients.get(client_id).and_then(|c| c.client())
    }

    /// Returns the names of all live (non-crashed, initialized) simulated clients in deterministic order.
    pub fn client_names(&self) -> Vec<String> {
        self.clients
            .iter()
            .filter(|(_, h)| !h.crashed.load(Ordering::SeqCst) && h.client().is_some())
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub fn client_data_dir_named(&self, client_id: &str) -> Option<&Path> {
        self.clients.get(client_id).map(|c| c.data_dir.as_path())
    }

    pub fn drain_client_events(&self) -> Vec<ClientEvent> {
        self.drain_client_events_named(PRIMARY_CLIENT)
    }

    pub fn drain_client_events_named(&self, client_id: &str) -> Vec<ClientEvent> {
        let Some(handle) = self.clients.get(client_id) else {
            return Vec::new();
        };
        handle.events.lock().unwrap().drain(..).collect()
    }

    /// Executes an arbitrary typed async operation inside `client_id`'s Turmoil Tokio runtime,
    /// pumping `sim.step()` deterministically until the future resolves (or `timeout_ms` elapse).
    pub fn client_run_on<T, F, Fut>(&mut self, client_id: &str, timeout_ms: u64, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Arc<MultiHostClient>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        let tx_sender = self
            .clients
            .get(client_id)
            .and_then(|c| c.cmd_tx.lock().unwrap().clone())
            .with_context(|| {
                format!("Simulated client '{client_id}' is crashed or uninitialized")
            })?;
        let (tx, mut rx) = oneshot::channel::<Result<T>>();
        tx_sender
            .send(ClientCommand {
                run: Box::new(move |c| {
                    Box::pin(async move {
                        let _ = tx.send(f(c).await);
                    })
                }),
            })
            .map_err(|_| anyhow!("'{client_id}' command channel closed"))?;
        let deadline = self
            .sim
            .elapsed()
            .saturating_add(Duration::from_millis(timeout_ms));
        loop {
            if let Ok(res) = rx.try_recv() {
                return res;
            }
            if self.sim.elapsed() >= deadline {
                bail!(
                    "{}",
                    self.trace.format_failure_with_state(
                        &format!(
                            "SimWorld::client_run_on({client_id}) timed out after {timeout_ms}ms virtual time"
                        ),
                        Some(&self.state_summary()),
                    )
                );
            }
            self.step_once()?;
        }
    }

    /// Executes an async `Result<Value>` operation inside `client_id`'s Turmoil Tokio runtime.
    pub fn client_exec_on<F, Fut>(
        &mut self,
        client_id: &str,
        timeout_ms: u64,
        f: F,
    ) -> Result<Value>
    where
        F: FnOnce(Arc<MultiHostClient>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        self.client_run_on(client_id, timeout_ms, f)
    }

    /// Opens a raw authenticated `yamux/1` [`ClientTransport`] over `turmoil::net::TcpStream`
    /// from inside `client_id`'s Turmoil host (for `transport_auth` pairing, revocation, and
    /// Tailscale WhoIs testing).
    pub fn connect_raw_transport_on(
        &mut self,
        client_id: &str,
        addr: &str,
        token: &str,
        device_name: &str,
    ) -> Result<Arc<ClientTransport>> {
        let addr_owned = canban_types::normalize_daemon_addr(addr);
        let token_owned = token.to_owned();
        let dev_owned = device_name.to_owned();
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            client_id,
            "net.connect",
            format!("{addr_owned} dev={dev_owned}"),
        );
        self.client_run_on(client_id, 30_000, move |_| async move {
            let stream = turmoil::net::TcpStream::connect(addr_owned.as_str())
                .await
                .with_context(|| format!("turmoil connect {addr_owned}"))?;
            let transport =
                ClientTransport::connect_stream_async(stream, addr_owned, token_owned, dev_owned)
                    .await?;
            Ok(Arc::new(transport))
        })
    }

    /// Opens a raw authenticated `yamux/1` [`ClientTransport`] from `"ui-client"`'s Turmoil host.
    pub fn connect_raw_transport(
        &mut self,
        addr: &str,
        token: &str,
        device_name: &str,
    ) -> Result<Arc<ClientTransport>> {
        self.connect_raw_transport_on(PRIMARY_CLIENT, addr, token, device_name)
    }

    /// Executes an async operation inside `"ui-client"`'s Turmoil Tokio runtime.
    pub fn client_exec<F, Fut>(&mut self, timeout_ms: u64, f: F) -> Result<Value>
    where
        F: FnOnce(Arc<MultiHostClient>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        self.client_exec_on(PRIMARY_CLIENT, timeout_ms, f)
    }

    /// Spawns a non-blocking routed RPC on `client_id` without waiting for completion, returning
    /// a [`ClientCallHandle`] that can be awaited later (allowing concurrent in-flight RPC races).
    pub fn client_spawn_on(
        &mut self,
        client_id: &str,
        method: &str,
        params: Value,
        active_project_id: Option<&str>,
    ) -> Result<ClientCallHandle> {
        let tx_sender = self
            .clients
            .get(client_id)
            .and_then(|c| c.cmd_tx.lock().unwrap().clone())
            .with_context(|| {
                format!("Simulated client '{client_id}' is crashed or uninitialized")
            })?;
        let local_driver_opt = self
            .clients
            .get(client_id)
            .and_then(|c| c.local_driver.lock().unwrap().clone())
            .filter(|d| d.is_framed());
        let m = method.to_owned();
        let active = active_project_id.map(str::to_owned);
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            client_id,
            "client.spawn",
            format!("{m} has_active={}", active.is_some()),
        );
        self.hit("client.concurrent_spawn");
        let (tx, rx) = oneshot::channel();
        let m_run = m.clone();
        tx_sender
            .send(ClientCommand {
                run: Box::new(move |c| {
                    Box::pin(async move {
                        let res = if let Some(driver) = local_driver_opt
                            && c.resolve_target_host(&m_run, &params, active.as_deref()) == "local"
                            && !(m_run == "notifications.read_all"
                                && params["host_id"].is_null()
                                && params["project_id"]
                                    .as_str()
                                    .is_none_or(|s| s.trim().is_empty()))
                        {
                            let is_global_env =
                                m_run == "env.save" && params["scope"].as_str() == Some("global");
                            match driver.call_framed_async(&m_run, params).await {
                                Ok(val) => {
                                    let _ = driver.refresh_framed_cache_async().await;
                                    if is_global_env {
                                        c.wake_all();
                                    }
                                    Ok(val)
                                }
                                Err(err) => Err(err),
                            }
                        } else {
                            c.call_routed_async(&m_run, params, active.as_deref()).await
                        };
                        let _ = tx.send(res);
                    })
                }),
            })
            .map_err(|_| anyhow!("'{client_id}' command channel closed"))?;
        Ok(ClientCallHandle {
            client_id: client_id.to_owned(),
            method: m,
            rx,
        })
    }

    /// Spawns a non-blocking routed RPC on `"ui-client"`.
    pub fn client_spawn(
        &mut self,
        method: &str,
        params: Value,
        active_project_id: Option<&str>,
    ) -> Result<ClientCallHandle> {
        self.client_spawn_on(PRIMARY_CLIENT, method, params, active_project_id)
    }

    /// Pumps virtual time until a spawned [`ClientCallHandle`] completes or `timeout_ms` elapses.
    pub fn await_call(&mut self, mut handle: ClientCallHandle, timeout_ms: u64) -> Result<Value> {
        let deadline = self
            .sim
            .elapsed()
            .saturating_add(Duration::from_millis(timeout_ms));
        loop {
            if let Some(res) = handle.try_recv() {
                return res;
            }
            if self.sim.elapsed() >= deadline {
                bail!(
                    "{}",
                    self.trace.format_failure_with_state(
                        &format!(
                            "SimWorld::await_call({} on {}) timed out after {timeout_ms}ms virtual time",
                            handle.method, handle.client_id
                        ),
                        Some(&self.state_summary()),
                    )
                );
            }
            self.step_once()?;
        }
    }

    pub fn client_call_on(
        &mut self,
        client_id: &str,
        method: &str,
        params: Value,
        active_project_id: Option<&str>,
    ) -> Result<Value> {
        let m = method.to_owned();
        let active = active_project_id.map(str::to_owned);
        let local_driver_opt = self
            .clients
            .get(client_id)
            .and_then(|c| c.local_driver.lock().unwrap().clone())
            .filter(|d| d.is_framed());
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            client_id,
            "client.rpc",
            format!("{m} has_active={}", active.is_some()),
        );
        self.client_exec_on(client_id, 120_000, move |c| async move {
            if let Some(driver) = local_driver_opt
                && c.resolve_target_host(&m, &params, active.as_deref()) == "local"
                && !(m == "notifications.read_all"
                    && params["host_id"].is_null()
                    && params["project_id"]
                        .as_str()
                        .is_none_or(|s| s.trim().is_empty()))
            {
                let is_global_env = m == "env.save" && params["scope"].as_str() == Some("global");
                let val = driver.call_framed_async(&m, params).await?;
                if !matches!(
                    m.as_str(),
                    "app.ping"
                        | "app.state"
                        | "runtime.state"
                        | "schedule.list"
                        | "workflow.sources"
                        | "viewer.presence"
                        | "review.preview"
                ) {
                    let _ = driver.refresh_framed_cache_async().await;
                }
                if is_global_env {
                    c.wake_all();
                }
                return Ok(val);
            }
            c.call_routed_async(&m, params, active.as_deref()).await
        })
    }

    pub fn client_call(
        &mut self,
        method: &str,
        params: Value,
        active_project_id: Option<&str>,
    ) -> Result<Value> {
        self.client_call_on(PRIMARY_CLIENT, method, params, active_project_id)
    }

    pub fn client_call_all(&mut self, method: &str, params: Value) -> Result<Value> {
        let m = method.to_owned();
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            PRIMARY_CLIENT,
            "client.all",
            m.clone(),
        );
        self.client_exec(120_000, move |c| async move {
            let results = c.call_all_hosts_async(&m, params).await;
            let mut map = serde_json::Map::new();
            for (hid, res) in results {
                match res {
                    Ok(v) => {
                        map.insert(hid, json!({ "ok": v }));
                    }
                    Err(e) => {
                        map.insert(hid, json!({ "err": format!("{e:#}") }));
                    }
                }
            }
            Ok(Value::Object(map))
        })
    }

    pub fn client_drain_outbox(&mut self, host_id: &str) -> Result<Value> {
        let hid = host_id.to_owned();
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            PRIMARY_CLIENT,
            "client.drain",
            hid.clone(),
        );
        self.client_exec(120_000, move |c| async move {
            let report = c.drain_outbox_async(&hid).await?;
            Ok(json!({
                "applied": report.applied,
                "conflicts": report.conflicts,
                "failed": report.failed,
            }))
        })
    }

    /// Reads `"ui-client"`'s merged snapshot inside `"ui-client"`'s Turmoil runtime context.
    pub fn client_merged_snapshot(&self) -> Result<Snapshot> {
        self.client_merged_snapshot_on(PRIMARY_CLIENT)
    }

    pub fn client_merged_snapshot_on(&self, client_id: &str) -> Result<Snapshot> {
        let handle = self
            .clients
            .get(client_id)
            .with_context(|| format!("Unknown client '{client_id}'"))?;
        handle.with_runtime(MultiHostClient::merged_snapshot)?
    }

    /// Reads `"ui-client"`'s merged snapshot via an async round-trip inside `"ui-client"`'s Turmoil task.
    pub fn client_merged_snapshot_exec(&mut self) -> Result<Snapshot> {
        let local_driver_opt = self
            .clients
            .get(PRIMARY_CLIENT)
            .and_then(|c| c.local_driver.lock().unwrap().clone())
            .filter(|d| d.is_framed());
        let val = self.client_exec(30_000, move |c| async move {
            if let Some(driver) = local_driver_opt {
                driver.refresh_framed_cache_async().await?;
            }
            let snap = c.merged_snapshot()?;
            Ok(serde_json::to_value(snap)?)
        })?;
        Ok(serde_json::from_value(val)?)
    }

    pub fn client_merged_runtime(&self) -> Result<RuntimeState> {
        self.client_merged_runtime_on(PRIMARY_CLIENT)
    }

    pub fn client_merged_runtime_on(&self, client_id: &str) -> Result<RuntimeState> {
        let handle = self
            .clients
            .get(client_id)
            .with_context(|| format!("Unknown client '{client_id}'"))?;
        handle.with_runtime(MultiHostClient::merged_runtime_state)?
    }

    pub fn client_host_summaries(&self) -> Vec<HostSummary> {
        self.client_host_summaries_on(PRIMARY_CLIENT)
    }

    pub fn client_host_summaries_on(&self, client_id: &str) -> Vec<HostSummary> {
        self.clients
            .get(client_id)
            .and_then(|h| h.with_runtime(MultiHostClient::host_summaries).ok())
            .unwrap_or_default()
    }

    pub fn client_sync_items(&self) -> Vec<SyncItemView> {
        self.client_sync_items_on(PRIMARY_CLIENT)
    }

    pub fn client_sync_items_on(&self, client_id: &str) -> Vec<SyncItemView> {
        self.clients
            .get(client_id)
            .and_then(|h| h.with_runtime(MultiHostClient::sync_items).ok())
            .unwrap_or_default()
    }

    /// Executes `MultiHostClient::sync_action` inside `"ui-client"`'s Turmoil runtime.
    pub fn client_sync_action(
        &mut self,
        host_id: &str,
        mutation_id: &str,
        action: SyncAction,
    ) -> Result<()> {
        self.client_sync_action_on(PRIMARY_CLIENT, host_id, mutation_id, action)
    }

    pub fn client_sync_action_on(
        &mut self,
        client_id: &str,
        host_id: &str,
        mutation_id: &str,
        action: SyncAction,
    ) -> Result<()> {
        let hid = host_id.to_owned();
        let mid = mutation_id.to_owned();
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            client_id,
            "sync.action",
            format!("{hid} {action:?}"),
        );
        self.hit("client.sync_action");
        self.client_exec_on(client_id, 30_000, move |c| async move {
            c.sync_action(&hid, &mid, action)?;
            Ok(Value::Null)
        })?;
        Ok(())
    }

    /// Triggers `MultiHostClient::sync_global_env_now` inside `"ui-client"`'s Turmoil runtime.
    pub fn client_sync_global_env_now(&mut self) -> Result<()> {
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            PRIMARY_CLIENT,
            "env.sync",
            "trigger global env sync",
        );
        self.hit("env.sync_triggered");
        self.client_exec(30_000, |c| async move {
            c.sync_global_env_now();
            Ok(Value::Null)
        })?;
        Ok(())
    }

    /// Triggers `MultiHostClient::reconnect` inside `"ui-client"`'s Turmoil runtime.
    pub fn client_reconnect(&mut self, host_id: Option<&str>) -> Result<()> {
        let hid = host_id.map(str::to_owned);
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            PRIMARY_CLIENT,
            "client.reconn",
            format!("{hid:?}"),
        );
        self.client_exec(30_000, move |c| async move {
            c.reconnect(hid.as_deref());
            Ok(Value::Null)
        })?;
        Ok(())
    }

    /// Clears retry backoff for `host_id` inside `"ui-client"`'s Turmoil runtime.
    pub fn client_clear_retry_backoff(&mut self, host_id: &str) -> Result<()> {
        let hid = host_id.to_owned();
        self.client_exec(30_000, move |c| async move {
            c.clear_retry_backoff(&hid);
            Ok(Value::Null)
        })?;
        Ok(())
    }

    pub fn client_global_env_status(&self) -> GlobalEnvSyncStatus {
        self.client_global_env_status_on(PRIMARY_CLIENT)
    }

    pub fn client_global_env_status_on(&self, client_id: &str) -> GlobalEnvSyncStatus {
        self.clients
            .get(client_id)
            .and_then(|h| h.with_runtime(MultiHostClient::global_env_sync_status).ok())
            .unwrap_or_default()
    }

    // ---- Network & process fault injection (directly backed by Turmoil) ----

    pub fn partition(&mut self, a: &str, b: &str) {
        self.sim.partition(a, b);
        self.held_pairs.remove(&(a.to_owned(), b.to_owned()));
        self.held_pairs.remove(&(b.to_owned(), a.to_owned()));
        self.partitioned_pairs.insert((a.to_owned(), b.to_owned()));
        self.faulted_hosts.insert(a.to_owned());
        self.faulted_hosts.insert(b.to_owned());
        self.fault_count += 1;
        self.hit("fault.partition");
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            "net",
            "fault.part",
            format!("{a} <-> {b}"),
        );
    }

    pub fn partition_oneway(&mut self, from: &str, to: &str) {
        self.sim.partition_oneway(from, to);
        self.partitioned_pairs
            .insert((from.to_owned(), to.to_owned()));
        self.faulted_hosts.insert(from.to_owned());
        self.faulted_hosts.insert(to.to_owned());
        self.fault_count += 1;
        self.hit("fault.partition_oneway");
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            "net",
            "fault.1way",
            format!("{from} -/-> {to}"),
        );
    }

    pub fn repair(&mut self, a: &str, b: &str) {
        self.sim.repair(a, b);
        self.partitioned_pairs.remove(&(a.to_owned(), b.to_owned()));
        self.partitioned_pairs.remove(&(b.to_owned(), a.to_owned()));
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            "net",
            "fault.heal",
            format!("{a} <-> {b}"),
        );
    }

    pub fn hold(&mut self, a: &str, b: &str) {
        self.sim.hold(a, b);
        self.partitioned_pairs.remove(&(a.to_owned(), b.to_owned()));
        self.partitioned_pairs.remove(&(b.to_owned(), a.to_owned()));
        self.held_pairs.insert((a.to_owned(), b.to_owned()));
        self.faulted_hosts.insert(a.to_owned());
        self.faulted_hosts.insert(b.to_owned());
        self.fault_count += 1;
        self.hit("fault.hold");
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            "net",
            "fault.hold",
            format!("{a} <-> {b}"),
        );
    }

    pub fn release(&mut self, a: &str, b: &str) {
        self.sim.release(a, b);
        self.held_pairs.remove(&(a.to_owned(), b.to_owned()));
        self.held_pairs.remove(&(b.to_owned(), a.to_owned()));
        self.partitioned_pairs.remove(&(a.to_owned(), b.to_owned()));
        self.partitioned_pairs.remove(&(b.to_owned(), a.to_owned()));
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            "net",
            "fault.rel",
            format!("{a} <-> {b}"),
        );
    }

    pub fn set_fail_rate(&mut self, rate: f64) {
        self.sim.set_fail_rate(rate);
        if rate > 0.0 {
            self.global_net_faulted = true;
            self.fault_count += 1;
            self.hit("fault.fail_rate");
        }
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            "net",
            "fault.rate",
            format!("fail_rate={rate:.3}"),
        );
    }

    pub fn set_link_latency(&mut self, a: &str, b: &str, latency: Duration) {
        self.sim.set_link_latency(a, b, latency);
        self.hit("fault.link_latency");
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            "net",
            "fault.lat",
            format!("{a} <-> {b} latency={}ms", latency.as_millis()),
        );
    }

    pub fn set_link_max_message_latency(&mut self, a: &str, b: &str, latency: Duration) {
        self.sim.set_link_max_message_latency(a, b, latency);
        self.hit("fault.link_latency");
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            "net",
            "fault.maxlat",
            format!("{a} <-> {b} max_latency={}ms", latency.as_millis()),
        );
    }

    pub fn set_link_fail_rate(&mut self, a: &str, b: &str, rate: f64) {
        self.sim.set_link_fail_rate(a, b, rate);
        if rate > 0.0 {
            self.link_fail_pairs.insert((a.to_owned(), b.to_owned()));
            self.faulted_hosts.insert(a.to_owned());
            self.faulted_hosts.insert(b.to_owned());
            self.fault_count += 1;
            self.hit("fault.link_fail_rate");
        } else {
            self.link_fail_pairs.remove(&(a.to_owned(), b.to_owned()));
            self.link_fail_pairs.remove(&(b.to_owned(), a.to_owned()));
        }
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            "net",
            "fault.lrate",
            format!("{a} <-> {b} fail_rate={rate:.3}"),
        );
    }

    /// Repairs all network partitions, holds, global/link fail rates, and byte-stream faults.
    pub fn repair_all(&mut self) {
        self.sim.set_fail_rate(0.0);
        let link_fails: Vec<(String, String)> = self.link_fail_pairs.iter().cloned().collect();
        for (a, b) in link_fails {
            self.sim.set_link_fail_rate(a.as_str(), b.as_str(), 0.0);
        }
        self.link_fail_pairs.clear();
        let parts: Vec<(String, String)> = self.partitioned_pairs.iter().cloned().collect();
        for (a, b) in parts {
            self.repair(&a, &b);
        }
        let holds: Vec<(String, String)> = self.held_pairs.iter().cloned().collect();
        for (a, b) in holds {
            self.release(&a, &b);
        }
        self.stream_fault_policy.clear_all();
    }

    /// Abruptly crashes an `AppServer` host (`SIGKILL` semantics: sets `SimRunning::set_abrupt_crash(true)`
    /// to skip graceful `finish_agent_work()`, drops its Tokio runtime & sockets, and preserves SQLite).
    pub fn crash_server(&mut self, name: &str) -> Result<()> {
        let name = self.resolve_server_name(name).to_owned();
        let Some(handle) = self.servers.get(&name) else {
            bail!("Unknown server host '{name}'");
        };
        if let Some(flag) = handle.abrupt_crash.lock().unwrap().as_ref() {
            flag.store(true, Ordering::SeqCst);
        }
        if let Some(srv) = handle.server_slot.lock().unwrap().take() {
            srv.set_abrupt_crash(true);
            drop(srv);
        }
        handle.crashed.store(true, Ordering::SeqCst);
        self.faulted_hosts.insert(name.clone());
        self.sim.crash(name.as_str());
        self.fault_count += 1;
        self.hit("fault.server_crash_abrupt");
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            &name,
            "fault.crash",
            "abrupt SIGKILL",
        );
        self.step_once()?;
        Ok(())
    }

    /// Gracefully stops an `AppServer` host (runs `SimRunning` graceful teardown including `finish_agent_work()`).
    pub fn stop_server_graceful(&mut self, name: &str) -> Result<()> {
        let name = self.resolve_server_name(name).to_owned();
        let Some(handle) = self.servers.get(&name) else {
            bail!("Unknown server host '{name}'");
        };
        if let Some(flag) = handle.abrupt_crash.lock().unwrap().as_ref() {
            flag.store(false, Ordering::SeqCst);
        }
        if let Some(srv) = handle.server_slot.lock().unwrap().take() {
            srv.set_abrupt_crash(false);
            drop(srv);
        }
        handle.crashed.store(true, Ordering::SeqCst);
        self.sim.crash(name.as_str());
        self.hit("fault.server_stop_graceful");
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            &name,
            "fault.stop",
            "graceful",
        );
        self.step_once()?;
        Ok(())
    }

    /// Restarts a crashed (or running) `AppServer` host on the same SQLite directory.
    /// Accepts `"local"` as an alias for `"local-server"`.
    pub fn bounce_server(&mut self, name: &str) -> Result<()> {
        let name = self.resolve_server_name(name).to_owned();
        let Some(handle) = self.servers.get(&name) else {
            bail!("Unknown server host '{name}'");
        };
        if let Some(flag) = handle.abrupt_crash.lock().unwrap().as_ref() {
            flag.store(true, Ordering::SeqCst);
        }
        if let Some(srv) = handle.server_slot.lock().unwrap().take() {
            srv.set_abrupt_crash(true);
            drop(srv);
        }
        handle.crashed.store(false, Ordering::SeqCst);
        handle.ever_bounced.store(true, Ordering::SeqCst);
        self.faulted_hosts.insert(name.clone());
        self.sim.bounce(name.as_str());
        self.hit("fault.server_bounce");
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            &name,
            "fault.bounce",
            "restart",
        );
        for _ in 0..400 {
            self.step_once()?;
            if self.servers[&name].server().is_some() {
                if name == "local-server" {
                    let local_drivers: Vec<(String, Arc<SimLocalHostDriver>)> = self
                        .clients
                        .iter()
                        .filter(|(_, h)| !h.crashed.load(Ordering::SeqCst) && h.client().is_some())
                        .filter_map(|(cname, h)| {
                            h.local_driver
                                .lock()
                                .unwrap()
                                .clone()
                                .map(|d| (cname.clone(), d))
                        })
                        .collect();
                    for (cname, driver) in local_drivers {
                        let _ = self.client_exec_on(&cname, 10_000, move |_| async move {
                            driver.refresh_transport().await?;
                            if driver.is_framed() {
                                driver.reconnect_framed_async().await?;
                            }
                            Ok(Value::Null)
                        });
                    }
                }
                return Ok(());
            }
        }
        bail!("Server '{name}' did not finish booting after bounce");
    }

    /// Abruptly crashes `"ui-client"` while preserving `client_data_dir` (including `.outbox.json`).
    pub fn crash_client(&mut self) -> Result<()> {
        self.crash_client_named(PRIMARY_CLIENT)
    }

    /// Abruptly crashes a named simulated client host while preserving its on-disk `data_dir`.
    pub fn crash_client_named(&mut self, client_name: &str) -> Result<()> {
        let Some(handle) = self.clients.get(client_name) else {
            bail!("Unknown client host '{client_name}'");
        };
        handle.cmd_tx.lock().unwrap().take();
        handle.local_driver.lock().unwrap().take();
        handle.rt_handle.lock().unwrap().take();
        if let Some(c) = handle.client_slot.lock().unwrap().take() {
            drop(c);
        }
        handle.crashed.store(true, Ordering::SeqCst);
        self.sim.crash(client_name);
        self.fault_count += 1;
        self.hit("fault.client_crash");
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            client_name,
            "fault.ccrash",
            "abrupt client crash",
        );
        self.step_once()?;
        Ok(())
    }

    /// Restarts `"ui-client"` on the same `client_data_dir` (recovering persisted `.outbox.json` items).
    pub fn bounce_client(&mut self) -> Result<()> {
        self.bounce_client_named(PRIMARY_CLIENT)
    }

    /// Restarts a named simulated client host on its persisted `data_dir`.
    pub fn bounce_client_named(&mut self, client_name: &str) -> Result<()> {
        let Some(handle) = self.clients.get(client_name) else {
            bail!("Unknown client host '{client_name}'");
        };
        handle.cmd_tx.lock().unwrap().take();
        handle.local_driver.lock().unwrap().take();
        handle.rt_handle.lock().unwrap().take();
        if let Some(c) = handle.client_slot.lock().unwrap().take() {
            drop(c);
        }
        handle.crashed.store(false, Ordering::SeqCst);
        self.sim.bounce(client_name);
        self.hit("fault.client_bounce");
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            client_name,
            "fault.cbounce",
            "client restart",
        );
        for _ in 0..600 {
            self.step_once()?;
            if self.clients[client_name].client().is_some() {
                return Ok(());
            }
        }
        bail!("Client '{client_name}' did not finish booting after bounce");
    }

    fn all_clients_quiesced(&self) -> bool {
        for (client_name, handle) in &self.clients {
            if handle.crashed.load(Ordering::SeqCst) || handle.client().is_none() {
                return false;
            }
            let summaries = self.client_host_summaries_on(client_name);
            let all_connected = self.config.remote_hosts.iter().all(|name| {
                summaries
                    .iter()
                    .find(|s| s.id == *name)
                    .is_some_and(|s| s.is_connected())
            });
            if !all_connected {
                return false;
            }
            let items = self.client_sync_items_on(client_name);
            fn raw_card(id: &str) -> &str {
                canban_types::split_qualified_host_id(id)
                    .map(|(_, raw)| raw)
                    .unwrap_or(id)
            }
            // Per-card FIFO outbox items queued behind a `Conflict` or `Failed` item on the same
            // `(host_id, card_id)` cannot drain until the head item is resolved/discarded.
            let blocked_cards: BTreeSet<(&str, &str)> = items
                .iter()
                .filter(|it| matches!(it.state, SyncItemState::Conflict | SyncItemState::Failed))
                .map(|it| (it.host_id.as_str(), raw_card(&it.card_id)))
                .collect();
            let pending_active = items.iter().any(|it| match it.state {
                SyncItemState::InFlight => true,
                SyncItemState::Queued => {
                    !blocked_cards.contains(&(it.host_id.as_str(), raw_card(&it.card_id)))
                }
                SyncItemState::Conflict | SyncItemState::Failed => false,
            });
            if pending_active {
                return false;
            }
            let env_status = self.client_global_env_status_on(client_name);
            if !env_status.queued_hosts.is_empty() || !env_status.syncing_hosts.is_empty() {
                return false;
            }
        }
        true
    }

    fn all_servers_herdr_observed(&self) -> bool {
        self.servers.values().all(|h| {
            !h.crashed.load(Ordering::SeqCst)
                && h.server().is_some()
                && h.herdr.subscriber_count() > 0
        })
    }

    fn all_framed_local_caches_converged(&self) -> bool {
        if !self.config.framed_local_transport {
            return true;
        }
        let Some(local_srv) = self.server("local-server") else {
            return false;
        };
        let Ok(state) = local_srv.call("app.state", json!({})) else {
            return false;
        };
        // Compare `[identity, state_instance, db_changes, state_revision]` while ignoring element
        // `[4]` (`observed_at`), which ticks silently on periodic 15s Herdr heartbeats without
        // emitting a `StateChanged` push event when no snapshot state changed.
        fn revision_state_prefix(rev: &Value) -> Option<&[Value]> {
            rev.as_array().map(|arr| &arr[..arr.len().min(4)])
        }
        let Some(expected_prefix) = revision_state_prefix(&state["revision"]) else {
            return false;
        };
        for handle in self.clients.values() {
            if let Some(driver) = handle.local_driver.lock().unwrap().clone()
                && driver.is_framed()
            {
                let cached = driver.framed_cached_revision();
                if cached.as_ref().and_then(revision_state_prefix) != Some(expected_prefix) {
                    return false;
                }
            }
        }
        true
    }

    /// Waits for natural unassisted reconnect, outbox drain, framed local watcher convergence,
    /// Herdr observation, and global env sync WITHOUT forcefully kicking `reconnect`,
    /// `clear_retry_backoff`, or `poll_remotes_once_async`.
    /// Returns `Err(...)` with full diagnostic state if convergence does not complete in time.
    pub fn quiesce_natural(&mut self, max_virtual_ms: u64) -> Result<()> {
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            "world",
            "quiesce.nat",
            format!("max_ms={max_virtual_ms}"),
        );
        let deadline = self
            .sim
            .elapsed()
            .saturating_add(Duration::from_millis(max_virtual_ms));
        let mut stable_checks = 0u32;
        let mut iter = 0u32;
        let mut converged = false;
        while self.sim.elapsed() < deadline {
            self.step_once()?;
            iter += 1;
            if !iter.is_multiple_of(5) {
                continue;
            }
            if self.all_clients_quiesced()
                && self.all_servers_herdr_observed()
                && self.all_framed_local_caches_converged()
            {
                stable_checks += 1;
                if stable_checks >= 5 {
                    converged = true;
                    break;
                }
            } else {
                stable_checks = 0;
            }
        }
        if !converged {
            bail!(
                "{}",
                self.trace.format_failure_with_state(
                    &format!(
                        "SimWorld::quiesce_natural timed out after {max_virtual_ms}ms virtual time without unassisted convergence"
                    ),
                    Some(&self.state_summary()),
                )
            );
        }
        self.hit("world.quiesce_natural");
        Ok(())
    }

    /// Heals all temporary network/GitHub/Herdr/stream/clock/failpoint faults and restarts any crashed
    /// servers or clients, wakes client reconnect loops inside their Turmoil runtimes, and
    /// advances virtual time until all enabled remote hosts reconnect, drain pending outbox
    /// items, and converge.
    ///
    /// Returns `Err(...)` with a full diagnostic dump if convergence does not complete in time.
    pub fn heal_and_quiesce(&mut self, max_virtual_ms: u64) -> Result<()> {
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            "world",
            "world.heal",
            format!("max_ms={max_virtual_ms}"),
        );
        self.repair_all();
        self.github.clear_injected_faults();
        for h in self.servers.values() {
            h.storage_faults.clear();
            h.failpoints.clear();
        }
        for c in self.clients.values() {
            c.failpoints.clear();
        }

        let crashed_servers: Vec<String> = self
            .servers
            .iter()
            .filter(|(_, h)| h.crashed.load(Ordering::SeqCst))
            .map(|(n, _)| n.clone())
            .collect();
        for name in crashed_servers {
            self.bounce_server(&name)?;
        }
        for h in self.servers.values() {
            h.clock.reset_adjustments();
            h.herdr.clear_injected_faults();
            h.herdr.set_offline(false);
        }

        let crashed_clients: Vec<String> = self
            .clients
            .iter()
            .filter(|(_, h)| h.crashed.load(Ordering::SeqCst))
            .map(|(n, _)| n.clone())
            .collect();
        for name in crashed_clients {
            self.bounce_client_named(&name)?;
        }

        // Wake client hosts INSIDE their own Turmoil Tokio runtimes (never evaluating Instant::now
        // on the driver thread). Only force `reconnect(Some(name))` for hosts that experienced
        // network/process faults or are currently disconnected; healthy untouched hosts are merely
        // woken without tearing down their active sessions.
        let faulted_hosts = std::mem::take(&mut self.faulted_hosts);
        let global_faulted = std::mem::replace(&mut self.global_net_faulted, false);
        let client_names: Vec<String> = self.clients.keys().cloned().collect();
        let remotes = self.config.remote_hosts.clone();
        let mut had_reconnect = false;
        for cname in &client_names {
            let summaries = self.client_host_summaries_on(cname);
            let reconnect_targets: Vec<String> = remotes
                .iter()
                .filter(|name| {
                    let disconnected = !summaries
                        .iter()
                        .find(|s| s.id == **name)
                        .is_some_and(|s| s.is_connected());
                    global_faulted || faulted_hosts.contains(*name) || disconnected
                })
                .cloned()
                .collect();
            if !reconnect_targets.is_empty() {
                had_reconnect = true;
            }
            let remotes_c = remotes.clone();
            self.client_exec_on(cname, 30_000, move |c| async move {
                c.on_foreground();
                for name in &reconnect_targets {
                    c.reconnect(Some(name));
                }
                for name in &remotes_c {
                    c.clear_retry_backoff(name);
                }
                c.wake_all();
                Ok(Value::Null)
            })?;
        }

        // A supervisor parked on a half-open Turmoil stream may wait up to one heartbeat interval
        // (~30s virtual time) before observing teardown and redialing; similarly a Herdr observer
        // waking from reconnect backoff may need up to ~30s virtual time. Ensure sufficient
        // virtual budget while still failing loudly if convergence stalls.
        let needs_herdr_reconnect = !self.all_servers_herdr_observed();
        let budget_ms = if had_reconnect {
            max_virtual_ms.max(120_000)
        } else if needs_herdr_reconnect {
            max_virtual_ms.max(35_000)
        } else {
            max_virtual_ms.max(1)
        };
        let deadline = self
            .sim
            .elapsed()
            .saturating_add(Duration::from_millis(budget_ms));
        let mut stable_checks = 0u32;
        let mut iter = 0u32;
        let mut converged = false;
        while self.sim.elapsed() < deadline {
            self.step_once()?;
            iter += 1;
            if !iter.is_multiple_of(5) {
                continue;
            }
            if self.all_clients_quiesced() && self.all_servers_herdr_observed() {
                stable_checks += 1;
                if stable_checks >= 5 {
                    converged = true;
                    break;
                }
            } else {
                stable_checks = 0;
            }
        }
        if !converged {
            bail!(
                "{}",
                self.trace.format_failure_with_state(
                    &format!(
                        "SimWorld::heal_and_quiesce timed out after {budget_ms}ms virtual time without convergence"
                    ),
                    Some(&self.state_summary()),
                )
            );
        }

        // Let any just-emitted Herdr/server events settle before the final synchronous client poll,
        // and do NOT advance virtual time afterward so no background timer mutates server state
        // between the final poll and subsequent quiescent oracle assertions.
        self.advance(Duration::from_millis(200))?;
        if client_names.len() <= 1 {
            for cname in &client_names {
                let local_driver_opt = self
                    .clients
                    .get(cname)
                    .and_then(|h| h.local_driver.lock().unwrap().clone())
                    .filter(|d| d.is_framed());
                self.client_exec_on(cname, 15_000, move |c| async move {
                    c.poll_remotes_once_async().await;
                    if let Some(driver) = local_driver_opt {
                        driver.refresh_framed_cache_async().await?;
                    }
                    Ok(json!({}))
                })?;
            }
        } else {
            // Multi-client worlds: dispatch the final poll to ALL clients in the exact same
            // virtual tick and verify server revisions stayed stable across the poll so a periodic
            // background tick (e.g. `github_sync_loop`) firing mid-poll cannot leave one client
            // 1 RTT behind another.
            let capture_server_revs = |w: &SimWorld| -> BTreeMap<String, Value> {
                let mut revs = BTreeMap::new();
                for (host, h) in &w.servers {
                    if let Some(srv) = h.server()
                        && let Ok(st) = srv.call("app.state", json!({}))
                    {
                        revs.insert(host.clone(), st["revision"].clone());
                    }
                }
                revs
            };
            for attempt in 0..3 {
                let revs_before = capture_server_revs(self);
                let mut pending = Vec::with_capacity(client_names.len());
                for cname in &client_names {
                    let Some(handle) = self.clients.get(cname) else {
                        continue;
                    };
                    let Some(tx) = handle.cmd_tx.lock().unwrap().clone() else {
                        continue;
                    };
                    let local_driver_opt = handle
                        .local_driver
                        .lock()
                        .unwrap()
                        .clone()
                        .filter(|d| d.is_framed());
                    let (resp_tx, resp_rx) = oneshot::channel();
                    tx.send(ClientCommand {
                        run: Box::new(move |client| {
                            Box::pin(async move {
                                client.poll_remotes_once_async().await;
                                let res = if let Some(driver) = local_driver_opt {
                                    driver
                                        .refresh_framed_cache_async()
                                        .await
                                        .map(|()| json!({}))
                                } else {
                                    Ok(json!({}))
                                };
                                let _ = resp_tx.send(res);
                            })
                        }),
                    })
                    .map_err(|_| anyhow!("Client '{cname}' command channel closed"))?;
                    pending.push((cname.clone(), resp_rx));
                }
                let poll_deadline = self
                    .sim
                    .elapsed()
                    .saturating_add(Duration::from_millis(15_000));
                while !pending.is_empty() && self.sim.elapsed() < poll_deadline {
                    self.step_once()?;
                    let mut i = 0;
                    while i < pending.len() {
                        match pending[i].1.try_recv() {
                            Ok(Ok(_)) => {
                                pending.remove(i);
                            }
                            Ok(Err(e)) => return Err(e),
                            Err(oneshot::error::TryRecvError::Empty) => {
                                i += 1;
                            }
                            Err(oneshot::error::TryRecvError::Closed) => {
                                bail!(
                                    "Client '{}' dropped final heal_and_quiesce poll channel",
                                    pending[i].0
                                );
                            }
                        }
                    }
                }
                if !pending.is_empty() {
                    bail!(
                        "Final multi-client heal_and_quiesce poll timed out after 15000ms (attempt {})",
                        attempt + 1
                    );
                }
                if capture_server_revs(self) == revs_before {
                    break;
                }
            }
        }
        self.hit("world.heal_and_quiesce");
        Ok(())
    }

    /// Produces a compact diagnostic summary of servers, clients, host connections, and outboxes.
    pub fn state_summary(&self) -> String {
        let mut out = format!(
            "elapsed={}ms steps={} faults={} partitions={:?} holds={:?}\n",
            self.elapsed_ms(),
            self.step_count,
            self.fault_count,
            self.partitioned_pairs,
            self.held_pairs
        );
        for (name, h) in &self.servers {
            let crashed = h.crashed.load(Ordering::SeqCst);
            if let Some(srv) = h.server() {
                match srv.snapshot() {
                    Ok(snap) => out.push_str(&format!(
                        "  server[{name}]: UP projects={} cards={} wf_runs={} terminals={} notices={} reminders={} herdr_subs={} exec_cmds={}\n",
                        snap.projects.len(),
                        snap.cards.len(),
                        snap.workflow_runs.len(),
                        snap.terminals.len(),
                        snap.notices.len(),
                        snap.reminders.len(),
                        h.herdr.subscriber_count(),
                        h.exec.command_count(),
                    )),
                    Err(e) => out.push_str(&format!("  server[{name}]: UP (snapshot err: {e})\n")),
                }
            } else {
                out.push_str(&format!("  server[{name}]: DOWN (crashed={crashed})\n"));
            }
        }
        for (cname, h) in &self.clients {
            let crashed = h.crashed.load(Ordering::SeqCst);
            if h.client().is_none() {
                out.push_str(&format!("  client[{cname}]: DOWN (crashed={crashed})\n"));
                continue;
            }
            let summaries = self.client_host_summaries_on(cname);
            let items = self.client_sync_items_on(cname);
            let env_st = self.client_global_env_status_on(cname);
            out.push_str(&format!(
                "  client[{cname}]: UP hosts={:?} sync_items={} env_queued={:?} env_syncing={:?} env_failed={:?}\n",
                summaries
                    .iter()
                    .map(|s| format!("{}:{:?}", s.id, s.connection))
                    .collect::<Vec<_>>(),
                items.len(),
                env_st.queued_hosts,
                env_st.syncing_hosts,
                env_st.failed_hosts,
            ));
            for it in items.iter().take(10) {
                out.push_str(&format!(
                    "    outbox[{}:{}] kind={} card={} state={:?} msg={:?}\n",
                    it.host_id, it.mutation_id, it.kind, it.card_id, it.state, it.message
                ));
            }
        }
        out
    }

    /// Enriches an error with structured replay info, state summary, and recent trace events.
    pub fn annotate_err(&self, err: anyhow::Error) -> anyhow::Error {
        let msg = format!("{err:#}");
        if msg.contains("=== CANBAN DST FAILURE ===") {
            return err;
        }
        anyhow!(
            "{}",
            self.trace
                .format_failure_with_state(&msg, Some(&self.state_summary()))
        )
    }
}

impl Drop for SimWorld {
    fn drop(&mut self) {
        self.flush_coverage();
    }
}

/// Runs a simulation closure with a constructed [`SimWorld`], automatically attaching full trace
/// & state diagnostics on error and flushing property coverage on exit.
pub fn run_with_world<T, F>(config: SimWorldConfig, f: F) -> Result<T>
where
    F: FnOnce(&mut SimWorld) -> Result<T>,
{
    let mut world = SimWorld::new(config)?;
    let res = f(&mut world);
    world.flush_coverage();
    match res {
        Ok(v) => Ok(v),
        Err(err) => Err(world.annotate_err(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn same_seed_produces_identical_server_ids_tokens_and_post_bounce_ids() {
        let mut world_a = SimWorld::new(SimWorldConfig::desktop_default(202_677, "unit_det_ids_a"))
            .expect("world_a init");
        let mut world_b = SimWorld::new(SimWorldConfig::desktop_default(202_677, "unit_det_ids_b"))
            .expect("world_b init");
        let world_diff = SimWorld::new(SimWorldConfig::desktop_default(
            202_678,
            "unit_det_ids_diff",
        ))
        .expect("world_diff init");

        let mut distinct_ids = BTreeSet::new();
        for host in ["local-server", "remote-a", "remote-b"] {
            let srv_a = world_a.server(host).expect("srv_a");
            let srv_b = world_b.server(host).expect("srv_b");
            let srv_diff = world_diff.server(host).expect("srv_diff");

            let state_a = srv_a.call("app.state", json!({})).expect("state_a");
            let state_b = srv_b.call("app.state", json!({})).expect("state_b");
            let state_diff = srv_diff.call("app.state", json!({})).expect("state_diff");
            let inst_a = state_a["revision"][1].as_str().expect("state_instance_a");
            let inst_b = state_b["revision"][1].as_str().expect("state_instance_b");
            let inst_diff = state_diff["revision"][1]
                .as_str()
                .expect("state_instance_diff");

            assert_eq!(
                srv_a.server_id(),
                srv_b.server_id(),
                "[{host}] server_id must be identical for the same seed"
            );
            assert_eq!(
                inst_a, inst_b,
                "[{host}] state_instance must be identical for the same seed"
            );
            assert_ne!(
                inst_a, inst_diff,
                "[{host}] different seeds must produce distinct state_instances"
            );
            assert_eq!(
                world_a.servers[host].token(),
                world_b.servers[host].token(),
                "[{host}] issued device token must be identical for the same seed"
            );
            assert_ne!(
                srv_a.server_id(),
                srv_diff.server_id(),
                "[{host}] different seeds must produce distinct server_ids"
            );
            assert!(
                distinct_ids.insert(srv_a.server_id().to_owned()),
                "[{host}] each host in a world must derive a unique server_id"
            );
            assert_eq!(
                world_a.servers[host].boot_generation.load(Ordering::SeqCst),
                1
            );
        }

        // Create a project and card directly on remote-a in both worlds -> identical deterministic IDs
        // generated by the host's injected `DeterministicIdGen`.
        let dir_a = world_a.project_dir("det-repo").expect("dir_a");
        let dir_b = world_b.project_dir("det-repo").expect("dir_b");
        let pa = world_a
            .server_call(
                "remote-a",
                "project.create",
                json!({ "name": "Det", "directory": dir_a }),
            )
            .expect("pa");
        let pb = world_b
            .server_call(
                "remote-a",
                "project.create",
                json!({ "name": "Det", "directory": dir_b }),
            )
            .expect("pb");
        let pid_a = pa["id"].as_str().unwrap().to_owned();
        let pid_b = pb["id"].as_str().unwrap().to_owned();
        assert_eq!(pid_a, pid_b, "project ID must be deterministic");

        let pre_card_a = world_a
            .server_call(
                "remote-a",
                "card.create",
                json!({ "project_id": pid_a, "title": "Pre-bounce" }),
            )
            .expect("pre_card_a")["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let pre_card_b = world_b
            .server_call(
                "remote-a",
                "card.create",
                json!({ "project_id": pid_b, "title": "Pre-bounce" }),
            )
            .expect("pre_card_b")["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(pre_card_a, pre_card_b, "card ID must be deterministic");

        // Bounce remote-a in both worlds: persistent server_id stays the same, boot_generation
        // advances, state_instance & newly created card IDs are non-colliding AND identical across
        // both same-seed worlds!
        let pre_server_id = world_a.server("remote-a").unwrap().server_id().to_owned();
        let pre_state_instance = world_a
            .server("remote-a")
            .unwrap()
            .call("app.state", json!({}))
            .unwrap()["revision"][1]
            .as_str()
            .unwrap()
            .to_owned();

        world_a.bounce_server("remote-a").expect("bounce a");
        world_b.bounce_server("remote-a").expect("bounce b");
        world_a.heal_and_quiesce(15_000).expect("heal a");
        world_b.heal_and_quiesce(15_000).expect("heal b");

        assert_eq!(
            world_a.servers["remote-a"]
                .boot_generation
                .load(Ordering::SeqCst),
            2
        );
        assert_eq!(
            world_a.server("remote-a").unwrap().server_id(),
            pre_server_id,
            "server_id must persist across bounce"
        );
        assert_eq!(
            world_b.server("remote-a").unwrap().server_id(),
            pre_server_id
        );
        let post_state_a = world_a
            .server("remote-a")
            .unwrap()
            .call("app.state", json!({}))
            .unwrap()["revision"][1]
            .as_str()
            .unwrap()
            .to_owned();
        let post_state_b = world_b
            .server("remote-a")
            .unwrap()
            .call("app.state", json!({}))
            .unwrap()["revision"][1]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(
            post_state_a, pre_state_instance,
            "post-bounce boot generation must derive a distinct state_instance"
        );
        assert_eq!(
            post_state_a, post_state_b,
            "post-bounce state_instance must remain deterministic across same-seed runs"
        );

        let post_card_a = world_a
            .server_call(
                "remote-a",
                "card.create",
                json!({ "project_id": pid_a, "title": "Post-bounce" }),
            )
            .expect("post_card_a")["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let post_card_b = world_b
            .server_call(
                "remote-a",
                "card.create",
                json!({ "project_id": pid_b, "title": "Post-bounce" }),
            )
            .expect("post_card_b")["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(
            post_card_a, pre_card_a,
            "post-bounce card ID must not collide with pre-bounce card ID"
        );
        assert_eq!(
            post_card_a, post_card_b,
            "post-bounce card ID must be identical across same-seed runs"
        );
    }

    #[test]
    fn sim_world_boot_and_agent_launch_incur_zero_login_shell_probe() {
        // Drop any cached host login-shell snapshot so any accidental call into
        // `canban_exec::local_shell_env_snapshot()` / `local_search_paths()` would trigger a real
        // interactive login-shell probe (~3s) and inject host env values instead of SimExec's.
        canban_exec::invalidate_local_shell_env_cache();
        let started = Instant::now();

        let mut cfg = SimWorldConfig::desktop_default(202_699, "unit_zero_shell_probe");
        cfg.remote_hosts = vec!["remote-a".into()];
        let mut world = SimWorld::new(cfg).expect("world init");

        // Exercise host-env, catalog, diagnostics, and env preview RPCs on remote-a.
        let srv = world.server("remote-a").expect("remote-a server");
        let refreshed = srv
            .call("host.env.refresh", json!({}))
            .expect("host.env.refresh");
        assert_eq!(
            refreshed["shell"].as_str(),
            Some("/bin/sh"),
            "host.env.refresh must read SimExec::shell_env(), not host login shell"
        );
        assert_eq!(refreshed["var_count"].as_u64(), Some(4));
        let _ = srv.call("agent.catalog", json!({})).expect("agent.catalog");
        let _ = srv
            .call("diagnostics.status", json!({}))
            .expect("diagnostics.status");
        let preview = srv.call("env.preview", json!({})).expect("env.preview");
        let preview_path = preview["rows"]
            .as_array()
            .and_then(|rows| rows.iter().find(|r| r["key"].as_str() == Some("PATH")))
            .and_then(|r| r["display_value"].as_str());
        assert_eq!(
            preview_path,
            Some("/sim/bin:/usr/bin:/bin"),
            "env.preview must resolve PATH from SimExec::shell_env(), not host login shell"
        );

        // Launch a real agent terminal through `terminal.create` (which exercises both
        // `canban_profiles::executable_with_runner` and
        // `canban_herdr::Client::launch_with_policy_unset_and_size`).
        let dir = world.project_dir("zero-probe-proj").expect("project_dir");
        let proj = world
            .client_call(
                "project.create",
                json!({ "host_id": "remote-a", "name": "ZeroProbe", "directory": dir }),
                None,
            )
            .expect("project.create");
        let pid = proj["id"].as_str().unwrap().to_owned();
        let _ = world
            .client_call(
                "terminal.create",
                json!({
                    "project_id": pid,
                    "agent": true,
                    "profile_id": "developer",
                    "prompt": "verify zero shell probe"
                }),
                Some(&pid),
            )
            .expect("terminal.create");

        let herdr = world.herdr("remote-a").expect("remote-a herdr");
        let addrs = herdr.terminal_addresses();
        assert!(
            !addrs.is_empty(),
            "expected terminal.create to launch a simulated terminal in SimHerdr"
        );
        assert!(
            world.servers["remote-a"].exec.blocked_commands().is_empty(),
            "unexpected OS commands blocked during zero-probe test: {:?}",
            world.servers["remote-a"].exec.blocked_commands()
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "cold-start SimWorld + terminal launch took {:?} (>= 2s implies a real shell probe leaked!)",
            started.elapsed()
        );
    }

    #[test]
    fn local_server_bounce_refreshes_local_driver_transport() {
        let mut cfg = SimWorldConfig::desktop_default(888_010, "unit_local_bounce_transport");
        cfg.remote_hosts = vec!["remote-a".into()];
        let mut world = SimWorld::new(cfg).expect("world init");

        let driver = world.clients[PRIMARY_CLIENT]
            .local_driver
            .lock()
            .unwrap()
            .clone()
            .expect("desktop client must have SimLocalHostDriver");
        let pre_transport = driver.transport();
        let pre_t = Arc::clone(&pre_transport);
        let pre_ping = world
            .client_exec(10_000, move |_| async move {
                pre_t.call_async("app.ping", &json!({})).await
            })
            .expect("pre-bounce local transport app.ping");
        let pre_srv_id = pre_ping["server_id"].as_str().unwrap().to_owned();

        // Bounce `local-server`: `SimLocalHostDriver::transport()` must swap to a fresh
        // `ClientTransport` connected to the new `local-server` Turmoil listener!
        world
            .bounce_server("local-server")
            .expect("bounce local-server");
        let post_transport = driver.transport();
        assert!(
            !Arc::ptr_eq(&pre_transport, &post_transport),
            "SimLocalHostDriver::transport() must refresh after bounce_server(\"local-server\")"
        );
        let post_t = Arc::clone(&post_transport);
        let post_ping = world
            .client_exec(10_000, move |_| async move {
                post_t.call_async("app.ping", &json!({})).await
            })
            .expect("post-bounce local transport app.ping must succeed on new listener");
        assert_eq!(
            post_ping["server_id"].as_str(),
            Some(pre_srv_id.as_str()),
            "bounced local-server must retain its persistent server_id"
        );

        // Mutations on `local` AND push events from the bounced `local-server` work seamlessly.
        let dir = world.project_dir("post-bounce-local").expect("dir");
        let proj = world
            .client_call(
                "project.create",
                json!({ "host_id": "local", "name": "Post Bounce Local", "directory": dir }),
                None,
            )
            .expect("local project.create after bounce");
        let pid = proj["id"].as_str().unwrap().to_owned();
        let _ = world.drain_client_events();
        let _ = world
            .server_call(
                "local-server",
                "card.create",
                json!({ "project_id": pid, "title": "Server Push After Local Bounce" }),
            )
            .expect("direct local-server card.create");
        world
            .wait_until(10_000, |w| Ok(!w.drain_client_events().is_empty()))
            .expect("local_push_supervisor must receive push events after local-server bounce");
        world.heal_and_quiesce(10_000).expect("heal_and_quiesce");
        crate::oracle::InvariantOracle::default()
            .check_quiescent_invariants(&world)
            .expect("quiescent invariants after local-server bounce");
    }

    #[test]
    fn storage_faults_persist_across_server_bounce_and_clear_on_heal() {
        let cfg = SimWorldConfig::desktop_default(888_020, "unit_storage_faults");
        let mut world = SimWorld::new(cfg).expect("world init");

        for host in ["local-server", "remote-a", "remote-b"] {
            assert!(
                world.storage_faults(host).is_some(),
                "expected storage_faults injector for {host}"
            );
        }
        assert!(world.storage_faults("no-such-host").is_none());

        let pre_ptr = Arc::clone(world.storage_faults("remote-a").unwrap());
        world.bounce_server("remote-a").expect("bounce remote-a");
        let post_ptr = Arc::clone(world.storage_faults("remote-a").unwrap());
        assert!(
            Arc::ptr_eq(&pre_ptr, &post_ptr),
            "per-host StorageFaultInjector must persist across bounce_server"
        );

        let dir = world.project_dir("sf-unit").expect("dir");
        post_ptr.inject_next(StorageFaultKind::Busy);
        let err = world
            .server_call(
                "remote-a",
                "project.create",
                json!({ "name": "SF", "directory": dir }),
            )
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("SQLITE_BUSY"),
            "expected SQLITE_BUSY after bounce, got {err:#}"
        );
        assert_eq!(post_ptr.triggered_count(), 1);

        // Pending storage faults are cleared by heal_and_quiesce().
        post_ptr.inject_next(StorageFaultKind::IoError);
        world.heal_and_quiesce(10_000).expect("heal");
        let ok = world
            .server_call(
                "remote-a",
                "project.create",
                json!({ "name": "SF-Clean", "directory": dir }),
            )
            .expect("write after heal_and_quiesce must succeed");
        assert!(ok["id"].as_str().is_some());
        assert_eq!(post_ptr.triggered_count(), 1);
    }

    #[test]
    fn failpoints_persist_across_bounce_emit_observer_telemetry_and_clear_on_heal() {
        use canban_types::{
            FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB, FP_OUTBOX_MID_WRITE_TRUNCATE, FailpointAction,
        };

        crate::trace::reset_thread_coverage();
        let mut cfg = SimWorldConfig::desktop_default(888_030, "unit_failpoints_wiring");
        cfg.remote_hosts = vec!["remote-a".into()];
        let mut world = SimWorld::new(cfg).expect("world init");

        let host_fp_pre = world.failpoints("remote-a");
        let local_fp = world.failpoints("local");
        assert!(Arc::ptr_eq(&local_fp, &world.failpoints("local-server")));
        let client_fp_pre = world.client_failpoints(PRIMARY_CLIENT);
        assert!(world.try_failpoints("no-such-host").is_none());
        assert!(world.try_client_failpoints("no-such-client").is_none());

        world.bounce_server("remote-a").expect("bounce remote-a");
        world.bounce_client().expect("bounce ui-client");
        assert!(
            Arc::ptr_eq(&host_fp_pre, &world.failpoints("remote-a")),
            "per-host Failpoints must persist across bounce_server"
        );
        assert!(
            Arc::ptr_eq(&client_fp_pre, &world.client_failpoints(PRIMARY_CLIENT)),
            "per-client Failpoints must persist across bounce_client"
        );

        // Arm & fire a server failpoint via terminal.create -> observer mirrors into thread coverage.
        let dir = world.project_dir("fp-wire-dir").expect("dir");
        host_fp_pre.arm(
            FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB,
            FailpointAction::FailOnce("unit-herdr-fp".into()),
        );
        let created = world
            .server_call(
                "remote-a",
                "terminal.create",
                json!({ "cwd": dir, "request_key": "unit-fp-term" }),
            )
            .expect("terminal.create returns unknown entry on failpoint");
        assert_eq!(created["runtime"]["state"], "unknown");
        assert_eq!(
            host_fp_pre
                .stats_for(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB)
                .fired,
            1
        );

        // Arm an unhit client failpoint and verify heal_and_quiesce disarms it cleanly.
        client_fp_pre.arm(
            FP_OUTBOX_MID_WRITE_TRUNCATE,
            FailpointAction::FailAlways("unit-cleared-on-heal".into()),
        );
        assert!(client_fp_pre.is_armed(FP_OUTBOX_MID_WRITE_TRUNCATE));
        world.heal_and_quiesce(15_000).expect("heal_and_quiesce");
        assert!(
            !client_fp_pre.is_armed(FP_OUTBOX_MID_WRITE_TRUNCATE),
            "heal_and_quiesce must disarm client failpoints"
        );

        let cov = crate::trace::take_thread_coverage();
        assert_eq!(
            cov.fault_armed_count(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB),
            1
        );
        assert_eq!(
            cov.fault_fired_count(FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB),
            1
        );
        assert_eq!(cov.fault_armed_count(FP_OUTBOX_MID_WRITE_TRUNCATE), 1);
    }

    #[test]
    fn framed_local_transport_routes_over_serve_local_stream_and_recovers_after_bounce() {
        crate::trace::reset_thread_coverage();
        let mut cfg = SimWorldConfig::desktop_default(888_040, "unit_framed_local");
        cfg.remote_hosts = vec!["remote-a".into()];
        cfg.framed_local_transport = true;
        let mut world = SimWorld::new(cfg).expect("world init with framed_local_transport");

        let dir = world.project_dir("framed-local-repo").expect("dir");
        let proj = world
            .client_call(
                "project.create",
                json!({ "host_id": "local", "name": "Framed Local Proj", "directory": dir }),
                None,
            )
            .expect("framed local project.create");
        let pid = proj["id"].as_str().unwrap().to_owned();
        let card = world
            .client_call(
                "card.create",
                json!({ "project_id": pid, "title": "Framed Local Card" }),
                Some(&pid),
            )
            .expect("framed local card.create");
        let cid = card["id"].as_str().unwrap().to_owned();

        // Verify local merged snapshot reflects the framed write immediately.
        let snap = world.client_merged_snapshot().expect("merged snapshot");
        assert!(snap.cards.iter().any(|c| c.id == cid));

        // Bounce `"local"` using the alias -> old framed listener stops, client reconnects to new listener!
        let spawned = world
            .client_spawn(
                "card.update",
                json!({
                    "card_id": cid,
                    "revision": card["revision"],
                    "title": "Mid-Bounce Framed Write",
                    "request_key": "framed-mid-bounce-1",
                }),
                Some(&pid),
            )
            .expect("spawn framed local rpc");
        world.bounce_server("local").expect("bounce_server(local)");
        let _mid_res = world.await_call(spawned, 10_000);

        // Post-bounce framed RPC succeeds cleanly over the restarted local listener.
        let ping = world
            .client_call("app.ping", json!({ "host_id": "local" }), None)
            .expect("post-bounce framed local app.ping");
        assert!(ping["server_id"].as_str().is_some());

        // Direct server mutation converges naturally via framed_local_watcher + quiesce_natural!
        let _ = world
            .server_call(
                "local",
                "card.create",
                json!({ "project_id": pid, "title": "Server Direct Framed Watch" }),
            )
            .expect("server_call local card.create");
        world
            .quiesce_natural(15_000)
            .expect("natural quiesce with framed local transport");
        let post_snap = world.client_merged_snapshot().expect("post merged snap");
        assert!(
            post_snap
                .cards
                .iter()
                .any(|c| c.title == "Server Direct Framed Watch"),
            "framed_local_watcher must converge server-side local writes naturally"
        );
        crate::oracle::InvariantOracle::default()
            .check_quiescent_invariants(&world)
            .expect("quiescent invariants with framed_local_transport");
        world.flush_coverage();
        let cov = crate::trace::take_thread_coverage();
        assert!(
            cov.count("transport.local_framed_rpc") >= 4,
            "expected transport.local_framed_rpc coverage hits, got {}",
            cov.count("transport.local_framed_rpc")
        );
    }
}
