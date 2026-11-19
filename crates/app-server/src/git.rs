//! Git client RPC surface for the Git workbench (checkout switching, branch commit logs,
//! single-commit diff inspection, file-level stage/unstage/discard, commit/amend, fetch/pull/push).
use super::*;

type IdempotentKey = (String, String);

pub(super) struct CheckoutResolution {
    pub project: Project,
    pub cwd: String,
    pub is_project_root: bool,
    pub card_id: Option<String>,
    pub checkouts: Vec<Value>,
}

impl AppServer {
    pub(super) fn git_call(&self, method: &str, p: Value) -> Result<Value> {
        match method {
            "git.refs" => self.git_refs_rpc(p),
            "git.compare" => self.git_compare(p),
            "git.file_lines" => self.git_file_lines_rpc(p),
            "git.log" => self.git_log_rpc(p),
            "git.commit_detail" => self.git_commit_detail_rpc(p),
            "git.stage" => self.git_stage_rpc(p),
            "git.unstage" => self.git_unstage_rpc(p),
            "git.discard" => self.git_discard_rpc(p),
            "git.commit" => self.git_commit_rpc(p),
            "git.fetch" => self.git_fetch_rpc(p),
            "git.pull" => self.git_pull_rpc(p),
            "git.push" => self.git_push_rpc(p),
            _ => bail!("Unknown Git operation: {method}"),
        }
    }

    /// Resolves the target `Project`, available checkouts (`project` + active `Card` worktrees +
    /// standalone `ManagedWorktree`s), and validates that any requested `cwd` belongs to one of
    /// this Project's registered checkouts (rejecting arbitrary filesystem traversal).
    pub(super) fn resolve_project_checkout(&self, p: &Value) -> Result<CheckoutResolution> {
        let (project, card_from_param, cards, managed) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let (proj, card_opt) = if p["card_id"].as_str().is_some_and(|s| !s.trim().is_empty())
                || super::parse_card_number(&p["card"]).is_some()
            {
                let c = super::resolve_card(db, p)?;
                let base_proj: Project = db.get(&c.project_id)?;
                let bound = self.bound_project(&base_proj, &c);
                (bound, Some(c))
            } else {
                let pid = super::resolve_project_id(db, strp(p, "project_id")?)?;
                let proj: Project = db.get(&pid)?;
                (proj, None)
            };
            let proj_id = proj.id.clone();
            let cards: Vec<Card> = db
                .list::<Card>()?
                .into_iter()
                .filter(|c| c.project_id == proj_id && c.removed_at.is_none())
                .collect();
            let managed: Vec<ManagedWorktree> = db
                .list::<ManagedWorktree>()?
                .into_iter()
                .filter(|w| w.project_id == proj_id)
                .collect();
            (proj, card_opt, cards, managed)
        };

        let machine = project.work_location().machine;
        let ops = self.ops(&machine);

        let mut checkouts = Vec::<Value>::new();
        checkouts.push(json!({
            "kind": "project",
            "label": format!("{} (main)", project.name),
            "path": project.directory,
            "branch": project.target_branch,
        }));

        let mut allowed_paths: Vec<(String, Option<String>)> =
            vec![(project.directory.clone(), None)];

        for c in &cards {
            if c.workspace
                .as_ref()
                .is_some_and(|w| w.location.machine != machine)
            {
                continue;
            }
            if let Some(wt_path) = c.worktree.as_deref().filter(|s| !s.is_empty()) {
                allowed_paths.push((wt_path.to_owned(), Some(c.id.clone())));
                if c.active() {
                    checkouts.push(json!({
                        "kind": "card",
                        "card_id": c.id,
                        "card_number": c.number,
                        "label": format!("#{} {}", c.number, c.title),
                        "path": wt_path,
                        "branch": c.branch,
                    }));
                }
            }
        }
        for wt in &managed {
            if !wt.path.is_empty() {
                allowed_paths.push((wt.path.clone(), None));
                checkouts.push(json!({
                    "kind": "worktree",
                    "worktree_id": wt.id,
                    "label": format!("{} (worktree)", wt.branch),
                    "path": wt.path,
                    "branch": wt.branch,
                }));
            }
        }

        let requested_cwd = p["cwd"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                card_from_param
                    .as_ref()
                    .and_then(|c| c.worktree.as_deref())
                    .filter(|s| !s.is_empty())
            });

        let (resolved_cwd, matched_card) = if let Some(req) = requested_cwd {
            ensure!(
                !Path::new(req)
                    .components()
                    .any(|comp| matches!(comp, std::path::Component::ParentDir)),
                "Checkout path cannot contain '..'"
            );
            if let Some((path, cid)) = allowed_paths.iter().find(|(ap, _)| ap == req) {
                (path.clone(), cid.clone())
            } else {
                // Canonical path fallback (e.g. `/var/...` vs `/private/var/...` on macOS).
                let req_canon = ops
                    .host
                    .canonical(Path::new(req))
                    .ok()
                    .map(|pb| pb.to_string_lossy().into_owned());
                let found = req_canon.and_then(|rc| {
                    allowed_paths.iter().find_map(|(ap, cid)| {
                        let ap_canon = ops
                            .host
                            .canonical(Path::new(ap))
                            .ok()
                            .map(|pb| pb.to_string_lossy().into_owned())?;
                        (ap_canon == rc).then(|| (ap.clone(), cid.clone()))
                    })
                });
                let Some((path, cid)) = found else {
                    bail!(
                        "Directory '{req}' is outside registered checkouts for Project {}",
                        project.name
                    );
                };
                (path, cid)
            }
        } else {
            (
                project.directory.clone(),
                card_from_param.as_ref().map(|c| c.id.clone()),
            )
        };

        let is_project_root = resolved_cwd == project.directory;
        let card_id = matched_card.or_else(|| card_from_param.map(|c| c.id));

        Ok(CheckoutResolution {
            project,
            cwd: resolved_cwd,
            is_project_root,
            card_id,
            checkouts,
        })
    }

    fn check_idempotent_operation(
        &self,
        method: &str,
        p: &Value,
    ) -> Result<(Option<IdempotentKey>, Option<Value>)> {
        let Some(key) = p["request_key"].as_str().filter(|s| !s.is_empty()) else {
            return Ok((None, None));
        };
        let fingerprint = canban_context::digest(&serde_json::to_vec(&json!([method, p]))?);
        let existing = self
            .store
            .lock()
            .unwrap()
            .db()
            .conn
            .query_row(
                "SELECT fingerprint,result FROM operations WHERE id=?1",
                [key],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((old_fp, res_str)) = existing {
            ensure!(
                old_fp == fingerprint,
                "request_key_conflict: same key, different payload"
            );
            return Ok((None, Some(serde_json::from_str(&res_str)?)));
        }
        Ok((Some((key.to_owned(), fingerprint)), None))
    }

    fn save_idempotent_operation(&self, op: Option<IdempotentKey>, result: &Value) -> Result<()> {
        if let Some((key, fingerprint)) = op {
            let encoded = serde_json::to_string(result)?;
            self.store.lock().unwrap().write(|db| {
                db.conn.execute(
                    "INSERT OR REPLACE INTO operations(id,fingerprint,result) VALUES(?1,?2,?3)",
                    params![key, fingerprint, encoded],
                )?;
                Ok(())
            })?;
        }
        Ok(())
    }

    fn git_refs_rpc(&self, p: Value) -> Result<Value> {
        let ctx = self.resolve_project_checkout(&p)?;
        let machine = ctx.project.work_location().machine;
        let _permit = self.check(&machine);
        let mut resp = self
            .ops(&machine)
            .git_refs_in(&ctx.project, Some(&ctx.cwd))?;
        // Match on this host's canonical checkout path, never on a branch name.
        // Multiple active Cards sharing a directory are deliberately unassigned.
        let ops = self.ops(&machine);
        let canonical = |path: &str| {
            ops.host
                .canonical(Path::new(path))
                .unwrap_or_else(|_| PathBuf::from(path))
        };
        let active = canonical(&ctx.cwd);
        let mut related = std::collections::BTreeSet::new();
        for checkout in &ctx.checkouts {
            if checkout["kind"] == "card"
                && canonical(checkout["path"].as_str().unwrap_or("")) == active
                && let Some(id) = checkout["card_id"].as_str()
            {
                related.insert(id.to_owned());
            }
        }
        resp["related_card_id"] = if related.len() == 1 {
            json!(related.into_iter().next())
        } else {
            Value::Null
        };
        let mut seen = std::collections::BTreeSet::new();
        resp["checkouts"] = json!(
            ctx.checkouts
                .into_iter()
                .filter(|checkout| {
                    seen.insert(canonical(checkout["path"].as_str().unwrap_or("")))
                })
                .collect::<Vec<_>>()
        );
        Ok(resp)
    }

    fn git_log_rpc(&self, p: Value) -> Result<Value> {
        let ctx = self.resolve_project_checkout(&p)?;
        let machine = ctx.project.work_location().machine;
        let _permit = self.check(&machine);
        let rev = p["rev"].as_str();
        let skip = p["skip"].as_u64().unwrap_or(0) as usize;
        let limit = p["limit"].as_u64().unwrap_or(40) as usize;
        let query = p["query"].as_str();
        self.ops(&machine)
            .git_log(Path::new(&ctx.cwd), rev, skip, limit, query)
    }

    fn git_commit_detail_rpc(&self, p: Value) -> Result<Value> {
        let ctx = self.resolve_project_checkout(&p)?;
        let machine = ctx.project.work_location().machine;
        let _permit = self.check(&machine);
        let sha = strp(&p, "sha")?;
        let parent_index = p["parent_index"].as_u64().unwrap_or(0) as usize;
        self.ops(&machine)
            .git_commit_detail(&ctx.project, Some(&ctx.cwd), sha, parent_index)
    }

    fn git_file_lines_rpc(&self, p: Value) -> Result<Value> {
        let ctx = self.resolve_project_checkout(&p)?;
        let machine = ctx.project.work_location().machine;
        let _permit = self.check(&machine);
        let path = strp(&p, "path")?;
        let source = p["source"].as_str().unwrap_or("workdir");
        let rev = p["rev"].as_str();
        let start_line = p["start_line"].as_u64().map(|n| n as usize);
        let end_line = p["end_line"].as_u64().map(|n| n as usize);
        self.ops(&machine)
            .git_file_lines(canban_workspaces::compare::FileLinesRequest {
                project: &ctx.project,
                cwd_override: Some(&ctx.cwd),
                path,
                source,
                rev,
                start_line,
                end_line,
            })
    }

    fn git_stage_rpc(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Staging files in Git requires a direct user action"
        );
        let (idem, cached) = self.check_idempotent_operation("git.stage", &p)?;
        if let Some(res) = cached {
            return Ok(res);
        }
        let ctx = self.resolve_project_checkout(&p)?;
        let machine = ctx.project.work_location().machine;
        let paths = strings(&p, "paths");
        let all = p["all"].as_bool().unwrap_or(false);
        let res = {
            let _permit = self.check(&machine);
            self.ops(&machine)
                .git_stage(Path::new(&ctx.cwd), &paths, all)?
        };
        self.save_idempotent_operation(idem, &res)?;
        self.git_changed(&machine);
        self.emit("review", ctx.card_id);
        Ok(res)
    }

    fn git_unstage_rpc(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Unstaging files in Git requires a direct user action"
        );
        let (idem, cached) = self.check_idempotent_operation("git.unstage", &p)?;
        if let Some(res) = cached {
            return Ok(res);
        }
        let ctx = self.resolve_project_checkout(&p)?;
        let machine = ctx.project.work_location().machine;
        let paths = strings(&p, "paths");
        let all = p["all"].as_bool().unwrap_or(false);
        let res = {
            let _permit = self.check(&machine);
            self.ops(&machine)
                .git_unstage(Path::new(&ctx.cwd), &paths, all)?
        };
        self.save_idempotent_operation(idem, &res)?;
        self.git_changed(&machine);
        self.emit("review", ctx.card_id);
        Ok(res)
    }

    fn git_discard_rpc(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Discarding working tree changes requires a direct user action"
        );
        let (idem, cached) = self.check_idempotent_operation("git.discard", &p)?;
        if let Some(res) = cached {
            return Ok(res);
        }
        let ctx = self.resolve_project_checkout(&p)?;
        let machine = ctx.project.work_location().machine;
        let paths = strings(&p, "paths");
        let res = {
            let _permit = self.check(&machine);
            self.ops(&machine)
                .git_discard(Path::new(&ctx.cwd), &paths)?
        };
        self.save_idempotent_operation(idem, &res)?;
        self.git_changed(&machine);
        self.request_deliveries();
        self.emit("review", ctx.card_id);
        Ok(res)
    }

    fn git_commit_rpc(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Committing from Git requires a direct user action"
        );
        let (idem, cached) = self.check_idempotent_operation("git.commit", &p)?;
        if let Some(res) = cached {
            return Ok(res);
        }
        let ctx = self.resolve_project_checkout(&p)?;
        let _gate = if ctx.is_project_root {
            let holder = self.checkout_holder_label(
                "Git commit",
                &ctx.project,
                ctx.project.target_branch.as_deref(),
            );
            Some(
                self.try_lock_checkout(&ctx.project, holder)
                    .map_err(anyhow::Error::msg)?,
            )
        } else {
            None
        };
        let machine = ctx.project.work_location().machine;
        let message = strp(&p, "message")?;
        let amend = p["amend"].as_bool().unwrap_or(false);
        let stage_all = p["stage_all"].as_bool().unwrap_or(false);
        let and_push = p["push"].as_bool().unwrap_or(false);

        let mut res = {
            let _permit = self.check(&machine);
            let ops = self.ops(&machine);
            let mut commit_res = ops.git_commit(Path::new(&ctx.cwd), message, amend, stage_all)?;
            if and_push {
                match ops.git_push(
                    &ctx.project,
                    Path::new(&ctx.cwd),
                    p["remote"].as_str(),
                    p["set_upstream"].as_bool().unwrap_or(false),
                ) {
                    Ok(push_res) => {
                        commit_res["pushed"] = json!(true);
                        commit_res["push"] = push_res;
                    }
                    Err(e) => {
                        commit_res["pushed"] = json!(false);
                        commit_res["push_error"] = json!(format!("{e:#}"));
                    }
                }
            }
            commit_res
        };
        if res.get("pushed").is_none() {
            res["pushed"] = json!(false);
        }

        self.save_idempotent_operation(idem, &res)?;
        self.git_changed(&machine);
        drop(_gate);
        self.request_deliveries();
        self.emit("review", ctx.card_id);
        Ok(res)
    }

    fn git_fetch_rpc(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Git fetch requires a direct user action"
        );
        let (idem, cached) = self.check_idempotent_operation("git.fetch", &p)?;
        if let Some(res) = cached {
            return Ok(res);
        }
        let ctx = self.resolve_project_checkout(&p)?;
        let machine = ctx.project.work_location().machine;
        let prune = p["prune"].as_bool().unwrap_or(true);
        let res = {
            let _permit = self.check(&machine);
            self.ops(&machine).git_fetch(
                &ctx.project,
                Path::new(&ctx.cwd),
                p["remote"].as_str(),
                prune,
            )?
        };
        self.save_idempotent_operation(idem, &res)?;
        self.git_changed(&machine);
        self.request_deliveries();
        self.emit("review", ctx.card_id);
        Ok(res)
    }

    fn git_pull_rpc(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Git pull requires a direct user action"
        );
        let (idem, cached) = self.check_idempotent_operation("git.pull", &p)?;
        if let Some(res) = cached {
            return Ok(res);
        }
        let ctx = self.resolve_project_checkout(&p)?;
        let _gate = if ctx.is_project_root {
            let branch = p["branch"]
                .as_str()
                .or(ctx.project.target_branch.as_deref());
            let holder = self.checkout_holder_label("Git pull", &ctx.project, branch);
            Some(
                self.try_lock_checkout(&ctx.project, holder)
                    .map_err(anyhow::Error::msg)?,
            )
        } else {
            None
        };
        let machine = ctx.project.work_location().machine;
        let res = {
            let _permit = self.check(&machine);
            self.ops(&machine).git_pull(
                &ctx.project,
                Path::new(&ctx.cwd),
                p["remote"].as_str(),
                p["branch"].as_str(),
            )?
        };
        self.save_idempotent_operation(idem, &res)?;
        self.git_changed(&machine);
        drop(_gate);
        self.request_deliveries();
        self.emit("review", ctx.card_id);
        Ok(res)
    }

    fn git_push_rpc(&self, p: Value) -> Result<Value> {
        ensure!(
            p["execution"].is_null(),
            "Git push requires a direct user action"
        );
        let (idem, cached) = self.check_idempotent_operation("git.push", &p)?;
        if let Some(res) = cached {
            return Ok(res);
        }
        let ctx = self.resolve_project_checkout(&p)?;
        let machine = ctx.project.work_location().machine;
        let set_upstream = p["set_upstream"].as_bool().unwrap_or(false);
        let res = {
            let _permit = self.check(&machine);
            self.ops(&machine).git_push(
                &ctx.project,
                Path::new(&ctx.cwd),
                p["remote"].as_str(),
                set_upstream,
            )?
        };
        self.save_idempotent_operation(idem, &res)?;
        self.git_changed(&machine);
        self.emit("review", ctx.card_id);
        Ok(res)
    }
}
