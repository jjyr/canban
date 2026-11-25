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
pub use canban_app_server::FsDurability;
use canban_app_server::{AppServer, ServerRuntimeEnv, hosts::DesktopGlobalEnvProvider};
use canban_client::{
    ClientCapabilities, ClientEvent, InlineHostsFileStore, LocalHostDriver, MultiHostClient,
    MultiHostClientOptions, RemoteClientConfig,
};
use canban_transport::ClientTransport;
use canban_types::{
    Config, GlobalEnvSyncStatus, HostSummary, HostTransport, RuntimeState, SavedHost, Snapshot,
    SyncAction, SyncItemState, SyncItemView,
};
use rand::{SeedableRng, rngs::StdRng};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    future::Future,
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
pub const PRIMARY_CLIENT: &str = "ui-client";
const MAX_BUFFERED_CLIENT_EVENTS: usize = 2_048;

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
        }
    }
}

struct SimLocalHostDriver {
    home: PathBuf,
    server_slot: Arc<Mutex<Option<Arc<AppServer>>>>,
    transport: Arc<ClientTransport>,
}

impl LocalHostDriver for SimLocalHostDriver {
    fn home(&self) -> &Path {
        &self.home
    }

    fn endpoint_label(&self) -> String {
        "turmoil://local-server".into()
    }

    fn server_id(&self) -> Option<String> {
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
        Arc::clone(&self.transport)
    }

    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let srv = self
            .server_slot
            .lock()
            .unwrap()
            .clone()
            .context("Simulated local AppServer is crashed")?;
        srv.call(method, params)
    }

    fn snapshot(&self) -> Result<Snapshot> {
        let srv = self
            .server_slot
            .lock()
            .unwrap()
            .clone()
            .context("Simulated local AppServer is crashed")?;
        srv.snapshot()
    }

    fn runtime_state(&self) -> Result<RuntimeState> {
        let srv = self
            .server_slot
            .lock()
            .unwrap()
            .clone()
            .context("Simulated local AppServer is crashed")?;
        srv.runtime_state()
    }

    fn config(&self) -> Config {
        self.server_slot
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.config())
            .unwrap_or_default()
    }
}

type ClientTaskFn = Box<
    dyn FnOnce(Arc<MultiHostClient>) -> Pin<Box<dyn Future<Output = Result<Value>> + Send>> + Send,
>;

struct ClientCommand {
    run: ClientTaskFn,
    reply: oneshot::Sender<Result<Value>>,
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
    pub client_slot: Arc<Mutex<Option<Arc<MultiHostClient>>>>,
    pub rt_handle: Arc<Mutex<Option<tokio::runtime::Handle>>>,
    cmd_tx: Arc<Mutex<Option<mpsc::UnboundedSender<ClientCommand>>>>,
    events: Arc<Mutex<VecDeque<ClientEvent>>>,
    total_events: Arc<AtomicU64>,
    pub crashed: Arc<AtomicBool>,
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
        let github = SimGitHub::new();
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
            let h_slot = Arc::clone(&server_slot);
            let h_abrupt = Arc::clone(&abrupt_slot);
            let h_token = Arc::clone(&token_slot);
            let h_boot_gen = Arc::clone(&boot_generation);
            let h_name = host_name.clone();
            let h_seed = config.seed;
            let h_durability = config.fs_durability;
            let h_sqlite_sync_off = config.sqlite_synchronous_off;
            let h_disable_watchers = config.disable_os_watchers;
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
                    #[allow(clippy::needless_update)]
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
                        ..Default::default()
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
                    *slot.lock().unwrap() = Some(Arc::clone(&server));
                    let res = serve_turmoil_listener_with_faults(
                        Arc::clone(&server),
                        listener,
                        stream_policy,
                        Some(host_label.clone()),
                    )
                    .await;
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
        let client_events: Arc<Mutex<VecDeque<ClientEvent>>> =
            Arc::new(Mutex::new(VecDeque::new()));
        let total_events = Arc::new(AtomicU64::new(0));
        let crashed = Arc::new(AtomicBool::new(false));

        let ui_data_dir = data_dir.clone();
        let ui_clock = Arc::new(TurmoilClock::new(Arc::clone(&self.shared_elapsed_ms)));
        let ui_client_slot = Arc::clone(&client_slot);
        let ui_rt_handle = Arc::clone(&rt_handle);
        let ui_cmd_tx = Arc::clone(&cmd_tx_slot);
        let ui_events = Arc::clone(&client_events);
        let ui_total_events = Arc::clone(&total_events);
        let ui_durability = self.config.fs_durability;
        let ui_policy = self.stream_fault_policy.clone();
        let ui_name = client_name.to_owned();

        let local_info = if desktop_local {
            self.servers.get("local-server").map(|s| {
                (
                    s.home.clone(),
                    Arc::clone(&s.server_slot),
                    s.token().expect("local token"),
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
            let events_sink = Arc::clone(&ui_events);
            let total_events_ctr = Arc::clone(&ui_total_events);
            let local_info = local_info.clone();
            let remotes = initial_remotes.clone();
            let policy = ui_policy.clone();
            let device_label = format!("Sim Client ({ui_name})");
            async move {
                let (tx, mut rx) = mpsc::unbounded_channel::<ClientCommand>();
                #[allow(clippy::type_complexity)]
                let (local_driver, global_env_provider, capabilities): (
                    Option<Arc<dyn LocalHostDriver>>,
                    Option<Arc<dyn canban_client::GlobalEnvSyncProvider>>,
                    ClientCapabilities,
                ) = if desktop_local {
                    let (local_home, srv_slot, local_tok) = local_info.expect("local server info");
                    let addr = format!("local-server:{SIM_PORT}");
                    let stream = turmoil::net::TcpStream::connect(addr.as_str())
                        .await
                        .map_err(|e| format!("connect local-server: {e}"))?;
                    let transport = Arc::new(
                        ClientTransport::connect_stream_async(
                            stream,
                            addr,
                            local_tok,
                            "Sim Desktop".into(),
                        )
                        .await
                        .map_err(|e| format!("local mux: {e:#}"))?,
                    );
                    let driver: Arc<dyn LocalHostDriver> = Arc::new(SimLocalHostDriver {
                        home: local_home.clone(),
                        server_slot: srv_slot,
                        transport,
                    });
                    let env_provider: Arc<dyn canban_client::GlobalEnvSyncProvider> =
                        Arc::new(DesktopGlobalEnvProvider::new(local_home));
                    (
                        Some(driver),
                        Some(env_provider),
                        ClientCapabilities::desktop(),
                    )
                } else {
                    (None, None, ClientCapabilities::mobile())
                };

                let mut rc_cfg = RemoteClientConfig::new(data_dir.clone());
                rc_cfg.device_name = device_label;
                let host_store = Arc::new(InlineHostsFileStore::new_with_durability(
                    data_dir.join("hosts.json"),
                    ui_durability,
                ));
                let rt_cur = tokio::runtime::Handle::current();
                let client = MultiHostClient::from_options_with_runtime(
                    MultiHostClientOptions {
                        config: rc_cfg,
                        host_store,
                        connector: Arc::new(FaultyTurmoilConnector::new(policy)),
                        capabilities,
                        local_host: local_driver,
                        global_env_provider,
                        min_api_version: Some(canban_app_server::API_VERSION),
                        clock,
                        fs_durability: ui_durability,
                    },
                    rt_cur.clone(),
                )
                .map_err(|e| format!("MultiHostClient init: {e:#}"))?;

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
                    let reply = cmd.reply;
                    let fut = (cmd.run)(c);
                    tokio::spawn(async move {
                        let _ = reply.send(fut.await);
                    });
                }
                *client_slot.lock().unwrap() = None;
                *cmd_tx_holder.lock().unwrap() = None;
                *rt_slot.lock().unwrap() = None;
                Ok(())
            }
        });

        self.clients.insert(
            client_name.to_owned(),
            SimClientHandle {
                name: client_name.to_owned(),
                data_dir,
                client_slot,
                rt_handle,
                cmd_tx: cmd_tx_slot,
                events: client_events,
                total_events,
                crashed,
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

    pub fn step_once(&mut self) -> Result<()> {
        self.sim
            .step()
            .map_err(|e| anyhow!("turmoil step error: {e}"))?;
        self.step_count += 1;
        self.shared_elapsed_ms
            .store(self.sim.elapsed().as_millis() as u64, Ordering::SeqCst);
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

    pub fn server(&self, name: &str) -> Option<Arc<AppServer>> {
        self.servers.get(name).and_then(|s| s.server())
    }

    pub fn herdr(&self, name: &str) -> Option<Arc<SimHerdr>> {
        self.servers.get(name).map(|s| Arc::clone(&s.herdr))
    }

    pub fn github_host(&self, name: &str) -> Option<Arc<SimGitHubHost>> {
        self.servers.get(name).map(|s| Arc::clone(&s.github))
    }

    pub fn exec_runner(&self, name: &str) -> Option<Arc<SimExec>> {
        self.servers.get(name).map(|s| Arc::clone(&s.exec))
    }

    /// Traced direct RPC against a server host (`AppServer::call`), recording the actor in `trace`.
    pub fn server_call(&mut self, host: &str, method: &str, params: Value) -> Result<Value> {
        let srv = self
            .server(host)
            .with_context(|| format!("Server host '{host}' is not running"))?;
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            host,
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

    /// Executes an async operation inside `client_id`'s Turmoil Tokio runtime, pumping
    /// `sim.step()` deterministically until the future resolves (or `timeout_ms` virtual ms elapse).
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
        let tx_sender = self
            .clients
            .get(client_id)
            .and_then(|c| c.cmd_tx.lock().unwrap().clone())
            .with_context(|| {
                format!("Simulated client '{client_id}' is crashed or uninitialized")
            })?;
        let (tx, mut rx) = oneshot::channel();
        tx_sender
            .send(ClientCommand {
                run: Box::new(move |c| Box::pin(f(c))),
                reply: tx,
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
                            "SimWorld::client_exec_on({client_id}) timed out after {timeout_ms}ms virtual time"
                        ),
                        Some(&self.state_summary()),
                    )
                );
            }
            self.step_once()?;
        }
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
                    Box::pin(
                        async move { c.call_routed_async(&m_run, params, active.as_deref()).await },
                    )
                }),
                reply: tx,
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
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            client_id,
            "client.rpc",
            format!("{m} has_active={}", active.is_some()),
        );
        self.client_exec_on(client_id, 120_000, move |c| async move {
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
        let val = self.client_exec(30_000, |c| async move {
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
            format!("{hid}:{mid} {action:?}"),
        );
        self.hit("client.sync_action");
        self.client_exec_on(client_id, 30_000, move |c| async move {
            c.sync_action(&hid, &mid, action)?;
            Ok(Value::Null)
        })?;
        Ok(())
    }

    /// Triggers `MultiHostClient::sync_global_env_now` inside `"ui-client"`'s Turmoil runtime.
    pub fn client_sync_global_env_now(&mut self) {
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            PRIMARY_CLIENT,
            "env.sync",
            "trigger global env sync",
        );
        self.hit("env.sync_triggered");
        let _ = self.client_exec(30_000, |c| async move {
            c.sync_global_env_now();
            Ok(Value::Null)
        });
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
        let Some(handle) = self.servers.get(name) else {
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
        self.faulted_hosts.insert(name.to_owned());
        self.sim.crash(name);
        self.fault_count += 1;
        self.hit("fault.server_crash_abrupt");
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            name,
            "fault.crash",
            "abrupt SIGKILL",
        );
        self.step_once()?;
        Ok(())
    }

    /// Gracefully stops an `AppServer` host (runs `SimRunning` graceful teardown including `finish_agent_work()`).
    pub fn stop_server_graceful(&mut self, name: &str) -> Result<()> {
        let Some(handle) = self.servers.get(name) else {
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
        self.sim.crash(name);
        self.hit("fault.server_stop_graceful");
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            name,
            "fault.stop",
            "graceful",
        );
        self.step_once()?;
        Ok(())
    }

    /// Restarts a crashed (or running) `AppServer` host on the same SQLite directory.
    pub fn bounce_server(&mut self, name: &str) -> Result<()> {
        let Some(handle) = self.servers.get(name) else {
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
        self.faulted_hosts.insert(name.to_owned());
        self.sim.bounce(name);
        self.hit("fault.server_bounce");
        self.trace.record_actor(
            self.step_count,
            self.elapsed_ms(),
            name,
            "fault.bounce",
            "restart",
        );
        for _ in 0..400 {
            self.step_once()?;
            if self.servers[name].server().is_some() {
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
            let pending_active = items
                .iter()
                .any(|it| matches!(it.state, SyncItemState::Queued | SyncItemState::InFlight));
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

    /// Waits for natural unassisted reconnect, outbox drain, and global env sync WITHOUT
    /// forcefully kicking `reconnect`, `clear_retry_backoff`, or `poll_remotes_once_async`.
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
            if self.all_clients_quiesced() {
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

    /// Heals all temporary network/GitHub/Herdr/stream faults and restarts any crashed servers or
    /// clients, wakes client reconnect loops inside their Turmoil runtimes, and advances virtual
    /// time until all enabled remote hosts reconnect, drain pending outbox items, and converge.
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
        // (~30s virtual time) before observing teardown and redialing; ensure sufficient virtual
        // budget when recovering faulted links while still failing loudly if convergence stalls.
        let budget_ms = if had_reconnect {
            max_virtual_ms.max(120_000)
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
            if self.all_clients_quiesced() {
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

        // Final explicit poll pass so client cached snapshots reflect all latest server mutations.
        for cname in &client_names {
            self.client_exec_on(cname, 15_000, |c| async move {
                c.poll_remotes_once_async().await;
                Ok(json!({}))
            })?;
        }
        self.advance(Duration::from_millis(200))?;
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
                        "  server[{name}]: UP projects={} cards={} wf_runs={} terminals={} notices={} reminders={} exec_cmds={}\n",
                        snap.projects.len(),
                        snap.cards.len(),
                        snap.workflow_runs.len(),
                        snap.terminals.len(),
                        snap.notices.len(),
                        snap.reminders.len(),
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
}
