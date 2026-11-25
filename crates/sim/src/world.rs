//! Core `turmoil`-backed Deterministic Simulation World (`SimWorld`).
//!
//! Runs 1..N real [`AppServer`] nodes (each with real SQLite, Workflow Engine, Scheduler,
//! Notifications, GitHub sync, and Herdr observation in `inline_workers` mode) and a real
//! [`canban_client::MultiHostClient`] (`"ui-client"`) inside a single-threaded `turmoil::Sim`
//! stepped deterministically via `sim.step()`.
use crate::{
    clock::TurmoilClock,
    github::SimGitHub,
    herdr::SimHerdr,
    net::{TurmoilConnector, serve_turmoil_listener},
    trace::EventTrace,
};
use anyhow::{Context, Result, anyhow, bail};
use canban_app_server::{AppServer, ServerRuntimeEnv, hosts::DesktopGlobalEnvProvider};
use canban_client::{
    ClientCapabilities, ClientEvent, InlineHostsFileStore, LocalHostDriver, MultiHostClient,
    MultiHostClientOptions, RemoteClientConfig,
};
use canban_transport::ClientTransport;
use canban_types::{
    Config, GlobalEnvSyncStatus, HostSummary, HostTransport, RuntimeState, SavedHost, Snapshot,
    SyncAction, SyncItemView,
};
use rand::{SeedableRng, rngs::StdRng};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
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
            tick_ms: 50,
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
            tick_ms: 50,
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

/// Shared handles for a simulated [`AppServer`] host.
pub struct SimServerHandle {
    pub name: String,
    pub home: PathBuf,
    pub herdr: Arc<SimHerdr>,
    pub clock: Arc<TurmoilClock>,
    pub server_slot: Arc<Mutex<Option<Arc<AppServer>>>>,
    pub token: Arc<Mutex<Option<String>>>,
    pub crashed: Arc<AtomicBool>,
}

impl SimServerHandle {
    pub fn server(&self) -> Option<Arc<AppServer>> {
        self.server_slot.lock().unwrap().clone()
    }

    pub fn token(&self) -> Option<String> {
        self.token.lock().unwrap().clone()
    }
}

/// Deterministic simulation world orchestrating `turmoil::Sim`, server nodes, and `MultiHostClient`.
pub struct SimWorld {
    _tempdir: TempDir,
    pub config: SimWorldConfig,
    pub rng: StdRng,
    pub trace: EventTrace,
    pub github: Arc<SimGitHub>,
    pub client_data_dir: PathBuf,
    pub servers: BTreeMap<String, SimServerHandle>,
    sim: turmoil::Sim<'static>,
    shared_elapsed_ms: Arc<AtomicU64>,
    client_slot: Arc<Mutex<Option<Arc<MultiHostClient>>>>,
    client_cmd_tx: mpsc::UnboundedSender<ClientCommand>,
    client_events: Arc<Mutex<Vec<ClientEvent>>>,
    partitioned_pairs: BTreeSet<(String, String)>,
    held_pairs: BTreeSet<(String, String)>,
    step_count: u64,
    fault_count: u64,
}

impl SimWorld {
    pub fn new(config: SimWorldConfig) -> Result<Self> {
        let tempdir = tempfile::tempdir().context("create sim tempdir")?;
        let root = tempdir.path().to_path_buf();
        let client_data_dir = root.join("ui-client");
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

        let mut sim = builder.build();
        let shared_elapsed_ms = Arc::new(AtomicU64::new(0));
        let github = SimGitHub::new();

        let mut servers = BTreeMap::new();
        let mut host_names = Vec::new();
        if config.desktop_local {
            host_names.push("local-server".to_owned());
        }
        host_names.extend(config.remote_hosts.clone());

        for host_name in &host_names {
            let home = root.join(format!("srv-{host_name}"));
            std::fs::create_dir_all(&home)?;
            let cfg_path = home.join("config.json");
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
            let server_slot: Arc<Mutex<Option<Arc<AppServer>>>> = Arc::new(Mutex::new(None));
            let token_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
            let crashed = Arc::new(AtomicBool::new(false));

            let h_home = home.clone();
            let h_clock = clock.clone();
            let h_herdr = herdr.clone();
            let h_github = github.clone();
            let h_slot = Arc::clone(&server_slot);
            let h_token = Arc::clone(&token_slot);
            let h_name = host_name.clone();

            sim.host(host_name.as_str(), move || {
                let home = h_home.clone();
                let clock = h_clock.clone();
                let herdr = h_herdr.clone();
                let github = h_github.clone();
                let slot = Arc::clone(&h_slot);
                let token_slot = Arc::clone(&h_token);
                let host_label = h_name.clone();
                async move {
                    let env = ServerRuntimeEnv {
                        clock,
                        herdr_backend: Some(herdr),
                        github_transport: Some(github),
                        inline_workers: true,
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
                    let _running = server
                        .start_in_runtime(tokio::runtime::Handle::current())
                        .map_err(|e| format!("start_in_runtime({host_label}): {e:#}"))?;
                    let listener = turmoil::net::TcpListener::bind(("0.0.0.0", SIM_PORT))
                        .await
                        .map_err(|e| format!("bind({host_label}): {e:#}"))?;
                    *slot.lock().unwrap() = Some(Arc::clone(&server));
                    let res = serve_turmoil_listener(Arc::clone(&server), listener).await;
                    *slot.lock().unwrap() = None;
                    res.map_err(|e| format!("serve({host_label}): {e:#}").into())
                }
            });

            servers.insert(
                host_name.clone(),
                SimServerHandle {
                    name: host_name.clone(),
                    home,
                    herdr,
                    clock,
                    server_slot,
                    token: token_slot,
                    crashed,
                },
            );
        }

        // Wait until all server hosts have booted and bound their simulated TCP listeners.
        for _ in 0..200 {
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
                bail!("Simulated server '{name}' failed to boot within 200 steps");
            }
        }

        // Now register `"ui-client"` hosting the real `MultiHostClient`.
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ClientCommand>();
        let cmd_rx_slot = Arc::new(Mutex::new(Some(cmd_rx)));
        let client_slot: Arc<Mutex<Option<Arc<MultiHostClient>>>> = Arc::new(Mutex::new(None));
        let client_events: Arc<Mutex<Vec<ClientEvent>>> = Arc::new(Mutex::new(Vec::new()));

        let ui_data_dir = client_data_dir.clone();
        let ui_clock = Arc::new(TurmoilClock::new(Arc::clone(&shared_elapsed_ms)));
        let ui_client_slot = Arc::clone(&client_slot);
        let ui_events = Arc::clone(&client_events);
        let desktop_local = config.desktop_local;
        let local_info = servers.get("local-server").map(|s| {
            (
                s.home.clone(),
                Arc::clone(&s.server_slot),
                s.token().expect("local token"),
            )
        });
        let initial_remotes: Vec<(String, String)> = config
            .remote_hosts
            .iter()
            .map(|name| {
                let tok = servers[name].token().expect("remote token");
                (name.clone(), tok)
            })
            .collect();

        sim.host("ui-client", move || {
            let data_dir = ui_data_dir.clone();
            let clock = ui_clock.clone();
            let client_slot = Arc::clone(&ui_client_slot);
            let events_sink = Arc::clone(&ui_events);
            let local_info = local_info.clone();
            let remotes = initial_remotes.clone();
            let rx_holder = Arc::clone(&cmd_rx_slot);
            async move {
                let mut rx = rx_holder
                    .lock()
                    .unwrap()
                    .take()
                    .expect("ui-client receiver");
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
                        .map_err(|e| format!("ui-client connect local-server: {e}"))?;
                    let transport = Arc::new(
                        ClientTransport::connect_stream_async(
                            stream,
                            addr,
                            local_tok,
                            "Sim Desktop".into(),
                        )
                        .await
                        .map_err(|e| format!("ui-client local mux: {e:#}"))?,
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
                rc_cfg.device_name = "Sim Client".into();
                let host_store = Arc::new(InlineHostsFileStore::new(data_dir.join("hosts.json")));
                let client = MultiHostClient::from_options_with_runtime(
                    MultiHostClientOptions {
                        config: rc_cfg,
                        host_store,
                        connector: Arc::new(TurmoilConnector),
                        capabilities,
                        local_host: local_driver,
                        global_env_provider,
                        min_api_version: Some(canban_app_server::API_VERSION),
                        clock,
                        fs_durability: canban_client::FsDurability::Full,
                    },
                    tokio::runtime::Handle::current(),
                )
                .map_err(|e| format!("MultiHostClient init: {e:#}"))?;

                for (name, token) in remotes {
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
                tokio::spawn(async move {
                    while let Some(ev) = ev_rx.recv().await {
                        ev_sink.lock().unwrap().push(ev);
                    }
                });

                *client_slot.lock().unwrap() = Some(Arc::clone(&client));

                while let Some(cmd) = rx.recv().await {
                    let c = Arc::clone(&client);
                    let reply = cmd.reply;
                    let fut = (cmd.run)(c);
                    tokio::spawn(async move {
                        let _ = reply.send(fut.await);
                    });
                }
                Ok(())
            }
        });

        let rng = StdRng::seed_from_u64(config.seed);
        let trace = EventTrace::new(config.seed, config.scenario.clone());
        let mut world = Self {
            _tempdir: tempdir,
            config,
            rng,
            trace,
            github,
            client_data_dir,
            servers,
            sim,
            shared_elapsed_ms,
            client_slot,
            client_cmd_tx: cmd_tx,
            client_events,
            partitioned_pairs: BTreeSet::new(),
            held_pairs: BTreeSet::new(),
            step_count: 0,
            fault_count: 0,
        };

        // Pump until MultiHostClient has initialized and connected all initial remote hosts.
        for _ in 0..500 {
            world.step_once()?;
            if world.client_slot.lock().unwrap().is_some() {
                break;
            }
        }
        if world.client_slot.lock().unwrap().is_none() {
            bail!("ui-client failed to initialize within 500 steps");
        }
        world.advance(Duration::from_millis(200))?;
        Ok(world)
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
                bail!("SimWorld::wait_until timed out after {timeout_ms}ms virtual time");
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

    pub fn client(&self) -> Arc<MultiHostClient> {
        self.client_slot
            .lock()
            .unwrap()
            .clone()
            .expect("ui-client initialized")
    }

    pub fn drain_client_events(&self) -> Vec<ClientEvent> {
        std::mem::take(&mut *self.client_events.lock().unwrap())
    }

    /// Executes an async operation inside `"ui-client"`'s Turmoil Tokio runtime, pumping
    /// `sim.step()` deterministically until the future resolves (or `timeout_ms` virtual ms elapse).
    pub fn client_exec<F, Fut>(&mut self, timeout_ms: u64, f: F) -> Result<Value>
    where
        F: FnOnce(Arc<MultiHostClient>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        let (tx, mut rx) = oneshot::channel();
        self.client_cmd_tx
            .send(ClientCommand {
                run: Box::new(move |c| Box::pin(f(c))),
                reply: tx,
            })
            .map_err(|_| anyhow!("ui-client command channel closed"))?;
        let deadline = self
            .sim
            .elapsed()
            .saturating_add(Duration::from_millis(timeout_ms));
        loop {
            if let Ok(res) = rx.try_recv() {
                return res;
            }
            if self.sim.elapsed() >= deadline {
                bail!("SimWorld::client_exec timed out after {timeout_ms}ms virtual time");
            }
            self.step_once()?;
        }
    }

    pub fn client_call(
        &mut self,
        method: &str,
        params: Value,
        active_project_id: Option<&str>,
    ) -> Result<Value> {
        let m = method.to_owned();
        let active = active_project_id.map(str::to_owned);
        self.trace.record(
            self.step_count,
            self.elapsed_ms(),
            "client.rpc",
            format!("{m} has_active={}", active.is_some()),
        );
        self.client_exec(120_000, move |c| async move {
            c.call_routed_async(&m, params, active.as_deref()).await
        })
    }

    pub fn client_call_all(&mut self, method: &str, params: Value) -> Result<Value> {
        let m = method.to_owned();
        self.trace
            .record(self.step_count, self.elapsed_ms(), "client.all", m.clone());
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
        self.client_exec(120_000, move |c| async move {
            let report = c.drain_outbox_async(&hid).await?;
            Ok(json!({
                "applied": report.applied,
                "conflicts": report.conflicts,
                "failed": report.failed,
            }))
        })
    }

    pub fn client_merged_snapshot(&self) -> Result<Snapshot> {
        self.client().merged_snapshot()
    }

    pub fn client_merged_runtime(&self) -> Result<RuntimeState> {
        self.client().merged_runtime_state()
    }

    pub fn client_host_summaries(&self) -> Vec<HostSummary> {
        self.client().host_summaries()
    }

    pub fn client_sync_items(&self) -> Vec<SyncItemView> {
        self.client().sync_items()
    }

    pub fn client_sync_action(
        &self,
        host_id: &str,
        mutation_id: &str,
        action: SyncAction,
    ) -> Result<()> {
        self.client().sync_action(host_id, mutation_id, action)
    }

    pub fn client_sync_global_env_now(&mut self) {
        self.trace.record(
            self.step_count,
            self.elapsed_ms(),
            "env.sync",
            "trigger global env sync",
        );
        self.client().sync_global_env_now();
    }

    pub fn client_global_env_status(&self) -> GlobalEnvSyncStatus {
        self.client().global_env_sync_status()
    }

    // ---- Network & process fault injection (directly backed by Turmoil) ----

    pub fn partition(&mut self, a: &str, b: &str) {
        self.sim.partition(a, b);
        self.partitioned_pairs.insert((a.to_owned(), b.to_owned()));
        self.fault_count += 1;
        self.trace.record(
            self.step_count,
            self.elapsed_ms(),
            "fault.part",
            format!("{a} <-> {b}"),
        );
    }

    pub fn partition_oneway(&mut self, from: &str, to: &str) {
        self.sim.partition_oneway(from, to);
        self.partitioned_pairs
            .insert((from.to_owned(), to.to_owned()));
        self.fault_count += 1;
        self.trace.record(
            self.step_count,
            self.elapsed_ms(),
            "fault.1way",
            format!("{from} -/-> {to}"),
        );
    }

    pub fn repair(&mut self, a: &str, b: &str) {
        self.sim.repair(a, b);
        self.partitioned_pairs.remove(&(a.to_owned(), b.to_owned()));
        self.partitioned_pairs.remove(&(b.to_owned(), a.to_owned()));
        self.trace.record(
            self.step_count,
            self.elapsed_ms(),
            "fault.heal",
            format!("{a} <-> {b}"),
        );
    }

    pub fn hold(&mut self, a: &str, b: &str) {
        self.sim.hold(a, b);
        self.held_pairs.insert((a.to_owned(), b.to_owned()));
        self.fault_count += 1;
        self.trace.record(
            self.step_count,
            self.elapsed_ms(),
            "fault.hold",
            format!("{a} <-> {b}"),
        );
    }

    pub fn release(&mut self, a: &str, b: &str) {
        self.sim.release(a, b);
        self.held_pairs.remove(&(a.to_owned(), b.to_owned()));
        self.held_pairs.remove(&(b.to_owned(), a.to_owned()));
        self.trace.record(
            self.step_count,
            self.elapsed_ms(),
            "fault.rel",
            format!("{a} <-> {b}"),
        );
    }

    pub fn set_fail_rate(&mut self, rate: f64) {
        self.sim.set_fail_rate(rate);
        self.trace.record(
            self.step_count,
            self.elapsed_ms(),
            "fault.rate",
            format!("fail_rate={rate:.3}"),
        );
    }

    /// Crashes a remote `AppServer` host (dropping its Tokio runtime & sockets while preserving SQLite).
    pub fn crash_server(&mut self, name: &str) -> Result<()> {
        let Some(handle) = self.servers.get(name) else {
            bail!("Unknown server host '{name}'");
        };
        if let Some(srv) = handle.server_slot.lock().unwrap().take() {
            drop(srv);
        }
        handle.crashed.store(true, Ordering::SeqCst);
        self.sim.crash(name);
        self.fault_count += 1;
        self.trace.record(
            self.step_count,
            self.elapsed_ms(),
            "fault.crash",
            name.to_owned(),
        );
        self.step_once()?;
        Ok(())
    }

    /// Restarts a crashed (or running) remote `AppServer` host on the same SQLite directory.
    pub fn bounce_server(&mut self, name: &str) -> Result<()> {
        let Some(handle) = self.servers.get(name) else {
            bail!("Unknown server host '{name}'");
        };
        if let Some(srv) = handle.server_slot.lock().unwrap().take() {
            drop(srv);
        }
        handle.crashed.store(false, Ordering::SeqCst);
        self.sim.bounce(name);
        self.trace.record(
            self.step_count,
            self.elapsed_ms(),
            "fault.bounce",
            name.to_owned(),
        );
        for _ in 0..300 {
            self.step_once()?;
            if self.servers[name].server().is_some() {
                return Ok(());
            }
        }
        bail!("Server '{name}' did not finish booting after bounce");
    }

    /// Heals all temporary network faults and restarts any crashed servers, then advances virtual
    /// time until all enabled remote hosts reconnect, drain pending outbox items, and converge.
    pub fn heal_and_quiesce(&mut self, max_virtual_ms: u64) -> Result<()> {
        self.sim.set_fail_rate(0.0);
        let parts: Vec<(String, String)> = self.partitioned_pairs.iter().cloned().collect();
        for (a, b) in parts {
            self.repair(&a, &b);
        }
        let holds: Vec<(String, String)> = self.held_pairs.iter().cloned().collect();
        for (a, b) in holds {
            self.release(&a, &b);
        }
        let crashed: Vec<String> = self
            .servers
            .iter()
            .filter(|(_, h)| h.crashed.load(Ordering::SeqCst))
            .map(|(n, _)| n.clone())
            .collect();
        for name in crashed {
            self.bounce_server(&name)?;
        }
        // Bring all Herdr backends back online as well.
        for h in self.servers.values() {
            h.herdr.set_offline(false);
        }

        // Wake client hosts to reconnect immediately rather than waiting full exponential backoff.
        self.client().reconnect(None);
        for name in &self.config.remote_hosts {
            self.client().clear_retry_backoff(name);
        }

        let deadline = self
            .sim
            .elapsed()
            .saturating_add(Duration::from_millis(max_virtual_ms.max(120_000)));
        let mut stable_checks = 0u32;
        let mut iter = 0u32;
        while self.sim.elapsed() < deadline {
            self.step_once()?;
            iter += 1;
            if !iter.is_multiple_of(5) {
                continue;
            }
            let summaries = self.client_host_summaries();
            let all_connected = self.config.remote_hosts.iter().all(|name| {
                summaries
                    .iter()
                    .find(|s| s.id == *name)
                    .is_some_and(|s| s.is_connected())
            });
            let items = self.client_sync_items();
            let pending_active = items.iter().any(|it| {
                matches!(
                    it.state,
                    canban_types::SyncItemState::Queued | canban_types::SyncItemState::InFlight
                )
            });
            let env_status = self.client_global_env_status();
            let env_busy =
                !env_status.queued_hosts.is_empty() || !env_status.syncing_hosts.is_empty();
            if all_connected && !pending_active && !env_busy {
                stable_checks += 1;
                if stable_checks >= 5 {
                    break;
                }
            } else {
                stable_checks = 0;
            }
        }
        // Final explicit poll pass so client cached snapshots reflect all latest server mutations.
        let _ = self.client_exec(15_000, |c| async move {
            c.poll_remotes_once_async().await;
            Ok(json!({}))
        });
        self.advance(Duration::from_millis(200))?;
        Ok(())
    }
}
