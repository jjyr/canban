//! Schedule timer due-evaluation, sleep-skip policy, and overlap admission.
use canban_types::cron::parse_schedule;
use canban_types::{ScheduleOverlap, ScheduleState, ScheduledRun, Snapshot};

/// Maximum delay (2 minutes) within which an on-time runtime wakeup fires the scheduled tick.
/// Ticks older than this window (e.g. machine slept or daemon was offline) advance directly to
/// the next future tick without replaying missed historical runs.
pub const SCHEDULE_LATE_GRACE_MS: u64 = 120_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScheduleAction {
    /// Advance `next_run_at` without triggering a run (e.g. initialized or slept past grace window).
    AdvanceOnly {
        schedule_id: String,
        next_run_at: Option<u64>,
        clear_queued: bool,
    },
    /// Record a skipped run (`overlap: skip` while an earlier run is still active) and advance `next_run_at`.
    SkipOverlap {
        schedule_id: String,
        scheduled_at: u64,
        next_run_at: Option<u64>,
    },
    /// Queue one pending trigger (`overlap: queue` while an earlier run is still active) and advance `next_run_at`.
    QueueOverlap {
        schedule_id: String,
        next_run_at: Option<u64>,
    },
    /// Fire a scheduled run now (either due tick or draining `queued_triggers` after prior run ended).
    Trigger {
        schedule_id: String,
        scheduled_at: u64,
        next_run_at: Option<u64>,
        consume_queued: bool,
    },
}

/// True when `schedule` currently has an in-flight execution (either a Cardless `ScheduledRun`
/// or an active Card `WorkflowRun` started by this schedule).
pub fn schedule_has_active_run(snapshot: &Snapshot, schedule_id: &str) -> bool {
    for run in snapshot
        .scheduled_runs
        .iter()
        .filter(|r| r.schedule_id == schedule_id)
    {
        if !run.creates_card && run.active() {
            return true;
        }
        if run.creates_card
            && let Some(wf_run_id) = &run.workflow_run_id
            && snapshot
                .workflow_runs
                .iter()
                .any(|w| &w.id == wf_run_id && w.active())
        {
            return true;
        }
    }
    false
}

/// Computes the next future run timestamp strictly after `now_ms` for `(cron, timezone)`.
pub fn compute_next_run_at(cron: &str, timezone: Option<&str>, now_ms: u64) -> Option<u64> {
    let (sched, tz) = parse_schedule(cron, timezone).ok()?;
    sched.next_after_ms(now_ms, tz)
}

/// Evaluates all `ScheduleState` records against `now_ms` and returns deterministic actions.
pub fn evaluate_schedules(snapshot: &Snapshot, now_ms: u64) -> Vec<ScheduleAction> {
    let mut actions = Vec::new();
    for state in &snapshot.schedule_states {
        if !state.enabled {
            continue;
        }
        let active = schedule_has_active_run(snapshot, &state.id);

        // Drain a queued overlap trigger as soon as the previous run finishes.
        if !active && state.queued_triggers > 0 {
            let next_after = state
                .next_run_at
                .filter(|&t| t > now_ms)
                .or_else(|| compute_next_run_at(&state.cron, state.timezone.as_deref(), now_ms));
            actions.push(ScheduleAction::Trigger {
                schedule_id: state.id.clone(),
                scheduled_at: now_ms,
                next_run_at: next_after,
                consume_queued: true,
            });
            continue;
        }

        let Some(due_at) = state.next_run_at else {
            let next = compute_next_run_at(&state.cron, state.timezone.as_deref(), now_ms);
            if next.is_some() {
                actions.push(ScheduleAction::AdvanceOnly {
                    schedule_id: state.id.clone(),
                    next_run_at: next,
                    clear_queued: false,
                });
            }
            continue;
        };

        if now_ms < due_at {
            continue;
        }

        let next_after = compute_next_run_at(&state.cron, state.timezone.as_deref(), now_ms);
        if now_ms.saturating_sub(due_at) > SCHEDULE_LATE_GRACE_MS {
            // Machine slept or daemon was offline past the grace window: skip missed ticks cleanly.
            actions.push(ScheduleAction::AdvanceOnly {
                schedule_id: state.id.clone(),
                next_run_at: next_after,
                clear_queued: false,
            });
            continue;
        }

        if active {
            match state.overlap {
                ScheduleOverlap::Skip => actions.push(ScheduleAction::SkipOverlap {
                    schedule_id: state.id.clone(),
                    scheduled_at: due_at,
                    next_run_at: next_after,
                }),
                ScheduleOverlap::Queue => actions.push(ScheduleAction::QueueOverlap {
                    schedule_id: state.id.clone(),
                    next_run_at: next_after,
                }),
                ScheduleOverlap::Allow => actions.push(ScheduleAction::Trigger {
                    schedule_id: state.id.clone(),
                    scheduled_at: due_at,
                    next_run_at: next_after,
                    consume_queued: false,
                }),
            }
        } else {
            actions.push(ScheduleAction::Trigger {
                schedule_id: state.id.clone(),
                scheduled_at: due_at,
                next_run_at: next_after,
                consume_queued: false,
            });
        }
    }
    actions
}

/// Earliest `next_run_at` among enabled `ScheduleState` records.
pub fn next_schedule_deadline(states: &[ScheduleState]) -> Option<u64> {
    states
        .iter()
        .filter(|s| s.enabled)
        .filter_map(|s| {
            if s.queued_triggers > 0 {
                Some(0)
            } else {
                s.next_run_at
            }
        })
        .min()
}

/// Next queued Cardless `ScheduledRun` eligible for execution or Agent slot admission.
/// Script-starting runs (`terminal_id.is_none()`) execute without consuming an Agent slot.
pub fn next_scheduled_run(snapshot: &Snapshot, max_agents: usize) -> Option<ScheduledRun> {
    if let Some(script_run) = snapshot
        .scheduled_runs
        .iter()
        .filter(|r| !r.creates_card && r.state == "queued" && r.terminal_id.is_none())
        .min_by_key(|r| (r.scheduled_at, r.number))
    {
        return Some(script_run.clone());
    }
    if crate::active_count(snapshot) >= max_agents {
        return None;
    }
    snapshot
        .scheduled_runs
        .iter()
        .filter(|r| !r.creates_card && r.state == "queued" && r.terminal_id.is_some())
        .min_by_key(|r| (r.scheduled_at, r.number))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::Snapshot;

    fn sample_state(
        cron: &str,
        overlap: ScheduleOverlap,
        next_run_at: Option<u64>,
    ) -> ScheduleState {
        ScheduleState {
            id: "schedule:p1:wf1:nightly".into(),
            project_id: "p1".into(),
            workflow_id: "wf1".into(),
            workflow_name: "Nightly".into(),
            filename: "nightly.yaml".into(),
            rule_id: "nightly".into(),
            cron: cron.into(),
            timezone: Some("UTC".into()),
            overlap,
            creates_card: false,
            starts_with_script: false,
            target_column: None,
            profile_id: Some("developer".into()),
            enabled: true,
            last_scheduled_at: None,
            next_run_at,
            last_run_id: None,
            run_count: 0,
            queued_triggers: 0,
            updated_at: 1,
        }
    }

    #[test]
    fn skips_stale_missed_ticks_after_sleep_and_fires_within_grace_window() {
        let due = 1_763_200_800_000u64; // exact minute boundary
        let mut snap = Snapshot::default();
        snap.schedule_states
            .push(sample_state("0 * * * *", ScheduleOverlap::Skip, Some(due)));

        // Within 2-minute grace window -> triggers once.
        let on_time = evaluate_schedules(&snap, due + 5_000);
        assert!(matches!(
            on_time.as_slice(),
            [ScheduleAction::Trigger { scheduled_at, consume_queued: false, .. }] if *scheduled_at == due
        ));

        // 10 minutes late (system slept) -> advances without firing missed run.
        let after_sleep = evaluate_schedules(&snap, due + 600_000);
        assert!(matches!(
            after_sleep.as_slice(),
            [ScheduleAction::AdvanceOnly { .. }]
        ));
    }

    #[test]
    fn respects_skip_queue_and_allow_overlap_policies() {
        let due = 1_763_200_800_000u64;
        let active_run = ScheduledRun {
            id: "scheduled:p1:r1".into(),
            schedule_id: "schedule:p1:wf1:nightly".into(),
            project_id: "p1".into(),
            workflow_id: "wf1".into(),
            workflow_name: "Nightly".into(),
            filename: "nightly.yaml".into(),
            rule_id: "nightly".into(),
            cron: "0 * * * *".into(),
            number: 1,
            trigger_kind: "cron".into(),
            creates_card: false,
            card_id: None,
            card_number: None,
            workflow_run_id: None,
            start_step: None,
            current_step: None,
            profile_id: Some("developer".into()),
            provider_session_id: None,
            terminal_id: Some("scheduled:p1:r1".into()),
            cwd: "/tmp".into(),
            state: "running".into(),
            summary: None,
            report_path: None,
            output_path: None,
            error: None,
            steps: Vec::new(),
            scheduled_at: due - 3_600_000,
            started_at: Some(due - 3_600_000),
            completed_at: None,
            close_requested_at: None,
            updated_at: due - 3_600_000,
        };

        for (policy, expected) in [
            (ScheduleOverlap::Skip, "skip"),
            (ScheduleOverlap::Queue, "queue"),
            (ScheduleOverlap::Allow, "trigger"),
        ] {
            let mut snap = Snapshot::default();
            snap.schedule_states
                .push(sample_state("0 * * * *", policy, Some(due)));
            snap.scheduled_runs.push(active_run.clone());
            let actions = evaluate_schedules(&snap, due + 1_000);
            match (expected, actions.as_slice()) {
                ("skip", [ScheduleAction::SkipOverlap { .. }]) => {}
                ("queue", [ScheduleAction::QueueOverlap { .. }]) => {}
                ("trigger", [ScheduleAction::Trigger { .. }]) => {}
                other => panic!("Unexpected overlap decision: {other:?}"),
            }
        }
    }
}
