//! Independent terminals share Herdr transport, never Scheduler admission.
use super::*;

pub(super) const DEFAULT_LAUNCH_TERMINAL_COLS: u16 = 120;
pub(super) const DEFAULT_LAUNCH_TERMINAL_ROWS: u16 = 36;

impl AppServer {
    pub(super) fn parse_terminal_size(value: &Value) -> Option<canban_herdr::TerminalSize> {
        let cols = u16::try_from(value.get("cols")?.as_u64()?).ok()?;
        let rows = u16::try_from(value.get("rows")?.as_u64()?).ok()?;
        canban_herdr::TerminalSize::normalized(cols, rows)
    }

    pub(super) fn record_terminal_size(&self, size: canban_herdr::TerminalSize, persist: bool) {
        *self.last_terminal_size.lock().unwrap() = Some(size);
        if persist {
            let _ = self.utilities.save_terminal_size(&self.home, size);
        }
    }

    pub(super) fn resolve_launch_terminal_size(
        &self,
        explicit: Option<canban_herdr::TerminalSize>,
    ) -> canban_herdr::TerminalSize {
        if let Some(sz) = explicit {
            return sz;
        }
        if let Some(sz) = *self.last_terminal_size.lock().unwrap() {
            return sz;
        }
        canban_herdr::TerminalSize {
            cols: DEFAULT_LAUNCH_TERMINAL_COLS,
            rows: DEFAULT_LAUNCH_TERMINAL_ROWS,
        }
    }

    /// Close is a product lifecycle operation, independent of UI tab shortcuts.
    pub(super) fn close_terminal(&self, p: Value) -> Result<Value> {
        let _gate = self.terminal_gate.lock().unwrap();
        ensure!(
            p["execution"].is_null(),
            "Closing a terminal requires a user action"
        );
        let terminal_id = strp(&p, "terminal_id")?;
        let mut target = None;
        let mut closing_agent = false;
        let saved = self.mutate("terminal.close", &p, |db| {
            let mut item: TerminalEntry = db.get(terminal_id)?;
            if let Ok(mut batch) = db.get::<canban_types::MergeBatch>(terminal_id)
                && batch.active()
            {
                self.cancel_batch_from_terminal_close(db, &mut batch, &mut item)?;
            }
            if let Ok(mut sched) = db.get::<canban_types::ScheduledRun>(terminal_id)
                && !sched.creates_card
                && (sched.active() || sched.state == "blocked")
            {
                self.cancel_scheduled_run_from_terminal_close(db, &mut sched, &mut item)?;
            }
            if let Ok(mut configuration) = db.get::<ConfigurationSession>(terminal_id)
                && configuration.state == "queued"
            {
                configuration.state = "cancelled".into();
                db.save(&configuration)?;
                item.runtime = Some(TerminalRuntime {
                    core_id: None,
                    terminal: None,
                    state: "finished".into(),
                    connection: "connected".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                    observed_agent: None,
                    waiting: None,
                });
                db.save(&item)?;
            }
            if let Some(run) = &mut item.agent {
                if !run.finished() && run.terminal.is_some() {
                    let core = run
                        .core_id
                        .clone()
                        .context("Original Core binding unavailable")?;
                    let terminal = run
                        .terminal
                        .clone()
                        .context("Terminal is still starting; wait before closing")?;
                    target = Some((core, terminal));
                    run.close_state = "closing".into();
                    run.close_requested_at = Some(now());
                    let card_id = run.card_id.clone();
                    let run_num = run.number;
                    db.save(&item)?;
                    let card: Card = db.get(&card_id)?;
                    log(
                        db,
                        &card,
                        "execution.close_requested",
                        "user",
                        "Terminal closed by user",
                        Some(item.id.clone()),
                        json!({"run_number":run_num}),
                    )?;
                    closing_agent = true;
                } else {
                    ensure!(
                        run.state != "starting",
                        "Terminal is still starting; wait before closing"
                    );
                }
            } else if let Some(runtime) = &mut item.runtime
                && runtime.state != "finished"
            {
                target = Some((
                    runtime
                        .core_id
                        .clone()
                        .context("Original Core binding unavailable")?,
                    runtime
                        .terminal
                        .clone()
                        .context("Terminal binding unavailable; inspect before closing")?,
                ));
                runtime.state = "closing".into();
                db.save(&item)?;
            }
            Ok(if target.is_some() {
                json!({
                    "state": "closing",
                    "terminal_id": terminal_id,
                    "message": "Terminal close is still in progress; inspect before retrying"
                })
            } else {
                json!({"state": "closed", "terminal_id": terminal_id})
            })
        })?;
        let Some((core, terminal)) = target else {
            return Ok(saved);
        };
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let result = (|| -> Result<Value> {
            ensure!(
                self.verified_core_id()? == core,
                "Original Herdr core is unavailable"
            );
            client.call("terminal.kill", json!({"terminal":terminal}))
        })();
        let value = match &result {
            Ok(_) => json!({"state":"closed","terminal_id":terminal_id}),
            Err(e) => {
                json!({"state":"unknown","terminal_id":terminal_id,"message":format!("{e:#}; inspect before retrying close")})
            }
        };
        self.store.lock().unwrap().write(|db| {
            if let Some(key) = p["request_key"].as_str() {
                db.conn.execute(
                    "UPDATE operations SET result=?2 WHERE id=?1",
                    params![key, serde_json::to_string(&value)?],
                )?;
            }
            if closing_agent {
                if let Err(e) = &result {
                    let mut item: TerminalEntry = db.get(terminal_id)?;
                    if let Some(run) = &mut item.agent {
                        run.close_state = "error".into();
                        run.error = Some(format!("Terminal close failed: {e:#}"));
                        db.save(&item)?;
                    }
                }
            } else if let Err(_e) = &result {
                let mut item: TerminalEntry = db.get(terminal_id)?;
                if let Some(r) = &mut item.runtime {
                    r.state = "unknown".into();
                    r.connection = "unavailable".into();
                    r.error = Some(value["message"].as_str().unwrap_or("").into());
                }
                db.save(&item)?;
            } else {
                let mut item: TerminalEntry = db.get(terminal_id)?;
                if let Some(r) = &mut item.runtime {
                    r.state = "finished".into();
                }
                db.save(&item)?;
            }
            Ok(())
        })?;
        self.emit("terminals", None);
        Ok(value)
    }
    /// Resolve the owning Core + PTY binding for either a Card-role Agent or an independent terminal.
    pub(super) fn terminal_binding(
        _s: &Snapshot,
        item: &TerminalEntry,
    ) -> Option<(String, String)> {
        if let Some(a) = &item.agent {
            return Some((a.core_id.clone()?, a.terminal.clone()?));
        }
        let r = item.runtime.as_ref()?;
        Some((r.core_id.clone()?, r.terminal.clone()?))
    }
    /// Resolves the Git remote (or fallback GitHub URLs) for a PR repository.
    /// Prefers `project.workflow.workspace.remote` when it points to `repo`,
    /// otherwise selects another local remote matching `repo` (e.g. `upstream`
    /// in a fork workflow). When the preferred remote points to a different
    /// GitHub repository and no local remote matches `repo`, falls back to
    /// direct GitHub repository URLs instead of fetching from the wrong fork.
    pub(super) fn resolve_pr_remote(
        &self,
        ops: &canban_workspaces::Ops,
        project: &Project,
        repo: &str,
    ) -> Option<canban_workspaces::compare::PrRemoteSource> {
        let repo = canban_github::repository(repo).ok()?;
        let preferred = if project.workflow.workspace.remote.trim().is_empty() {
            "origin"
        } else {
            project.workflow.workspace.remote.trim()
        };
        let remotes = ops.fetch_remote_urls(&project.directory, preferred);
        if let Some((remote, _)) = remotes.iter().find(|(_, url)| {
            canban_github::remote_repository(url).is_ok_and(|r| r.eq_ignore_ascii_case(&repo))
        }) {
            return Some(canban_workspaces::compare::PrRemoteSource::Named(
                remote.clone(),
            ));
        }
        let preferred_github_url = remotes
            .iter()
            .find(|(name, _)| name == preferred)
            .filter(|(_, url)| canban_github::remote_repository(url).is_ok())
            .map(|(_, url)| url.as_str());
        if let Some(url) = preferred_github_url {
            let prefer_ssh = url.starts_with("git@") || url.starts_with("ssh://");
            let ssh_url = format!("git@github.com:{repo}.git");
            let https_url = format!("https://github.com/{repo}.git");
            let urls = if prefer_ssh {
                vec![ssh_url, https_url]
            } else {
                vec![https_url, ssh_url]
            };
            return Some(canban_workspaces::compare::PrRemoteSource::Urls(urls));
        }
        None
    }

    pub(super) fn resolve_sync_remote_for_repo(
        &self,
        ops: &canban_workspaces::Ops,
        directory: &str,
        configured_remote: &str,
        github_repo: Option<&str>,
    ) -> String {
        let preferred = if configured_remote.trim().is_empty() || configured_remote.starts_with('-')
        {
            "origin"
        } else {
            configured_remote.trim()
        };
        if let Some(repo) = github_repo.and_then(|raw| canban_github::repository(raw).ok()) {
            let remotes = ops.fetch_remote_urls(directory, preferred);
            if let Some((remote, _)) = remotes.iter().find(|(_, url)| {
                canban_github::remote_repository(url).is_ok_and(|r| r.eq_ignore_ascii_case(&repo))
            }) {
                return remote.clone();
            }
        }
        let dir = Path::new(directory);
        if !ops.has_remote(dir, preferred) && ops.has_remote(dir, "upstream") {
            return "upstream".into();
        }
        preferred.to_owned()
    }

    pub(super) fn resolve_sync_remote(
        &self,
        ops: &canban_workspaces::Ops,
        project: &Project,
    ) -> String {
        self.resolve_sync_remote_for_repo(
            ops,
            &project.directory,
            &project.workflow.workspace.remote,
            project.github.as_ref().map(|g| g.repository.as_str()),
        )
    }

    pub(super) fn cached_github_default_branch(&self, project: &Project) -> Option<String> {
        let repo_raw = project.github.as_ref()?.repository.as_str();
        let repo = canban_github::repository(repo_raw).ok()?;
        let info = self
            .github_cached_client()
            .ok()?
            .cached::<Value>(&format!("/repos/{repo}"))?;
        info["default_branch"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }

    pub(super) fn prepare_pr_card_source(
        &self,
        card: &Card,
        project: &mut Project,
        ops: &canban_workspaces::Ops,
    ) -> Result<Option<canban_workspaces::compare::PrTrackingTarget>> {
        let pr_source = if card.is_github_pr() {
            card.github_issue().map(|src| {
                (
                    src.repository.clone(),
                    src.number,
                    None::<String>,
                    None::<String>,
                )
            })
        } else {
            card.pull_request
                .as_ref()
                .filter(|pr| pr.number > 0)
                .map(|pr| {
                    (
                        pr.repository.clone(),
                        pr.number,
                        Some(pr.head_branch.clone()),
                        Some(pr.base_branch.clone()),
                    )
                })
        };
        let Some((repo, pr_number, fallback_head, fallback_base)) = pr_source else {
            return Ok(None);
        };
        let pr_json = self
            .github_cached_client()
            .ok()
            .and_then(|c| c.cached::<Value>(&format!("/repos/{repo}/pulls/{pr_number}")))
            .or_else(|| {
                self.github_read(&format!("/repos/{repo}/pulls/{pr_number}"), false, false)
                    .ok()
                    .map(|r| r.data)
            });
        let head_ref = pr_json
            .as_ref()
            .and_then(|v| v["head"]["ref"].as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| fallback_head.filter(|s| !s.trim().is_empty()));
        let base_ref = pr_json
            .as_ref()
            .and_then(|v| v["base"]["ref"].as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| fallback_base.filter(|s| !s.trim().is_empty()));
        if let Some(base) = base_ref {
            project.target_branch = Some(base.clone());
            project.workflow.workspace.base_branch = Some(base);
        }
        let same_repo = if let Some(v) = &pr_json {
            let head_repo = v["head"]["repo"]["full_name"].as_str();
            let base_repo = v["base"]["repo"]["full_name"].as_str().unwrap_or(&repo);
            head_repo.is_some_and(|hr| hr.eq_ignore_ascii_case(base_repo))
        } else {
            !card.is_github_pr()
        };
        if card.worktree.is_some() || card.branch.is_some() {
            return Ok(None);
        }
        let remote = self.resolve_pr_remote(ops, project, &repo);
        ops.seed_pr_for_card(
            project,
            &card.id,
            &canban_workspaces::compare::PrCheckoutSpec {
                pr_number,
                head_ref,
                same_repo,
                remote,
            },
        )
    }

    pub(super) fn ensure_card_worktree(
        &self,
        card_id: &str,
        terminal_id: Option<String>,
    ) -> Result<(Card, Project)> {
        let (card, mut project) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let c: Card = db.get(card_id)?;
            let proj: Project = db.get(&c.project_id)?;
            let mut bound = self.bound_project(&proj, &c);
            if let Some(wf_run) = db
                .list::<WorkflowRun>()?
                .into_iter()
                .find(|r| r.card_id.as_deref() == Some(c.id.as_str()) && r.active())
            {
                bound.workflow.workspace = wf_run.definition.workspace;
            } else if let Some(primary) = db
                .list::<WorkflowSource>()?
                .into_iter()
                .filter(|f| f.project_id == proj.id)
                .find(|f| f.filename == "development.yaml")
                && let Ok(rev) = db.get::<WorkflowRevision>(&primary.revision_id)
            {
                bound.workflow.workspace = rev.definition.workspace;
            }
            (c, bound)
        };
        if !card.active() || card.resolved() {
            return Ok((card, project));
        }
        let directory = card
            .workspace
            .as_ref()
            .map(|w| w.directory.clone())
            .unwrap_or_else(|| project.directory.clone());
        let ops = self.ops();
        if let Some(wt) = card.worktree.as_deref().filter(|s| !s.is_empty())
            && card.branch.as_deref().is_some_and(|b| !b.is_empty())
            && card
                .workspace
                .as_ref()
                .is_some_and(|w| w.directory == directory && w.path == wt)
            && ops.host.is_dir(Path::new(wt))
        {
            return Ok((card, project));
        }
        project.directory = directory.clone();
        let source_dir = Path::new(&directory);
        if project.workflow.workspace.create_worktree
            && project.workflow.workspace.sync_base
            && (!ops.has_push_remote(&project) || ops.current_branch(&directory).is_none())
        {
            project.workflow.workspace.sync_base = false;
        }
        let mut prepared = card.clone();
        let pr_tracking = if prepared.worktree.is_none() && prepared.branch.is_none() {
            self.prepare_pr_card_source(&prepared, &mut project, &ops)?
        } else {
            None
        };
        let cwd = ops.prepare(&self.home, &project, &prepared)?;
        let _ = source_dir;
        let cwd_str = cwd.to_string_lossy().into_owned();
        if card.worktree.as_deref() != Some(cwd_str.as_str()) {
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
        let created_worktree = cwd != Path::new(&directory);
        let mut saved_card = card;
        if created_worktree || saved_card.worktree.is_some() {
            self.store.lock().unwrap().write(|db| {
                let mut current: Card = db.get(&saved_card.id)?;
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
                        terminal_id,
                        json!({"workspace":workspace}),
                    )?;
                    db.save(&current)?;
                }
                saved_card = current;
                Ok(())
            })?;
        }
        Ok((saved_card, project))
    }
    pub(super) fn create_terminal(&self, p: Value) -> Result<Value> {
        let _gate = self.terminal_gate.lock().unwrap();
        let explicit_size = Self::parse_terminal_size(&p["size"]);
        if let Some(sz) = explicit_size {
            self.record_terminal_size(sz, true);
        }
        let inherited = p["inherit_terminal_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(|id| self.terminal_context(json!({"terminal_id":id})))
            .transpose()?;
        let explicit_profile = p["profile_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let want_agent = p["agent"] == true || explicit_profile.is_some();
        if want_agent {
            ensure!(
                p["execution"].is_null(),
                "Standalone Agent CLI launch requires a user action"
            );
        }
        let explicit_card_arg = p["card_id"].as_str().is_some_and(|s| !s.trim().is_empty())
            || super::parse_card_number(&p["card"]).is_some();
        // Prepare the Card's dedicated worktree upfront (outside the SQLite write lock)
        // whenever opening a Terminal from an active unmerged Card without an explicit cwd.
        let pre_card_id = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            if explicit_card_arg {
                Some(super::resolve_card(db, &p)?.id)
            } else {
                inherited
                    .as_ref()
                    .and_then(|v| v["card_id"].as_str())
                    .filter(|s| !s.is_empty())
                    .and_then(|cid| db.get::<Card>(cid).ok().map(|c| c.id))
            }
        };
        if p["cwd"].as_str().is_none_or(|s| s.is_empty())
            && p["setup_argv"].is_null()
            && let Some(cid) = &pre_card_id
        {
            let check_card: Card = self.store.lock().unwrap().db().get(cid)?;
            if check_card.active() && !check_card.resolved() {
                let _ = self.ensure_card_worktree(cid, None)?;
            }
        }
        let mut launch = None;
        let saved = self.mutate("terminal.create", &p, |db| {
            let mut project = p["project_id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(|id| db.get::<Project>(id))
                .transpose()?;
            let mut card = if explicit_card_arg {
                Some(super::resolve_card(db, &p)?)
            } else {
                None
            };
            if card.is_none()
                && let Some(cid) = inherited.as_ref().and_then(|v| v["card_id"].as_str())
                && !cid.is_empty()
            {
                card = db.get::<Card>(cid).ok();
            }
            if project.is_none() {
                if let Some(c) = &card {
                    project = db.get::<Project>(&c.project_id).ok();
                } else if let Some(pid) = inherited.as_ref().and_then(|v| v["project_id"].as_str())
                    && !pid.is_empty()
                {
                    project = db.get::<Project>(pid).ok();
                }
            }
            if let Some(c) = &card {
                ensure!(
                    project.as_ref().is_none_or(|p| c.project_id == p.id),
                    "Card belongs to another Project"
                );
                ensure!(
                    c.archived_at.is_none(),
                    "Restore this Card before opening a terminal"
                );
            }
            let directory = card
                .as_ref()
                .and_then(|c| c.workspace.as_ref().map(|w| w.directory.clone()))
                .or_else(|| project.as_ref().map(|p| p.directory.clone()));
            let host = canban_herdr::host::Host::new(self.config());
            let cwd = p["cwd"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    card.as_ref()
                        .and_then(|c| c.worktree.clone())
                        .filter(|wt| Path::new(wt).is_absolute() && host.is_dir(Path::new(wt)))
                })
                .or(directory)
                .or_else(|| {
                    inherited
                        .as_ref()
                        .and_then(|v| v["cwd"].as_str())
                        .map(str::to_owned)
                })
                .unwrap_or(host.home()?);
            ensure!(
                Path::new(&cwd).is_absolute() && host.is_dir(Path::new(&cwd)),
                "Terminal directory no longer exists"
            );
            let custom_name = p["name"]
                .as_str()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_owned);
            if let Some(n) = &custom_name {
                ensure!(
                    n.chars().count() <= 80 && !n.chars().any(char::is_control),
                    "Use at most 80 printable characters"
                );
            }
            let agent_profile = if want_agent {
                let cfg = self.config();
                Some(canban_scheduler::select_in_project(
                    &cfg,
                    project.as_ref(),
                    explicit_profile.as_deref(),
                )?)
            } else {
                None
            };
            let agent_engine = agent_profile.as_ref().map(|prof| {
                let cmd = prof.command_name();
                match cmd.as_str() {
                    "codex" | "claude" => cmd,
                    _ => prof.preset.clone().unwrap_or(cmd),
                }
            });
            let origin_card_id = if explicit_card_arg {
                card.as_ref().map(|c| c.id.clone())
            } else {
                None
            };
            if project.is_none() {
                if let Some(wt) = db
                    .list::<ManagedWorktree>()?
                    .into_iter()
                    .find(|w| super::path_within(&w.path, &cwd))
                {
                    project = db.get::<Project>(&wt.project_id).ok();
                } else {
                    let mut best: Option<(usize, Project)> = None;
                    for proj in db.list::<Project>()? {
                        if super::path_within(&proj.directory, &cwd) {
                            let len = proj.directory.len();
                            if best.as_ref().is_none_or(|(b, _)| len > *b) {
                                best = Some((len, proj));
                            }
                        }
                    }
                    project = best.map(|(_, p)| p);
                }
            }
            let origin_project_id = project.as_ref().map(|p| p.id.clone());
            let resolved_name = match custom_name {
                Some(n) => Some(n),
                None => {
                    let mut snap = db.terminal_snapshot()?;
                    snap.terminal_contexts = self.terminal_contexts.lock().unwrap().clone();
                    Some(snap.next_terminal_label(
                        origin_project_id.as_deref(),
                        agent_engine.as_deref(),
                        None,
                    ))
                }
            };
            let prefix = if want_agent { "agent" } else { "shell" };
            let item = TerminalEntry {
                id: format!("{prefix}-{}", id()),
                conversation_id: None,
                number: db.counter("terminal-number")?,
                name: resolved_name,
                origin_project_id,
                origin_card_id,
                agent_engine,
                created_at: now(),
                runtime: Some(TerminalRuntime {
                    core_id: None,
                    terminal: None,
                    state: "starting".into(),
                    connection: "checking".into(),
                    error: None,
                    agent_status: None,
                    agent_turn: None,
                    observed_agent: None,
                    waiting: None,
                }),
                agent: None,
            };
            db.save(&item)?;
            let project_dir = project
                .as_ref()
                .map(|proj| proj.directory.clone())
                .unwrap_or_else(|| cwd.clone());
            // Persist the launch intent before network IO, but cwd is only an argument.
            launch = Some((item.clone(), cwd, project_dir, agent_profile, project, card));
            Ok(serde_json::to_value(item)?)
        })?;
        let Some((mut item, cwd, project_dir, agent_profile, project, card)) = launch else {
            return Ok(serde_json::to_value(TerminalEntry::from_json(saved)?)?);
        };
        let runtime = item.runtime.as_mut().unwrap();
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let result = (|| -> Result<()> {
            runtime.core_id = Some(self.verified_core_id()?);
            let is_setup = p["setup_argv"].is_array();
            let mut env = std::collections::BTreeMap::new();
            let mut labels =
                std::collections::BTreeMap::from([("canban.terminal".into(), item.id.clone())]);
            if !is_setup {
                env.insert(
                    "CANBAN_HOME".into(),
                    self.home.to_string_lossy().into_owned(),
                );
                env.insert(
                    "CANBAN_ENDPOINT".into(),
                    self.endpoint().to_string_lossy().into_owned(),
                );
                env.insert("CANBAN_CLI".into(), self.cli.to_string_lossy().into_owned());
                if let Some(proj) = &project {
                    env.insert("CANBAN_PROJECT".into(), proj.id.clone());
                    labels.insert("canban.project".into(), proj.id.clone());
                }
                if let Some(c) = &card {
                    env.entry("CANBAN_PROJECT".into())
                        .or_insert_with(|| c.project_id.clone());
                    labels
                        .entry("canban.project".into())
                        .or_insert_with(|| c.project_id.clone());
                    env.insert("CANBAN_CARD".into(), c.number.to_string());
                    env.insert("CARD_ID".into(), c.id.clone());
                    labels.insert("canban.card".into(), c.number.to_string());
                    labels.insert("canban.card_id".into(), c.id.clone());
                    let scratch = canban_context::scratch_dir(&self.home, &c.project_id, c.number);
                    let _ = std::fs::create_dir_all(&scratch);
                    env.insert(
                        "CANBAN_RUN_DIR".into(),
                        scratch.to_string_lossy().into_owned(),
                    );
                }
            }
            let argv = if let Some(profile) = &agent_profile {
                env.insert("CANBAN_PROFILE_ID".into(), profile.id.clone());
                let session_id = profile
                    .command()
                    .iter()
                    .any(|s| s.contains("{session_id}"))
                    .then(super::runtime::uuid_session_id);
                if let Some(sid) = &session_id {
                    env.insert("CANBAN_SESSION_ID".into(), sid.clone());
                }
                let mut argv = if let Some(prompt) = p["prompt"].as_str().filter(|s| !s.is_empty())
                {
                    canban_profiles::command(
                        profile,
                        session_id.as_deref(),
                        true,
                        &[
                            ("home", &self.home.to_string_lossy()),
                            ("cwd", &cwd),
                            ("project_dir", &project_dir),
                            ("prompt", prompt),
                            ("cli", &self.cli.to_string_lossy()),
                        ],
                    )?
                } else {
                    canban_profiles::interactive_command(
                        profile,
                        session_id.as_deref(),
                        &[
                            ("home", &self.home.to_string_lossy()),
                            ("cwd", &cwd),
                            ("project_dir", &project_dir),
                            ("cli", &self.cli.to_string_lossy()),
                        ],
                    )?
                };
                let is_herdr_template = profile
                    .preset
                    .as_deref()
                    .is_some_and(|s| s.starts_with("herdr:"));
                if !argv.is_empty() && !is_herdr_template {
                    argv[0] = canban_profiles::executable_with_env(&argv[0], &client.config.hc_env);
                }
                argv
            } else {
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
                if let Some(argv) = p["setup_argv"].as_array() {
                    let c = self.config();
                    env.extend(c.hc_env);
                    env.insert("HC_SESSION".into(), c.hc_session);
                    argv.iter()
                        .map(|v| Ok(v.as_str().context("Invalid command")?.to_owned()))
                        .collect::<Result<Vec<_>>>()?
                } else {
                    vec![shell, "-l".into()]
                }
            };
            let receipt = if is_setup {
                let spec = canban_profiles::prepare_launch(canban_profiles::LaunchRequest {
                    profile: agent_profile.as_ref(),
                    argv,
                    env,
                    cwd: cwd.clone(),
                })?;
                client.launch_with_policy_unset_and_size(
                    spec.argv,
                    &spec.cwd,
                    spec.env,
                    labels,
                    canban_exec::shell_env::LocalEnvPolicy::MinimalPathOnly,
                    &std::collections::BTreeSet::new(),
                    Some(self.resolve_launch_terminal_size(explicit_size)),
                )?
            } else {
                let trust = card.as_ref().map(Card::trust_level);
                self.launch_herdr_with_env(crate::env::HerdrLaunchRequest {
                    profile: agent_profile.as_ref(),
                    project: project.as_ref(),
                    protected_context: &env,
                    workflow_env: None,
                    source_trust: trust,
                    argv,
                    cwd: cwd.clone(),
                    labels,
                    size: explicit_size,
                })?
                .receipt
            };
            let address = receipt["address"]
                .as_str()
                .context("Terminal launch receipt has no address")?
                .to_owned();
            runtime.terminal = Some(address.clone());
            // Seed the in-memory inventory immediately so an observe_terminals pass
            // racing ahead of Herdr's async "created" subscription event does not
            // transiently downgrade connection -> "unavailable" (read_only = true).
            {
                let mut inv = self.inventory.lock().unwrap();
                if let Some(core_id) = &runtime.core_id
                    && inv.core == *core_id
                {
                    inv.records.entry(address.clone()).or_insert_with(|| {
                        let mut seeded = receipt.clone();
                        seeded["address"] = json!(address);
                        if seeded["state"].is_null() {
                            seeded["state"] = json!("live");
                        }
                        if seeded["cwd"].is_null() {
                            seeded["cwd"] = json!(cwd);
                        }
                        seeded
                    });
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                runtime.state = "running".into();
                runtime.connection = "connected".into();
            }
            Err(e) => {
                runtime.state = "unknown".into();
                runtime.connection = "unavailable".into();
                runtime.error = Some(format!(
                    "{e:#}. Launch was not replayed; inspect Herdr before creating another terminal."
                ));
            }
        }
        self.store.lock().unwrap().write(|db| {
            let mut current: TerminalEntry = db.get(&item.id)?;
            current.runtime = item.runtime.clone();
            item = current;
            db.save(&item)?;
            if let Some(key) = p["request_key"].as_str() {
                db.conn.execute(
                    "UPDATE operations SET result=?2 WHERE id=?1",
                    params![key, serde_json::to_string(&item)?],
                )?;
            }
            Ok(())
        })?;
        // Publish an actual Core observation before selecting the new tab,
        // avoiding a temporary Other group when its directory is already known.
        if let Ok(value) = self.terminal_context(json!({"terminal_id":item.id}))
            && let Ok(context) = serde_json::from_value::<TerminalContext>(value)
        {
            let mut contexts = self.terminal_contexts.lock().unwrap();
            contexts.retain(|c| c.terminal_id != item.id);
            contexts.push(context);
        }
        self.emit("terminals", None);
        Ok(serde_json::to_value(item)?)
    }
    pub(super) fn rename_terminal(&self, p: Value) -> Result<Value> {
        self.mutate("terminal.rename", &p, |db| {
            let mut item: TerminalEntry = db.get(strp(&p, "terminal_id")?)?;
            let name = strp(&p, "name")?.trim();
            ensure!(
                name.chars().count() <= 80 && !name.chars().any(char::is_control),
                "Use at most 80 printable characters"
            );
            item.name = if !name.is_empty() {
                Some(name.into())
            } else if item.conversation_id.is_none() {
                let mut snap = db.terminal_snapshot()?;
                snap.terminal_contexts = self.terminal_contexts.lock().unwrap().clone();
                let pid = snap.terminal_project_id(&item).map(str::to_owned);
                Some(snap.next_terminal_label(
                    pid.as_deref(),
                    item.agent_engine.as_deref(),
                    Some(&item.id),
                ))
            } else {
                None
            };
            db.save(&item)?;
            Ok(serde_json::to_value(item)?)
        })
    }
    pub(super) fn control_terminal(&self, p: Value, end: bool) -> Result<Value> {
        let item_check: TerminalEntry = self
            .store
            .lock()
            .unwrap()
            .db()
            .get(strp(&p, "terminal_id")?)?;
        if item_check.agent.is_some() {
            return self.control_agent(p, end);
        }
        let _gate = self.terminal_gate.lock().unwrap();
        let item = item_check;
        let runtime = item
            .runtime
            .as_ref()
            .context("Select an independent terminal")?;
        let core = runtime
            .core_id
            .as_ref()
            .context("Terminal has no Core binding")?;
        let terminal = runtime
            .terminal
            .as_ref()
            .context("Terminal has no runtime binding")?;
        ensure!(runtime.state != "finished", "Terminal has already ended");
        let client = canban_herdr::Client {
            config: self.config(),
        };
        ensure!(
            self.verified_core_id()? == *core,
            "Original Herdr core is unavailable"
        );
        let method = if end {
            "terminal.end"
        } else {
            "terminal.interrupt"
        };
        let mut send = false;
        let saved = self.mutate(method, &p, |_| {
            send = true;
            Ok(json!({"state":"unknown","message":"Control requested; inspect before retrying"}))
        })?;
        if !send {
            return Ok(saved);
        }
        let result = match client.call(
            if end {
                "terminal.kill"
            } else {
                "terminal.interrupt"
            },
            json!({"terminal":terminal}),
        ) {
            Ok(value) => json!({"state":"sent","result":value}),
            Err(e) => {
                json!({"state":"unknown","message":format!("{e:#}; inspect before retrying")})
            }
        };
        self.store.lock().unwrap().write(|db| {
            if let Some(key) = p["request_key"].as_str() {
                db.conn.execute(
                    "UPDATE operations SET result=?2 WHERE id=?1",
                    params![key, serde_json::to_string(&result)?],
                )?;
            }
            Ok(())
        })?;
        self.emit("terminals", None);
        Ok(result)
    }
    #[cfg(test)]
    fn directory_context(
        &self,
        s: &Snapshot,
        id: &str,
        core: &str,
        terminal: &str,
        value: &Value,
    ) -> TerminalContext {
        let cwd = canban_herdr::cwd(value).map(str::to_owned);
        let resolved = cwd
            .as_ref()
            .map(|d| self.ops().directory_context_full(s, d))
            .unwrap_or_default();
        TerminalContext {
            terminal_id: id.into(),
            core_id: core.into(),
            terminal: terminal.into(),
            cwd,
            directory_root: resolved.directory_root,
            project_id: resolved.project_id,
            card_id: resolved.card_id,
            worktree_id: resolved.worktree_id,
            branch: resolved.branch,
            dirty: resolved.dirty,
            additions: resolved.additions,
            deletions: resolved.deletions,
            ahead: resolved.ahead,
            behind: resolved.behind,
        }
    }
    pub(super) fn terminal_context(&self, p: Value) -> Result<Value> {
        let s = self.store.lock().unwrap().db().terminal_snapshot()?;
        let key = strp(&p, "terminal_id")?;
        let item = s
            .terminals
            .iter()
            .find(|t| t.id == key)
            .context("Terminal not found")?;
        let result = (|| -> Result<TerminalContext> {
            let (core, terminal) =
                Self::terminal_binding(&s, item).context("No runtime binding")?;
            #[cfg(test)]
            if !self.observer_running.load(Ordering::SeqCst) {
                let client = canban_herdr::Client {
                    config: self.config(),
                };
                ensure!(
                    client.status()?["core"]["id"] == core,
                    "Original Core unavailable"
                );
                return Ok(self.directory_context(
                    &s,
                    key,
                    &core,
                    &terminal,
                    &client.get(&terminal)?,
                ));
            }
            let (observed_core, values) = self.runtime_inventory()?;
            ensure!(observed_core == core, "Original Core unavailable");
            let value = values
                .iter()
                .find(|v| v["address"] == terminal)
                .context("Terminal is not in the current inventory")?;
            if p["refresh"] == true {
                self.invalidate_contexts();
            }
            self.refresh_context(&s, key, &core, &terminal, value)
        })();
        match result {
            Ok(context) => Ok(serde_json::to_value(context)?),
            // Never present the task/launch directory as a live observation.
            Err(e) => Ok(
                json!({"terminal_id":key,"cwd":null,"project_id":null,"card_id":null,"branch":null,"error":format!("{e:#}")}),
            ),
        }
    }
    pub(super) fn observe_terminals(&self) -> Result<()> {
        let snapshot = self.terminal_snapshot()?;
        if snapshot.terminals.is_empty() {
            return Ok(());
        }
        let observed = self.runtime_inventory();
        let mut contexts = Vec::new();
        let mut wanted = std::collections::BTreeSet::new();
        for mut item in snapshot.terminals.clone() {
            let before = item.clone();
            let binding = Self::terminal_binding(&snapshot, &item);
            let value = binding.as_ref().and_then(|(core, terminal)| {
                let (observed_core, values) = observed.as_ref().ok()?;
                (core == observed_core)
                    .then(|| values.iter().find(|v| v["address"] == *terminal))
                    .flatten()
            });
            let already_ended = item.agent.as_ref().is_some_and(AgentRun::finished)
                || item.runtime.as_ref().is_some_and(|r| r.state == "finished");
            if !already_ended
                && let (Some((core, terminal)), Some(value)) = (&binding, value)
                && value["state"] != "finished"
            {
                #[cfg(test)]
                if !self.observer_running.load(Ordering::SeqCst) {
                    contexts
                        .push(self.directory_context(&snapshot, &item.id, core, terminal, value));
                }
                wanted.insert(item.id.clone());
                self.queue_context(&snapshot, &item.id, core, terminal, value);
                if let Some(context) =
                    self.cached_context(&item.id, core, terminal, canban_herdr::cwd(value))
                {
                    contexts.push(context);
                }
            }
            let is_special_agent = item.is_special_session()
                || snapshot
                    .configuration_sessions
                    .iter()
                    .any(|c| c.id == item.id)
                || snapshot.merge_batches.iter().any(|b| b.id == item.id);
            if let Some(runtime) = &mut item.runtime {
                match (&observed, value) {
                    (Ok(_), Some(v)) => {
                        runtime.connection = "connected".into();
                        runtime.error = None;
                        if v["state"] == "finished" {
                            runtime.state = "finished".into();
                            runtime.agent_status = None;
                            runtime.agent_turn = None;
                            runtime.observed_agent = None;
                            runtime.waiting = None;
                        } else {
                            let observed_agent =
                                canban_profiles::identify_herdr_agent(v).map(str::to_owned);
                            runtime.observed_agent = observed_agent.clone();
                            let has_agent = item.agent_engine.is_some()
                                || observed_agent.is_some()
                                || is_special_agent;
                            if has_agent {
                                let status = v["status"]["state"].as_str().map(str::to_owned);
                                runtime.agent_turn = v["turn"].as_u64();
                                if status.as_deref() == Some("blocked") {
                                    let kind =
                                        v["status"]["kind"].as_str().unwrap_or("input").to_owned();
                                    let title = v["status"]["title"]
                                        .as_str()
                                        .unwrap_or("Agent needs input")
                                        .to_owned();
                                    let message =
                                        v["status"]["msg"].as_str().unwrap_or("").to_owned();
                                    let old = runtime.waiting.as_ref();
                                    if old.is_none_or(|w| {
                                        w.kind != kind || w.title != title || w.message != message
                                    }) {
                                        runtime.waiting = Some(RuntimeWait {
                                            id: old.map(|w| w.id.clone()).unwrap_or_else(id),
                                            since: old.map(|w| w.since).unwrap_or_else(now),
                                            kind,
                                            title,
                                            message,
                                        });
                                    }
                                } else {
                                    runtime.waiting = None;
                                }
                                runtime.agent_status = status;
                            } else {
                                // Clear stale PTY OSC 7501 state once a manual shell returns to zsh/bash.
                                runtime.agent_status = None;
                                runtime.agent_turn = None;
                                runtime.waiting = None;
                            }
                        }
                    }
                    (Ok((core, _)), _) if runtime.core_id.as_ref().is_some_and(|id| id != core) => {
                        runtime.connection = "replaced".into();
                        runtime.observed_agent = None;
                        runtime.waiting = None;
                        runtime.error = Some(
                            "Original Core identity changed; this terminal was not replaced".into(),
                        );
                    }
                    (Err(e), _) => {
                        runtime.connection = "unavailable".into();
                        runtime.observed_agent = None;
                        runtime.waiting = None;
                        runtime.error = Some(format!("{e:#}"));
                    }
                    _ if runtime.state != "finished" => {
                        // Guard against async subscription lag immediately after launch.
                        let fresh_launch = runtime.connection == "connected"
                            && now().saturating_sub(item.created_at) < 3_000;
                        if !fresh_launch {
                            runtime.connection = "unavailable".into();
                            runtime.observed_agent = None;
                            runtime.waiting = None;
                        }
                    }
                    _ => {}
                }
            }
            if item != before {
                self.store.lock().unwrap().write(|db| {
                    let mut current: TerminalEntry = db.get(&item.id)?;
                    // A launch receipt may have arrived after this observation
                    // began. Never erase its binding with an older starting row.
                    if current.runtime == before.runtime {
                        current.runtime = item.runtime.clone();
                        db.save(&current)?;
                    }
                    Ok(())
                })?;
                self.emit("terminals", None);
            }
            let entry_ended = item.agent.as_ref().is_some_and(AgentRun::finished)
                || item.runtime.as_ref().is_some_and(|r| r.state == "finished");
            // Preserve only a confirmed location for the same binding while the entry is
            // still live in the product DB (including the brief window after Herdr reports
            // `finished`/`removed` before Card Agent reconciliation completes) or while
            // observation/discovery is unavailable. Never invent the launch directory.
            if !entry_ended
                && !contexts.iter().any(|c| c.terminal_id == item.id)
                && let Some((core, terminal)) = &binding
                && !observed
                    .as_ref()
                    .is_ok_and(|(observed_core, _)| observed_core != core)
                && let Some(previous) = snapshot.terminal_contexts.iter().find(|c| {
                    c.terminal_id == item.id && c.core_id == *core && c.terminal == *terminal
                })
            {
                contexts.push(previous.clone());
            }
        }
        if observed.is_ok() {
            let mut q = self.contexts.lock().unwrap();
            q.retain_targets(&wanted);
        }
        let changed = {
            let mut current = self.terminal_contexts.lock().unwrap();
            if *current == contexts {
                false
            } else {
                *current = contexts;
                true
            }
        };
        if changed {
            self.emit("terminals", None);
        }
        Ok(())
    }
    /// Any independent terminal in this checkout protects it, regardless of origin.
    pub(super) fn terminal_retains_worktree(&self, card: &Card) -> Result<bool> {
        let Some(worktree) = &card.worktree else {
            return Ok(false);
        };
        self.observe_terminals()?;
        let s = self.snapshot()?;
        Ok(s.terminals
            .iter()
            .filter(|t| {
                t.conversation_id.is_none()
                    && !t.id.starts_with("merge-queue:")
                    && !t.id.starts_with("configuration:")
                    && !t.id.starts_with("scheduled:")
            })
            .any(|t| {
                let live = t.runtime.as_ref().is_some_and(|r| r.state != "finished");
                live && s
                    .terminal_contexts
                    .iter()
                    .find(|c| c.terminal_id == t.id)
                    .and_then(|c| c.cwd.as_deref())
                    .is_none_or(|cwd| self.ops().c_contains(worktree, cwd))
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshly_launched_terminal_stays_connected_before_subscription_catches_up() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        app.observer_running.store(true, Ordering::SeqCst);
        {
            let mut inv = app.inventory.lock().unwrap();
            inv.core = "core-live".into();
            inv.last_at = now();
        }
        let created = now();
        let entry = TerminalEntry {
            id: "shell-fresh".into(),
            conversation_id: None,
            number: 1,
            name: Some("Terminal 1".into()),
            origin_project_id: None,
            origin_card_id: None,
            agent_engine: None,
            created_at: created,
            runtime: Some(TerminalRuntime {
                core_id: Some("core-live".into()),
                terminal: Some("tty-1".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
        };
        app.store
            .lock()
            .unwrap()
            .write(|db| db.save(&entry))
            .unwrap();

        // While within the launch grace window, async inventory lag must not mark it unavailable.
        app.observe_terminals().unwrap();
        let s = app.snapshot().unwrap();
        assert_eq!(
            s.terminals[0].runtime.as_ref().unwrap().connection,
            "connected"
        );

        // Once older than the launch grace window and still absent from inventory, it becomes unavailable.
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                let mut stale: TerminalEntry = db.get("shell-fresh")?;
                stale.created_at = created.saturating_sub(5_000);
                db.save(&stale)
            })
            .unwrap();
        app.observe_terminals().unwrap();
        let s2 = app.snapshot().unwrap();
        assert_eq!(
            s2.terminals[0].runtime.as_ref().unwrap().connection,
            "unavailable"
        );
    }

    #[test]
    fn observe_terminals_tracks_foreground_agent_from_herdr_push_and_clears_on_shell_or_disconnect()
    {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        app.observer_running.store(true, Ordering::SeqCst);
        let created = now().saturating_sub(10_000);
        let entry = TerminalEntry {
            id: "shell-1".into(),
            conversation_id: None,
            number: 1,
            name: Some("Terminal 1".into()),
            origin_project_id: None,
            origin_card_id: None,
            agent_engine: None,
            created_at: created,
            runtime: Some(TerminalRuntime {
                core_id: Some("core-1".into()),
                terminal: Some("t1".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
        };
        app.store
            .lock()
            .unwrap()
            .write(|db| db.save(&entry))
            .unwrap();

        // 1. Plain shell in Herdr inventory -> no observed_agent, title stays "Terminal 1".
        {
            let mut inv = app.inventory.lock().unwrap();
            inv.core = "core-1".into();
            inv.last_at = now();
            inv.records.insert(
                "t1".into(),
                json!({
                    "address": "t1",
                    "state": "live",
                    "foreground": {"pid": 100, "name": "zsh", "argv": ["/bin/zsh", "-l"], "cwd": "/tmp"}
                }),
            );
        }
        app.observe_terminals().unwrap();
        let t = &app.snapshot().unwrap().terminals[0];
        assert_eq!(t.title(), "Terminal 1");
        assert_eq!(t.runtime.as_ref().unwrap().observed_agent, None);

        // 2. User launches `agy` inside the shell and hits a permission block ->
        // Herdr foreground push identifies `"agy"`, captures `RuntimeWait`, and preserves wait ID across polls.
        {
            let mut inv = app.inventory.lock().unwrap();
            inv.last_at = now();
            inv.records.insert(
                "t1".into(),
                json!({
                    "address": "t1",
                    "state": "live",
                    "turn": 1,
                    "status": {
                        "state": "blocked",
                        "app": "agy",
                        "kind": "permission",
                        "title": "Allow command?",
                        "msg": "cargo test"
                    },
                    "foreground": {"pid": 101, "name": "agy", "argv": ["/opt/homebrew/bin/agy"], "cwd": "/tmp"}
                }),
            );
        }
        app.observe_terminals().unwrap();
        let t = app.snapshot().unwrap().terminals[0].clone();
        assert_eq!(t.title(), "Terminal 1");
        assert_eq!(t.agent_engine, None);
        let rt = t.runtime.as_ref().unwrap();
        assert_eq!(rt.observed_agent.as_deref(), Some("agy"));
        assert_eq!(rt.agent_status.as_deref(), Some("blocked"));
        assert_eq!(t.herdr_status(), HerdrStatus::Blocked);
        let first_wait = rt
            .waiting
            .clone()
            .expect("expected RuntimeWait on blocked shell agent");
        assert_eq!(first_wait.kind, "permission");
        assert_eq!(first_wait.title, "Allow command?");
        assert_eq!(first_wait.message, "cargo test");

        // Re-observing the same block keeps the wait ID and timestamp stable.
        app.observe_terminals().unwrap();
        let second_wait = app.snapshot().unwrap().terminals[0]
            .runtime
            .as_ref()
            .unwrap()
            .waiting
            .clone()
            .unwrap();
        assert_eq!(second_wait.id, first_wait.id);
        assert_eq!(second_wait.since, first_wait.since);

        // 3. Agent exits back to `zsh` (even if last OSC 7501 status remains on PTY) ->
        // clears `observed_agent`, stale `agent_status`, `agent_turn`, and `waiting`.
        {
            let mut inv = app.inventory.lock().unwrap();
            inv.last_at = now();
            inv.records.insert(
                "t1".into(),
                json!({
                    "address": "t1",
                    "state": "live",
                    "turn": 1,
                    "status": {"state": "done", "app": "agy"},
                    "foreground": {"pid": 100, "name": "zsh", "argv": ["/bin/zsh", "-l"], "cwd": "/tmp"}
                }),
            );
        }
        app.observe_terminals().unwrap();
        let cleaned = app.snapshot().unwrap().terminals[0].clone();
        assert_eq!(cleaned.herdr_status(), HerdrStatus::PlainShell);
        let cleaned_rt = cleaned.runtime.as_ref().unwrap();
        assert_eq!(cleaned_rt.observed_agent, None);
        assert_eq!(cleaned_rt.agent_status, None);
        assert_eq!(cleaned_rt.agent_turn, None);
        assert_eq!(cleaned_rt.waiting, None);

        // 4. Subscription disconnect / error clears `observed_agent` so unreadable sessions show no icon.
        {
            let mut inv = app.inventory.lock().unwrap();
            inv.last_at = now();
            inv.records.insert(
                "t1".into(),
                json!({
                    "address": "t1",
                    "state": "live",
                    "foreground": {"pid": 102, "name": "codex", "argv": ["codex"], "cwd": "/tmp"}
                }),
            );
        }
        app.observe_terminals().unwrap();
        assert_eq!(
            app.snapshot().unwrap().terminals[0]
                .runtime
                .as_ref()
                .unwrap()
                .observed_agent
                .as_deref(),
            Some("codex")
        );
        {
            let mut inv = app.inventory.lock().unwrap();
            inv.error = Some("Herdr disconnected".into());
        }
        app.observe_terminals().unwrap();
        let rt = app.snapshot().unwrap().terminals[0]
            .runtime
            .clone()
            .unwrap();
        assert_eq!(rt.connection, "unavailable");
        assert_eq!(rt.observed_agent, None);
    }

    #[test]
    fn observe_terminals_keeps_card_agent_context_until_agent_is_marked_finished() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServer::open(dir.path().join("home"), PathBuf::from("/bin/echo")).unwrap();
        app.observer_running.store(true, Ordering::SeqCst);

        let proj_dir = dir.path().join("project");
        let worktree_dir = dir.path().join("worktree");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::create_dir_all(&worktree_dir).unwrap();
        let project: Project = serde_json::from_value(json!({
            "id": "p1",
            "name": "Project",
            "directory": proj_dir.to_string_lossy(),
            "target_branch": "main",
            "workflow": Workflow::default(),
            "created_at": 1
        }))
        .unwrap();
        let card: Card = serde_json::from_value(json!({
            "id": "c1",
            "project_id": "p1",
            "number": 1,
            "title": "Test card",
            "body": "",
            "stage": "develop",
            "worktree": worktree_dir.to_string_lossy(),
            "branch": "canban/1",
            "revision": 1,
            "created_at": 1
        }))
        .unwrap();
        let profile = Config::default().profiles[0].clone();
        let mut run = canban_scheduler::agent_run(
            "p1",
            "c1",
            1,
            "develop",
            "interactive",
            profile,
            "work".into(),
            None,
        );
        run.state = "completed".into();
        run.core_id = Some("core-1".into());
        run.terminal = Some("pty-agent".into());
        run.connection_state = "connected".into();
        run.observed = Some(json!({"state": "live", "status": {"state": "idle"}}));
        let agent_id = Conversation::key("c1", &run.profile.id);
        let agent_entry = TerminalEntry {
            id: agent_id.clone(),
            conversation_id: Some(agent_id.clone()),
            number: 1,
            name: None,
            origin_project_id: Some("p1".into()),
            origin_card_id: Some("c1".into()),
            agent_engine: Some("codex".into()),
            created_at: now().saturating_sub(10_000),
            runtime: None,
            agent: Some(run),
        };
        let shell_entry = TerminalEntry {
            id: "shell-1".into(),
            conversation_id: None,
            number: 2,
            name: Some("Shell".into()),
            origin_project_id: Some("p1".into()),
            origin_card_id: None,
            agent_engine: None,
            created_at: now().saturating_sub(10_000),
            runtime: Some(TerminalRuntime {
                core_id: Some("core-1".into()),
                terminal: Some("pty-shell".into()),
                state: "running".into(),
                connection: "connected".into(),
                error: None,
                agent_status: None,
                agent_turn: None,
                observed_agent: None,
                waiting: None,
            }),
            agent: None,
        };
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                db.save(&project)?;
                db.save(&card)?;
                db.save(&agent_entry)?;
                db.save(&shell_entry)
            })
            .unwrap();

        let confirmed_agent_ctx = TerminalContext {
            terminal_id: agent_id.clone(),
            core_id: "core-1".into(),
            terminal: "pty-agent".into(),
            cwd: Some(worktree_dir.to_string_lossy().into_owned()),
            directory_root: Some(worktree_dir.to_string_lossy().into_owned()),
            project_id: Some("p1".into()),
            card_id: Some("c1".into()),
            ..Default::default()
        };
        let confirmed_shell_ctx = TerminalContext {
            terminal_id: "shell-1".into(),
            core_id: "core-1".into(),
            terminal: "pty-shell".into(),
            cwd: Some(proj_dir.to_string_lossy().into_owned()),
            directory_root: Some(proj_dir.to_string_lossy().into_owned()),
            project_id: Some("p1".into()),
            ..Default::default()
        };
        *app.terminal_contexts.lock().unwrap() =
            vec![confirmed_agent_ctx.clone(), confirmed_shell_ctx];

        // 1. Herdr reports `finished` on both PTYs before tick() reconciles the Card Agent.
        // The Card Agent is still unfinished in SQLite, so its confirmed context is preserved;
        // the independent shell transitions to `runtime.state == "finished"` and clears its context.
        {
            let mut inv = app.inventory.lock().unwrap();
            inv.core = "core-1".into();
            inv.last_at = now();
            inv.records.insert(
                "pty-agent".into(),
                json!({"address": "pty-agent", "state": "finished"}),
            );
            inv.records.insert(
                "pty-shell".into(),
                json!({"address": "pty-shell", "state": "finished"}),
            );
        }
        app.observe_terminals().unwrap();
        let s1 = app.snapshot().unwrap();
        assert_eq!(s1.terminal_contexts, vec![confirmed_agent_ctx.clone()]);
        assert_eq!(
            s1.terminals
                .iter()
                .find(|t| t.id == "shell-1")
                .unwrap()
                .runtime
                .as_ref()
                .unwrap()
                .state,
            "finished"
        );

        // 2. Herdr removes the PTY record completely while the Card Agent is still unfinished.
        {
            let mut inv = app.inventory.lock().unwrap();
            inv.last_at = now();
            inv.records.remove("pty-agent");
        }
        app.observe_terminals().unwrap();
        assert_eq!(
            app.snapshot().unwrap().terminal_contexts,
            vec![confirmed_agent_ctx]
        );

        // 3. Once SQLite marks the Card Agent finished, observe_terminals drops its context.
        app.store
            .lock()
            .unwrap()
            .write(|db| {
                let mut saved: TerminalEntry = db.get(&agent_id)?;
                let a = saved.agent.as_mut().unwrap();
                a.observed = Some(json!({"state": "finished"}));
                a.close_state = "closed".into();
                db.save(&saved)
            })
            .unwrap();
        app.observe_terminals().unwrap();
        assert!(app.snapshot().unwrap().terminal_contexts.is_empty());
    }
}
