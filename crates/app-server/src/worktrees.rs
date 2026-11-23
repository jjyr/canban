//! Standalone Canban-managed branch worktrees created from Terminals / Command Palette.
use super::*;

impl AppServer {
    pub(super) fn worktree_call(&self, method: &str, p: Value) -> Result<Value> {
        if method != "worktree.gc" {
            ensure!(
                p["execution"].is_null(),
                "Managed worktree operations require a direct user action"
            );
        }
        match method {
            "worktree.create" => self.create_managed_worktree(p),
            "worktree.status" => self.managed_worktree_status(p),
            "worktree.promote" => self.promote_managed_worktree(p),
            "worktree.remove" => self.remove_managed_worktree(p),
            "worktree.gc" => self.gc_worktrees(p),
            _ => bail!("Unknown worktree operation: {method}"),
        }
    }

    fn create_managed_worktree(&self, p: Value) -> Result<Value> {
        let project_id = strp(&p, "project_id")?;
        let branch = strp(&p, "branch")?.trim();
        ensure!(!branch.is_empty(), "Branch name is required");
        let base_branch = p["base_branch"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let sync_base = p["sync_base"].as_bool().unwrap_or(false);

        let project: Project = self.store.lock().unwrap().db().get(project_id)?;

        // Check duplicate managed worktree branch in same project
        {
            let db = self.store.lock().unwrap();
            let existing = db.db().list::<ManagedWorktree>()?;
            ensure!(
                !existing
                    .iter()
                    .any(|w| w.project_id == project.id && w.branch == branch),
                "A managed worktree for branch `{branch}` already exists in {}",
                project.name
            );
        }

        let ops = self.ops();
        let sync_remote = self.resolve_sync_remote(&ops, &project);
        let github_default = self.cached_github_default_branch(&project);
        let (path_buf, resolved_base, base_commit) = {
            let _permit = self.check();
            ops.create_branch_worktree_with_options(
                &self.home,
                &project,
                branch,
                base_branch,
                sync_base,
                canban_workspaces::WorktreeBaseOptions {
                    preferred_remote: Some(sync_remote.as_str()),
                    github_default_branch: github_default.as_deref(),
                },
            )?
        };
        let path_str = path_buf.to_string_lossy().into_owned();
        let wt = ManagedWorktree {
            id: format!("wt-{}", id()),
            project_id: project.id.clone(),
            branch: branch.to_owned(),
            base_branch: resolved_base,
            base_commit: Some(base_commit),
            path: path_str.clone(),
            directory: project.directory.clone(),
            created_at: now(),
        };

        let saved_wt: ManagedWorktree =
            serde_json::from_value(self.mutate("worktree.create", &p, |db| {
                db.save(&wt)?;
                Ok(serde_json::to_value(&wt)?)
            })?)?;
        self.git_changed();

        let launch_mode = p["launch"].as_str().unwrap_or("shell");
        let terminal = if launch_mode == "none" {
            None
        } else {
            let mut launch_params = json!({
                "project_id": project.id,
                "cwd": path_str,
            });
            if launch_mode == "agent" {
                let profile_id = p["profile_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| self.config().default_agent);
                launch_params["profile_id"] = json!(profile_id.clone());
                launch_params["name"] = json!(format!("{} · @{profile_id}", saved_wt.branch));
            } else {
                launch_params["name"] = json!(saved_wt.branch.clone());
            }
            Some(self.create_terminal(launch_params)?)
        };

        Ok(json!({
            "worktree": saved_wt,
            "terminal": terminal,
        }))
    }

    fn managed_worktree_status(&self, p: Value) -> Result<Value> {
        let wt_id = strp(&p, "worktree_id")?;
        let wt: ManagedWorktree = self.store.lock().unwrap().db().get(wt_id)?;
        let _permit = self.check();
        let (state, dirty, branch) = self.ops().worktree_status(Path::new(&wt.path))?;
        Ok(json!({
            "worktree_id": wt.id,
            "state": state,
            "dirty": dirty,
            "branch": branch.unwrap_or(wt.branch),
            "path": wt.path,
            "base_branch": wt.base_branch,
        }))
    }

    fn promote_managed_worktree(&self, p: Value) -> Result<Value> {
        let wt_id = strp(&p, "worktree_id")?;
        let title = strp(&p, "title")?.trim();
        ensure!(!title.is_empty(), "Card title is required");
        let body = p["body"].as_str().unwrap_or("");
        let promoted_target = {
            let wt_peek: Option<ManagedWorktree> = self.store.lock().unwrap().db().get(wt_id).ok();
            wt_peek.and_then(|wt| {
                let stripped = self
                    .ops()
                    .strip_remote_branch_prefix(Path::new(&wt.directory), &wt.base_branch);
                (!stripped.is_empty()).then_some(stripped)
            })
        };

        let card_val = self.mutate("worktree.promote", &p, |db| {
            let wt: ManagedWorktree = db.get(wt_id)?;
            let project: Project = db.get(&wt.project_id)?;
            let mut card = canban_kanban::create(db, &project, title, body, "user")?;
            card.created_by = self
                .github
                .status()
                .login
                .or_else(|| self.github_cli.login())
                .unwrap_or_else(|| "Local".into());
            card.branch = Some(wt.branch.clone());
            card.base_commit = wt.base_commit.clone();
            card.worktree = Some(wt.path.clone());
            card.workspace = Some(WorkspaceBinding {
                directory: wt.directory.clone(),
                path: wt.path.clone(),
                branch: Some(wt.branch.clone()),
                base_commit: wt.base_commit.clone(),
                target_branch: promoted_target
                    .clone()
                    .or_else(|| (!wt.base_branch.is_empty()).then(|| wt.base_branch.clone())),
            });
            db.save(&card)?;
            log(
                db,
                &card,
                "workspace.promoted",
                "user",
                &format!(
                    "Promoted standalone worktree `{}` to Card #{}",
                    wt.branch, card.number
                ),
                None,
                json!({"branch": wt.branch, "path": wt.path, "base_branch": wt.base_branch}),
            )?;
            db.conn
                .execute("DELETE FROM managed_worktrees WHERE id=?1", params![wt.id])?;
            let mut val = serde_json::to_value(card)?;
            val["promoted_from_worktree_id"] = json!(wt.id);
            Ok(val)
        })?;
        self.invalidate_contexts();
        Ok(card_val)
    }

    fn remove_managed_worktree(&self, p: Value) -> Result<Value> {
        let wt_id = strp(&p, "worktree_id")?;
        let force = p["force"].as_bool().unwrap_or(false);
        let (wt, snapshot) = {
            let wt: ManagedWorktree = self.store.lock().unwrap().db().get(wt_id)?;
            let snap = self.terminal_snapshot()?;
            (wt, snap)
        };

        // Ensure no live terminal PTY is currently inside this worktree
        let ops = self.ops();
        for ctx in &snapshot.terminal_contexts {
            if let Some(cwd) = &ctx.cwd
                && ops.c_contains(&wt.path, cwd)
            {
                let live = snapshot.terminals.iter().any(|t| {
                    t.id == ctx.terminal_id
                        && t.runtime.as_ref().is_some_and(|r| r.state != "finished")
                });
                ensure!(
                    !live,
                    "Close live Terminal tabs running inside `{}` before deleting this worktree",
                    wt.branch
                );
            }
        }

        {
            let _permit = self.check();
            ops.remove_worktree(
                &self.home,
                Path::new(&wt.directory),
                Path::new(&wt.path),
                force,
            )?;
        }

        let res = self.mutate("worktree.remove", &p, |db| {
            db.conn
                .execute("DELETE FROM managed_worktrees WHERE id=?1", params![wt.id])?;
            Ok(json!({"removed": true, "worktree_id": wt.id}))
        })?;
        self.git_changed();
        Ok(res)
    }

    fn parse_duration_ms(raw: &str) -> Result<u64> {
        let s = raw.trim().to_ascii_lowercase();
        ensure!(!s.is_empty(), "Empty duration");
        if let Some(num) = s.strip_suffix('d') {
            let days: u64 = num.trim().parse().context("Invalid days duration")?;
            return Ok(days.saturating_mul(24 * 60 * 60 * 1_000));
        }
        if let Some(num) = s.strip_suffix('h') {
            let hours: u64 = num.trim().parse().context("Invalid hours duration")?;
            return Ok(hours.saturating_mul(60 * 60 * 1_000));
        }
        if let Some(num) = s.strip_suffix('m') {
            let mins: u64 = num.trim().parse().context("Invalid minutes duration")?;
            return Ok(mins.saturating_mul(60 * 1_000));
        }
        if let Some(num) = s.strip_suffix('s') {
            let secs: u64 = num.trim().parse().context("Invalid seconds duration")?;
            return Ok(secs.saturating_mul(1_000));
        }
        let secs: u64 = s
            .parse()
            .context("Invalid duration; use e.g. 7d, 24h, 30m")?;
        Ok(secs.saturating_mul(1_000))
    }

    fn gc_worktrees(&self, p: Value) -> Result<Value> {
        let dry_run = p["dry_run"].as_bool().unwrap_or(false);
        let merged_before_ms = if let Some(ms) = p["merged_before_ms"].as_u64() {
            ms
        } else if let Some(raw) = p["merged_before"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Self::parse_duration_ms(raw)?
        } else {
            7 * 24 * 60 * 60 * 1_000
        };

        let snapshot = self.snapshot()?;
        let filter_pid = if let Some(raw) = p["project_id"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let store = self.store.lock().unwrap();
            Some(super::resolve_project_id(store.db(), raw)?)
        } else {
            None
        };

        let now_ms = now();
        let cutoff = now_ms.saturating_sub(merged_before_ms);
        let mut removed = Vec::<Value>::new();
        let mut dirty = Vec::<Value>::new();
        let mut skipped = Vec::<Value>::new();
        let mut cleared_card_ids = Vec::<String>::new();
        let mut removed_wt_ids = Vec::<String>::new();
        let mut changed = false;

        let has_live_terminal = |path: &str, ops: &canban_workspaces::Ops| -> bool {
            snapshot.terminal_contexts.iter().any(|ctx| {
                ctx.cwd
                    .as_deref()
                    .is_some_and(|cwd| ops.c_contains(path, cwd))
                    && snapshot.terminals.iter().any(|t| {
                        t.id == ctx.terminal_id
                            && t.runtime.as_ref().is_some_and(|r| r.state != "finished")
                    })
            })
        };

        for card in &snapshot.cards {
            if filter_pid
                .as_deref()
                .is_some_and(|pid| card.project_id != pid)
            {
                continue;
            }
            let Some(wt_path) = card.worktree.as_deref().filter(|s| !s.is_empty()) else {
                continue;
            };
            if !card.resolved() {
                continue;
            }
            let resolved_at = card
                .resolved_at()
                .or(card.archived_at)
                .unwrap_or(card.created_at);
            if resolved_at > cutoff {
                skipped.push(json!({
                    "kind": "card",
                    "card_id": card.id,
                    "card_number": card.number,
                    "project_id": card.project_id,
                    "path": wt_path,
                    "reason": "merged_within_retention",
                }));
                continue;
            }
            let Some(project) = snapshot.projects.iter().find(|pr| pr.id == card.project_id) else {
                continue;
            };
            let source_dir = card
                .workspace
                .as_ref()
                .map(|w| w.directory.as_str())
                .unwrap_or(project.directory.as_str());
            let ops = self.ops();
            if has_live_terminal(wt_path, &ops) {
                dirty.push(json!({
                    "kind": "card",
                    "card_id": card.id,
                    "card_number": card.number,
                    "project_id": card.project_id,
                    "project_name": project.name,
                    "branch": card.branch,
                    "path": wt_path,
                    "reason": "live_terminal_open",
                }));
                continue;
            }
            let (state, is_dirty, branch) = ops.worktree_status(Path::new(wt_path))?;
            if state == "missing" {
                if !dry_run {
                    cleared_card_ids.push(card.id.clone());
                }
                removed.push(json!({
                    "kind": "card",
                    "card_id": card.id,
                    "card_number": card.number,
                    "project_id": card.project_id,
                    "project_name": project.name,
                    "branch": branch.or_else(|| card.branch.clone()),
                    "path": wt_path,
                    "dry_run": dry_run,
                    "missing": true,
                }));
                continue;
            }
            if is_dirty {
                dirty.push(json!({
                    "kind": "card",
                    "card_id": card.id,
                    "card_number": card.number,
                    "project_id": card.project_id,
                    "project_name": project.name,
                    "branch": branch.or_else(|| card.branch.clone()),
                    "path": wt_path,
                    "reason": "uncommitted_changes",
                }));
                continue;
            }
            if !dry_run {
                if let Err(err) = ops.remove_worktree(
                    &self.home,
                    Path::new(source_dir),
                    Path::new(wt_path),
                    false,
                ) {
                    dirty.push(json!({
                        "kind": "card",
                        "card_id": card.id,
                        "card_number": card.number,
                        "project_id": card.project_id,
                        "project_name": project.name,
                        "branch": branch.or_else(|| card.branch.clone()),
                        "path": wt_path,
                        "reason": format!("{err:#}"),
                    }));
                    continue;
                }
                cleared_card_ids.push(card.id.clone());
                changed = true;
            }
            removed.push(json!({
                "kind": "card",
                "card_id": card.id,
                "card_number": card.number,
                "project_id": card.project_id,
                "project_name": project.name,
                "branch": branch.or_else(|| card.branch.clone()),
                "path": wt_path,
                "dry_run": dry_run,
            }));
        }

        for wt in &snapshot.worktrees {
            if filter_pid
                .as_deref()
                .is_some_and(|pid| wt.project_id != pid)
            {
                continue;
            }
            if wt.created_at > cutoff {
                continue;
            }
            let ops = self.ops();
            let (state, is_dirty, branch) = ops.worktree_status(Path::new(&wt.path))?;
            if state == "missing" {
                if !dry_run {
                    removed_wt_ids.push(wt.id.clone());
                }
                removed.push(json!({
                    "kind": "managed_worktree",
                    "worktree_id": wt.id,
                    "project_id": wt.project_id,
                    "branch": branch.unwrap_or_else(|| wt.branch.clone()),
                    "path": wt.path,
                    "dry_run": dry_run,
                    "missing": true,
                }));
                continue;
            }
            // Only collect standalone managed worktrees whose HEAD is already merged into base_branch.
            let merged_into_base =
                ops.branch_merged(Path::new(&wt.directory), &wt.branch, &wt.base_branch);
            if !merged_into_base {
                continue;
            }
            if has_live_terminal(&wt.path, &ops) {
                dirty.push(json!({
                    "kind": "managed_worktree",
                    "worktree_id": wt.id,
                    "project_id": wt.project_id,
                    "branch": wt.branch,
                    "path": wt.path,
                    "reason": "live_terminal_open",
                }));
                continue;
            }
            if is_dirty {
                dirty.push(json!({
                    "kind": "managed_worktree",
                    "worktree_id": wt.id,
                    "project_id": wt.project_id,
                    "branch": wt.branch,
                    "path": wt.path,
                    "reason": "uncommitted_changes",
                }));
                continue;
            }
            if !dry_run {
                if let Err(err) = ops.remove_worktree(
                    &self.home,
                    Path::new(&wt.directory),
                    Path::new(&wt.path),
                    false,
                ) {
                    dirty.push(json!({
                        "kind": "managed_worktree",
                        "worktree_id": wt.id,
                        "project_id": wt.project_id,
                        "branch": wt.branch,
                        "path": wt.path,
                        "reason": format!("{err:#}"),
                    }));
                    continue;
                }
                removed_wt_ids.push(wt.id.clone());
                changed = true;
            }
            removed.push(json!({
                "kind": "managed_worktree",
                "worktree_id": wt.id,
                "project_id": wt.project_id,
                "branch": wt.branch,
                "path": wt.path,
                "dry_run": dry_run,
            }));
        }

        if !dry_run && (!cleared_card_ids.is_empty() || !removed_wt_ids.is_empty()) {
            self.store.lock().unwrap().write(|db| {
                for cid in &cleared_card_ids {
                    if let Ok(mut card) = db.get::<Card>(cid) {
                        card.worktree = None;
                        db.save(&card)?;
                    }
                }
                for wid in &removed_wt_ids {
                    db.conn
                        .execute("DELETE FROM managed_worktrees WHERE id=?1", params![wid])?;
                }
                Ok(())
            })?;
            if changed {
                self.git_changed();
            }
            self.emit("application", None);
        }

        Ok(json!({
            "dry_run": dry_run,
            "merged_before_ms": merged_before_ms,
            "removed": removed,
            "dirty": dirty,
            "skipped": skipped,
        }))
    }
}
