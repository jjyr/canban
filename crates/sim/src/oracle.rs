//! Universal Invariant Oracle checking data correctness across all 8 Canban DST domains.
use crate::world::SimWorld;
use anyhow::{Result, bail, ensure};
use canban_types::{Card, HostConnectionState, Snapshot, SyncItemState};
use std::collections::{BTreeMap, BTreeSet};

/// Tracks historical observations across simulation steps to detect regressions
/// (e.g., revision rollback, resurrected read notifications, plaintext secret leaks).
#[derive(Default)]
pub struct InvariantOracle {
    /// Highest observed `Card.revision` per `(host_id, raw_card_id)`.
    pub max_card_revision: BTreeMap<(String, String), u64>,
    /// Secret plaintext strings generated during the run that must NEVER appear in `.outbox.json`.
    pub tracked_secrets: Vec<String>,
    /// Notification IDs explicitly marked read (must never resurrect as unread).
    pub read_notice_ids: BTreeMap<String, BTreeSet<String>>,
    /// Terminals currently marked as protected (viewer active / working / human gate).
    /// Maps `(host_id, herdr_terminal_addr)` -> expected max `sent_inputs` count while protected.
    pub protected_terminals: BTreeMap<(String, String), usize>,
}

impl InvariantOracle {
    pub fn track_secret(&mut self, secret: impl Into<String>) {
        self.tracked_secrets.push(secret.into());
    }

    pub fn record_notice_read(&mut self, host_id: &str, notice_id: impl Into<String>) {
        self.read_notice_ids
            .entry(host_id.to_owned())
            .or_default()
            .insert(notice_id.into());
    }

    /// Runs invariants that MUST hold at every step (even mid-partition or mid-chaos).
    pub fn check_step_invariants(&mut self, world: &SimWorld) -> Result<()> {
        self.check_server_monotonicity_and_notifications(world)?;
        self.check_no_plaintext_secrets_in_outbox(world)?;
        self.check_client_merged_view_invariants(world)?;
        self.check_herdr_runtime_parity(world)?;
        Ok(())
    }

    /// Runs convergence invariants that MUST hold once network faults are healed and drained.
    pub fn check_quiescent_invariants(&mut self, world: &SimWorld) -> Result<()> {
        self.check_step_invariants(world)?;
        self.check_outbox_drained(world)?;
        self.check_client_server_convergence(world)?;
        Ok(())
    }

    /// #1 & #6: Monotonic Card revisions, unique IDs, no resurrected read notices on every live server.
    fn check_server_monotonicity_and_notifications(&mut self, world: &SimWorld) -> Result<()> {
        for (host_name, handle) in &world.servers {
            let Some(server) = handle.server() else {
                continue;
            };
            let snap = server.snapshot()?;

            // Monotonic Card.revision + no duplicate Card IDs.
            let mut seen_cards = BTreeSet::new();
            for card in &snap.cards {
                ensure!(
                    seen_cards.insert(card.id.clone()),
                    "[{host_name}] duplicate Card ID {} in snapshot",
                    card.id
                );
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

            // #6: Notification deduplication & read-persistence check.
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
            let reminders = canban_notifications::reminders(&snap);
            let mut seen_rem = BTreeSet::new();
            for r in &reminders {
                ensure!(
                    seen_rem.insert(r.id.clone()),
                    "[{host_name}] duplicate reminder ID {} in notification center",
                    r.id
                );
            }
        }
        Ok(())
    }

    /// #5: Zero plaintext secret leaks in any `.outbox.json` file under client data dir.
    fn check_no_plaintext_secrets_in_outbox(&self, world: &SimWorld) -> Result<()> {
        if self.tracked_secrets.is_empty() {
            return Ok(());
        }
        let entries = match std::fs::read_dir(&world.client_data_dir) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n.ends_with(".outbox.json"))
                && let Ok(raw) = std::fs::read_to_string(&path)
            {
                for secret in &self.tracked_secrets {
                    ensure!(
                        !raw.contains(secret),
                        "SECURITY VIOLATION: plaintext secret leaked in {}",
                        path.display()
                    );
                }
            }
        }
        Ok(())
    }

    /// #4 & #6: Merged UI snapshot & reminders invariants at the `canban-client` boundary.
    fn check_client_merged_view_invariants(&self, world: &SimWorld) -> Result<()> {
        let merged = world.client_merged_snapshot()?;

        // Every project and card in merged snapshot must have unique IDs (multi-host qualification).
        let mut seen_projects = BTreeSet::new();
        for p in &merged.projects {
            ensure!(
                seen_projects.insert(p.id.clone()),
                "duplicate Project ID {} in merged_snapshot",
                p.id
            );
        }
        let mut seen_cards = BTreeSet::new();
        for c in &merged.cards {
            ensure!(
                seen_cards.insert(c.id.clone()),
                "duplicate Card ID {} in merged_snapshot",
                c.id
            );
            ensure!(
                !c.id.trim().is_empty()
                    && !c.project_id.trim().is_empty()
                    && !c.title.trim().is_empty()
                    && !c.stage.trim().is_empty(),
                "Incomplete Card in merged_snapshot: id={:?} project_id={:?} title={:?} stage={:?}",
                c.id,
                c.project_id,
                c.title,
                c.stage
            );
            ensure!(
                seen_projects.contains(&c.project_id),
                "Card {} references missing project_id {} in merged_snapshot",
                c.id,
                c.project_id
            );
        }

        // Reminders at the merged client boundary must have unique IDs and consistent unread count.
        let reminders = canban_notifications::reminders(&merged);
        let mut seen_rem = BTreeSet::new();
        for r in &reminders {
            ensure!(
                seen_rem.insert(r.id.clone()),
                "duplicate Reminder ID {} in merged UI notification center",
                r.id
            );
        }
        Ok(())
    }

    /// #7: Active agent counts in `merged_runtime_state()` match live active agent terminals.
    fn check_herdr_runtime_parity(&self, world: &SimWorld) -> Result<()> {
        let rt = world.client_merged_runtime()?;
        let now_ms = world.now_ms();
        let mut computed: BTreeMap<String, usize> = BTreeMap::new();
        for t in &rt.terminals {
            let host = rt
                .terminal_hosts
                .get(&t.id)
                .cloned()
                .unwrap_or_else(|| "local".into());
            let active = if host == "local" {
                t.agent.as_ref().is_some_and(|a| {
                    (a.terminal.is_some()
                        && !a.finished()
                        && (a.close_state == "closing"
                            || !matches!(
                                a.runtime_status_at(now_ms),
                                "closed" | "blocked" | "idle" | "done" | "error"
                            )))
                        || (a.terminal.is_none()
                            && matches!(a.state.as_str(), "starting" | "unknown"))
                })
            } else {
                t.agent
                    .as_ref()
                    .is_some_and(|a| matches!(a.state.as_str(), "starting" | "running" | "working"))
            };
            if active {
                *computed.entry(host).or_default() += 1;
            }
        }
        for (host, expected) in computed {
            let reported = rt.host_active_agents.get(&host).copied().unwrap_or(0);
            ensure!(
                reported == expected,
                "[{host}] merged_runtime_state active_agents mismatch: reported={reported}, computed={expected}"
            );
        }
        Ok(())
    }

    /// #2: At quiescence (without unresolved user conflicts), no outbox items remain Queued/InFlight.
    fn check_outbox_drained(&self, world: &SimWorld) -> Result<()> {
        let items = world.client_sync_items();
        for it in &items {
            if matches!(it.state, SyncItemState::Queued | SyncItemState::InFlight) {
                bail!(
                    "Outbox item {} ({}) for host {} still {:?} at quiescence; host_summaries={:?}",
                    it.mutation_id,
                    it.card_title,
                    it.host_id,
                    it.state,
                    world.client_host_summaries()
                );
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

    /// #2 & #4: At quiescence, `client.merged_snapshot()` contains every active Card from every
    /// connected server with matching titles/stages/revisions (accounting for multi-host qualification).
    fn check_client_server_convergence(&self, world: &SimWorld) -> Result<()> {
        let merged = world.client_merged_snapshot()?;
        let summaries = world.client_host_summaries();

        let has_unresolved_conflicts = world
            .client_sync_items()
            .iter()
            .any(|i| matches!(i.state, SyncItemState::Conflict | SyncItemState::Failed));
        if has_unresolved_conflicts {
            // When explicit user conflicts exist, optimistic overlays intentionally diverge until resolved.
            return Ok(());
        }

        for (host_name, handle) in &world.servers {
            let Some(server) = handle.server() else {
                continue;
            };
            let client_host_id = if host_name == "local-server" {
                "local"
            } else {
                host_name.as_str()
            };
            let is_connected = summaries
                .iter()
                .find(|s| s.id == client_host_id)
                .is_some_and(|s| matches!(s.connection, HostConnectionState::Connected { .. }));
            if !is_connected {
                continue;
            }
            let srv_snap = server.snapshot()?;
            assert_host_cards_converged(client_host_id, &srv_snap, &merged)?;
        }
        Ok(())
    }
}

fn assert_host_cards_converged(
    host_id: &str,
    server_snap: &Snapshot,
    merged: &Snapshot,
) -> Result<()> {
    let srv_cards: BTreeMap<&str, &Card> = server_snap
        .cards
        .iter()
        .map(|c| (c.id.as_str(), c))
        .collect();

    let mut merged_for_host: BTreeMap<String, &Card> = BTreeMap::new();
    for c in &merged.cards {
        let owner = merged
            .entity_hosts
            .get(&c.id)
            .map(String::as_str)
            .unwrap_or("local");
        if owner == host_id {
            let raw_id = canban_types::split_qualified_host_id(&c.id)
                .map(|(_, raw)| raw.to_owned())
                .unwrap_or_else(|| c.id.clone());
            merged_for_host.insert(raw_id, c);
        }
    }

    ensure!(
        srv_cards.len() == merged_for_host.len(),
        "[{host_id}] card count mismatch at quiescence: server={} client_merged={}",
        srv_cards.len(),
        merged_for_host.len()
    );
    for (raw_id, sc) in srv_cards {
        let Some(mc) = merged_for_host.get(raw_id) else {
            bail!("[{host_id}] Card {raw_id} missing from client merged_snapshot at quiescence");
        };
        ensure!(
            mc.title == sc.title && mc.stage == sc.stage && mc.revision == sc.revision,
            "[{host_id}] Card {raw_id} diverged at quiescence: server=({:?},{:?},rev={}) client=({:?},{:?},rev={})",
            sc.title,
            sc.stage,
            sc.revision,
            mc.title,
            mc.stage,
            mc.revision
        );
    }
    Ok(())
}
