//! Standalone Canban-managed branch worktrees created from Terminals / Command Palette.
use super::*;

impl AppServer {
    pub(super) fn worktree_call(&self, method: &str, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Managed worktree operations require a direct user action"
        );
        match method {
            "worktree.create" => self.create_managed_worktree(p),
            "worktree.status" => self.managed_worktree_status(p),
            "worktree.promote" => self.promote_managed_worktree(p),
            "worktree.remove" => self.remove_managed_worktree(p),
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
        let location = project.work_location();
        self.resolve_machine(&location.machine, true)?;

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

        let exec_home = self.execution_home(&location.machine)?;
        let ops = self.ops(&location.machine);
        let (path_buf, resolved_base, base_commit) = {
            let _permit = self.check(&location.machine);
            ops.create_branch_worktree(&exec_home, &project, branch, base_branch, sync_base)?
        };
        let path_str = path_buf.to_string_lossy().into_owned();
        let wt = ManagedWorktree {
            id: format!("wt-{}", id()),
            project_id: project.id.clone(),
            branch: branch.to_owned(),
            base_branch: resolved_base,
            base_commit: Some(base_commit),
            path: path_str.clone(),
            location: location.clone(),
            created_at: now(),
        };

        let saved_wt: ManagedWorktree =
            serde_json::from_value(self.mutate("worktree.create", &p, |db| {
                db.save(&wt)?;
                Ok(serde_json::to_value(&wt)?)
            })?)?;
        self.git_changed(&location.machine);

        let launch_mode = p["launch"].as_str().unwrap_or("shell");
        let terminal = if launch_mode == "none" {
            None
        } else {
            let mut launch_params = json!({
                "project_id": project.id,
                "machine": location.machine,
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
        let _permit = self.check(&wt.location.machine);
        let (state, dirty, branch) = self
            .ops(&wt.location.machine)
            .worktree_status(Path::new(&wt.path))?;
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
                location: wt.location.clone(),
                path: wt.path.clone(),
                branch: Some(wt.branch.clone()),
                base_commit: wt.base_commit.clone(),
                target_branch: Some(wt.base_branch.clone()),
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
            Ok(serde_json::to_value(card)?)
        })?;
        self.invalidate_contexts();
        Ok(card_val)
    }

    fn remove_managed_worktree(&self, p: Value) -> Result<Value> {
        let wt_id = strp(&p, "worktree_id")?;
        let force = p["force"].as_bool().unwrap_or(false);
        let (wt, snapshot) = {
            let wt: ManagedWorktree = self.store.lock().unwrap().db().get(wt_id)?;
            let snap = self.snapshot()?;
            (wt, snap)
        };

        // Ensure no live terminal PTY is currently inside this worktree
        let ops = self.ops(&wt.location.machine);
        for ctx in &snapshot.terminal_contexts {
            if ctx.machine == wt.location.machine
                && let Some(cwd) = &ctx.cwd
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

        let exec_home = self.execution_home(&wt.location.machine)?;
        {
            let _permit = self.check(&wt.location.machine);
            ops.remove_worktree(
                &exec_home,
                Path::new(&wt.location.directory),
                Path::new(&wt.path),
                force,
            )?;
        }

        let res = self.mutate("worktree.remove", &p, |db| {
            db.conn
                .execute("DELETE FROM managed_worktrees WHERE id=?1", params![wt.id])?;
            Ok(json!({"removed": true, "worktree_id": wt.id}))
        })?;
        self.git_changed(&wt.location.machine);
        Ok(res)
    }
}
