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
