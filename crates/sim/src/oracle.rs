//! Universal Invariant Oracle checking data correctness, durability, SQLite integrity,
//! workflow/Herdr safety, and canonical state digests across all Canban DST domains.
use crate::world::SimWorld;
use anyhow::{Context, Result, bail, ensure};
use canban_types::{
    Card, EnvEntryMeta, HostConnectionState, HumanGate, Snapshot, SyncItemState, WorkflowRun,
};
use rusqlite::{Connection, OpenFlags};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::atomic::Ordering,
};

/// Durable expectation recorded after a server/client write commits.
#[derive(Clone, Debug)]
pub struct CommittedCardExpectation {
    pub project_id: String,
    pub min_revision: u64,
}

/// Durable expectation recorded after a `WorkflowRun` commits on a server.
#[derive(Clone, Debug)]
pub struct CommittedWorkflowRunExpectation {
    pub project_id: String,
    pub workflow_id: String,
    pub min_visit: u32,
}

/// Durable expectation recorded after a Project `Note` commits on a server.
#[derive(Clone, Debug)]
pub struct CommittedNoteExpectation {
    pub project_id: String,
    pub min_revision: u64,
}

/// Idempotency intent tracked for a mutating RPC `request_key` (at-most-once model).
#[derive(Clone, Debug)]
pub struct RequestKeyAttempt {
    pub host: String,
    pub request_key: String,
    pub kind: String,
    pub attempts: u64,
    pub acknowledged: bool,
    /// Optional unique domain marker (e.g. Card title, Comment body, Note title) used to assert
    /// `<= 1` (and `== 1` once acknowledged) materialized domain side effects in addition to SQLite `operations`.
    pub effect_marker: Option<String>,
}

/// Tracks historical observations across simulation steps to detect regressions
/// (e.g., revision rollback, `CardRead` regression, lost durable writes, resurrected read
/// notifications, plaintext secret leaks, or unsafe idle reclamation).
#[derive(Default)]
pub struct InvariantOracle {
    /// Highest observed `Card.revision` per `(host_name, raw_card_id)`.
    pub max_card_revision: BTreeMap<(String, String), u64>,
    /// Read-Your-Writes / monotonic-read floor per `(client_id, host_id, raw_card_id) -> min_seen_rev`.
    pub min_seen_rev: BTreeMap<(String, String, String), i64>,
    /// Highest observed `CardRead.sequence` per `(host_name, raw_card_id)`.
    pub max_card_read_seq: BTreeMap<(String, String), u64>,
    /// Highest observed `WorkflowRun.visit` per `(host_name, raw_run_id)`.
    pub max_workflow_visit: BTreeMap<(String, String), u32>,
    /// Secret plaintext strings generated during the run that must NEVER leak to disk, snapshots, or PTYs.
    pub tracked_secrets: Vec<String>,
    /// Opt-in assertion flag: when true, quiescence requires at least one `.outbox.json` scan.
    pub secrets_queued_offline: bool,
    /// Number of `.outbox.json` files scanned across all checks during this run.
    pub outbox_files_scanned: usize,
    /// Notification IDs explicitly marked read (must never resurrect as unread).
    pub read_notice_ids: BTreeMap<String, BTreeSet<String>>,
    /// Terminals currently marked as protected (viewer active / working / human gate).
    /// Maps `(host_name, herdr_terminal_addr)` -> expected max `sent_inputs` count while protected.
    pub protected_terminals: BTreeMap<(String, String), usize>,
    /// Explicit opt-in mutation IDs allowed to remain in `SyncItemState::Failed` (for intentional failure tests).
    pub allow_failed_ids: BTreeSet<String>,
    /// Committed cards that must never vanish or regress below `min_revision` across restarts.
    pub committed_cards: BTreeMap<(String, String), CommittedCardExpectation>,
    /// Committed comment bodies per `(host_name, raw_card_id)` that must survive restarts.
    pub committed_comments: BTreeMap<(String, String), BTreeSet<String>>,
    /// Committed global env entries per `(host_name, key)` that must survive restarts.
    pub committed_env: BTreeMap<(String, String), EnvEntryMeta>,
    /// Committed `WorkflowRun`s that must survive crashes/bounces with monotonic `visit`.
    pub committed_workflow_runs: BTreeMap<(String, String), CommittedWorkflowRunExpectation>,
    /// Committed persistent `MergeBatch`es (`@merger` batches) per `(host_name, raw_batch_id)` -> `raw_project_id`.
    pub committed_merge_batches: BTreeMap<(String, String), String>,
    /// Committed `ScheduledRun`s per `(host_name, raw_run_id)` -> `(raw_project_id, rule_id)`.
    pub committed_scheduled_runs: BTreeMap<(String, String), (String, String)>,
    /// Committed Project `Note`s per `(host_name, raw_note_id)` that must survive restarts.
    pub committed_notes: BTreeMap<(String, String), CommittedNoteExpectation>,
    /// Tracked `request_key` mutating RPC intents per `(server_host_name, request_key)`.
    pub request_key_attempts: BTreeMap<(String, String), RequestKeyAttempt>,
    /// Baseline `(dial_count, reconnect_count)` observed when a remote host enters `AuthFailed`,
    /// used to verify zero background reconnect dials while halted.
    pub auth_failed_baselines: BTreeMap<(String, String), (u64, u32)>,
}

impl InvariantOracle {
    pub fn track_secret(&mut self, secret: impl Into<String>) {
        self.tracked_secrets.push(secret.into());
    }

    /// Marks that at least one tracked secret was queued while offline, requiring the recursive
    /// secret scanner to observe at least one `.outbox.json` file before quiescence completes.
    pub fn mark_secret_queued_offline(&mut self) {
        self.secrets_queued_offline = true;
    }

    /// Explicitly allows `mutation_id` to remain `SyncItemState::Failed` at quiescence.
    pub fn allow_failed_mutation(&mut self, mutation_id: impl Into<String>) {
        self.allow_failed_ids.insert(mutation_id.into());
    }

    pub fn record_notice_read(&mut self, host_id: &str, notice_id: impl Into<String>) {
        let raw = strip_host_prefix(&notice_id.into());
        self.read_notice_ids
            .entry(host_id.to_owned())
            .or_default()
            .insert(raw);
    }

    /// Registers a Herdr terminal as protected against unsolicited inputs or kills, snapshotting
    /// its current `sent_inputs` count on `host_name`.
    pub fn protect_terminal(
        &mut self,
        world: &SimWorld,
        host_name: &str,
        herdr_addr: impl Into<String>,
    ) {
        let addr = herdr_addr.into();
        let current_sends = world
            .herdr(host_name)
            .map(|h| {
                h.sent_inputs()
                    .iter()
                    .filter(|s| s.terminal == addr)
                    .count()
            })
            .unwrap_or(0);
        self.protected_terminals
            .insert((host_name.to_owned(), addr), current_sends);
    }

    pub fn unprotect_terminal(&mut self, host_name: &str, herdr_addr: &str) {
        self.protected_terminals
            .remove(&(host_name.to_owned(), herdr_addr.to_owned()));
    }

    /// Records a committed Card write that must remain durable across server bounces.
    pub fn record_committed_card(
        &mut self,
        host_name: &str,
        card_id: &str,
        project_id: &str,
        revision: u64,
    ) {
        let raw_card = strip_host_prefix(card_id);
        let raw_proj = strip_host_prefix(project_id);
        let key = (host_name.to_owned(), raw_card);
        self.committed_cards
            .entry(key)
            .and_modify(|e| {
                e.min_revision = e.min_revision.max(revision);
                if e.project_id.is_empty() {
                    e.project_id = raw_proj.clone();
                }
            })
            .or_insert(CommittedCardExpectation {
                project_id: raw_proj,
                min_revision: revision,
            });
    }

    /// Records a committed comment body that must remain durable across server bounces.
    pub fn record_committed_comment(
        &mut self,
        host_name: &str,
        card_id: &str,
        comment_body: impl Into<String>,
    ) {
        let raw_card = strip_host_prefix(card_id);
        self.committed_comments
            .entry((host_name.to_owned(), raw_card))
            .or_default()
            .insert(comment_body.into());
    }

    /// Records a committed global env entry that must remain durable across server bounces.
    pub fn record_committed_env(
        &mut self,
        host_name: &str,
        key: impl Into<String>,
        meta: EnvEntryMeta,
    ) {
        self.committed_env
            .insert((host_name.to_owned(), key.into()), meta);
    }

    pub fn remove_committed_env(&mut self, host_name: &str, key: &str) {
        self.committed_env
            .remove(&(host_name.to_owned(), key.to_owned()));
    }

    /// Records a committed `WorkflowRun` that must remain durable across server bounces.
    pub fn record_committed_workflow_run(
        &mut self,
        host_name: &str,
        run_id: &str,
        project_id: &str,
        workflow_id: &str,
        visit: u32,
    ) {
        let raw_run = strip_host_prefix(run_id);
        let raw_proj = strip_host_prefix(project_id);
        self.committed_workflow_runs
            .entry((host_name.to_owned(), raw_run))
            .and_modify(|e| {
                e.min_visit = e.min_visit.max(visit);
                if e.project_id.is_empty() {
                    e.project_id = raw_proj.clone();
                }
            })
            .or_insert(CommittedWorkflowRunExpectation {
                project_id: raw_proj,
                workflow_id: workflow_id.to_owned(),
                min_visit: visit,
            });
    }

    /// Records a committed persistent `MergeBatch` (`@merger` batch) that must survive bounces.
    pub fn record_committed_merge_batch(
        &mut self,
        host_name: &str,
        batch_id: &str,
        project_id: &str,
    ) {
        let raw_batch = strip_host_prefix(batch_id);
        let raw_proj = strip_host_prefix(project_id);
        self.committed_merge_batches
            .insert((host_name.to_owned(), raw_batch), raw_proj);
    }

    /// Records a committed `ScheduledRun` that must survive bounces.
    pub fn record_committed_scheduled_run(
        &mut self,
        host_name: &str,
        run_id: &str,
        project_id: &str,
        rule_id: &str,
    ) {
        let raw_run = strip_host_prefix(run_id);
        let raw_proj = strip_host_prefix(project_id);
        self.committed_scheduled_runs.insert(
            (host_name.to_owned(), raw_run),
            (raw_proj, rule_id.to_owned()),
        );
    }

    /// Records a committed Project `Note` that must remain durable across bounces.
    pub fn record_committed_note(
        &mut self,
        host_name: &str,
        note_id: &str,
        project_id: &str,
        revision: u64,
    ) {
        let raw_note = strip_host_prefix(note_id);
        let raw_proj = strip_host_prefix(project_id);
        self.committed_notes
            .entry((host_name.to_owned(), raw_note))
            .and_modify(|e| {
                e.min_revision = e.min_revision.max(revision);
                if e.project_id.is_empty() {
                    e.project_id = raw_proj.clone();
                }
            })
            .or_insert(CommittedNoteExpectation {
                project_id: raw_proj,
                min_revision: revision,
            });
    }

    /// Clears committed durability expectations belonging to a project intentionally archived via `project.archive`.
    pub fn clear_committed_project(&mut self, host_name: &str, project_id: &str) {
        let raw_proj = strip_host_prefix(project_id);
        let removed_cards: BTreeSet<String> = self
            .committed_cards
            .iter()
            .filter(|((h, _), exp)| h == host_name && exp.project_id == raw_proj)
            .map(|((_, cid), _)| cid.clone())
            .collect();
        self.committed_cards
            .retain(|(h, _), exp| !(h == host_name && exp.project_id == raw_proj));
        self.committed_comments
            .retain(|(h, cid), _| !(h == host_name && removed_cards.contains(cid)));
        self.committed_workflow_runs
            .retain(|(h, _), exp| !(h == host_name && exp.project_id == raw_proj));
        self.committed_merge_batches
            .retain(|(h, _), pid| !(h == host_name && *pid == raw_proj));
        self.committed_scheduled_runs
            .retain(|(h, _), (pid, _)| !(h == host_name && *pid == raw_proj));
        self.committed_notes
            .retain(|(h, _), exp| !(h == host_name && exp.project_id == raw_proj));
    }

    /// Records an observed `Card.revision` for `(client, host, card_id)` and immediately fails
    /// on any monotonic-read regression (`rev < prev`).
    pub fn observe_card_revision(
        &mut self,
        client: &str,
        host: &str,
        card_id: &str,
        rev: i64,
    ) -> Result<()> {
        let norm_host = normalize_client_host_id(host);
        let raw_card = strip_host_prefix(card_id);
        let key = (client.to_owned(), norm_host.clone(), raw_card.clone());
        if let Some(&prev) = self.min_seen_rev.get(&key) {
            ensure!(
                rev >= prev,
                "[{client}][{norm_host}] MONOTONIC READ / RYW VIOLATION: Card {raw_card} revision regressed from {prev} to {rev}"
            );
            if rev > prev {
                self.min_seen_rev.insert(key, rev);
            }
        } else {
            self.min_seen_rev.insert(key, rev);
        }
        Ok(())
    }

    /// Extracts a card ID and revision from an RPC write receipt (top-level `id`/`card_id` +
    /// `revision`, or nested `card`/`card_record`) and records it in the Read-Your-Writes model.
    pub fn observe_write_receipt(
        &mut self,
        client: &str,
        host: &str,
        receipt: &serde_json::Value,
    ) -> Result<()> {
        let candidate_obj = receipt
            .get("card")
            .filter(|v| v.is_object())
            .or_else(|| receipt.get("card_record").filter(|v| v.is_object()))
            .unwrap_or(receipt);
        let card_id = candidate_obj
            .get("id")
            .or_else(|| candidate_obj.get("card_id"))
            .or_else(|| receipt.get("card_id"))
            .or_else(|| receipt.get("id"))
            .and_then(serde_json::Value::as_str);
        let rev = candidate_obj
            .get("revision")
            .or_else(|| receipt.get("revision"))
            .and_then(serde_json::Value::as_i64);
        if let (Some(cid), Some(r)) = (card_id, rev) {
            self.observe_card_revision(client, host, cid, r)?;
        }
        Ok(())
    }

    /// Verifies Read-Your-Writes and monotonic-read invariants across every live simulated client
    /// `merged_snapshot` and every live server `snapshot`.
    pub fn check_read_your_writes(&mut self, world: &SimWorld) -> Result<()> {
        world.hit("oracle.read_your_writes_checked");

        // 1. Scan every live client's merged_snapshot for monotonic Card revisions and RYW visibility.
        for client_name in world.client_names() {
            let merged = world.client_merged_snapshot_on(&client_name)?;
            let summaries = world.client_host_summaries_on(&client_name);
            let sync_items = world.client_sync_items_on(&client_name);

            let conflict_cards: BTreeSet<(String, String)> = sync_items
                .iter()
                .filter(|it| matches!(it.state, SyncItemState::Conflict | SyncItemState::Failed))
                .map(|it| {
                    (
                        normalize_client_host_id(&it.host_id),
                        strip_host_prefix(&it.card_id),
                    )
                })
                .collect();

            let mut client_cards: BTreeMap<(String, String), i64> = BTreeMap::new();
            for card in &merged.cards {
                let host_id = merged
                    .entity_hosts
                    .get(&card.id)
                    .map(String::as_str)
                    .or_else(|| canban_types::split_qualified_host_id(&card.id).map(|(h, _)| h))
                    .unwrap_or("local");
                let norm_host = normalize_client_host_id(host_id);
                let raw_cid = strip_host_prefix(&card.id);
                let rev = card.revision as i64;
                self.observe_card_revision(&client_name, &norm_host, &raw_cid, rev)?;
                client_cards.insert((norm_host, raw_cid), rev);
            }

            // Ensure every card previously observed/written by this client on a connected live host
            // has not disappeared or regressed below its recorded floor.
            let expected_for_client: Vec<(String, String, i64)> = self
                .min_seen_rev
                .iter()
                .filter(|((c, _, _), _)| c == &client_name)
                .map(|((_, h, cid), &min_rev)| (h.clone(), cid.clone(), min_rev))
                .collect();
            for (norm_host, raw_cid, min_rev) in expected_for_client {
                if conflict_cards.contains(&(norm_host.clone(), raw_cid.clone())) {
                    continue;
                }
                if let Some(&seen_rev) = client_cards.get(&(norm_host.clone(), raw_cid.clone())) {
                    ensure!(
                        seen_rev >= min_rev,
                        "[{client_name}][{norm_host}] RYW VIOLATION: Card {raw_cid} regressed to rev {seen_rev} < {min_rev}"
                    );
                    continue;
                }
                let host_connected = if norm_host == "local" {
                    world.server("local-server").is_some()
                } else {
                    summaries
                        .iter()
                        .find(|s| s.id == norm_host)
                        .is_some_and(|s| {
                            matches!(s.connection, HostConnectionState::Connected { .. })
                        })
                };
                if !host_connected {
                    continue;
                }
                let server_key = normalize_server_host_name(&norm_host);
                if let Some(srv) = world.server(&server_key)
                    && srv.snapshot()?.cards.iter().any(|c| c.id == raw_cid)
                {
                    bail!(
                        "[{client_name}][{norm_host}] RYW VIOLATION: Card {raw_cid} (min_rev={min_rev}) exists on live connected server but vanished from client merged_snapshot"
                    );
                }
            }
        }

        // 2. Scan every live server snapshot to verify authoritative revisions never drop below
        //    any revision previously observed or acknowledged to any client.
        for (host_name, handle) in &world.servers {
            let Some(server) = handle.server() else {
                continue;
            };
            let norm_host = normalize_client_host_id(host_name);
            let snap = server.snapshot()?;
            let srv_revs: BTreeMap<&str, i64> = snap
                .cards
                .iter()
                .map(|c| (c.id.as_str(), c.revision as i64))
                .collect();
            for ((client_name, tracked_host, raw_cid), &min_rev) in &self.min_seen_rev {
                if tracked_host != &norm_host {
                    continue;
                }
                if let Some(&srv_rev) = srv_revs.get(raw_cid.as_str()) {
                    ensure!(
                        srv_rev >= min_rev,
                        "[{host_name}] SERVER RYW VIOLATION: Card {raw_cid} server rev {srv_rev} < floor {min_rev} seen by client '{client_name}'"
                    );
                }
            }
        }
        Ok(())
    }

    /// Records a mutating RPC `request_key` attempt on `host` (idempotent across retries, dropped
    /// replies, `SQLITE_IOERR`, failpoint crashes, and `bounce_server`).
    pub fn record_request_key_attempt(
        &mut self,
        host: &str,
        request_key: &str,
        kind: &str,
        acknowledged: bool,
    ) {
        let srv_host = normalize_server_host_name(host);
        self.request_key_attempts
            .entry((srv_host.clone(), request_key.to_owned()))
            .and_modify(|rec| {
                rec.attempts = rec.attempts.saturating_add(1);
                rec.acknowledged |= acknowledged;
                if rec.kind.is_empty() && !kind.is_empty() {
                    rec.kind = kind.to_owned();
                }
            })
            .or_insert_with(|| RequestKeyAttempt {
                host: srv_host,
                request_key: request_key.to_owned(),
                kind: kind.to_owned(),
                attempts: 1,
                acknowledged,
                effect_marker: None,
            });
    }

    /// Records a mutating RPC `request_key` attempt with an optional unique domain marker (such as
    /// a unique Card title or Comment body) so [`Self::check_at_most_once`] also asserts exact
    /// domain-level cardinality in addition to SQLite `operations` deduplication.
    pub fn record_request_key_attempt_with_marker(
        &mut self,
        host: &str,
        request_key: &str,
        kind: &str,
        acknowledged: bool,
        effect_marker: impl Into<String>,
    ) {
        self.record_request_key_attempt(host, request_key, kind, acknowledged);
        let srv_host = normalize_server_host_name(host);
        if let Some(rec) = self
            .request_key_attempts
            .get_mut(&(srv_host, request_key.to_owned()))
        {
            rec.effect_marker = Some(effect_marker.into());
        }
    }

    /// Verifies At-Most-Once (idempotency) invariants for every tracked `request_key`:
    /// `<= 1` SQLite `operations` row and materialized domain effect always, and `== 1` once
    /// the write was acknowledged.
    pub fn check_at_most_once(&self, world: &SimWorld) -> Result<()> {
        world.hit("oracle.at_most_once_checked");
        if self.request_key_attempts.is_empty() {
            return Ok(());
        }

        let mut by_host: BTreeMap<&str, Vec<&RequestKeyAttempt>> = BTreeMap::new();
        for rec in self.request_key_attempts.values() {
            by_host.entry(rec.host.as_str()).or_default().push(rec);
        }

        for (host_name, records) in by_host {
            let Some(handle) = world.servers.get(host_name) else {
                bail!("At-most-once oracle references unknown server host '{host_name}'");
            };
            let db_path = handle.home.join("canban.sqlite");
            if !db_path.exists() {
                continue;
            }
            let conn = Connection::open_with_flags(
                &db_path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .with_context(|| {
                format!(
                    "[{host_name}] failed to open {} read-only for at-most-once check",
                    db_path.display()
                )
            })?;
            let snap_opt = handle.server().map(|s| s.snapshot()).transpose()?;
            let events_opt = handle.server().map(|s| s.events()).transpose()?;

            for rec in records {
                let mut stmt = conn.prepare("SELECT result FROM operations WHERE id=?1")?;
                let results: Vec<String> = stmt
                    .query_map([rec.request_key.as_str()], |row| row.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                ensure!(
                    results.len() <= 1,
                    "[{host_name}] AT-MOST-ONCE VIOLATION: request_key={} ({}) has {} operations rows",
                    rec.request_key,
                    rec.kind,
                    results.len()
                );

                // `batch.report` persists its idempotency record in the `reports` table
                // (via `report_keys` unique index on `json_extract(data, '$.request_key')`) rather
                // than `operations`; `execution.report` / `schedule.report` persist in both.
                let report_rows: Vec<String> = if matches!(
                    rec.kind.as_str(),
                    "batch.report" | "execution.report" | "schedule.report"
                ) {
                    let mut rstmt = conn.prepare(
                        "SELECT data FROM reports WHERE json_extract(data, '$.request_key')=?1",
                    )?;
                    let rows = rstmt
                        .query_map([rec.request_key.as_str()], |row| row.get::<_, String>(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    ensure!(
                        rows.len() <= 1,
                        "[{host_name}] AT-MOST-ONCE VIOLATION: request_key={} ({}) has {} reports rows",
                        rec.request_key,
                        rec.kind,
                        rows.len()
                    );
                    rows
                } else {
                    Vec::new()
                };

                if rec.acknowledged {
                    if rec.kind == "batch.report" {
                        ensure!(
                            report_rows.len() == 1,
                            "[{host_name}] DURABILITY / AT-MOST-ONCE VIOLATION: acknowledged request_key={} ({}) missing from SQLite reports table",
                            rec.request_key,
                            rec.kind
                        );
                    } else {
                        ensure!(
                            results.len() == 1,
                            "[{host_name}] DURABILITY / AT-MOST-ONCE VIOLATION: acknowledged request_key={} ({}) missing from SQLite operations table",
                            rec.request_key,
                            rec.kind
                        );
                        if matches!(rec.kind.as_str(), "execution.report" | "schedule.report") {
                            ensure!(
                                report_rows.len() == 1,
                                "[{host_name}] DURABILITY / AT-MOST-ONCE VIOLATION: acknowledged request_key={} ({}) missing from SQLite reports table",
                                rec.request_key,
                                rec.kind
                            );
                        }
                    }
                }

                let parsed_result: Option<serde_json::Value> = results
                    .first()
                    .and_then(|raw| serde_json::from_str(raw).ok());
                if let Some(snap) = snap_opt.as_ref() {
                    if let Some(res) = parsed_result.as_ref() {
                        match rec.kind.as_str() {
                            "card.create" => {
                                if let Some(cid) = res.get("id").and_then(serde_json::Value::as_str)
                                {
                                    let count = snap.cards.iter().filter(|c| c.id == cid).count();
                                    ensure!(
                                        count == 1,
                                        "[{host_name}] AT-MOST-ONCE VIOLATION: card.create request_key={} expected 1 card {cid}, found {count}",
                                        rec.request_key
                                    );
                                }
                            }
                            "note.create" => {
                                if let Some(nid) = res.get("id").and_then(serde_json::Value::as_str)
                                {
                                    let count = snap.notes.iter().filter(|n| n.id == nid).count();
                                    ensure!(
                                        count == 1,
                                        "[{host_name}] AT-MOST-ONCE VIOLATION: note.create request_key={} expected 1 note {nid}, found {count}",
                                        rec.request_key
                                    );
                                }
                            }
                            "workflow.start" => {
                                if let Some(rid) = res
                                    .get("id")
                                    .or_else(|| res.get("run_id"))
                                    .and_then(serde_json::Value::as_str)
                                {
                                    let count =
                                        snap.workflow_runs.iter().filter(|r| r.id == rid).count();
                                    ensure!(
                                        count == 1,
                                        "[{host_name}] AT-MOST-ONCE VIOLATION: workflow.start request_key={} expected 1 run {rid}, found {count}",
                                        rec.request_key
                                    );
                                }
                            }
                            "terminal.create" | "agent.open" => {
                                if let Some(tid) = res
                                    .get("terminal_id")
                                    .or_else(|| res.get("id"))
                                    .and_then(serde_json::Value::as_str)
                                {
                                    let raw_tid = strip_host_prefix(tid);
                                    let count =
                                        snap.terminals.iter().filter(|t| t.id == raw_tid).count();
                                    ensure!(
                                        count <= 1,
                                        "[{host_name}] AT-MOST-ONCE VIOLATION: {} request_key={} duplicated terminal {raw_tid} ({count} entries)",
                                        rec.kind,
                                        rec.request_key
                                    );
                                }
                            }
                            _ => {}
                        }
                    }

                    if let Some(marker) = rec.effect_marker.as_deref() {
                        let materialized_count = match rec.kind.as_str() {
                            "card.create" => {
                                snap.cards.iter().filter(|c| c.title == marker).count()
                            }
                            "note.create" => {
                                snap.notes.iter().filter(|n| n.title == marker).count()
                            }
                            "comment.add" => events_opt
                                .as_ref()
                                .map(|evs| {
                                    evs.iter()
                                        .filter(|e| {
                                            e.kind == "comment.created"
                                                && (e.data["body"].as_str() == Some(marker)
                                                    || e.message == marker)
                                        })
                                        .count()
                                })
                                .unwrap_or(0),
                            "terminal.create" | "agent.open" => snap
                                .terminals
                                .iter()
                                .filter(|t| t.name.as_deref() == Some(marker) || t.id == marker)
                                .count(),
                            _ => results.len(),
                        };
                        ensure!(
                            materialized_count <= 1,
                            "[{host_name}] AT-MOST-ONCE VIOLATION: request_key={} ({}) materialized {materialized_count} effects for marker={marker:?}",
                            rec.request_key,
                            rec.kind
                        );
                        if rec.acknowledged {
                            ensure!(
                                materialized_count == 1,
                                "[{host_name}] AT-MOST-ONCE VIOLATION: acknowledged request_key={} ({}) expected 1 effect for marker={marker:?}, found {materialized_count}",
                                rec.request_key,
                                rec.kind
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Verifies that every remote host slot currently in `HostConnectionState::AuthFailed`
    /// issues 0 background reconnect attempts (`client_dial_count` and `reconnect_count` stay flat)
    /// while halted. Records the baseline on first observation and clears it once a host leaves
    /// `AuthFailed` (e.g. after re-pairing).
    pub fn check_auth_failed_zero_dials(&mut self, world: &SimWorld) -> Result<()> {
        world.hit("oracle.auth_failed_zero_dials_checked");
        for client_name in world.client_names() {
            let summaries = world.client_host_summaries_on(&client_name);
            for summary in summaries {
                let key = (client_name.clone(), summary.id.clone());
                if matches!(summary.connection, HostConnectionState::AuthFailed { .. }) {
                    let cur_dials = world.client_dial_count(&client_name, &summary.id);
                    let cur_reconn = summary.reconnect_count;
                    if let Some(&(base_dials, base_reconn)) = self.auth_failed_baselines.get(&key) {
                        ensure!(
                            cur_dials == base_dials,
                            "[{client_name}][{}] AUTH_FAILED LIVENESS VIOLATION: halted AuthFailed slot issued {} background dial attempts (base={base_dials}, now={cur_dials})",
                            summary.id,
                            cur_dials.saturating_sub(base_dials)
                        );
                        ensure!(
                            cur_reconn == base_reconn,
                            "[{client_name}][{}] AUTH_FAILED LIVENESS VIOLATION: halted AuthFailed slot incremented reconnect_count (base={base_reconn}, now={cur_reconn})",
                            summary.id
                        );
                    } else {
                        self.auth_failed_baselines
                            .insert(key, (cur_dials, cur_reconn));
                    }
                } else {
                    self.auth_failed_baselines.remove(&key);
                }
            }
        }
        Ok(())
    }

    /// Targeted check asserting that `(client_id, host_id)` is currently in `AuthFailed` and issued
    /// 0 new dials and 0 new reconnects relative to `(baseline_dials, baseline_reconnects)`.
    pub fn assert_auth_failed_zero_dials_for(
        &mut self,
        world: &SimWorld,
        client_id: &str,
        host_id: &str,
        baseline_dials: u64,
        baseline_reconnects: u32,
    ) -> Result<()> {
        world.hit("oracle.auth_failed_zero_dials_checked");
        let summary = world
            .client_host_summaries_on(client_id)
            .into_iter()
            .find(|s| s.id == host_id)
            .with_context(|| {
                format!("[{client_id}][{host_id}] missing from client host_summaries")
            })?;
        ensure!(
            matches!(summary.connection, HostConnectionState::AuthFailed { .. }),
            "[{client_id}][{host_id}] expected HostConnectionState::AuthFailed, got {:?}",
            summary.connection
        );
        let cur_dials = world.client_dial_count(client_id, host_id);
        ensure!(
            cur_dials == baseline_dials,
            "[{client_id}][{host_id}] AuthFailed slot issued {} background dial attempts (base={baseline_dials}, now={cur_dials})",
            cur_dials.saturating_sub(baseline_dials)
        );
        ensure!(
            summary.reconnect_count == baseline_reconnects,
            "[{client_id}][{host_id}] AuthFailed slot incremented reconnect_count (base={baseline_reconnects}, now={})",
            summary.reconnect_count
        );
        self.auth_failed_baselines.insert(
            (client_id.to_owned(), host_id.to_owned()),
            (cur_dials, summary.reconnect_count),
        );
        Ok(())
    }

    /// Runs invariants that MUST hold at every step (even mid-partition or mid-chaos).
    pub fn check_step_invariants(&mut self, world: &SimWorld) -> Result<()> {
        self.check_server_monotonicity_and_notifications(world)?;
        self.check_no_plaintext_secrets_in_outbox(world)?;
        self.check_client_merged_view_invariants(world)?;
        self.check_herdr_runtime_parity(world)?;
        self.check_workflow_invariants(world)?;
        self.check_snapshot_boundedness(world)?;
        self.check_committed_durability(world)?;
        Ok(())
    }

    /// Runs convergence, durability, recovery, and SQLite integrity invariants that MUST hold
    /// once network faults are healed and drained.
    pub fn check_quiescent_invariants(&mut self, world: &SimWorld) -> Result<()> {
        self.check_step_invariants(world)?;
        if !self.min_seen_rev.is_empty() {
            self.check_read_your_writes(world)?;
        }
        if !self.request_key_attempts.is_empty() {
            self.check_at_most_once(world)?;
        }
        if self.secrets_queued_offline {
            ensure!(
                self.outbox_files_scanned > 0,
                "Secret-leak check coverage violation: secrets were queued offline but 0 .outbox.json files were scanned"
            );
        }
        self.check_outbox_drained(world)?;
        self.check_client_server_convergence(world)?;
        Self::check_sqlite_integrity(world)?;
        Self::check_post_bounce_recovery_all(world)?;
        Ok(())
    }

    /// #1 & #6: Monotonic Card revisions, monotonic `CardRead` sequences, unique IDs,
    /// `reminders_from_events` consistency, zero reminders for archived/removed cards,
    /// unique `sound_event_ids`, and no resurrected read notices on every live server.
    fn check_server_monotonicity_and_notifications(&mut self, world: &SimWorld) -> Result<()> {
        for (host_name, handle) in &world.servers {
            let Some(server) = handle.server() else {
                continue;
            };
            let snap = server.snapshot()?;

            // Monotonic Card.revision + no duplicate Card IDs.
            let mut seen_cards = BTreeSet::new();
            let mut active_cards = BTreeSet::new();
            for card in &snap.cards {
                ensure!(
                    seen_cards.insert(card.id.clone()),
                    "[{host_name}] duplicate Card ID {} in snapshot",
                    card.id
                );
                if card.active() {
                    active_cards.insert(card.id.clone());
                }
                let key = (host_name.clone(), card.id.clone());
                let prev = self.max_card_revision.get(&key).copied().unwrap_or(0);
                ensure!(
                    card.revision >= prev,
                    "[{host_name}] Card {} revision regressed from {prev} to {}",
                    card.id,
                    card.revision
                );
                self.max_card_revision.insert(key, card.revision);
            }

            // Monotonic CardRead.sequence across restarts and reconnects.
            let mut seen_reads = BTreeSet::new();
            for cr in &snap.card_reads {
                ensure!(
                    seen_reads.insert(cr.card_id.clone()),
                    "[{host_name}] duplicate CardRead entry for card {}",
                    cr.card_id
                );
                let key = (host_name.clone(), cr.card_id.clone());
                let prev = self.max_card_read_seq.get(&key).copied().unwrap_or(0);
                ensure!(
                    cr.sequence >= prev,
                    "[{host_name}] CardRead sequence for card {} regressed from {prev} to {}",
                    cr.card_id,
                    cr.sequence
                );
                self.max_card_read_seq.insert(key, cr.sequence);
            }

            // Notice deduplication & read-persistence check.
            let mut seen_notices = BTreeSet::new();
            for n in &snap.notices {
                ensure!(
                    seen_notices.insert(n.id.clone()),
                    "[{host_name}] duplicate Notice ID {} in server snapshot",
                    n.id
                );
                if let Some(read_ids) = self.read_notice_ids.get(host_name)
                    && read_ids.contains(&n.id)
                {
                    ensure!(
                        n.read,
                        "[{host_name}] read Notice {} resurrected as unread!",
                        n.id
                    );
                }
            }

            // Reminder consistency against `canban_notifications::reminders_from_events_at`.
            let mut events = server.events()?;
            events.retain(|e| seen_cards.contains(&e.card_id));
            let expected_reminders =
                canban_notifications::reminders_from_events_at(&snap, &events, server.now_ms());
            let mut seen_snap_rem = BTreeSet::new();
            let mut snap_rem_map: BTreeMap<String, (String, bool, bool)> = BTreeMap::new();
            for r in &snap.reminders {
                ensure!(
                    seen_snap_rem.insert(r.id.clone()),
                    "[{host_name}] duplicate Reminder ID {} in server snapshot.reminders",
                    r.id
                );
                ensure!(
                    active_cards.contains(&r.card_id),
                    "[{host_name}] stale Reminder {} references non-active/missing card {}",
                    r.id,
                    r.card_id
                );
                snap_rem_map.insert(r.id.clone(), (r.card_id.clone(), r.unread, r.pending));
            }
            let expected_map: BTreeMap<String, (String, bool, bool)> = expected_reminders
                .iter()
                .map(|r| (r.id.clone(), (r.card_id.clone(), r.unread, r.pending)))
                .collect();
            ensure!(
                snap_rem_map == expected_map,
                "[{host_name}] snapshot.reminders diverged from reminders_from_events: snap={snap_rem_map:?} expected={expected_map:?}"
            );

            let reminders = canban_notifications::reminders(&snap);
            let mut seen_rem = BTreeSet::new();
            for r in &reminders {
                ensure!(
                    seen_rem.insert(r.id.clone()),
                    "[{host_name}] duplicate reminder ID {} in notification center",
                    r.id
                );
                ensure!(
                    active_cards.contains(&r.card_id),
                    "[{host_name}] notification center returned reminder {} for archived/removed card {}",
                    r.id,
                    r.card_id
                );
                if r.unread && r.sequence > 0 {
                    let read_seq = canban_notifications::read_sequence(&snap, &r.card_id);
                    ensure!(
                        r.sequence > read_seq,
                        "[{host_name}] reminder {} (seq={}) remained unread despite card_read seq={read_seq}",
                        r.id,
                        r.sequence
                    );
                }
            }

            // Unique `sound_event_ids`.
            let mut seen_sounds = BTreeSet::new();
            for sid in &snap.sound_event_ids {
                ensure!(
                    seen_sounds.insert(sid.clone()),
                    "[{host_name}] duplicate sound_event_id {sid} in server snapshot"
                );
            }

            // Monotonic WorkflowRun.visit + unique WorkflowRun / MergeBatch / ScheduledRun / Note IDs.
            let mut seen_runs = BTreeSet::new();
            for run in &snap.workflow_runs {
                ensure!(
                    seen_runs.insert(run.id.clone()),
                    "[{host_name}] duplicate WorkflowRun ID {} in server snapshot",
                    run.id
                );
                let key = (host_name.clone(), run.id.clone());
                let prev = self.max_workflow_visit.get(&key).copied().unwrap_or(0);
                ensure!(
                    run.visit >= prev,
                    "[{host_name}] WorkflowRun {} visit regressed from {prev} to {}",
                    run.id,
                    run.visit
                );
                self.max_workflow_visit.insert(key, run.visit);
            }

            let mut seen_batches = BTreeSet::new();
            for batch in &snap.merge_batches {
                ensure!(
                    seen_batches.insert(batch.id.clone()),
                    "[{host_name}] duplicate MergeBatch ID {} in server snapshot",
                    batch.id
                );
            }

            let mut seen_sched_runs = BTreeSet::new();
            for sr in &snap.scheduled_runs {
                ensure!(
                    seen_sched_runs.insert(sr.id.clone()),
                    "[{host_name}] duplicate ScheduledRun ID {} in server snapshot",
                    sr.id
                );
            }

            let mut seen_notes = BTreeSet::new();
            for note in &snap.notes {
                ensure!(
                    seen_notes.insert(note.id.clone()),
                    "[{host_name}] duplicate Note ID {} in server snapshot",
                    note.id
                );
            }
        }
        Ok(())
    }

    /// #5 (P0 #1): Recursively scans ALL client data dirs (including `cache/hosts/*.outbox.json`,
    /// `cache/hosts/*.json`, read caches, `hosts.json`), every live server `Snapshot` JSON, and
    /// every `SimHerdr::sent_inputs()` payload for plaintext tracked secrets.
    fn check_no_plaintext_secrets_in_outbox(&mut self, world: &SimWorld) -> Result<()> {
        // Walk all client data dirs (primary + any secondary clients) recursively.
        let mut scan_roots = BTreeSet::new();
        scan_roots.insert(world.client_data_dir.clone());
        for c in world.clients.values() {
            scan_roots.insert(c.data_dir.clone());
        }

        let mut files = Vec::new();
        for root in &scan_roots {
            collect_files_recursive(root, &mut files);
        }

        for path in &files {
            let is_outbox = path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n.ends_with(".outbox.json"));
            if is_outbox {
                self.outbox_files_scanned += 1;
                world.hit("oracle.outbox_file_scanned");
            }
            if self.tracked_secrets.is_empty() {
                continue;
            }
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            let raw = String::from_utf8_lossy(&bytes);
            for secret in &self.tracked_secrets {
                ensure!(
                    !raw.contains(secret.as_str()),
                    "SECURITY VIOLATION: plaintext secret leaked on disk in {}",
                    path.display()
                );
            }
        }

        if self.tracked_secrets.is_empty() {
            return Ok(());
        }

        // Scan serialized server snapshots and Herdr terminal inputs as well.
        for (host_name, handle) in &world.servers {
            if let Some(srv) = handle.server() {
                let snap = srv.snapshot()?;
                let serialized = serde_json::to_string(&snap)?;
                for secret in &self.tracked_secrets {
                    ensure!(
                        !serialized.contains(secret.as_str()),
                        "SECURITY VIOLATION: plaintext secret leaked in [{host_name}] server Snapshot JSON"
                    );
                }
            }
            for send in handle.herdr.sent_inputs() {
                for secret in &self.tracked_secrets {
                    ensure!(
                        !send.text.contains(secret.as_str()),
                        "SECURITY VIOLATION: plaintext secret leaked into [{host_name}] Herdr terminal {} input",
                        send.terminal
                    );
                }
            }
        }
        Ok(())
    }

    /// #4 & #6: Merged UI snapshot, `entity_hosts` integrity, reminders, and `sound_event_ids`
    /// invariants at the `canban-client` boundary across ALL live simulated clients.
    fn check_client_merged_view_invariants(&self, world: &SimWorld) -> Result<()> {
        let mut known_hosts: BTreeSet<&str> = BTreeSet::new();
        known_hosts.insert("local");
        for h in &world.config.remote_hosts {
            known_hosts.insert(h.as_str());
        }

        for client_name in world.client_names() {
            let merged = world.client_merged_snapshot_on(&client_name)?;

            // Every project and card in merged snapshot must have unique IDs (multi-host qualification).
            let mut seen_projects = BTreeSet::new();
            for p in &merged.projects {
                ensure!(
                    seen_projects.insert(p.id.clone()),
                    "[{client_name}] duplicate Project ID {} in merged_snapshot",
                    p.id
                );
                if let Some(owner) = merged.entity_hosts.get(&p.id) {
                    ensure!(
                        known_hosts.contains(owner.as_str()),
                        "[{client_name}] Project {} has unknown entity_hosts owner {owner:?}",
                        p.id
                    );
                }
            }
            let mut seen_cards = BTreeSet::new();
            let mut active_cards = BTreeSet::new();
            for c in &merged.cards {
                ensure!(
                    seen_cards.insert(c.id.clone()),
                    "[{client_name}] duplicate Card ID {} in merged_snapshot",
                    c.id
                );
                if c.active() {
                    active_cards.insert(c.id.clone());
                }
                ensure!(
                    !c.id.trim().is_empty()
                        && !c.project_id.trim().is_empty()
                        && !c.title.trim().is_empty()
                        && !c.stage.trim().is_empty(),
                    "[{client_name}] Incomplete Card in merged_snapshot: id={:?} project_id={:?} title={:?} stage={:?}",
                    c.id,
                    c.project_id,
                    c.title,
                    c.stage
                );
                ensure!(
                    seen_projects.contains(&c.project_id),
                    "[{client_name}] Card {} references missing project_id {} in merged_snapshot",
                    c.id,
                    c.project_id
                );
                if let Some(owner) = merged.entity_hosts.get(&c.id) {
                    ensure!(
                        known_hosts.contains(owner.as_str()),
                        "[{client_name}] Card {} has unknown entity_hosts owner {owner:?}",
                        c.id
                    );
                    if let Some(proj_owner) = merged.entity_hosts.get(&c.project_id) {
                        ensure!(
                            owner == proj_owner,
                            "[{client_name}] Card {} owner ({owner}) disagrees with its Project {} owner ({proj_owner})",
                            c.id,
                            c.project_id
                        );
                    }
                }
            }

            for t in &merged.terminals {
                if let Some(owner) = merged.entity_hosts.get(&t.id) {
                    ensure!(
                        known_hosts.contains(owner.as_str()),
                        "[{client_name}] Terminal {} has unknown entity_hosts owner {owner:?}",
                        t.id
                    );
                }
            }
            for r in &merged.workflow_runs {
                if let Some(owner) = merged.entity_hosts.get(&r.id) {
                    ensure!(
                        known_hosts.contains(owner.as_str()),
                        "[{client_name}] WorkflowRun {} has unknown entity_hosts owner {owner:?}",
                        r.id
                    );
                }
            }

            // Reminders at the merged client boundary must have unique IDs and never reference archived/removed cards.
            let reminders = canban_notifications::reminders(&merged);
            let mut seen_rem = BTreeSet::new();
            for r in &reminders {
                ensure!(
                    seen_rem.insert(r.id.clone()),
                    "[{client_name}] duplicate Reminder ID {} in merged UI notification center",
                    r.id
                );
                ensure!(
                    active_cards.contains(&r.card_id),
                    "[{client_name}] merged UI notification center returned reminder {} for non-active card {}",
                    r.id,
                    r.card_id
                );
            }

            let mut seen_sounds = BTreeSet::new();
            for sid in &merged.sound_event_ids {
                ensure!(
                    seen_sounds.insert(sid.clone()),
                    "[{client_name}] duplicate sound_event_id {sid} in merged_snapshot"
                );
            }
        }
        Ok(())
    }

    /// #7: Active agent counts in `merged_runtime_state()` match `canban_scheduler::active_count_at`
    /// identically for both local and remote hosts across the UNION of computed & reported host keys
    /// on ALL live clients, cross-checked against `scheduler.max_agents`, Herdr terminal binding
    /// exclusivity, and `protected_terminals` safety!
    fn check_herdr_runtime_parity(&self, world: &SimWorld) -> Result<()> {
        let local_now_ms = world
            .server("local-server")
            .map(|s| s.now_ms())
            .unwrap_or_else(|| world.now_ms());
        let client_now_ms = world.now_ms();
        for client_name in world.client_names() {
            let rt = world.client_merged_runtime_on(&client_name)?;
            let merged = world.client_merged_snapshot_on(&client_name)?;
            let mut per_host: BTreeMap<String, Snapshot> = BTreeMap::new();
            for t in &rt.terminals {
                let host = rt
                    .terminal_hosts
                    .get(&t.id)
                    .cloned()
                    .unwrap_or_else(|| "local".into());
                per_host.entry(host).or_default().terminals.push(t.clone());
            }
            for cs in &merged.configuration_sessions {
                let host = merged
                    .entity_hosts
                    .get(&cs.id)
                    .cloned()
                    .unwrap_or_else(|| "local".into());
                per_host
                    .entry(host)
                    .or_default()
                    .configuration_sessions
                    .push(cs.clone());
            }
            for mb in &merged.merge_batches {
                let host = merged
                    .entity_hosts
                    .get(&mb.id)
                    .cloned()
                    .unwrap_or_else(|| "local".into());
                per_host
                    .entry(host)
                    .or_default()
                    .merge_batches
                    .push(mb.clone());
            }
            for sr in &merged.scheduled_runs {
                let host = if !sr.host_id.is_empty() {
                    sr.host_id.clone()
                } else {
                    merged
                        .entity_hosts
                        .get(&sr.id)
                        .cloned()
                        .unwrap_or_else(|| "local".into())
                };
                per_host
                    .entry(host)
                    .or_default()
                    .scheduled_runs
                    .push(sr.clone());
            }

            let mut computed: BTreeMap<String, usize> = BTreeMap::new();
            for (host, host_snap) in &per_host {
                let host_now = if host == "local" {
                    local_now_ms
                } else {
                    client_now_ms
                };
                let count = canban_scheduler::active_count_at(host_snap, host_now);
                if count > 0 {
                    computed.insert(host.clone(), count);
                }
            }

            // Check UNION of computed and reported host keys (so stale non-zero reported counts are caught!).
            let all_hosts: BTreeSet<String> = computed
                .keys()
                .chain(rt.host_active_agents.keys())
                .cloned()
                .collect();
            for host in all_hosts {
                let expected = computed.get(&host).copied().unwrap_or(0);
                let reported = rt.host_active_agents.get(&host).copied().unwrap_or(0);
                ensure!(
                    reported == expected,
                    "[{client_name}][{host}] merged_runtime_state active_agents mismatch: reported={reported}, computed={expected}"
                );
            }
        }

        // Cross-check scheduler `active_count_at`, `max_agents`, and Herdr binding exclusivity per server.
        for (host_name, handle) in &world.servers {
            let Some(server) = handle.server() else {
                continue;
            };
            let srv_snap = server.snapshot()?;
            let srv_now = server.now_ms();
            let srv_active = canban_scheduler::active_count_at(&srv_snap, srv_now);
            let max_agents = server.config().scheduler.max_agents;
            // Per `crates/scheduler/src/lib.rs:123`: `max_agents` is an automatic admission
            // budget, not a hard cap on retained terminals. When a retained `completed`/`backlog`
            // Card PTY temporarily releases its slot (e.g. `idle`/`blocked`), the scheduler may
            // legitimately admit up to `max_agents` new runs; if that retained PTY later resumes
            // `working` in Herdr (interactive follow-up), `active_count_at` counts it to pause
            // *further* admissions without implying the scheduler over-admitted.
            let retained_resumed = srv_snap
                .terminals
                .iter()
                .filter_map(|t| t.agent.as_ref())
                .filter(|a| {
                    matches!(a.state.as_str(), "completed" | "backlog")
                        && a.terminal.is_some()
                        && !a.finished()
                        && (a.close_state == "closing"
                            || !matches!(
                                a.runtime_status_at(srv_now),
                                "closed" | "blocked" | "idle" | "done" | "error"
                            ))
                })
                .count();
            let admitted_active = srv_active.saturating_sub(retained_resumed);
            ensure!(
                admitted_active <= max_agents,
                "[{host_name}] scheduler admitted active count ({admitted_active}, total active_count_at={srv_active}, retained_resumed={retained_resumed}) exceeded max_agents ({max_agents})"
            );

            // Terminal binding exclusivity: at most 1 live non-finished AgentRun per (core_id, herdr_addr).
            let mut live_bindings: BTreeMap<(String, String), String> = BTreeMap::new();
            let current_core = handle.herdr.core_id();
            for t in &srv_snap.terminals {
                let Some(agent) = t.agent.as_ref() else {
                    continue;
                };
                if agent.finished() {
                    continue;
                }
                if let Some(addr) = agent.terminal.as_deref() {
                    let core = agent.core_id.clone().unwrap_or_default();
                    let key = (core.clone(), addr.to_owned());
                    if let Some(existing_tid) = live_bindings.insert(key, t.id.clone()) {
                        bail!(
                            "[{host_name}] Herdr terminal binding exclusivity violated: ({core}, {addr}) bound to both {existing_tid} and {}",
                            t.id
                        );
                    }
                    if agent.connection_state == "connected" && core == current_core {
                        ensure!(
                            handle.herdr.terminal_record(addr).is_some(),
                            "[{host_name}] connected AgentRun {} references missing Herdr terminal {addr} on core {current_core}",
                            t.id
                        );
                    }
                }
            }
        }

        // Enforce `protected_terminals`: zero unsolicited sent_inputs beyond snapshot & zero kills!
        for ((host_name, herdr_addr), &max_sends) in &self.protected_terminals {
            let Some(handle) = world.servers.get(host_name) else {
                continue;
            };
            let actual_sends = handle
                .herdr
                .sent_inputs()
                .iter()
                .filter(|s| s.terminal == *herdr_addr)
                .count();
            ensure!(
                actual_sends <= max_sends,
                "[{host_name}] SAFETY VIOLATION: protected terminal {herdr_addr} received unsolicited input (sends={actual_sends} > allowed={max_sends})"
            );
            ensure!(
                !handle.herdr.killed_terminals().contains(herdr_addr),
                "[{host_name}] SAFETY VIOLATION: protected terminal {herdr_addr} was killed!"
            );
        }
        Ok(())
    }

    /// Workflow state-machine invariants across all live servers.
    fn check_workflow_invariants(&self, world: &SimWorld) -> Result<()> {
        const VALID_WF_STATES: &[&str] = &[
            "running",
            "waiting_agent",
            "waiting_approval",
            "waiting_admit",
            "waiting_capacity",
            "waiting_delivery",
            "waiting_batch",
            "waiting_script",
            "blocked",
            "stale",
            "completed",
            "failed",
            "cancelled",
            "superseded",
        ];

        for (host_name, handle) in &world.servers {
            let Some(server) = handle.server() else {
                continue;
            };
            let snap = server.snapshot()?;
            let cards_by_id: BTreeMap<&str, &Card> =
                snap.cards.iter().map(|c| (c.id.as_str(), c)).collect();
            let term_ids: BTreeSet<&str> = snap.terminals.iter().map(|t| t.id.as_str()).collect();
            let mut active_runs_per_card: BTreeMap<&str, &WorkflowRun> = BTreeMap::new();

            for run in &snap.workflow_runs {
                ensure!(
                    VALID_WF_STATES.contains(&run.state.as_str()),
                    "[{host_name}] WorkflowRun {} has invalid state {:?}",
                    run.id,
                    run.state
                );
                if run.active()
                    && let Some(step) = run.step.as_deref()
                {
                    ensure!(
                        run.definition.steps.contains_key(step),
                        "[{host_name}] active WorkflowRun {} points to unknown step {:?} not in definition.steps",
                        run.id,
                        step
                    );
                }
                if run.definition.max_steps > 0 && run.state != "failed" {
                    ensure!(
                        run.visit <= run.definition.max_steps,
                        "[{host_name}] WorkflowRun {} visit={} exceeded max_steps={} without failing (state={})",
                        run.id,
                        run.visit,
                        run.definition.max_steps,
                        run.state
                    );
                }
                if let Some(cid) = run.card_id.as_deref() {
                    if run.active()
                        && let Some(prev) = active_runs_per_card.insert(cid, run)
                    {
                        bail!(
                            "[{host_name}] Card {cid} has multiple active WorkflowRuns: {} ({}) and {} ({})",
                            prev.id,
                            prev.state,
                            run.id,
                            run.state
                        );
                    }
                    if run.state == "waiting_approval" {
                        let card = cards_by_id.get(cid).with_context(|| {
                            format!(
                                "[{host_name}] waiting_approval WorkflowRun {} references missing Card {cid}",
                                run.id
                            )
                        })?;
                        ensure!(
                            card.human_gate.is_some(),
                            "[{host_name}] WorkflowRun {} is waiting_approval but Card {cid} has human_gate=None",
                            run.id
                        );
                    }
                }
                if run.state == "waiting_agent"
                    && let Some(tid) = run.terminal_id.as_deref()
                {
                    ensure!(
                        term_ids.contains(tid),
                        "[{host_name}] waiting_agent WorkflowRun {} references missing terminal_id {tid}",
                        run.id
                    );
                }
            }

            const VALID_BATCH_STATES: &[&str] = &[
                "integrating",
                "testing",
                "queued",
                "running",
                "completed",
                "blocked",
                "failed",
                "cancelled",
            ];
            const VALID_SCHED_STATES: &[&str] = &[
                "queued",
                "starting",
                "unknown",
                "running",
                "working",
                "completed",
                "blocked",
                "failed",
                "skipped",
                "cancelled",
            ];
            let proj_ids: BTreeSet<&str> = snap.projects.iter().map(|p| p.id.as_str()).collect();
            for batch in &snap.merge_batches {
                ensure!(
                    VALID_BATCH_STATES.contains(&batch.state.as_str()),
                    "[{host_name}] MergeBatch {} has invalid state {:?}",
                    batch.id,
                    batch.state
                );
                ensure!(
                    proj_ids.contains(batch.project_id.as_str()),
                    "[{host_name}] MergeBatch {} references missing project {}",
                    batch.id,
                    batch.project_id
                );
            }
            for sr in &snap.scheduled_runs {
                ensure!(
                    VALID_SCHED_STATES.contains(&sr.state.as_str()),
                    "[{host_name}] ScheduledRun {} has invalid state {:?}",
                    sr.id,
                    sr.state
                );
                ensure!(
                    proj_ids.contains(sr.project_id.as_str()),
                    "[{host_name}] ScheduledRun {} references missing project {}",
                    sr.id,
                    sr.project_id
                );
            }
        }
        Ok(())
    }

    /// Snapshot boundedness invariants across all live servers.
    fn check_snapshot_boundedness(&self, world: &SimWorld) -> Result<()> {
        for (host_name, handle) in &world.servers {
            let Some(server) = handle.server() else {
                continue;
            };
            let snap = server.snapshot()?;
            let term_by_id: BTreeMap<&str, &canban_types::TerminalEntry> =
                snap.terminals.iter().map(|t| (t.id.as_str(), t)).collect();
            let card_ids: BTreeSet<&str> = snap.cards.iter().map(|c| c.id.as_str()).collect();

            for rs in &snap.reclaim_status {
                let Some(term) = term_by_id.get(rs.terminal_id.as_str()) else {
                    bail!(
                        "[{host_name}] reclaim_status entry references missing terminal {}",
                        rs.terminal_id
                    );
                };
                if let Some(agent) = term.agent.as_ref() {
                    ensure!(
                        !agent.finished(),
                        "[{host_name}] reclaim_status retained finished agent terminal {} (state={})",
                        rs.terminal_id,
                        agent.state
                    );
                }
            }

            for ctx in &snap.terminal_contexts {
                ensure!(
                    term_by_id.contains_key(ctx.terminal_id.as_str()),
                    "[{host_name}] terminal_contexts retained orphan entry for missing terminal {}",
                    ctx.terminal_id
                );
            }

            for cid in snap.card_summaries.keys() {
                ensure!(
                    card_ids.contains(cid.as_str()),
                    "[{host_name}] card_summaries retained orphan summary for missing card {cid}"
                );
            }

            let proj_ids: BTreeSet<&str> = snap.projects.iter().map(|p| p.id.as_str()).collect();
            let mut seen_gh_issues = BTreeSet::new();
            for gi in &snap.github_issues {
                ensure!(
                    seen_gh_issues.insert(gi.id.clone()),
                    "[{host_name}] duplicate github_issues entry {} in server Snapshot",
                    gi.id
                );
                ensure!(
                    proj_ids.contains(gi.project_id.as_str()),
                    "[{host_name}] github_issues entry {} references missing project {}",
                    gi.id,
                    gi.project_id
                );
            }
            ensure!(
                snap.github_issues.len() <= snap.cards.len(),
                "[{host_name}] server Snapshot github_issues ({}) exceeded card count ({})",
                snap.github_issues.len(),
                snap.cards.len()
            );
        }
        Ok(())
    }

    /// Verifies that all recorded committed writes (cards, comments, global env, workflow runs,
    /// persistent merge batches, scheduled runs, and notes) remain durable on every live server
    /// (including after crashes and bounces).
    pub fn check_committed_durability(&self, world: &SimWorld) -> Result<()> {
        if self.committed_cards.is_empty()
            && self.committed_comments.is_empty()
            && self.committed_env.is_empty()
            && self.committed_workflow_runs.is_empty()
            && self.committed_merge_batches.is_empty()
            && self.committed_scheduled_runs.is_empty()
            && self.committed_notes.is_empty()
        {
            return Ok(());
        }
        world.hit("oracle.committed_durability_checked");
        for (host_name, handle) in &world.servers {
            let Some(server) = handle.server() else {
                continue;
            };
            let snap = server.snapshot()?;
            for ((h, cid), exp) in &self.committed_cards {
                if h != host_name {
                    continue;
                }
                let card = snap.cards.iter().find(|c| c.id == *cid).with_context(|| {
                    format!("[{host_name}] DURABILITY VIOLATION: committed Card {cid} vanished!")
                })?;
                ensure!(
                    card.revision >= exp.min_revision,
                    "[{host_name}] DURABILITY VIOLATION: committed Card {cid} regressed to rev {} < {}",
                    card.revision,
                    exp.min_revision
                );
                if !exp.project_id.is_empty() {
                    ensure!(
                        card.project_id == exp.project_id,
                        "[{host_name}] committed Card {cid} project_id changed from {} to {}",
                        exp.project_id,
                        card.project_id
                    );
                }
            }

            let has_comments = self.committed_comments.keys().any(|(h, _)| h == host_name);
            if has_comments {
                let events = server.events()?;
                for ((h, cid), expected_bodies) in &self.committed_comments {
                    if h != host_name {
                        continue;
                    }
                    let present: BTreeSet<String> = events
                        .iter()
                        .filter(|e| e.card_id == *cid && e.kind == "comment.created")
                        .filter_map(|e| {
                            e.data["body"]
                                .as_str()
                                .or((!e.message.is_empty()).then_some(e.message.as_str()))
                                .map(str::to_owned)
                        })
                        .collect();
                    for body in expected_bodies {
                        ensure!(
                            present.contains(body),
                            "[{host_name}] DURABILITY VIOLATION: committed comment {body:?} on Card {cid} vanished (present={present:?})"
                        );
                    }
                }
            }

            for ((h, key), exp_meta) in &self.committed_env {
                if h != host_name {
                    continue;
                }
                let got = snap.global_env.get(key);
                ensure!(
                    got == Some(exp_meta),
                    "[{host_name}] DURABILITY VIOLATION: committed global_env[{key}] expected={exp_meta:?} got={got:?}"
                );
            }

            for ((h, rid), exp) in &self.committed_workflow_runs {
                if h != host_name {
                    continue;
                }
                let run = snap
                    .workflow_runs
                    .iter()
                    .find(|r| r.id == *rid)
                    .with_context(|| {
                        format!(
                            "[{host_name}] DURABILITY VIOLATION: committed WorkflowRun {rid} vanished!"
                        )
                    })?;
                ensure!(
                    run.visit >= exp.min_visit,
                    "[{host_name}] DURABILITY VIOLATION: committed WorkflowRun {rid} regressed to visit {} < {}",
                    run.visit,
                    exp.min_visit
                );
                if !exp.project_id.is_empty() {
                    ensure!(
                        run.project_id == exp.project_id,
                        "[{host_name}] committed WorkflowRun {rid} project_id changed from {} to {}",
                        exp.project_id,
                        run.project_id
                    );
                }
                if !exp.workflow_id.is_empty() {
                    ensure!(
                        run.workflow_id == exp.workflow_id,
                        "[{host_name}] committed WorkflowRun {rid} workflow_id changed from {} to {}",
                        exp.workflow_id,
                        run.workflow_id
                    );
                }
            }

            for ((h, bid), exp_pid) in &self.committed_merge_batches {
                if h != host_name {
                    continue;
                }
                world.hit("oracle.committed_merge_batch_checked");
                let batch = snap
                    .merge_batches
                    .iter()
                    .find(|b| b.id == *bid)
                    .with_context(|| {
                        format!(
                            "[{host_name}] DURABILITY VIOLATION: committed MergeBatch {bid} vanished!"
                        )
                    })?;
                if !exp_pid.is_empty() {
                    ensure!(
                        batch.project_id == *exp_pid,
                        "[{host_name}] committed MergeBatch {bid} project_id changed from {exp_pid} to {}",
                        batch.project_id
                    );
                }
            }

            for ((h, srid), (exp_pid, exp_rule)) in &self.committed_scheduled_runs {
                if h != host_name {
                    continue;
                }
                let sr = snap
                    .scheduled_runs
                    .iter()
                    .find(|r| r.id == *srid)
                    .with_context(|| {
                        format!(
                            "[{host_name}] DURABILITY VIOLATION: committed ScheduledRun {srid} vanished!"
                        )
                    })?;
                if !exp_pid.is_empty() {
                    ensure!(
                        sr.project_id == *exp_pid,
                        "[{host_name}] committed ScheduledRun {srid} project_id changed from {exp_pid} to {}",
                        sr.project_id
                    );
                }
                if !exp_rule.is_empty() {
                    ensure!(
                        sr.rule_id == *exp_rule,
                        "[{host_name}] committed ScheduledRun {srid} rule_id changed from {exp_rule} to {}",
                        sr.rule_id
                    );
                }
            }

            for ((h, nid), exp) in &self.committed_notes {
                if h != host_name {
                    continue;
                }
                let note = snap.notes.iter().find(|n| n.id == *nid).with_context(|| {
                    format!("[{host_name}] DURABILITY VIOLATION: committed Note {nid} vanished!")
                })?;
                ensure!(
                    note.revision >= exp.min_revision,
                    "[{host_name}] DURABILITY VIOLATION: committed Note {nid} regressed to rev {} < {}",
                    note.revision,
                    exp.min_revision
                );
                if !exp.project_id.is_empty() {
                    ensure!(
                        note.project_id == exp.project_id,
                        "[{host_name}] committed Note {nid} project_id changed from {} to {}",
                        exp.project_id,
                        note.project_id
                    );
                }
            }
        }
        Ok(())
    }

    /// Runs SQLite `PRAGMA foreign_key_check` and `PRAGMA quick_check` read-only on every
    /// simulated server's `canban.sqlite` database.
    pub fn check_sqlite_integrity(world: &SimWorld) -> Result<()> {
        for (host_name, handle) in &world.servers {
            let db_path = handle.home.join("canban.sqlite");
            if !db_path.exists() {
                continue;
            }
            let conn = Connection::open_with_flags(
                &db_path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .with_context(|| {
                format!(
                    "[{host_name}] failed to open {} read-only for SQLite integrity check",
                    db_path.display()
                )
            })?;

            let mut fk_stmt = conn.prepare("PRAGMA foreign_key_check")?;
            let fk_rows: Vec<(String, i64, String, i64)> = fk_stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            ensure!(
                fk_rows.is_empty(),
                "[{host_name}] SQLite PRAGMA foreign_key_check failed on {}: {fk_rows:?}",
                db_path.display()
            );

            let mut qc_stmt = conn.prepare("PRAGMA quick_check")?;
            let qc_rows: Vec<String> = qc_stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            ensure!(
                qc_rows == vec!["ok".to_owned()],
                "[{host_name}] SQLite PRAGMA quick_check failed on {}: {qc_rows:?}",
                db_path.display()
            );

            // GitHub source bodies must stay in disposable cache, never persisted into SQLite `github_issues`.
            let gh_rows: i64 =
                conn.query_row("SELECT COUNT(*) FROM github_issues", [], |row| row.get(0))?;
            ensure!(
                gh_rows == 0,
                "[{host_name}] SQLite table github_issues must remain empty, found {gh_rows} rows"
            );
            world.hit("oracle.sqlite_integrity_checked");
            if world.config.fs_durability == crate::world::FsDurability::Full
                && !world.config.sqlite_synchronous_off
            {
                world.hit("oracle.full_durability_checked");
            }
        }
        Ok(())
    }

    /// Post-bounce startup recovery check on a single server host: asserts zero `AgentRun`s stuck
    /// in `"starting"` and zero `Question`s stuck in `"delivering"`.
    pub fn check_post_bounce_recovery(world: &SimWorld, host_name: &str) -> Result<()> {
        let Some(handle) = world.servers.get(host_name) else {
            bail!("Unknown server host '{host_name}'");
        };
        let Some(server) = handle.server() else {
            bail!("Server '{host_name}' is not running for post-bounce recovery check");
        };
        let snap = server.snapshot()?;
        for t in &snap.terminals {
            if let Some(agent) = t.agent.as_ref() {
                ensure!(
                    agent.state != "starting",
                    "[{host_name}] post-bounce recovery failed: AgentRun {} stuck in state=\"starting\"",
                    t.id
                );
            }
        }
        for q in &snap.questions {
            ensure!(
                q.state != "delivering",
                "[{host_name}] post-bounce recovery failed: Question {} stuck in state=\"delivering\"",
                q.id
            );
        }
        world.hit("oracle.post_bounce_recovery_checked");
        Ok(())
    }

    fn check_post_bounce_recovery_all(world: &SimWorld) -> Result<()> {
        for (host_name, handle) in &world.servers {
            if handle.server().is_some() && handle.ever_bounced.load(Ordering::SeqCst) {
                Self::check_post_bounce_recovery(world, host_name)?;
            }
        }
        Ok(())
    }

    /// #2 (P0 #2): At quiescence, no outbox items remain `Queued`, `InFlight`, OR unexpected `Failed`
    /// on ANY live simulated client.
    fn check_outbox_drained(&self, world: &SimWorld) -> Result<()> {
        for client_name in world.client_names() {
            let items = world.client_sync_items_on(&client_name);
            for it in &items {
                match it.state {
                    SyncItemState::Queued | SyncItemState::InFlight => {
                        bail!(
                            "[{client_name}] Outbox item {} ({}) for host {} still {:?} at quiescence; host_summaries={:?}",
                            it.mutation_id,
                            it.card_title,
                            it.host_id,
                            it.state,
                            world.client_host_summaries_on(&client_name)
                        );
                    }
                    SyncItemState::Failed if !self.allow_failed_ids.contains(&it.mutation_id) => {
                        bail!(
                            "[{client_name}] Unexpected SyncItemState::Failed outbox item {} ({}) for host {} at quiescence: msg={:?}",
                            it.mutation_id,
                            it.card_title,
                            it.host_id,
                            it.message
                        );
                    }
                    SyncItemState::Conflict | SyncItemState::Failed => {}
                }
            }
        }
        Ok(())
    }

    /// Verifies that a visible Card is completely readable via `MultiHostClient`
    /// (`card.get` returns a complete deserializable `Card` + `timeline.list` returns a valid events array).
    pub fn assert_card_complete_and_readable(
        world: &mut SimWorld,
        card_id: &str,
        project_id: &str,
    ) -> Result<Card> {
        let raw_card = world.client_call(
            "card.get",
            serde_json::json!({ "card_id": card_id, "view": "full" }),
            Some(project_id),
        )?;
        let card: Card = serde_json::from_value(raw_card.clone()).map_err(|e| {
            anyhow::anyhow!(
                "card.get({card_id}) returned incomplete/unparseable Card payload: {e}; raw={raw_card}"
            )
        })?;
        ensure!(
            card.id == card_id
                && !card.project_id.trim().is_empty()
                && !card.title.trim().is_empty()
                && !card.stage.trim().is_empty(),
            "card.get({card_id}) returned incomplete Card fields: {card:?}"
        );
        let timeline = world.client_call(
            "timeline.list",
            serde_json::json!({
                "card_id": card_id,
                "tail": true,
                "limit": 50,
                "view": "full",
            }),
            Some(project_id),
        )?;
        ensure!(
            timeline.get("events").and_then(|v| v.as_array()).is_some(),
            "timeline.list({card_id}) returned malformed payload without events array: {timeline}"
        );
        Ok(card)
    }

    /// Verifies that a newly created project terminal is IMMEDIATELY placed under its Project
    /// in the pure UI terminal projection (`canban_ui::terminals::entries` + `worktrees::entry_scope`)
    /// — never transiently falling into `"Other"` / `WorktreeScope::Unassigned`, BOTH in the live
    /// merged snapshot AND even if `terminal_contexts` has not arrived yet.
    pub fn assert_terminal_in_project(
        world: &SimWorld,
        terminal_id: &str,
        expected_project_id: &str,
    ) -> Result<()> {
        let merged = world.client_merged_snapshot()?;
        let rows = canban_ui::terminals::entries(&merged);
        let entry = rows
            .iter()
            .find(|e| e.saved.id == terminal_id)
            .ok_or_else(|| {
                anyhow::anyhow!("terminal {terminal_id} missing from merged_snapshot entries")
            })?;
        let scope = canban_ui::terminals::worktrees::entry_scope(&merged, entry);
        ensure!(
            entry.project_id.as_deref() == Some(expected_project_id)
                && entry.project_name != "Other"
                && !matches!(
                    scope,
                    canban_ui::terminals::worktrees::WorktreeScope::Unassigned(_)
                )
                && scope.project_id(&merged).as_deref() == Some(expected_project_id),
            "terminal {terminal_id} landed outside project {expected_project_id}: project_id={:?}, project_name={:?}, scope={:?}",
            entry.project_id,
            entry.project_name,
            scope
        );

        // Also assert mid-flight safety when `terminal_contexts` for this terminal hasn't arrived yet.
        let mut without_ctx = merged.clone();
        without_ctx
            .terminal_contexts
            .retain(|c| c.terminal_id != terminal_id);
        let rows_no_ctx = canban_ui::terminals::entries(&without_ctx);
        let entry_no_ctx = rows_no_ctx
            .iter()
            .find(|e| e.saved.id == terminal_id)
            .ok_or_else(|| {
                anyhow::anyhow!("terminal {terminal_id} missing when contexts cleared")
            })?;
        let scope_no_ctx = canban_ui::terminals::worktrees::entry_scope(&without_ctx, entry_no_ctx);
        ensure!(
            entry_no_ctx.project_id.as_deref() == Some(expected_project_id)
                && entry_no_ctx.project_name != "Other"
                && !matches!(
                    scope_no_ctx,
                    canban_ui::terminals::worktrees::WorktreeScope::Unassigned(_)
                )
                && scope_no_ctx.project_id(&without_ctx).as_deref() == Some(expected_project_id),
            "terminal {terminal_id} flashed in Other before discovery: project_id={:?}, project_name={:?}, scope={:?}",
            entry_no_ctx.project_id,
            entry_no_ctx.project_name,
            scope_no_ctx
        );
        Ok(())
    }

    /// Verifies that `canban_ui::terminals::fallback_after_close` strictly obeys the 4-tier
    /// priority order: `same worktree -> same project -> same host -> cross host`, and never
    /// selects a closed, closing, or missing terminal.
    pub fn assert_close_fallback_priority(
        snapshot: &Snapshot,
        rows: &[canban_ui::terminals::Entry],
        closing: &BTreeSet<String>,
        closed_id: &str,
    ) -> Result<Option<String>> {
        let Some(closed) = rows.iter().find(|e| e.saved.id == closed_id) else {
            bail!("closed terminal {closed_id} missing from rows");
        };
        let actual = canban_ui::terminals::fallback_after_close(snapshot, rows, closing, closed_id);
        let others: Vec<&canban_ui::terminals::Entry> = rows
            .iter()
            .filter(|e| e.saved.id != closed_id && e.listed() && !closing.contains(&e.saved.id))
            .collect();
        if others.is_empty() {
            ensure!(
                actual.is_none(),
                "expected None when no live terminals remain after closing {closed_id}, got {actual:?}"
            );
            return Ok(None);
        }
        let chosen_id = actual.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "fallback_after_close returned None while {} live terminals remained after closing {closed_id}",
                others.len()
            )
        })?;
        let chosen = others
            .iter()
            .copied()
            .find(|e| e.saved.id == chosen_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "fallback_after_close picked non-live/missing/closing terminal {chosen_id} after closing {closed_id}"
                )
            })?;

        let closed_loc = canban_ui::terminals::worktrees::location_key(closed);
        let chosen_loc = canban_ui::terminals::worktrees::location_key(chosen);
        let closed_pid =
            canban_ui::terminals::worktrees::entry_scope(snapshot, closed).project_id(snapshot);
        let chosen_pid =
            canban_ui::terminals::worktrees::entry_scope(snapshot, chosen).project_id(snapshot);

        let has_same_worktree = others
            .iter()
            .any(|e| canban_ui::terminals::worktrees::location_key(e) == closed_loc);
        let has_same_project = closed_pid.is_some()
            && others.iter().any(|e| {
                canban_ui::terminals::worktrees::entry_scope(snapshot, e).project_id(snapshot)
                    == closed_pid
            });
        let has_same_host = others.iter().any(|e| e.host_id == closed.host_id);

        if has_same_worktree {
            ensure!(
                chosen_loc == closed_loc,
                "closing {closed_id} violated Tier 1 (same worktree {closed_loc}): picked {chosen_id} in {chosen_loc}"
            );
        } else if has_same_project {
            ensure!(
                chosen_pid == closed_pid,
                "closing {closed_id} violated Tier 2 (same project {closed_pid:?}): picked {chosen_id} in {chosen_pid:?}"
            );
        } else if has_same_host {
            ensure!(
                chosen.host_id == closed.host_id,
                "closing {closed_id} violated Tier 3 (same host {}): picked {chosen_id} on host {}",
                closed.host_id,
                chosen.host_id
            );
        }

        Ok(Some(chosen_id.to_owned()))
    }

    /// #2 & #4 (P0 #2 + P1 full convergence): At quiescence, `client.merged_snapshot()` on EVERY
    /// live simulated client matches every enabled live server across normalized Cards, Projects,
    /// WorkflowRuns, Terminals, CardReads, Reminders, GlobalEnv, and empty `pending_sync` —
    /// exempting ONLY the exact `(host_id, raw_card_id)` pairs with an active
    /// `SyncItemState::Conflict`, and FAILING if any live enabled host is still disconnected!
    fn check_client_server_convergence(&self, world: &SimWorld) -> Result<()> {
        let local_global_env = world
            .server("local-server")
            .and_then(|s| s.snapshot().ok())
            .map(|s| s.global_env);

        for client_name in world.client_names() {
            let has_local = world
                .client_named(&client_name)
                .is_some_and(|c| c.capabilities().local_host);
            let merged = world.client_merged_snapshot_on(&client_name)?;
            let summaries = world.client_host_summaries_on(&client_name);
            let sync_items = world.client_sync_items_on(&client_name);

            let mut conflict_cards: BTreeSet<(String, String)> = BTreeSet::new();
            for item in &sync_items {
                match item.state {
                    SyncItemState::Conflict => {
                        let raw_cid = strip_host_prefix(&item.card_id);
                        conflict_cards.insert((item.host_id.clone(), raw_cid));
                    }
                    SyncItemState::Failed => {
                        if !self.allow_failed_ids.contains(&item.mutation_id) {
                            bail!(
                                "[{client_name}] Unexpected Failed sync item {} on host {} during convergence check: {:?}",
                                item.mutation_id,
                                item.host_id,
                                item.message
                            );
                        }
                        let raw_cid = strip_host_prefix(&item.card_id);
                        conflict_cards.insert((item.host_id.clone(), raw_cid));
                    }
                    SyncItemState::Queued | SyncItemState::InFlight => {}
                }
            }

            let env_status = world.client_global_env_status_on(&client_name);

            for (host_name, handle) in &world.servers {
                let Some(server) = handle.server() else {
                    continue;
                };
                let client_host_id = if host_name == "local-server" {
                    "local"
                } else {
                    host_name.as_str()
                };
                if client_host_id == "local" && !has_local {
                    continue;
                }
                if let Some(summary) = summaries.iter().find(|s| s.id == client_host_id) {
                    let is_disabled = matches!(
                        &summary.connection,
                        HostConnectionState::Offline { error, .. }
                            if error.eq_ignore_ascii_case("disabled")
                    );
                    if is_disabled {
                        continue;
                    }
                    ensure!(
                        matches!(summary.connection, HostConnectionState::Connected { .. }),
                        "[{client_name}][{client_host_id}] enabled server is NOT Connected at quiescence (state={:?})",
                        summary.connection
                    );
                } else if client_host_id != "local" {
                    bail!(
                        "[{client_name}][{client_host_id}] live remote host missing from client host_summaries at quiescence"
                    );
                }

                let srv_snap = server.snapshot()?;
                assert_host_converged(client_host_id, &srv_snap, &merged, &conflict_cards)
                    .with_context(|| format!("client '{client_name}' convergence check failed"))?;

                if has_local
                    && client_host_id != "local"
                    && env_status.synced_hosts.iter().any(|h| h == client_host_id)
                    && let Some(local_env) = &local_global_env
                {
                    ensure!(
                        &srv_snap.global_env == local_env,
                        "[{client_name}][{client_host_id}] global_env diverged from local-server despite synced_hosts membership: server={:?} local={:?}",
                        srv_snap.global_env,
                        local_env
                    );
                }
            }
        }
        Ok(())
    }
}

fn strip_host_prefix(id: &str) -> String {
    canban_types::split_qualified_host_id(id)
        .map(|(_, raw)| raw.to_owned())
        .unwrap_or_else(|| id.to_owned())
}

fn normalize_client_host_id(host: &str) -> String {
    if host == "local-server" {
        "local".to_owned()
    } else {
        host.to_owned()
    }
}

fn normalize_server_host_name(host: &str) -> String {
    if host == "local" {
        "local-server".to_owned()
    } else {
        host.to_owned()
    }
}

fn normalize_human_gate(
    gate: &Option<HumanGate>,
) -> Option<(String, Option<String>, Option<String>)> {
    gate.as_ref().map(|g| {
        (
            g.reason.clone(),
            g.terminal_id.as_deref().map(strip_host_prefix),
            g.next_stage.clone(),
        )
    })
}

fn assert_host_converged(
    host_id: &str,
    server_snap: &Snapshot,
    merged: &Snapshot,
    conflict_cards: &BTreeSet<(String, String)>,
) -> Result<()> {
    let owner_of = |entity_id: &str| -> &str {
        merged
            .entity_hosts
            .get(entity_id)
            .map(String::as_str)
            .unwrap_or("local")
    };

    // 1. Projects convergence.
    let srv_projects: BTreeMap<&str, &canban_types::Project> = server_snap
        .projects
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();
    let mut merged_projects: BTreeMap<String, &canban_types::Project> = BTreeMap::new();
    for p in &merged.projects {
        if owner_of(&p.id) == host_id {
            merged_projects.insert(strip_host_prefix(&p.id), p);
        }
    }
    ensure!(
        srv_projects.len() == merged_projects.len(),
        "[{host_id}] project count mismatch at quiescence: server={} client_merged={}",
        srv_projects.len(),
        merged_projects.len()
    );
    for (raw_id, sp) in &srv_projects {
        let Some(mp) = merged_projects.get(*raw_id) else {
            bail!("[{host_id}] Project {raw_id} missing from client merged_snapshot at quiescence");
        };
        ensure!(
            mp.name == sp.name && mp.directory == sp.directory,
            "[{host_id}] Project {raw_id} diverged: server=({:?},{:?}) client=({:?},{:?})",
            sp.name,
            sp.directory,
            mp.name,
            mp.directory
        );
    }

    // 2. Full normalized Card fields convergence (exempting only specific conflict cards).
    let srv_cards: BTreeMap<&str, &Card> = server_snap
        .cards
        .iter()
        .map(|c| (c.id.as_str(), c))
        .collect();
    let mut merged_cards: BTreeMap<String, &Card> = BTreeMap::new();
    for c in &merged.cards {
        if owner_of(&c.id) == host_id {
            merged_cards.insert(strip_host_prefix(&c.id), c);
        }
    }
    let non_conflict_srv: BTreeSet<&str> = srv_cards
        .keys()
        .copied()
        .filter(|cid| !conflict_cards.contains(&(host_id.to_owned(), (*cid).to_owned())))
        .collect();
    let non_conflict_merged: BTreeSet<&str> = merged_cards
        .keys()
        .map(String::as_str)
        .filter(|cid| !conflict_cards.contains(&(host_id.to_owned(), (*cid).to_owned())))
        .collect();
    ensure!(
        non_conflict_srv == non_conflict_merged,
        "[{host_id}] non-conflict card ID set mismatch at quiescence: server={non_conflict_srv:?} client_merged={non_conflict_merged:?}"
    );
    for raw_id in non_conflict_srv {
        let sc = srv_cards[raw_id];
        let mc = merged_cards[raw_id];
        ensure!(
            mc.title == sc.title
                && mc.body == sc.body
                && mc.stage == sc.stage
                && mc.revision == sc.revision
                && mc.archived_at == sc.archived_at
                && mc.removed_at == sc.removed_at
                && mc.resolution == sc.resolution
                && mc.attention == sc.attention
                && normalize_human_gate(&mc.human_gate) == normalize_human_gate(&sc.human_gate)
                && mc.delivery == sc.delivery
                && mc.pull_request == sc.pull_request,
            "[{host_id}] Card {raw_id} diverged at quiescence:\n  server: title={:?} stage={:?} rev={} archived={:?} res={:?} attn={:?} gate={:?} delivery={:?} pr={:?}\n  client: title={:?} stage={:?} rev={} archived={:?} res={:?} attn={:?} gate={:?} delivery={:?} pr={:?}",
            sc.title,
            sc.stage,
            sc.revision,
            sc.archived_at,
            sc.resolution,
            sc.attention,
            sc.human_gate,
            sc.delivery,
            sc.pull_request,
            mc.title,
            mc.stage,
            mc.revision,
            mc.archived_at,
            mc.resolution,
            mc.attention,
            mc.human_gate,
            mc.delivery,
            mc.pull_request,
        );
    }

    // 3. WorkflowRuns convergence.
    let srv_runs: BTreeMap<&str, &WorkflowRun> = server_snap
        .workflow_runs
        .iter()
        .map(|r| (r.id.as_str(), r))
        .collect();
    let mut merged_runs: BTreeMap<String, &WorkflowRun> = BTreeMap::new();
    for r in &merged.workflow_runs {
        if owner_of(&r.id) == host_id {
            merged_runs.insert(strip_host_prefix(&r.id), r);
        }
    }
    ensure!(
        srv_runs.len() == merged_runs.len(),
        "[{host_id}] workflow_runs count mismatch: server={} client={}",
        srv_runs.len(),
        merged_runs.len()
    );
    for (raw_id, sr) in &srv_runs {
        let Some(mr) = merged_runs.get(*raw_id) else {
            bail!("[{host_id}] WorkflowRun {raw_id} missing from client merged_snapshot");
        };
        ensure!(
            mr.state == sr.state
                && mr.step == sr.step
                && mr.visit == sr.visit
                && mr.card_id.as_deref().map(strip_host_prefix) == sr.card_id,
            "[{host_id}] WorkflowRun {raw_id} diverged: server=({},{:?},{}) client=({},{:?},{})",
            sr.state,
            sr.step,
            sr.visit,
            mr.state,
            mr.step,
            mr.visit
        );
    }

    // 4. Terminals convergence.
    let srv_terms: BTreeMap<&str, &canban_types::TerminalEntry> = server_snap
        .terminals
        .iter()
        .map(|t| (t.id.as_str(), t))
        .collect();
    let mut merged_terms: BTreeMap<String, &canban_types::TerminalEntry> = BTreeMap::new();
    for t in &merged.terminals {
        if owner_of(&t.id) == host_id {
            merged_terms.insert(strip_host_prefix(&t.id), t);
        }
    }
    ensure!(
        srv_terms.len() == merged_terms.len(),
        "[{host_id}] terminals count mismatch: server={} client={}",
        srv_terms.len(),
        merged_terms.len()
    );
    for (raw_id, st) in &srv_terms {
        let Some(mt) = merged_terms.get(*raw_id) else {
            bail!("[{host_id}] Terminal {raw_id} missing from client merged_snapshot");
        };
        let srv_agent = st
            .agent
            .as_ref()
            .map(|a| (&a.state, &a.connection_state, &a.terminal));
        let cli_agent = mt
            .agent
            .as_ref()
            .map(|a| (&a.state, &a.connection_state, &a.terminal));
        ensure!(
            srv_agent == cli_agent,
            "[{host_id}] Terminal {raw_id} agent diverged: server={srv_agent:?} client={cli_agent:?}"
        );
    }

    // 5. CardReads convergence (for non-conflict cards).
    let mut merged_reads: BTreeMap<String, u64> = BTreeMap::new();
    for cr in &merged.card_reads {
        if owner_of(&cr.card_id) == host_id {
            merged_reads.insert(strip_host_prefix(&cr.card_id), cr.sequence);
        }
    }
    for sr in &server_snap.card_reads {
        if conflict_cards.contains(&(host_id.to_owned(), sr.card_id.clone())) {
            continue;
        }
        let cli_seq = merged_reads.get(&sr.card_id).copied().unwrap_or(0);
        ensure!(
            cli_seq == sr.sequence,
            "[{host_id}] CardRead for {} diverged: server={} client={}",
            sr.card_id,
            sr.sequence,
            cli_seq
        );
    }

    // 6. Reminders convergence (excluding conflict cards).
    let srv_rems: BTreeSet<(String, String, bool, bool)> =
        canban_notifications::reminders(server_snap)
            .into_iter()
            .filter(|r| !conflict_cards.contains(&(host_id.to_owned(), r.card_id.clone())))
            .map(|r| (r.id, r.card_id, r.unread, r.pending))
            .collect();
    let merged_rems: BTreeSet<(String, String, bool, bool)> =
        canban_notifications::reminders(merged)
            .into_iter()
            .filter(|r| owner_of(&r.card_id) == host_id)
            .map(|r| {
                (
                    strip_host_prefix(&r.id),
                    strip_host_prefix(&r.card_id),
                    r.unread,
                    r.pending,
                )
            })
            .filter(|(_, raw_cid, _, _)| {
                !conflict_cards.contains(&(host_id.to_owned(), raw_cid.clone()))
            })
            .collect();
    ensure!(
        srv_rems == merged_rems,
        "[{host_id}] reminders diverged at quiescence:\n  server={srv_rems:?}\n  client={merged_rems:?}"
    );

    // 7. Empty `pending_sync` for all non-conflict cards owned by `host_id`.
    for (cid, mark) in &merged.pending_sync {
        if owner_of(cid) != host_id {
            continue;
        }
        let raw_cid = strip_host_prefix(cid);
        if !conflict_cards.contains(&(host_id.to_owned(), raw_cid.clone())) {
            bail!(
                "[{host_id}] non-conflict Card {raw_cid} still has pending_sync={mark:?} at quiescence"
            );
        }
    }

    // 8. MergeBatches convergence.
    let srv_batches: BTreeMap<&str, &canban_types::MergeBatch> = server_snap
        .merge_batches
        .iter()
        .map(|b| (b.id.as_str(), b))
        .collect();
    let mut merged_batches: BTreeMap<String, &canban_types::MergeBatch> = BTreeMap::new();
    for b in &merged.merge_batches {
        if owner_of(&b.id) == host_id {
            merged_batches.insert(strip_host_prefix(&b.id), b);
        }
    }
    ensure!(
        srv_batches.len() == merged_batches.len(),
        "[{host_id}] merge_batches count mismatch: server={} client={}",
        srv_batches.len(),
        merged_batches.len()
    );
    for (raw_id, sb) in &srv_batches {
        let Some(mb) = merged_batches.get(*raw_id) else {
            bail!("[{host_id}] MergeBatch {raw_id} missing from client merged_snapshot");
        };
        ensure!(
            mb.state == sb.state
                && mb.failure_kind == sb.failure_kind
                && mb.items.len() == sb.items.len(),
            "[{host_id}] MergeBatch {raw_id} diverged: server=({},{:?},{}) client=({},{:?},{})",
            sb.state,
            sb.failure_kind,
            sb.items.len(),
            mb.state,
            mb.failure_kind,
            mb.items.len()
        );
    }

    // 9. ScheduledRuns convergence.
    let srv_scheds: BTreeMap<&str, &canban_types::ScheduledRun> = server_snap
        .scheduled_runs
        .iter()
        .map(|r| (r.id.as_str(), r))
        .collect();
    let mut merged_scheds: BTreeMap<String, &canban_types::ScheduledRun> = BTreeMap::new();
    for r in &merged.scheduled_runs {
        let owner = if !r.host_id.is_empty() {
            r.host_id.as_str()
        } else {
            owner_of(&r.id)
        };
        if owner == host_id {
            merged_scheds.insert(strip_host_prefix(&r.id), r);
        }
    }
    ensure!(
        srv_scheds.len() == merged_scheds.len(),
        "[{host_id}] scheduled_runs count mismatch: server={} client={}",
        srv_scheds.len(),
        merged_scheds.len()
    );
    for (raw_id, sr) in &srv_scheds {
        let Some(mr) = merged_scheds.get(*raw_id) else {
            bail!("[{host_id}] ScheduledRun {raw_id} missing from client merged_snapshot");
        };
        ensure!(
            mr.state == sr.state && mr.rule_id == sr.rule_id,
            "[{host_id}] ScheduledRun {raw_id} diverged: server=({},{}) client=({},{})",
            sr.state,
            sr.rule_id,
            mr.state,
            mr.rule_id
        );
    }

    // 10. Project Notes convergence.
    let srv_notes: BTreeMap<&str, &canban_types::Note> = server_snap
        .notes
        .iter()
        .map(|n| (n.id.as_str(), n))
        .collect();
    let mut merged_notes: BTreeMap<String, &canban_types::Note> = BTreeMap::new();
    for n in &merged.notes {
        if owner_of(&n.id) == host_id {
            merged_notes.insert(strip_host_prefix(&n.id), n);
        }
    }
    ensure!(
        srv_notes.len() == merged_notes.len(),
        "[{host_id}] notes count mismatch: server={} client={}",
        srv_notes.len(),
        merged_notes.len()
    );
    for (raw_id, sn) in &srv_notes {
        let Some(mn) = merged_notes.get(*raw_id) else {
            bail!("[{host_id}] Note {raw_id} missing from client merged_snapshot");
        };
        ensure!(
            mn.title == sn.title && mn.body == sn.body && mn.revision == sn.revision,
            "[{host_id}] Note {raw_id} diverged: server=({:?},rev={}) client=({:?},rev={})",
            sn.title,
            sn.revision,
            mn.title,
            mn.revision
        );
    }

    Ok(())
}

fn collect_files_recursive(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_files_recursive(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

/// Computes a UUID-normalized deterministic canonical state fingerprint across all live servers
/// in `world` (projects, cards, stages, revisions, workflows, terminals, reminders, `global_env`).
///
/// Strips random entity UUIDs and temporary directory prefixes so two deterministic runs with the
/// same seed produce the exact same 64-bit digest.
pub fn canonical_state_digest(world: &SimWorld) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mix_u64 = |state: &mut u64, v: u64| {
        for b in v.to_le_bytes() {
            *state ^= u64::from(b);
            *state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    let mix_str = |state: &mut u64, s: &str| {
        mix_u64(state, s.len() as u64);
        for b in s.as_bytes() {
            *state ^= u64::from(*b);
            *state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };

    for (host_name, handle) in &world.servers {
        mix_str(&mut h, host_name);
        let Some(server) = handle.server() else {
            mix_u64(&mut h, 0);
            continue;
        };
        mix_u64(&mut h, 1);
        let Ok(snap) = server.snapshot() else {
            continue;
        };

        let mut proj_list: Vec<(&str, &str, &str)> = snap
            .projects
            .iter()
            .map(|p| {
                let base = Path::new(&p.directory)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                (p.name.as_str(), base, p.id.as_str())
            })
            .collect();
        proj_list.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        let mut proj_idx_map: BTreeMap<&str, usize> = BTreeMap::new();
        mix_u64(&mut h, proj_list.len() as u64);
        for (idx, (name, base, pid)) in proj_list.iter().enumerate() {
            proj_idx_map.insert(*pid, idx);
            mix_str(&mut h, name);
            mix_str(&mut h, base);
        }

        let mut cards_sorted: Vec<(usize, u64, &Card)> = snap
            .cards
            .iter()
            .map(|c| {
                let pidx = proj_idx_map
                    .get(c.project_id.as_str())
                    .copied()
                    .unwrap_or(usize::MAX);
                (pidx, c.number, c)
            })
            .collect();
        cards_sorted.sort_by_key(|a| (a.0, a.1));
        let mut card_key_map: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
        mix_u64(&mut h, cards_sorted.len() as u64);
        for (pidx, num, c) in &cards_sorted {
            card_key_map.insert(c.id.as_str(), (*pidx, *num));
            mix_u64(&mut h, *pidx as u64);
            mix_u64(&mut h, *num);
            mix_str(&mut h, &c.title);
            mix_str(&mut h, &c.body);
            mix_str(&mut h, &c.stage);
            mix_u64(&mut h, c.revision);
            mix_u64(&mut h, u64::from(c.archived_at.is_some()));
            mix_u64(&mut h, u64::from(c.removed_at.is_some()));
            mix_str(&mut h, c.attention.as_deref().unwrap_or(""));
            mix_str(
                &mut h,
                c.human_gate
                    .as_ref()
                    .map(|g| g.reason.as_str())
                    .unwrap_or(""),
            );
            mix_str(
                &mut h,
                c.resolution.as_ref().map(|r| r.kind.as_str()).unwrap_or(""),
            );
            if let Some(pr) = &c.pull_request {
                mix_u64(&mut h, pr.number);
                mix_str(&mut h, &pr.state);
                mix_u64(&mut h, u64::from(pr.merged));
            }
        }

        type CardKey = (usize, u64);
        type WfDigestRow<'a> = (CardKey, &'a str, u64, u32, &'a str, &'a str);
        let mut wf_rows: Vec<WfDigestRow<'_>> = snap
            .workflow_runs
            .iter()
            .map(|r| {
                let ckey = r
                    .card_id
                    .as_deref()
                    .and_then(|cid| card_key_map.get(cid).copied())
                    .unwrap_or((usize::MAX, 0));
                (
                    ckey,
                    r.workflow_id.as_str(),
                    r.definition_revision,
                    r.visit,
                    r.state.as_str(),
                    r.step.as_deref().unwrap_or(""),
                )
            })
            .collect();
        wf_rows.sort();
        mix_u64(&mut h, wf_rows.len() as u64);
        for ((pidx, num), wfid, drev, visit, state, step) in wf_rows {
            mix_u64(&mut h, pidx as u64);
            mix_u64(&mut h, num);
            mix_str(&mut h, wfid);
            mix_u64(&mut h, drev);
            mix_u64(&mut h, u64::from(visit));
            mix_str(&mut h, state);
            mix_str(&mut h, step);
        }

        let mut term_rows: Vec<(CardKey, &str, &str, &str)> = snap
            .terminals
            .iter()
            .map(|t| {
                if let Some(a) = &t.agent {
                    let ckey = card_key_map
                        .get(a.card_id.as_str())
                        .copied()
                        .unwrap_or((usize::MAX, 0));
                    (
                        ckey,
                        a.profile.id.as_str(),
                        a.state.as_str(),
                        a.connection_state.as_str(),
                    )
                } else {
                    let pidx = t
                        .origin_project_id
                        .as_deref()
                        .and_then(|p| proj_idx_map.get(p).copied())
                        .unwrap_or(usize::MAX);
                    ((pidx, 0), "shell", "shell", "")
                }
            })
            .collect();
        term_rows.sort();
        mix_u64(&mut h, term_rows.len() as u64);
        for ((pidx, num), prof, state, conn) in term_rows {
            mix_u64(&mut h, pidx as u64);
            mix_u64(&mut h, num);
            mix_str(&mut h, prof);
            mix_str(&mut h, state);
            mix_str(&mut h, conn);
        }

        type ReminderDigestRow = (CardKey, String, bool, bool, String);
        let mut events = server.events().unwrap_or_default();
        events.retain(|e| card_key_map.contains_key(e.card_id.as_str()));
        let mut rem_rows: Vec<ReminderDigestRow> =
            canban_notifications::reminders_from_events_at(&snap, &events, server.now_ms())
                .into_iter()
                .map(|r| {
                    let ckey = card_key_map
                        .get(r.card_id.as_str())
                        .copied()
                        .unwrap_or((usize::MAX, 0));
                    let prefix = r.id.split(':').next().unwrap_or("other").to_owned();
                    (ckey, prefix, r.unread, r.pending, r.message)
                })
                .collect();
        rem_rows.sort();
        mix_u64(&mut h, rem_rows.len() as u64);
        for ((pidx, num), prefix, unread, pending, msg) in rem_rows {
            mix_u64(&mut h, pidx as u64);
            mix_u64(&mut h, num);
            mix_str(&mut h, &prefix);
            mix_u64(&mut h, u64::from(unread));
            mix_u64(&mut h, u64::from(pending));
            mix_str(&mut h, &msg);
        }

        mix_u64(&mut h, snap.global_env.len() as u64);
        for (k, meta) in &snap.global_env {
            mix_str(&mut h, k);
            match meta {
                EnvEntryMeta::Plain { value } => {
                    mix_u64(&mut h, 1);
                    mix_str(&mut h, value);
                }
                EnvEntryMeta::Secret { configured, .. } => {
                    mix_u64(&mut h, 2);
                    mix_u64(&mut h, u64::from(*configured));
                }
                EnvEntryMeta::Unset => {
                    mix_u64(&mut h, 3);
                }
            }
        }

        let mut batch_rows: Vec<(usize, &str, &str, usize)> = snap
            .merge_batches
            .iter()
            .map(|b| {
                let pidx = proj_idx_map
                    .get(b.project_id.as_str())
                    .copied()
                    .unwrap_or(usize::MAX);
                (
                    pidx,
                    b.state.as_str(),
                    b.failure_kind.as_deref().unwrap_or(""),
                    b.items.len(),
                )
            })
            .collect();
        batch_rows.sort();
        mix_u64(&mut h, batch_rows.len() as u64);
        for (pidx, state, fail_kind, items_len) in batch_rows {
            mix_u64(&mut h, pidx as u64);
            mix_str(&mut h, state);
            mix_str(&mut h, fail_kind);
            mix_u64(&mut h, items_len as u64);
        }

        let mut sched_rows: Vec<(usize, &str, u64, &str)> = snap
            .scheduled_runs
            .iter()
            .map(|r| {
                let pidx = proj_idx_map
                    .get(r.project_id.as_str())
                    .copied()
                    .unwrap_or(usize::MAX);
                (pidx, r.rule_id.as_str(), r.number, r.state.as_str())
            })
            .collect();
        sched_rows.sort();
        mix_u64(&mut h, sched_rows.len() as u64);
        for (pidx, rule_id, num, state) in sched_rows {
            mix_u64(&mut h, pidx as u64);
            mix_str(&mut h, rule_id);
            mix_u64(&mut h, num);
            mix_str(&mut h, state);
        }

        let mut note_rows: Vec<(usize, &str, u64, &str)> = snap
            .notes
            .iter()
            .map(|n| {
                let pidx = proj_idx_map
                    .get(n.project_id.as_str())
                    .copied()
                    .unwrap_or(usize::MAX);
                (pidx, n.title.as_str(), n.revision, n.body.as_str())
            })
            .collect();
        note_rows.sort();
        mix_u64(&mut h, note_rows.len() as u64);
        for (pidx, title, rev, body) in note_rows {
            mix_u64(&mut h, pidx as u64);
            mix_str(&mut h, title);
            mix_u64(&mut h, rev);
            mix_str(&mut h, body);
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{PRIMARY_CLIENT, SimWorld, SimWorldConfig, StorageFaultKind};
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn read_your_writes_detects_revision_regression_and_validates_monotonic_writes() {
        let mut oracle = InvariantOracle::default();
        oracle
            .observe_card_revision(PRIMARY_CLIENT, "remote-a", "remote-a::c1", 1)
            .unwrap();
        oracle
            .observe_card_revision(PRIMARY_CLIENT, "remote-a", "c1", 2)
            .unwrap();
        let err = oracle
            .observe_card_revision(PRIMARY_CLIENT, "remote-a", "c1", 1)
            .expect_err("revision regression must fail immediately");
        assert!(format!("{err:#}").contains("MONOTONIC READ / RYW VIOLATION"));

        let mut cfg = SimWorldConfig::desktop_default(889_001, "unit_oracle_ryw");
        cfg.remote_hosts = vec!["remote-a".into()];
        let mut world = SimWorld::new(cfg).expect("world init");
        let dir = world.project_dir("ryw-repo").expect("dir");
        let proj = world
            .client_call(
                "project.create",
                json!({ "host_id": "remote-a", "name": "RYW", "directory": dir }),
                None,
            )
            .expect("project.create");
        let pid = proj["id"].as_str().unwrap().to_owned();
        let created = world
            .client_call(
                "card.create",
                json!({ "project_id": pid, "title": "RYW Card", "request_key": "ryw-create-1" }),
                Some(&pid),
            )
            .expect("card.create");
        let mut live_oracle = InvariantOracle::default();
        live_oracle
            .observe_write_receipt(PRIMARY_CLIENT, "remote-a", &created)
            .expect("observe create receipt");
        let updated = world
            .client_call(
                "card.update",
                json!({
                    "card_id": created["id"],
                    "revision": created["revision"],
                    "title": "RYW Card v2",
                    "request_key": "ryw-update-1",
                }),
                Some(&pid),
            )
            .expect("card.update");
        live_oracle
            .observe_write_receipt(PRIMARY_CLIENT, "remote-a", &updated)
            .expect("observe update receipt");
        world.heal_and_quiesce(10_000).expect("heal");
        live_oracle
            .check_read_your_writes(&world)
            .expect("RYW check must pass");
        assert!(
            world
                .coverage_counts()
                .get("oracle.read_your_writes_checked")
                .copied()
                .unwrap_or(0)
                >= 1
        );
    }

    #[test]
    fn at_most_once_verifies_idempotent_retries_across_ioerror_and_bounce() {
        let mut cfg = SimWorldConfig::desktop_default(889_002, "unit_oracle_at_most_once");
        cfg.remote_hosts = vec!["remote-a".into()];
        let mut world = SimWorld::new(cfg).expect("world init");
        let dir = world.project_dir("amo-repo").expect("dir");
        let proj = world
            .client_call(
                "project.create",
                json!({ "host_id": "remote-a", "name": "AMO", "directory": dir }),
                None,
            )
            .expect("project.create");
        let pid = proj["id"].as_str().unwrap().to_owned();
        let mut oracle = InvariantOracle::default();

        // Inject SQLITE_IOERR on first attempt -> unacknowledged attempt -> retry succeeds once!
        let rkey = "amo-card-1";
        let fixed_id = "amo-fixed-card-1";
        let title = "Idempotent Card Under IOERR";
        let payload = json!({
            "id": fixed_id,
            "project_id": pid,
            "title": title,
            "request_key": rkey,
        });
        world
            .storage_faults("remote-a")
            .unwrap()
            .inject_next(StorageFaultKind::IoError);
        let first = world.client_call("card.create", payload.clone(), Some(&pid));
        assert!(
            first.is_err(),
            "first attempt must hit injected SQLITE_IOERR"
        );
        oracle.record_request_key_attempt_with_marker(
            "remote-a",
            rkey,
            "card.create",
            false,
            title,
        );
        oracle
            .check_at_most_once(&world)
            .expect("0 effects allowed while unacknowledged");

        let ack = world
            .client_call("card.create", payload.clone(), Some(&pid))
            .expect("retry succeeds");
        oracle.record_request_key_attempt_with_marker("remote-a", rkey, "card.create", true, title);

        // Duplicate replay after bounce must return exact same ID and keep 1 materialized effect!
        world.bounce_server("remote-a").expect("bounce");
        world.heal_and_quiesce(15_000).expect("heal");
        let replay = world
            .client_call("card.create", payload, Some(&pid))
            .expect("idempotent replay");
        assert_eq!(replay["id"], ack["id"]);
        oracle.record_request_key_attempt_with_marker("remote-a", rkey, "card.create", true, title);
        oracle
            .check_at_most_once(&world)
            .expect("acknowledged at-most-once must have exactly 1 effect");
        assert!(
            world
                .coverage_counts()
                .get("oracle.at_most_once_checked")
                .copied()
                .unwrap_or(0)
                >= 1
        );
    }

    #[test]
    fn auth_failed_zero_dials_verifies_halted_revoked_host() {
        let mut cfg = SimWorldConfig::desktop_default(889_003, "unit_oracle_auth_zero_dials");
        cfg.remote_hosts = vec!["remote-a".into()];
        let mut world = SimWorld::new(cfg).expect("world init");

        // Revoke the active device token on remote-a and wait for AuthFailed.
        let devs = world
            .server_call("remote-a", "device.list", json!({}))
            .expect("device.list");
        let dev_id = devs["devices"][0]["id"]
            .as_str()
            .expect("device id")
            .to_owned();
        let _ = world
            .server_call("remote-a", "device.revoke", json!({ "device_id": dev_id }))
            .expect("device.revoke");
        world
            .wait_until(10_000, |w| {
                Ok(w.client_host_summaries().iter().any(|s| {
                    s.id == "remote-a"
                        && matches!(s.connection, HostConnectionState::AuthFailed { .. })
                }))
            })
            .expect("wait for AuthFailed");

        let mut oracle = InvariantOracle::default();
        oracle
            .check_auth_failed_zero_dials(&world)
            .expect("capture AuthFailed baseline");
        let base_dials = world.client_dial_count(PRIMARY_CLIENT, "remote-a");
        let base_reconn = world
            .client_host_summaries()
            .into_iter()
            .find(|s| s.id == "remote-a")
            .unwrap()
            .reconnect_count;

        world.advance(Duration::from_secs(12)).expect("advance");
        world
            .client_run_on(PRIMARY_CLIENT, 10_000, |c| async move {
                c.wake_host("remote-a");
                c.wake_all();
                c.on_foreground();
                c.poll_remotes_once_async().await;
                Ok(())
            })
            .expect("background wakes");
        world.advance(Duration::from_secs(8)).expect("advance");

        oracle
            .check_auth_failed_zero_dials(&world)
            .expect("AuthFailed must issue 0 background dials");
        oracle
            .assert_auth_failed_zero_dials_for(
                &world,
                PRIMARY_CLIENT,
                "remote-a",
                base_dials,
                base_reconn,
            )
            .expect("targeted AuthFailed zero-dial assertion");
        assert!(
            world
                .coverage_counts()
                .get("oracle.auth_failed_zero_dials_checked")
                .copied()
                .unwrap_or(0)
                >= 2
        );
    }
}
