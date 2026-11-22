use super::*;
pub(super) fn uuid_session_id() -> String {
    canban_types::uuid_v4()
}

pub(super) struct ActiveTcpStreamGuard {
    server: std::sync::Weak<AppServer>,
    conn_id: u64,
}

impl Drop for ActiveTcpStreamGuard {
    fn drop(&mut self) {
        if let Some(server) = self.server.upgrade() {
            server
                .active_tcp_streams
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.conn_id);
        }
    }
}

impl AppServer {
    pub fn start(self: &Arc<Self>) -> Result<Running> {
        self.start_with_listen(None)
    }

    pub fn start_with_listen(
        self: &Arc<Self>,
        override_listen: Option<ServerListenConfig>,
    ) -> Result<Running> {
        // The lock is held by the accept loop, including socket removal.
        let lock = self._service_lock.try_clone()?;
        let socket = self.endpoint();
        if socket.exists() {
            ensure!(
                UnixStream::connect(&socket).is_err(),
                "Another Canban AppServer already owns {}",
                socket.display()
            );
            std::fs::remove_file(&socket)?;
        }
        let listener = UnixListener::bind(&socket).context("Could not bind Canban local socket")?;
        listener.set_nonblocking(true)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        // Verify retained statuses again after restart; never reclaim immediately from old idle time.
        self.store.lock().unwrap().write(|db| {
            for mut item in db.list::<TerminalEntry>()? {
                let mut changed = false;
                if let Some(a) = &mut item.agent
                    && !a.finished()
                    && a.terminal.is_some()
                {
                    if matches!(a.close_state.as_str(), "waiting" | "pending" | "manual") {
                        a.close_state.clear();
                        a.turn_ended_at = None;
                    }
                    a.connection_state = "checking".into();
                    a.lifecycle.observed_at = None;
                    a.lifecycle.idle_since = None;
                    changed = true;
                }
                if let Some(TerminalRuntime {
                    state,
                    connection,
                    error,
                    ..
                }) = &mut item.runtime
                    && state == "starting"
                {
                    *state = "unknown".into();
                    *connection = "unavailable".into();
                    *error = Some("AppServer restarted before the launch receipt was saved; inspect Herdr before opening another terminal.".into());
                    changed = true;
                }
                if changed {
                    db.save(&item)?;
                }
            }
            Ok(())
        })?;
        // Persisted starting intents must not be replayed as fresh launches.
        // A known binding can reconnect. A lost launch reply remains unknown.
        self.store.lock().unwrap().write(|db| {
            for mut item in db.list::<TerminalEntry>()? {
                let Some(run) = &mut item.agent else {
                    continue;
                };
                if run.state != "starting" {
                    continue;
                }
                run.state = if run.terminal.is_some() {
                    "running"
                } else {
                    "unknown"
                }
                .into();
                let is_unknown = run.state == "unknown";
                let card_id = run.card_id.clone();
                db.save(&item)?;
                if is_unknown {
                    let mut card: Card = db.get(&card_id)?;
                    attention(
                        db,
                        &mut card,
                        "Launch interrupted before its receipt was saved. Inspect Herdr before explicitly retrying.",
                    )?;
                    if let Some(gate) = &mut card.human_gate
                        && gate.terminal_id.is_none()
                    {
                        gate.terminal_id = Some(item.id.clone());
                    }
                    card.revision += 1;
                    db.save(&card)?;
                }
            }
            Ok(())
        })?;
        self.store.lock().unwrap().write(|db| {
            for mut q in db.list::<Question>()?.into_iter().filter(|q| q.state == "delivering") {
                q.state = "delivery_unknown".into();
                q.error = Some("AppServer restarted during delivery; inspect the Agent terminal before resending".into());
                let mut card: Card = db.get(&q.card_id)?;
                attention(db, &mut card, q.error.as_deref().unwrap())?;
                card.revision += 1;
                db.save(&card)?;
                db.save(&q)?;
            }
            Ok(())
        })?;
        let mut accept_handles = Vec::new();
        let weak = Arc::downgrade(self);
        accept_handles.push(std::thread::spawn(move || {
            use std::os::fd::AsRawFd;
            unsafe extern "C" {
                fn poll(fds: *mut PollFd, nfds: u32, timeout: i32) -> i32;
            }
            #[repr(C)]
            struct PollFd {
                fd: i32,
                events: i16,
                revents: i16,
            }
            const POLLIN: i16 = 0x0001;
            let raw_fd = listener.as_raw_fd();
            while let Some(server) = weak.upgrade() {
                if server.stop.load(Ordering::SeqCst) {
                    break;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        let server = server.clone();
                        std::thread::spawn(move || {
                            let _ = server.serve(stream);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        drop(server);
                        let mut pfd = PollFd {
                            fd: raw_fd,
                            events: POLLIN,
                            revents: 0,
                        };
                        unsafe {
                            let _ = poll(&mut pfd, 1, 50);
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = std::fs::remove_file(socket);
            drop(lock);
        }));
        let listen_cfg = override_listen.unwrap_or_else(|| network::load_listen_config(&self.home));
        if listen_cfg.tcp_enabled() {
            let _ = network::ensure_server_token(&self.home)?;
            let bind_addrs = network::resolve_bind_addrs(&listen_cfg)?;
            let mut actual_addrs = Vec::new();
            for sa in bind_addrs {
                let tcp_listener = std::net::TcpListener::bind(sa)
                    .with_context(|| format!("Could not bind Canban TCP listener on {sa}"))?;
                let local_addr = tcp_listener.local_addr()?.to_string();
                actual_addrs.push(local_addr);
                tcp_listener.set_nonblocking(true)?;
                let weak_tcp = Arc::downgrade(self);
                accept_handles.push(std::thread::spawn(move || {
                    use std::os::fd::AsRawFd;
                    unsafe extern "C" {
                        fn poll(fds: *mut PollFd, nfds: u32, timeout: i32) -> i32;
                    }
                    #[repr(C)]
                    struct PollFd {
                        fd: i32,
                        events: i16,
                        revents: i16,
                    }
                    const POLLIN: i16 = 0x0001;
                    let raw_fd = tcp_listener.as_raw_fd();
                    while let Some(server) = weak_tcp.upgrade() {
                        if server.stop.load(Ordering::SeqCst) {
                            break;
                        }
                        match tcp_listener.accept() {
                            Ok((stream, _)) => {
                                let conn_id =
                                    server.next_tcp_conn_id.fetch_add(1, Ordering::Relaxed);
                                {
                                    let mut active = server.active_tcp_streams.lock().unwrap();
                                    if server.stopped() {
                                        let _ = stream.shutdown(std::net::Shutdown::Both);
                                        break;
                                    }
                                    if let Ok(cloned) = stream.try_clone() {
                                        active.insert(conn_id, cloned);
                                    }
                                }
                                let weak_conn = Arc::downgrade(&server);
                                let guard = ActiveTcpStreamGuard {
                                    server: weak_conn.clone(),
                                    conn_id,
                                };
                                std::thread::spawn(move || {
                                    let _ = AppServer::serve_tcp_weak(weak_conn, stream, guard);
                                });
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                drop(server);
                                let mut pfd = PollFd {
                                    fd: raw_fd,
                                    events: POLLIN,
                                    revents: 0,
                                };
                                unsafe {
                                    let _ = poll(&mut pfd, 1, 50);
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }));
            }
            *self.bound_tcp.lock().unwrap() = actual_addrs;
        }
        self.migrate_exit_failures()?;
        self.start_runtime();
        self.start_github_workers();
        self.start_update_checks();
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            let mut next_sync = std::time::Instant::now();
            let mut next_fast_pr = std::time::Instant::now() + Duration::from_secs(25);
            let mut prev_auth_state = String::new();
            while let Some(server) = weak.upgrade() {
                if server.stopped() {
                    break;
                }
                if canban_updater::handoff::active(&server.home) {
                    drop(server);
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                }
                if let Err(e) = server.github.poll() {
                    eprintln!("Canban GitHub sign-in: {e}");
                }
                let auth_state = server.github.status().state;
                if auth_state != prev_auth_state {
                    if auth_state == "connected" && !prev_auth_state.is_empty() {
                        server.github_sync_due.store(true, Ordering::SeqCst);
                        server.emit("github", None);
                    }
                    prev_auth_state = auth_state;
                }
                let now_instant = std::time::Instant::now();
                if now_instant >= next_sync || server.github_sync_due.swap(false, Ordering::SeqCst)
                {
                    next_sync = now_instant + Duration::from_secs(120);
                    next_fast_pr = now_instant + Duration::from_secs(25);
                    server.reconcile_project_github_bindings();
                    let projects = server
                        .store
                        .lock()
                        .unwrap()
                        .db()
                        .list::<Project>()
                        .unwrap_or_default();
                    for project in projects.into_iter().filter(|p| p.github.is_some()) {
                        let _ = server.queue_github_sync(&project.id, false);
                    }
                    server.github_refresh_queries();
                    let cards = server
                        .store
                        .lock()
                        .unwrap()
                        .db()
                        .list::<Card>()
                        .unwrap_or_default();
                    for c in cards.into_iter().filter(|c| {
                        c.active()
                            && !c.resolved()
                            && (c.pull_request.as_ref().is_some_and(|pr| !pr.merged)
                                || c.is_github_pr())
                    }) {
                        let _ = server.github_pr_refresh(json!({"card_id":c.id}));
                    }
                } else if now_instant >= next_fast_pr {
                    next_fast_pr = now_instant + Duration::from_secs(25);
                    let cards = server
                        .store
                        .lock()
                        .unwrap()
                        .db()
                        .list::<Card>()
                        .unwrap_or_default();
                    let client = server.github_cached_client().ok();
                    for c in cards.into_iter().filter(|c| {
                        if !c.active() || c.resolved() {
                            return false;
                        }
                        if let Some(pr) = &c.pull_request {
                            return !pr.merged
                                && matches!(
                                    pr.checks.as_str(),
                                    "" | "unknown" | "pending" | "running"
                                );
                        }
                        if c.is_github_pr()
                            && let Some(src) = c.github_issue()
                            && let Some(client) = client.as_ref()
                        {
                            let is_open = client
                                .cached::<Value>(&format!(
                                    "/repos/{}/pulls/{}",
                                    src.repository, src.number
                                ))
                                .is_some_and(|p| {
                                    p["merged"] != true && p["state"].as_str() == Some("open")
                                });
                            if !is_open {
                                return false;
                            }
                            return client
                                .cached::<Value>(&format!(
                                    "/repos/{}/pulls/{}/canban-summary",
                                    src.repository, src.number
                                ))
                                .and_then(|s| {
                                    s["checks_summary"]["state"].as_str().map(str::to_owned)
                                })
                                .is_some_and(|st| {
                                    matches!(st.as_str(), "running" | "pending" | "unknown")
                                });
                        }
                        false
                    }) {
                        let _ = server.github_pr_refresh(json!({"card_id":c.id}));
                    }
                }
                drop(server);
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        Ok(Running {
            server: self.clone(),
            accept_handles,
        })
    }
}

pub(crate) fn write_json_line<W: Write>(writer: &mut W, value: &Value) -> std::io::Result<()> {
    let mut buf = std::io::BufWriter::with_capacity(64 * 1024, writer);
    serde_json::to_writer(&mut buf, value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    buf.write_all(b"\n")?;
    buf.flush()
}

impl AppServer {
    pub(super) fn serve(&self, mut stream: UnixStream) -> Result<()> {
        // macOS accept() can inherit O_NONBLOCK from the listener. Explicitly
        // use blocking per-client IO so large JSON frames cannot be truncated.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(20)))?;
        stream.set_write_timeout(Some(Duration::from_secs(90)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut first = true;
        loop {
            let mut line = String::new();
            let n = (&mut reader).take(48 * 1024 * 1024).read_line(&mut line)?;
            if n == 0 || !line.ends_with('\n') {
                return Ok(());
            }
            let input: Value = serde_json::from_str(&line)?;
            if input["method"] == "herdr.connect" {
                ensure!(
                    first,
                    "herdr.connect must be the first frame on a relay socket"
                );
                drop(reader);
                return self.relay(stream, &input["params"]);
            }
            first = false;
            let id = input["id"].clone();
            let reply = match self.call(strp(&input, "method")?, input["params"].clone()) {
                Ok(result) => {
                    let mut reply = json!({"jsonrpc":"2.0","id":id});
                    reply["result"] = result;
                    reply
                }
                Err(e) => {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":format!("{e:#}")}})
                }
            };
            {
                let _span = canban_profiler::Span::new("rpc.encode");
                write_json_line(&mut stream, &reply)?;
            }
            if self.stopped() {
                return Ok(());
            }
            // Keep-alive idle timeout allows AppClient to reuse sockets across rapid RPCs.
            let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
        }
    }
    pub(super) fn serve_tcp_weak(
        weak: std::sync::Weak<Self>,
        mut stream: std::net::TcpStream,
        _active_guard: ActiveTcpStreamGuard,
    ) -> Result<()> {
        stream.set_nonblocking(false)?;
        let _ = stream.set_nodelay(true);
        stream.set_read_timeout(Some(Duration::from_secs(20)))?;
        stream.set_write_timeout(Some(Duration::from_secs(90)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut first = true;
        let mut authed = false;
        loop {
            let mut line = String::new();
            let max_frame = 48 * 1024 * 1024;
            let n = (&mut reader).take(max_frame).read_line(&mut line)?;
            if n == 0 || !line.ends_with('\n') {
                return Ok(());
            }
            let Some(this) = weak.upgrade() else {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return Ok(());
            };
            if this.stopped() {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return Ok(());
            }
            let input: Value = serde_json::from_str(&line)?;
            let id = input["id"].clone();
            let method = strp(&input, "method")?;
            if method == "host.probe" && first {
                let reply = json!({"jsonrpc":"2.0","id":id,"result":this.host_probe_payload()});
                write_json_line(&mut stream, &reply)?;
                first = false;
                continue;
            }
            if !authed {
                let provided = input["auth"].as_str().unwrap_or("");
                let expected = network::read_server_token(&this.home).unwrap_or_default();
                if !network::verify_token(&expected, provided) {
                    let err_reply = json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32001,
                            "message": "Authentication required: invalid or missing server token"
                        }
                    });
                    let _ = write_json_line(&mut stream, &err_reply);
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return Ok(());
                }
                authed = true;
            }
            if method == "herdr.connect" {
                ensure!(
                    first,
                    "herdr.connect must be the first frame on a relay socket"
                );
                drop(reader);
                return this.relay_tcp(stream, &input["params"]);
            }
            first = false;
            let reply = match this.call(method, input["params"].clone()) {
                Ok(result) => {
                    let mut reply = json!({"jsonrpc":"2.0","id":id});
                    reply["result"] = result;
                    reply
                }
                Err(e) => {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":format!("{e:#}")}})
                }
            };
            {
                let _span = canban_profiler::Span::new("rpc.encode");
                write_json_line(&mut stream, &reply)?;
            }
            if this.stopped() {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return Ok(());
            }
            drop(this);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
        }
    }
    pub(super) fn tick(&self) -> Result<()> {
        if canban_updater::handoff::active(&self.home) {
            return Ok(());
        }

        self.store
            .lock()
            .unwrap()
            .write(|db| events::pump(db, &self.config()))?;
        self.request_deliveries();
        self.request_scripts();
        let _ = self.pump_cardless_agents();
        let _ = self.pump_resolved_archives();
        self.observe_terminals()?;
        self.flush_agent_work()?;
        self.reconcile_merge_batches()?;
        self.pump_configuration()?;
        self.pump_schedules()?;
        self.pump_merge_batches()?;
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let bound: Vec<_> = self
            .store
            .lock()
            .unwrap()
            .db()
            .list::<TerminalEntry>()?
            .into_iter()
            .filter(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.terminal.is_some()
                        && (!a.finished()
                            || a.observed
                                .as_ref()
                                .is_some_and(|v| v["state"] == "finished")
                                && a.output_path.is_none())
                })
            })
            .collect();
        if !bound.is_empty() {
            let (core_id, values) = match self.runtime_inventory() {
                Ok(value) => value,
                Err(e) => {
                    for entry in &bound {
                        self.connection_lost(
                            entry,
                            "unavailable",
                            &format!("Herdr unavailable: {e:#}"),
                        )?;
                    }
                    (String::new(), Vec::new())
                }
            };
            for entry in bound {
                if core_id.is_empty() {
                    continue;
                }
                let run = entry.agent.as_ref().unwrap();
                if run.core_id.as_deref() != Some(core_id.as_str()) {
                    self.connection_lost(&entry,"replaced","Original Herdr core identity changed; inspect the original runtime before starting a replacement")?;
                    continue;
                }
                let Some(value) = values
                    .iter()
                    .find(|v| v["address"].as_str() == run.terminal.as_deref())
                else {
                    self.runtime_missing(&entry)?;
                    continue;
                };
                let finished = value["state"] == "finished";
                if finished && run.output_path.is_none() {
                    let _ = self.capture_output(&entry);
                }
                let observed = canban_herdr::observation(value);
                self.observe_lifecycle(&entry, &observed)?;
                if run.observed.as_ref() != Some(&observed)
                    || run.connection_state != "connected"
                    || (finished && run.close_state != "closed")
                {
                    let card_id = run.card_id.clone();
                    let run_terminal = run.terminal.clone();
                    let run_number = run.number;
                    self.store.lock().unwrap().write(|db| {
                        let mut current_entry: TerminalEntry = db.get(&entry.id)?;
                        let Some(current) = &mut current_entry.agent else {
                            return Ok(());
                        };
                        current.observed = Some(observed.clone());
                        if matches!(
                            current.connection_state.as_str(),
                            "unavailable" | "replaced"
                        ) {
                            log(
                                db,
                                &db.get::<Card>(&card_id)?,
                                "execution.reconnected",
                                "system",
                                "Reconnected to the original terminal",
                                Some(entry.id.clone()),
                                json!({"terminal":run_terminal}),
                            )?;
                        }
                        current.connection_state = "connected".into();
                        if finished {
                            current.close_state = "closed".into();
                            if current.lifecycle.close_reason == "idle" {
                                current.state = "idle_closed".into();
                                let closed_msg = if current.provider_session_id.is_some() {
                                    "Idle Agent exited; exact conversation retained for resume"
                                } else {
                                    "Idle Agent terminal closed; Card history retained"
                                };
                                log(
                                    db,
                                    &db.get::<Card>(&card_id)?,
                                    "execution.idle_closed",
                                    "system",
                                    closed_msg,
                                    Some(entry.id.clone()),
                                    json!({"run_number":run_number}),
                                )?;
                            } else if matches!(
                                current.state.as_str(),
                                "running" | "working" | "starting" | "needs_report"
                            ) {
                                let card = db.get::<Card>(&card_id)?;
                                if card.resolved() {
                                    current.state = "completed".into();
                                    current.completed_at = Some(now());
                                    current.close_state = String::new();
                                } else {
                                    current.state = "failed".into();
                                    current.error =
                                        Some("Agent exited without reporting an outcome".into());
                                    log(
                                        db,
                                        &card,
                                        "execution.failed",
                                        "system",
                                        current.error.as_deref().unwrap(),
                                        Some(entry.id.clone()),
                                        json!({"run_number":run_number,"exit_code":observed["exit_code"]}),
                                    )?;
                                }
                            }
                        }
                        db.save(&current_entry)
                    })?;
                    let agent_stopped = finished
                        || matches!(
                            observed["status"]["state"].as_str(),
                            Some("idle" | "done" | "blocked" | "error")
                        )
                        || observed["status"]["state"].is_null();
                    if agent_stopped {
                        self.request_deliveries();
                        self.request_scripts();
                    }
                    if finished {
                        let _ = self.try_auto_archive_resolved(&card_id);
                    }
                    self.emit("runtime", Some(card_id));
                }
            }
        }
        let timed_out_closing: Vec<TerminalEntry> = self
            .store
            .lock()
            .unwrap()
            .db()
            .list::<TerminalEntry>()?
            .into_iter()
            .filter(|t| {
                t.agent.as_ref().is_some_and(|a| {
                    a.close_state == "closing"
                        && a.close_requested_at
                            .is_some_and(|ts| now().saturating_sub(ts) > 30_000)
                })
            })
            .collect();
        for entry in timed_out_closing {
            let run = entry.agent.as_ref().unwrap();
            if run.lifecycle.close_reason == "idle" {
                let fallback = (|| -> Result<bool> {
                    let Some(terminal) = run.terminal.as_deref() else {
                        return Ok(false);
                    };
                    if self.verified_core_id().ok().as_deref() != run.core_id.as_deref() {
                        return Ok(false);
                    }
                    let actual = client.get(terminal)?;
                    if actual["state"] == "finished" {
                        return Ok(true);
                    }
                    if actual["state"] != "live"
                        || !matches!(actual["status"]["state"].as_str(), Some("idle" | "done"))
                        || self.is_refined_working_now(&client, &entry, &actual)
                    {
                        return Ok(false);
                    }
                    let _ = self.capture_output(&entry);
                    client.call("terminal.kill", json!({"terminal": terminal}))?;
                    Ok(true)
                })();
                match fallback {
                    Ok(true) => {
                        self.store.lock().unwrap().write(|db| {
                            let mut current_entry: TerminalEntry = db.get(&entry.id)?;
                            if let Some(current) = &mut current_entry.agent
                                && current.close_state == "closing"
                            {
                                current.close_requested_at = Some(now());
                                db.save(&current_entry)?;
                            }
                            Ok(())
                        })?;
                        continue;
                    }
                    Ok(false) => {
                        // Agent resumed working or Core changed while closing; cancel the idle close cleanly.
                        self.store.lock().unwrap().write(|db| {
                            let mut current_entry: TerminalEntry = db.get(&entry.id)?;
                            if let Some(current) = &mut current_entry.agent
                                && current.close_state == "closing"
                            {
                                current.close_state.clear();
                                current.close_requested_at = None;
                                current.lifecycle.close_reason.clear();
                                db.save(&current_entry)?;
                            }
                            Ok(())
                        })?;
                        continue;
                    }
                    Err(_) => {}
                }
            }
            self.store.lock().unwrap().write(|db| {
                let mut current_entry: TerminalEntry = db.get(&entry.id)?;
                let Some(current) = &mut current_entry.agent else {
                    return Ok(());
                };
                if current.close_state != "closing" {
                    return Ok(());
                }
                current.close_state = "error".into();
                current.error = Some(
                    "Agent did not exit after the normal exit request. Inspect its terminal or use End session."
                        .into(),
                );
                let err_msg = current.error.clone().unwrap();
                let card_id = current.card_id.clone();
                let close_reason = current.lifecycle.close_reason.clone();
                let run_num = current.number;
                let mut card: Card = db.get(&card_id)?;
                if close_reason == "idle" {
                    db.save(&canban_notifications::attention(
                        &card.project_id,
                        &card.id,
                        &err_msg,
                    ))?;
                    log(
                        db,
                        &card,
                        "execution.idle_close_failed",
                        "system",
                        &err_msg,
                        Some(entry.id.clone()),
                        json!({"run_number":run_num}),
                    )?;
                } else {
                    attention(db, &mut card, &err_msg)?;
                    card.revision += 1;
                    db.save(&card)?;
                }
                db.save(&current_entry)
            })?;
        }
        self.deliver_questions(&client)?;
        self.close_idle(&client)?;
        let _ = self.pump_admissions();
        let snapshot = self.snapshot()?;
        if self.observer_running.load(Ordering::SeqCst) && self.runtime_inventory().is_err() {
            return Ok(());
        }
        let next_entry = canban_scheduler::next(&snapshot, self.config().scheduler.max_agents);
        if let Some(next_entry) = next_entry {
            let admitted = self.store.lock().unwrap().write(|db| {
                let mut entry: TerminalEntry = db.get(&next_entry.id)?;
                let current = db.snapshot()?;
                if canban_scheduler::active_count(&current) >= self.config().scheduler.max_agents
                    || !canban_scheduler::eligible(&current, &entry)
                {
                    return Ok(None);
                }
                let continue_in_place = canban_scheduler::can_continue_in_place(&current, &entry);
                let Some(agent) = &mut entry.agent else {
                    return Ok(None);
                };
                let card: Card = db.get(&agent.card_id)?;
                if !card.active() {
                    if agent.state == "queued" {
                        agent.state = "cancelled".into();
                    }
                    agent.pending = None;
                    db.save(&entry)?;
                    return Ok(None);
                }
                if continue_in_place {
                    return Ok(Some((entry, true, None)));
                }
                let stale_terminal = (!agent.finished())
                    .then(|| {
                        agent
                            .terminal
                            .clone()
                            .map(|addr| (addr, agent.core_id.clone()))
                    })
                    .flatten();
                if agent.pending.is_some() {
                    let number = u32::try_from(db.counter(&format!("card-attempt:{}", card.id))?)?;
                    let had_session = agent.provider_session_id.is_some();
                    agent.promote_pending();
                    agent.card_number = card.number;
                    agent.number = number;
                    agent.resume_session = had_session && !agent.trigger.is_automatic();
                    if !agent.resume_session {
                        agent.workspace = None;
                        agent.cwd.clear();
                        agent.provider_session_id = None;
                    }
                    agent.terminal = None;
                    agent.core_id = None;
                    agent.observed = None;
                    agent.output_path = None;
                    agent.response_path = None;
                    agent.close_state.clear();
                    agent.connection_state.clear();
                }
                let project: Project = db.get(&agent.project_id)?;
                agent.directory = Some(if agent.resume_session {
                    agent
                        .directory
                        .clone()
                        .or_else(|| agent.workspace.as_ref().map(|w| w.directory.clone()))
                        .or_else(|| card.workspace.as_ref().map(|w| w.directory.clone()))
                        .unwrap_or_else(|| project.directory.clone())
                } else {
                    project.directory.clone()
                });
                if agent.started_at.is_none() {
                    agent.started_at = Some(now());
                }
                agent.state = "starting".into();
                db.save(&entry)?;
                Ok(Some((entry, false, stale_terminal)))
            })?;
            if let Some((entry, continue_in_place, stale_terminal)) = admitted {
                let res = if continue_in_place {
                    self.continue_assignment(&entry, &snapshot)
                } else {
                    if let Some((addr, expected_core)) = stale_terminal
                        && expected_core.as_deref().is_none_or(|cid| {
                            client
                                .status()
                                .ok()
                                .and_then(|s| s["core"]["id"].as_str().map(str::to_owned))
                                .as_deref()
                                == Some(cid)
                        })
                    {
                        let _ = client.call("terminal.kill", json!({ "terminal": addr }));
                    }
                    self.launch(&entry, &snapshot)
                };
                if let Err(e) = res
                    && !continue_in_place
                {
                    self.launch_failed(&entry, &format!("{e:#}"))?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn connection_lost(
        &self,
        entry: &TerminalEntry,
        state: &str,
        message: &str,
    ) -> Result<()> {
        let Some(run) = &entry.agent else {
            return Ok(());
        };
        if run.connection_state == state {
            return Ok(());
        }
        let card_id = run.card_id.clone();
        let terminal = run.terminal.clone();
        self.store.lock().unwrap().write(|db| {
            let mut current_entry: TerminalEntry = db.get(&entry.id)?;
            let Some(current) = &mut current_entry.agent else {
                return Ok(());
            };
            if current.connection_state == state {
                return Ok(());
            }
            current.connection_state = state.into();
            db.save(&current_entry)?;
            let card: Card = db.get(&card_id)?;
            log(
                db,
                &card,
                "execution.disconnected",
                "system",
                message,
                Some(entry.id.clone()),
                json!({"terminal":terminal,"connection_state":state}),
            )
        })?;
        self.emit("runtime", Some(card_id));
        Ok(())
    }
    pub(super) fn deliver_questions(&self, client: &canban_herdr::Client) -> Result<()> {
        let snapshot = self.snapshot()?;
        for question in snapshot
            .questions
            .iter()
            .filter(|q| matches!(q.state.as_str(), "pending" | "delivered"))
        {
            let target_entry = question
                .terminal_id
                .as_ref()
                .and_then(|tid| snapshot.terminals.iter().find(|t| &t.id == tid))
                .or_else(|| {
                    snapshot.terminals.iter().rev().find(|t| {
                        t.agent.as_ref().is_some_and(|a| {
                            a.card_id == question.card_id && a.profile.id == question.profile_id
                        })
                    })
                });
            let Some(entry) = target_entry else {
                continue;
            };
            let Some(att) = entry.agent.as_ref() else {
                continue;
            };
            if matches!(att.state.as_str(), "completed" | "backlog" | "idle_closed")
                && !(att.terminal.is_some() && !att.finished())
                && att.pending.is_none()
                && question.state == "pending"
            {
                // A question that raced task completion follows the same conversation
                // after close. It is never silently redirected to the Reviewer.
                let config = self.config();
                self.store.lock().unwrap().write(|db| {
                    let mut q: Question = db.get(&question.id)?;
                    if q.state == "replied" {
                        return Ok(());
                    }
                    let mut card: Card = db.get(&q.card_id)?;
                    if !card.active() {
                        return Ok(());
                    }
                    let project: Project = db.get(&card.project_id)?;
                    let stage = canban_workflow_engine::stage(&project.workflow, &card.stage)?;
                    let is_card_created = db
                        .get::<Event>(&q.comment_id)
                        .ok()
                        .is_some_and(|e| e.kind == "card.created");
                    let instruction = crate::sessions::render_card_mention_instruction(
                        &q.profile_id,
                        if is_card_created { None } else { Some(&q.body) },
                    );
                    let follow_up = enqueue(
                        db,
                        &config,
                        &mut card,
                        &project,
                        stage,
                        ExecutionTrigger::Comment {
                            comment_id: q.comment_id.clone(),
                        },
                        Some(&q.profile_id),
                        Some(instruction),
                    )?
                    .context("Could not retain question")?;
                    q.terminal_id = Some(follow_up.id);
                    q.state = "pending".into();
                    db.save(&q)
                })?;
                continue;
            }
            if question.state != "pending"
                || !matches!(
                    att.state.as_str(),
                    "running" | "working" | "blocked" | "completed"
                )
            {
                continue;
            }
            let Some(terminal) = &att.terminal else {
                continue;
            };
            if att
                .observed
                .as_ref()
                .is_some_and(|v| v["state"] == "finished")
            {
                continue;
            }
            if att.runtime_status() == "blocked" || att.connection_state != "connected" {
                continue;
            }
            let Some(card) = snapshot.cards.iter().find(|c| c.id == question.card_id) else {
                continue;
            };
            self.store.lock().unwrap().write(|db| {
                let mut q: Question = db.get(&question.id)?;
                q.state = "delivering".into();
                q.terminal_id = Some(entry.id.clone());
                db.save(&q)
            })?;
            let qid_quoted = canban_context::shell_quote(&question.id);
            let proj_quoted = canban_context::shell_quote(&card.project_id);
            let card_number = card.number.to_string();
            let text = canban_context::render_template(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../prompts/runtime/live_question_delivery.txt"
                ))
                .trim_end(),
                &[
                    ("question_id", &question.id),
                    ("question_body", &question.body),
                    ("question_id_quoted", &qid_quoted),
                    ("project_id", &proj_quoted),
                    ("card_number", &card_number),
                ],
            );
            let result = (|| -> Result<Value> {
                ensure!(
                    Some(self.verified_core_id()?) == att.core_id,
                    "Original core identity changed"
                );
                client.call("terminal.send", json!({"terminal":terminal,"text":text}))
            })();
            self.store.lock().unwrap().write(|db|{
                let mut q:Question=db.get(&question.id)?;
                if q.state=="replied"{return Ok(());}
                match &result {Ok(_)=>q.state="delivered".into(),Err(e)=>{q.state="delivery_unknown".into();q.error=Some(format!("{e:#}"));let mut c:Card=db.get(&q.card_id)?;attention(db,&mut c,"Question delivery needs inspection; open the Agent terminal. It was not automatically resent.")?;c.revision+=1;db.save(&c)?;}}
                db.save(&q)
            })?;
            self.emit("question", Some(question.card_id.clone()));
        }
        Ok(())
    }
    pub(super) fn launch(&self, entry: &TerminalEntry, snapshot: &Snapshot) -> Result<()> {
        let _gate = self.terminal_gate.lock().unwrap();
        let att = entry
            .agent
            .as_ref()
            .context("Terminal has no Agent execution to launch")?;
        ensure!(
            self.store
                .lock()
                .unwrap()
                .db()
                .get::<Card>(&att.card_id)?
                .active(),
            "Card was archived before launch"
        );
        let project = snapshot
            .projects
            .iter()
            .find(|p| p.id == att.project_id)
            .context("Project missing")?;
        let card = snapshot
            .cards
            .iter()
            .find(|c| c.id == att.card_id)
            .context("Card missing")?;
        let directory = att
            .directory
            .clone()
            .context("Execution directory was not admitted")?;
        let ops = self.ops();
        let resuming = att.resume_session;
        let mut project = project.clone();
        let wf_run = if let Some(id) = &att.workflow_run_id {
            let wf = self.store.lock().unwrap().db().get::<WorkflowRun>(id)?;
            project.workflow.workspace = wf.definition.workspace.clone();
            Some(wf)
        } else {
            None
        };
        project.directory = directory.clone();
        let mut prepared = card.clone();
        if resuming && !att.cwd.is_empty() {
            prepared.worktree = Some(att.cwd.clone());
            if let Some(w) = &att.workspace {
                prepared.branch = w.branch.clone();
                prepared.base_commit = w.base_commit.clone();
                project.target_branch = w.target_branch.clone();
            }
        } else if prepared
            .workspace
            .as_ref()
            .is_some_and(|w| w.directory != directory)
        {
            ensure!(
                card.delivery.is_some(),
                "This Card has undelivered work in its previous directory. Accept/hand off that work before starting a stage in the new directory; Resume still uses the original workspace."
            );
            prepared.worktree = None;
            prepared.branch = None;
            prepared.base_commit = None;
        }
        let mut pr_tracking = None;
        let cwd = if resuming && !att.cwd.is_empty() {
            ops.resume(&self.home, &project, &prepared)?
        } else {
            pr_tracking = self.prepare_pr_card_source(&prepared, &mut project, &ops)?;
            ops.prepare(&self.home, &project, &prepared)?
        };
        let cwd_str = cwd.to_string_lossy().into_owned();
        if !resuming && card.worktree.as_deref() != Some(cwd_str.as_str()) {
            self.git_changed();
        }
        prepared.worktree = Some(cwd_str.clone());
        let (branch, base_commit) = ops.workspace_head_meta(&cwd, &prepared);
        if let Some(target) = &pr_tracking
            && cwd != Path::new(&directory)
            && let Some(branch) = branch.as_deref().filter(|b| !b.is_empty())
        {
            ops.configure_pr_tracking(&cwd, branch, target)?;
        }
        let workspace = WorkspaceBinding {
            directory: directory.clone(),
            path: cwd_str,
            branch,
            base_commit,
            target_branch: project.target_branch.clone(),
        };
        if !resuming
            || card
                .workspace
                .as_ref()
                .is_none_or(|w| w.directory == directory)
        {
            self.store.lock().unwrap().write(|db| {
                let mut current: Card = db.get(&card.id)?;
                if current.workspace.as_ref() != Some(&workspace) {
                    if let Some(old) = current.workspace.replace(workspace.clone())
                        && old.directory != workspace.directory
                    {
                        current.workspace_history.push(old);
                        current.delivery = None;
                    }
                    current.worktree = Some(workspace.path.clone());
                    current.branch = workspace.branch.clone();
                    current.base_commit = workspace.base_commit.clone();
                    log(
                        db,
                        &current,
                        "workspace.prepared",
                        "system",
                        "Card workspace prepared",
                        Some(entry.id.clone()),
                        json!({"workspace":workspace}),
                    )?;
                    db.save(&current)?;
                }
                Ok(())
            })?;
        }
        let card_events = self
            .store
            .lock()
            .unwrap()
            .db()
            .events_for_card(&prepared.id)?;
        let input = canban_context::capture(
            &self.home,
            &project,
            &prepared,
            entry,
            snapshot,
            &card_events,
        )?;
        let (mut context, fresh) = self.store.lock().unwrap().write(|db| {
            let mut c = crate::sessions::conversation(db, card, &att.profile)?;
            if resuming && att.provider_session_id.is_some() {
                c.provider_session_id = att.provider_session_id.clone();
            } else if !resuming
                && snapshot
                    .terminals
                    .iter()
                    .find(|t| t.id == c.id)
                    .and_then(|t| t.agent.as_ref())
                    .and_then(|a| a.directory.as_deref())
                    .is_some_and(|d| d != directory)
            {
                c.provider_session_id = None;
            }
            let fresh = c.provider_session_id.is_none();
            if fresh
                && att
                    .profile
                    .command()
                    .iter()
                    .any(|a| a.contains("{session_id}"))
            {
                c.provider_session_id = Some(uuid_session_id());
            }
            db.save(&c)?;
            Ok((c, fresh))
        })?;
        let mut updated_entry = entry.clone();
        let mut run = att.clone();
        run.workspace = Some(workspace);
        run.lifecycle = Lifecycle::default();
        if run.started_at.is_none() {
            run.started_at = Some(now());
        }
        run.state = "starting".into();
        run.core_id = None;
        run.terminal = None;
        run.cwd = cwd.to_string_lossy().into_owned();
        run.input_path = input.clone();
        run.output_path = None;
        run.observed = None;
        run.error = None;
        run.provider_session_id = context.provider_session_id.clone();
        run.close_state.clear();
        run.turn_ended_at = None;
        run.response_path = None;
        run.completed_at = None;
        run.completion_turn = None;
        run.close_requested_at = None;
        run.connection_state.clear();
        updated_entry.agent = Some(run.clone());
        let run_dir_path = canban_context::run_dir(&self.home, &updated_entry);
        let endpoint = self.endpoint().to_string_lossy().into_owned();
        let (mut env, custom_env) =
            crate::scripts::build_step_env(crate::scripts::StepEnvContext {
                cli: &self.cli,
                exec_home: &self.home,
                endpoint: &endpoint,
                project: &project,
                card: Some(card),
                workspace: run.workspace.as_ref().or(card.workspace.as_ref()),
                run_dir: &run_dir_path,
                workflow_run: wf_run.as_ref(),
                step_ref: att
                    .workflow_step
                    .as_deref()
                    .or_else(|| wf_run.as_ref().and_then(|r| r.step.as_deref())),
                script_env_file: None,
                script_output_file: None,
            });
        env.insert(
            "CANBAN_SESSION_ID".into(),
            context.provider_session_id.clone().unwrap_or_default(),
        );
        // Do not misidentify a child invocation using its launching Agent's ID.
        env.insert("CANBAN_PROFILE_ID".into(), att.profile.id.clone());
        run.applied_env = custom_env;
        updated_entry.agent = Some(run.clone());
        self.store.lock().unwrap().write(|db| {
            let mut current: TerminalEntry = db.get(&entry.id)?;
            if let Some(cur_a) = &current.agent
                && cur_a.pending.is_some()
            {
                run.pending = cur_a.pending.clone();
            }
            current.agent = Some(run.clone());
            db.save(&current)
        })?;
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let core_id = self.verified_core_id()?;
        let labels = std::collections::BTreeMap::from([
            ("canban.project".into(), att.project_id.clone()),
            ("canban.card".into(), card.number.to_string()),
            ("canban.card_id".into(), att.card_id.clone()),
            ("canban.terminal".into(), entry.id.clone()),
        ]);
        let prompt = std::fs::read_to_string(self.home.join(input))?;
        let mut argv = canban_profiles::command(
            &att.profile,
            context.provider_session_id.as_deref(),
            fresh,
            &[
                ("home", &self.home.to_string_lossy()),
                ("cwd", &run.cwd),
                ("project_dir", &project.directory),
                ("prompt", &prompt),
                ("cli", &self.cli.to_string_lossy()),
            ],
        )?;
        let is_herdr_template = att
            .profile
            .preset
            .as_deref()
            .is_some_and(|s| s.starts_with("herdr:"));
        if !is_herdr_template {
            argv[0] = canban_profiles::executable_with_env(&argv[0], &client.config.hc_env);
        }
        let spec = canban_profiles::prepare_launch(canban_profiles::LaunchRequest {
            profile: Some(&att.profile),
            argv,
            env,
            cwd: run.cwd.clone(),
        })?;
        let env_policy = canban_exec::shell_env::LocalEnvPolicy::for_source_trust(
            client.config.terminal_env_mode.is_inherit(),
            Some(card.trust_level()),
        );
        let terminal =
            client.launch_with_policy(spec.argv, &spec.cwd, spec.env, labels, env_policy)?;
        let terminal_address = strp(&terminal, "address")?.to_owned();
        run.core_id = Some(core_id.clone());
        run.terminal = Some(terminal_address.clone());
        let delivery_error=terminal.get("first_message").filter(|v|v["sent"]==false).map(|v|format!("Herdr did not deliver the initial task: {v}. Inspect this terminal before retrying."));
        run.observed = Some(canban_herdr::observation(&terminal));
        run.lifecycle.observed_at = Some(now());
        run.connection_state = "connected".into();
        run.state = "running".into();
        // Seed the in-memory inventory immediately so a runtime tick racing ahead
        // of Herdr's async "created" subscription event does not transiently mark
        // the freshly launched terminal missing/unavailable.
        {
            let mut inv = self.inventory.lock().unwrap();
            if inv.core == core_id {
                inv.records
                    .entry(terminal_address.clone())
                    .or_insert_with(|| {
                        let mut seeded = terminal.clone();
                        seeded["address"] = json!(terminal_address);
                        if seeded["state"].is_null() {
                            seeded["state"] = json!("live");
                        }
                        if seeded["cwd"].is_null() {
                            seeded["cwd"] = json!(run.cwd);
                        }
                        seeded
                    });
            }
        }
        self.store.lock().unwrap().write(|db| {
            let mut current_entry: TerminalEntry = db.get(&entry.id)?;
            if let Some(current) = current_entry.agent.clone() {
                if current.state != "starting" {
                    run.state = current.state;
                    run.completed_at = current.completed_at;
                    run.completion_turn = current.completion_turn;
                    run.close_state = current.close_state;
                    run.turn_ended_at = current.turn_ended_at;
                    run.error = current.error;
                }
                if current.provider_session_id.is_some() {
                    run.provider_session_id = current.provider_session_id;
                }
                if current.response_path.is_some() {
                    run.response_path = current.response_path;
                }
                if current.pending.is_some() {
                    run.pending = current.pending;
                }
            }
            current_entry.agent = Some(run.clone());
            db.save(&current_entry)?;
            let c: Card = db.get(&att.card_id)?;
            log(
                db,
                &c,
                "execution.started",
                "system",
                &format!("@{} started", att.profile.id),
                Some(entry.id.clone()),
                json!({"run_number":run.number,"terminal":run.terminal,"core_id":run.core_id,"directory":run.directory,"cwd":run.cwd}),
            )
        })?;
        self.store.lock().unwrap().write(|db| {
            for q in snapshot
                .questions
                .iter()
                .filter(|q| q.terminal_id.as_deref() == Some(&entry.id) && q.state == "pending")
            {
                let mut current: Question = db.get(&q.id)?;
                if current.state == "pending" {
                    current.state = "delivered".into();
                    db.save(&current)?;
                }
            }
            // A callback can have bound the provider ID before launch returned.
            context = db.get(&context.id)?;
            Ok(())
        })?;
        self.emit("execution", Some(att.card_id.clone()));
        if let Some(error) = delivery_error {
            bail!("{error}");
        }
        Ok(())
    }
    pub(super) fn launch_failed(&self, entry: &TerminalEntry, error: &str) -> Result<()> {
        let Some(att) = &entry.agent else {
            return Ok(());
        };
        let card_id = att.card_id.clone();
        self.store.lock().unwrap().write(|db| {
            let mut current_entry: TerminalEntry = db.get(&entry.id)?;
            let mut is_unknown = false;
            let mut run_num = att.number;
            if let Some(a) = &mut current_entry.agent {
                run_num = a.number;
                if !matches!(a.state.as_str(), "completed" | "backlog") {
                    a.state = if a.state == "unknown"
                        || a.terminal.is_none() && error.contains("timed out")
                    {
                        "unknown"
                    } else {
                        "failed"
                    }
                    .into();
                    a.error = Some(error.into());
                }
                is_unknown = a.state == "unknown";
                db.save(&current_entry)?;
            }
            let mut c: Card = db.get(&card_id)?;
            if is_unknown || error.contains("undelivered work") {
                attention(db, &mut c, error)?;
                if let Some(gate) = &mut c.human_gate
                    && gate.terminal_id.is_none()
                {
                    gate.terminal_id = Some(entry.id.clone());
                }
            }
            c.revision += 1;
            db.save(&c)?;
            log(
                db,
                &c,
                "execution.failed",
                "system",
                error,
                Some(entry.id.clone()),
                json!({"run_number":run_num}),
            )
        })?;
        self.emit("execution", Some(card_id));
        Ok(())
    }

    /// Drives cardless `WorkflowRun`s (`card_id: None`) paused at `waiting_agent` steps.
    pub(crate) fn pump_cardless_agents(&self) -> Result<()> {
        if !self.observer_running.load(Ordering::SeqCst) {
            return Ok(());
        }
        let runs: Vec<WorkflowRun> = {
            let store = self.store.lock().unwrap();
            store
                .db()
                .list::<WorkflowRun>()?
                .into_iter()
                .filter(|r| r.is_cardless() && r.active() && r.state == "waiting_agent")
                .collect()
        };
        if runs.is_empty() {
            return Ok(());
        }
        let config = self.config();
        let client = canban_herdr::Client {
            config: config.clone(),
        };
        for run in runs {
            let Some(step_id) = run.step.clone() else {
                continue;
            };
            if let Some(meta) = run.inputs.get("agent_terminal").filter(|v| {
                v.is_object()
                    && v.get("step_id")
                        .and_then(|s| s.as_str())
                        .is_none_or(|s| s == step_id)
                    && v.get("visit")
                        .and_then(|visit| visit.as_u64())
                        .is_none_or(|visit| visit == u64::from(run.visit))
            }) {
                let Some(addr) = meta["address"].as_str() else {
                    continue;
                };
                let (finished, exit_code) = match client.get(addr) {
                    Ok(v) if v["state"] == "finished" => (true, v["exit_code"].as_i64()),
                    Ok(_) => (false, None),
                    Err(_) => (true, None),
                };
                if finished {
                    let report_state = if exit_code == Some(0) {
                        "completed"
                    } else {
                        "failed"
                    };
                    let output = json!({
                        "terminal": addr,
                        "exit_code": exit_code,
                    });
                    let _ = self.store.lock().unwrap().write(|db| {
                        let mut current: WorkflowRun = db.get(&run.id)?;
                        if current.active()
                            && current.state == "waiting_agent"
                            && current.step.as_deref() == Some(&step_id)
                            && current.visit == run.visit
                        {
                            canban_workflow_engine::runtime::complete_cardless_agent(
                                db,
                                &mut current,
                                output,
                                report_state,
                            )?;
                        }
                        Ok(())
                    });
                    self.emit("application", None);
                    self.request_scripts();
                    self.wake_runtime();
                }
                continue;
            }

            // Respect global `max_agents` capacity before launching a new cardless agent.
            let has_slot = self
                .snapshot()
                .ok()
                .is_some_and(|s| canban_scheduler::active_count(&s) < config.scheduler.max_agents);
            if !has_slot {
                continue;
            }

            let pending = &run.inputs["pending_agent"];
            let requested_profile = pending["profile"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let instruction = pending["instruction"].as_str().unwrap_or("");
            let profile = requested_profile
                .and_then(|pid| config.profiles.iter().find(|p| p.id == pid))
                .or_else(|| {
                    config
                        .profiles
                        .iter()
                        .find(|p| p.id == config.default_agent)
                })
                .or_else(|| config.profiles.first())
                .cloned();
            let Some(profile) = profile else {
                let _ = self.store.lock().unwrap().write(|db| {
                    let mut current: WorkflowRun = db.get(&run.id)?;
                    canban_workflow_engine::runtime::complete_cardless_agent(
                        db,
                        &mut current,
                        json!({"error": "No configured agent profile"}),
                        "failed",
                    )
                });
                continue;
            };

            let project = {
                let store = self.store.lock().unwrap();
                let db = store.db();
                if is_global_project_id(&run.project_id) {
                    Project {
                        id: GLOBAL_PROJECT_ID.into(),
                        name: "Global".into(),
                        directory: self.home.to_string_lossy().into_owned(),
                        target_branch: None,
                        workflow: Workflow::default(),
                        github: None,
                        detected_github: None,
                        github_disconnected: false,
                        delivery_mode: DeliveryMode::default(),
                        agent_overrides: std::collections::BTreeMap::new(),
                        tags: Vec::new(),
                        created_at: 0,
                    }
                } else {
                    match db.get::<Project>(&run.project_id) {
                        Ok(p) => p,
                        Err(_) => continue,
                    }
                }
            };
            let run_dir = crate::scripts::cardless_run_dir(&self.home, &run.project_id, &run.id);
            let _ = std::fs::create_dir_all(&run_dir);
            let endpoint = self.endpoint().to_string_lossy().into_owned();
            let (mut env, _) = crate::scripts::build_step_env(crate::scripts::StepEnvContext {
                cli: &self.cli,
                exec_home: &self.home,
                endpoint: &endpoint,
                project: &project,
                card: None,
                workspace: None,
                run_dir: &run_dir,
                workflow_run: Some(&run),
                step_ref: Some(&step_id),
                script_env_file: None,
                script_output_file: None,
            });
            env.insert("CANBAN_PROFILE_ID".into(), profile.id.clone());

            let Ok(mut argv) = canban_profiles::command(
                &profile,
                None,
                true,
                &[
                    ("home", &self.home.to_string_lossy()),
                    ("cwd", &project.directory),
                    ("project_dir", &project.directory),
                    ("prompt", instruction),
                    ("cli", &self.cli.to_string_lossy()),
                ],
            ) else {
                continue;
            };
            let is_herdr_template = profile
                .preset
                .as_deref()
                .is_some_and(|s| s.starts_with("herdr:"));
            if !is_herdr_template {
                argv[0] = canban_profiles::executable_with_env(&argv[0], &config.hc_env);
            }
            let Ok(spec) = canban_profiles::prepare_launch(canban_profiles::LaunchRequest {
                profile: Some(&profile),
                argv,
                env,
                cwd: project.directory.clone(),
            }) else {
                continue;
            };
            let env_policy = canban_exec::shell_env::LocalEnvPolicy::for_source_trust(
                config.terminal_env_mode.is_inherit(),
                crate::scripts::cardless_run_source_trust(&run),
            );
            let labels = std::collections::BTreeMap::from([
                ("canban.project".into(), run.project_id.clone()),
                ("canban.workflow_run".into(), run.id.clone()),
                ("canban.workflow_step".into(), step_id.clone()),
            ]);
            match client.launch_with_policy(spec.argv, &spec.cwd, spec.env, labels, env_policy) {
                Ok(launched) => {
                    let addr = launched["address"].as_str().unwrap_or("").to_owned();
                    let _ = self.store.lock().unwrap().write(|db| {
                        let mut current: WorkflowRun = db.get(&run.id)?;
                        if current.active()
                            && current.state == "waiting_agent"
                            && current.step.as_deref() == Some(&step_id)
                            && current.visit == run.visit
                        {
                            current.inputs["agent_terminal"] = json!({
                                "address": addr,
                                "step_id": step_id,
                                "visit": run.visit,
                                "started_at": now(),
                            });
                            current.updated_at = now();
                            db.save(&current)?;
                        }
                        Ok(())
                    });
                    self.emit("application", None);
                }
                Err(e) => {
                    let msg = format!("Failed to launch cardless agent `{step_id}`: {e:#}");
                    let _ = self.store.lock().unwrap().write(|db| {
                        let mut current: WorkflowRun = db.get(&run.id)?;
                        canban_workflow_engine::runtime::complete_cardless_agent(
                            db,
                            &mut current,
                            json!({"error": msg}),
                            "failed",
                        )
                    });
                    self.emit("application", None);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_tcp_streams_are_removed_immediately_after_client_disconnect_or_auth_rejection() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let app = AppServer::open(home.clone(), PathBuf::from("/bin/echo")).unwrap();
        app.config.write().unwrap().hc = "/usr/bin/false".into();
        let running = app
            .start_with_listen(Some(ServerListenConfig {
                tailscale: false,
                port: 0,
                listen: Some("127.0.0.1:0".into()),
            }))
            .unwrap();
        let addr = app.bound_tcp_addrs().into_iter().next().unwrap();
        let token = network::read_server_token(&home).unwrap();

        // 1. Open an idle authenticated TCP socket and verify it is tracked while open.
        let mut live_stream = std::net::TcpStream::connect(&addr).unwrap();
        live_stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        writeln!(
            live_stream,
            "{}",
            json!({"jsonrpc":"2.0","id":1,"method":"app.ping","params":{},"auth":token})
        )
        .unwrap();
        live_stream.flush().unwrap();
        let mut reply = String::new();
        BufReader::new(live_stream.try_clone().unwrap())
            .read_line(&mut reply)
            .unwrap();
        assert!(reply.contains("\"api_version\""));
        assert_eq!(app.active_tcp_streams.lock().unwrap().len(), 1);

        // Close the live client socket and wait for the handler thread to observe EOF.
        drop(live_stream);
        let wait_empty = || {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while std::time::Instant::now() < deadline {
                if app.active_tcp_streams.lock().unwrap().is_empty() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        };
        assert!(
            wait_empty(),
            "active_tcp_streams leaked after client closed TCP socket"
        );

        // 2. Churn short-lived probes, auth failures, and one-shot RPCs (simulating ApiMismatch
        //    probing / health checks) and verify zero sockets remain in active_tcp_streams.
        for _ in 0..15 {
            let probe = network::probe_tcp_host(&addr, Duration::from_secs(3)).unwrap();
            assert_eq!(probe["server_id"], app.server_id());

            let bad_auth = client::rpc_tcp(&addr, Some("invalid-token"), "app.ping", json!({}));
            assert!(bad_auth.is_err());

            let ping = client::rpc_tcp(&addr, Some(&token), "app.ping", json!({})).unwrap();
            assert_eq!(ping["server_id"], app.server_id());
        }
        assert!(
            wait_empty(),
            "active_tcp_streams accumulated leaked descriptors across short-lived TCP RPCs"
        );

        // 3. Verify shutdown still unblocks an actively open idle TCP connection cleanly.
        let mut held = std::net::TcpStream::connect(&addr).unwrap();
        held.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        writeln!(
            held,
            "{}",
            json!({"jsonrpc":"2.0","id":1,"method":"host.probe","params":{}})
        )
        .unwrap();
        held.flush().unwrap();
        let mut probe_line = String::new();
        let mut held_reader = BufReader::new(held.try_clone().unwrap());
        held_reader.read_line(&mut probe_line).unwrap();
        assert!(!app.active_tcp_streams.lock().unwrap().is_empty());

        drop(running);
        let mut eof_line = String::new();
        let n = held_reader.read_line(&mut eof_line).unwrap_or(0);
        assert_eq!(n, 0);
        assert!(wait_empty());
    }
}
