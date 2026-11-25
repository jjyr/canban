//! Durable journal dispatch. Channels are hints; cursors and receipts survive restart.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

const CARDLESS_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const ACTIVITY_RETENTION_MS: u64 = 400 * 24 * 60 * 60 * 1000;
const CARDLESS_KEEP_RECENT: usize = 2_000;
const PRUNE_INTERVAL_MS: u64 = 5 * 60 * 1000;

pub(super) fn highwater(db: Db<'_>) -> Result<u64> {
    let max_offset: u64 = db.conn.query_row(
        "SELECT coalesce(max(offset),0) FROM event_journal",
        [],
        |r| r.get(0),
    )?;
    let seq: u64 = db
        .conn
        .query_row(
            "SELECT coalesce((SELECT seq FROM sqlite_sequence WHERE name='event_journal'), 0)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(max_offset.max(seq))
}
fn page(db: Db<'_>, after: u64, limit: usize) -> Result<Vec<JournalEvent>> {
    let mut q=db.conn.prepare("SELECT j.offset,e.data FROM event_journal j JOIN events e ON e.id=j.event_id WHERE j.offset>?1 ORDER BY j.offset LIMIT ?2")?;
    let rows = q.query_map(params![after, limit as u64], |r| {
        Ok((r.get::<_, u64>(0)?, r.get::<_, String>(1)?))
    })?;
    rows.map(|r| {
        let (offset, data) = r?;
        Ok(JournalEvent {
            offset,
            event: serde_json::from_str(&data)?,
        })
    })
    .collect()
}
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn pump(db: Db<'_>, config: &Config) -> Result<()> {
    pump_at_with_prune_state(db, config, now(), None)
}
pub(super) fn pump_at_with_prune_state(
    db: Db<'_>,
    config: &Config,
    now_ms: u64,
    last_prune_at: Option<&AtomicU64>,
) -> Result<()> {
    super::workflow::reconcile(db)?;
    let top = highwater(db)?;
    let mut min_enabled_cursor: Option<u64> = None;
    for mut sub in db.list::<EventSubscription>()? {
        if !sub.enabled {
            continue;
        }
        let initial_cursor = sub.cursor;
        for row in page(db, sub.cursor, 256)? {
            if row.offset > top {
                break;
            }
            let initial_workflow = matches!(sub.target, SubscriptionTarget::Workflow { .. })
                && !sub.selector.include_initial
                && (row.event.data["initial_sync"] == true || row.event.data["baseline"] == true);
            let restoration_workflow = matches!(sub.target, SubscriptionTarget::Workflow { .. })
                && row.event.kind == "card.restored"
                && !sub.selector.types.iter().any(|t| t == "card.restored")
                && sub.selector.reason.as_deref() != Some("restoration");
            let agent_backlog_workflow = matches!(sub.target, SubscriptionTarget::Workflow { .. })
                && row.event.data["reason"] == "agent_returned_backlog"
                && sub.selector.reason.as_deref() != Some("agent_returned_backlog");
            let agent_adopted_workflow = matches!(sub.target, SubscriptionTarget::Workflow { .. })
                && row.event.data["reason"] == "agent_adopted_stage"
                && sub.selector.reason.as_deref() != Some("agent_adopted_stage");
            if !initial_workflow
                && !restoration_workflow
                && !agent_backlog_workflow
                && !agent_adopted_workflow
                && canban_workflow_engine::matches(&sub.selector, &row.event)
            {
                let key = format!("{}:{}", sub.id, row.event.id);
                if db.get::<EventDelivery>(&key).is_err() {
                    let enabled = match &sub.target {
                        SubscriptionTarget::Workflow { workflow_id, .. } => db
                            .list::<WorkflowSource>()?
                            .iter()
                            .find(|s| &s.workflow_id == workflow_id)
                            .is_none_or(|s| s.enabled),
                        _ => true,
                    };
                    db.save(&EventDelivery {
                        definition_revision: match &sub.target {
                            SubscriptionTarget::Workflow { workflow_id, .. } => {
                                Some(super::workflow::published(db, workflow_id)?.revision)
                            }
                            _ => None,
                        },
                        id: key,
                        subscription_id: sub.id.clone(),
                        subscription_revision: sub.revision,
                        event_id: row.event.id,
                        offset: row.offset,
                        selector: sub.selector.clone(),
                        target: sub.target.clone(),
                        state: if enabled { "pending" } else { "skipped" }.into(),
                        attempts: 0,
                        next_attempt_at: 0,
                        error: (!enabled).then(|| "Workflow source is disabled".into()),
                        workflow_run_id: None,
                    })?;
                }
            }
            sub.cursor = row.offset;
        }
        if sub.cursor != initial_cursor {
            db.save(&sub)?;
        }
        min_enabled_cursor = Some(match min_enabled_cursor {
            Some(prev) => prev.min(sub.cursor),
            None => sub.cursor,
        });
    }
    let pending_deliveries: Vec<EventDelivery> = {
        let mut q = db.conn.prepare(
            "SELECT data FROM event_deliveries WHERE json_extract(data, '$.state') = 'pending' ORDER BY rowid",
        )?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str::<EventDelivery>(&r?)?))
            .collect::<Result<Vec<_>>>()?
    };
    for mut d in pending_deliveries
        .into_iter()
        .filter(|d| {
            matches!(d.target, SubscriptionTarget::Workflow { .. })
                && d.state == "pending"
                && d.next_attempt_at <= now_ms
        })
        .take(64)
    {
        let SubscriptionTarget::Workflow {
            workflow_id,
            entry,
            card_id,
        } = &d.target
        else {
            continue;
        };
        let event: Event = db.get(&d.event_id)?;
        // Workflow-owned effects may not recursively re-enter automation.
        if workflow_ancestor(db, &event, workflow_id)? {
            d.state = "skipped".into();
            d.error = Some("Causal workflow cycle suppressed".into());
            db.save(&d)?;
            continue;
        }
        db.conn.execute_batch("SAVEPOINT event_delivery;")?;
        let result = (|| -> Result<String> {
            let mut w = super::workflow::published(db, workflow_id)?;
            if let Some(revision) = d.definition_revision {
                w = db.get(&format!("{workflow_id}:{revision}"))?;
            }
            let rule_cardless = db
                .list::<WorkflowSource>()?
                .iter()
                .find(|s| s.workflow_id == *workflow_id)
                .and_then(|s| s.rules.get(entry))
                .is_some_and(|r| r.cardless);
            let resolved_card_id = if rule_cardless {
                None
            } else {
                card_id
                    .as_deref()
                    .or_else(|| (!event.card_id.is_empty()).then_some(event.card_id.as_str()))
                    .or_else(|| {
                        event.data["related_cards"]
                            .as_array()?
                            .iter()
                            .find(|rc| rc["project_id"].as_str() == Some(w.project_id.as_str()))
                            .and_then(|rc| rc["card_id"].as_str())
                            .filter(|s| !s.is_empty())
                    })
            };
            if let Some(id) = resolved_card_id {
                let mut card: Card = db.get(id)?;
                ensure!(
                    w.project_id == card.project_id,
                    "Workflow target belongs to another Project"
                );
                let run = super::workflow::start_event(
                    db,
                    config,
                    &mut card,
                    w,
                    &entry.clone(),
                    &d.event_id,
                )?;
                if event.kind == "github.pr.ci_failed"
                    && card
                        .attention
                        .as_deref()
                        .is_some_and(crate::github::delivery::is_ci_attention_message)
                {
                    let mut latest_card: Card = db.get(&card.id)?;
                    if latest_card
                        .attention
                        .as_deref()
                        .is_some_and(crate::github::delivery::is_ci_attention_message)
                    {
                        latest_card.attention = None;
                        latest_card.revision += 1;
                        db.save(&latest_card)?;
                    }
                }
                Ok(run.id)
            } else {
                let run = canban_workflow_engine::runtime::start_cardless(
                    db,
                    &w.project_id,
                    w.definition,
                    w.revision,
                    entry,
                    Some(d.event_id.clone()),
                )?;
                Ok(run.id)
            }
        })();
        match result {
            Ok(id) => {
                db.conn.execute_batch("RELEASE event_delivery;")?;
                d.workflow_run_id = Some(id);
                d.state = "completed".into();
                d.error = None;
            }
            Err(e) => {
                db.conn
                    .execute_batch("ROLLBACK TO event_delivery; RELEASE event_delivery;")?;
                let waiting = e.to_string().contains("active workflow")
                    || e.to_string().contains("existing human decision");
                if !waiting {
                    d.attempts += 1;
                }
                d.error = Some(format!("{e:#}"));
                if d.attempts >= 5 {
                    d.state = "failed".into();
                } else {
                    d.next_attempt_at = now_ms + 1000 * (1u64 << d.attempts);
                }
            }
        }
        db.save(&d)?;
    }
    if let Some(last_prune_at) = last_prune_at {
        let current_ts = now_ms;
        let last_prune = last_prune_at.load(Ordering::Relaxed);
        if current_ts.saturating_sub(last_prune) >= PRUNE_INTERVAL_MS
            && last_prune_at
                .compare_exchange(last_prune, current_ts, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            let safe_offset = min_enabled_cursor.unwrap_or(top);
            let _ =
                db.prune_cardless_events(CARDLESS_RETENTION_MS, CARDLESS_KEEP_RECENT, safe_offset)?;
            let _ = db.prune_activity_segments(ACTIVITY_RETENTION_MS)?;
        }
    }
    Ok(())
}
impl AppServer {
    pub(super) fn event_call(&self, method: &str, p: Value) -> Result<Value> {
        match method {
            "event.list" => {
                let selector: EventSelector =
                    serde_json::from_value(p.get("selector").cloned().unwrap_or(json!({})))?;
                let store = self.store.lock().unwrap();
                let db = store.db();
                let rows = page(
                    db,
                    p["after"].as_u64().unwrap_or(0),
                    p["limit"].as_u64().unwrap_or(100).clamp(1, 500) as usize,
                )?;
                let cursor = rows
                    .last()
                    .map_or(p["after"].as_u64().unwrap_or(0), |r| r.offset);
                Ok(
                    json!({"events":rows.into_iter().filter(|r|canban_workflow_engine::matches(&selector,&r.event)).collect::<Vec<_>>(),"next_cursor":cursor,"highwater":highwater(db)?}),
                )
            }
            "subscription.list" => Ok(json!(
                self.store
                    .lock()
                    .unwrap()
                    .db()
                    .list::<EventSubscription>()?
            )),
            "subscription.deliveries" => Ok(json!(
                self.store
                    .lock()
                    .unwrap()
                    .db()
                    .list::<EventDelivery>()?
                    .into_iter()
                    .filter(|d| p["subscription_id"]
                        .as_str()
                        .is_none_or(|v| v == d.subscription_id))
                    .collect::<Vec<_>>()
            )),
            "subscription.save" => {
                let selector: EventSelector =
                    serde_json::from_value(p.get("selector").cloned().unwrap_or(json!({})))?;
                let target: SubscriptionTarget = serde_json::from_value(p["target"].clone())?;
                ensure!(selector.types.len() <= 64, "Too many event types");
                self.mutate(method, &p, |db| {
                    if let Some(project) = &selector.project_id {
                        db.get::<Project>(project)?;
                    }
                    if let Some(card) = &selector.card_id {
                        db.get::<Card>(card)?;
                    }
                    if let SubscriptionTarget::Workflow {
                        workflow_id,
                        entry,
                        card_id,
                    } = &target
                    {
                        let flow = super::workflow::published(db, workflow_id)?;
                        ensure!(
                            flow.definition.entries.contains_key(entry),
                            "Unknown workflow entry"
                        );
                        if let Some(card) = card_id {
                            ensure!(
                                db.get::<Card>(card)?.project_id == flow.project_id,
                                "Wrong target Project"
                            );
                        }
                    }
                    let id = p["subscription_id"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(canban_types::id);
                    let old = db.get::<EventSubscription>(&id).ok();
                    let rev = old.as_ref().map_or(0, |s| s.revision);
                    ensure!(
                        old.as_ref().is_none_or(
                            |s| !s.enabled || s.cursor >= highwater(db).unwrap_or(u64::MAX)
                        ),
                        "Subscription has undispatched events; retry after its backlog drains"
                    );
                    ensure!(
                        p["revision"].as_u64() == Some(rev),
                        "revision_conflict: supply current subscription revision (0 for creation)"
                    );
                    let sub = EventSubscription {
                        id,
                        revision: rev + 1,
                        selector: selector.clone(),
                        target: target.clone(),
                        enabled: p["enabled"].as_bool().unwrap_or(false),
                        cursor: highwater(db)?,
                        created_at: self.now_ms(),
                    };
                    db.save(&sub)?;
                    Ok(json!(sub))
                })
            }
            "subscription.ack" | "subscription.retry" => self.mutate(method, &p, |db| {
                let mut d: EventDelivery = db.get(strp(&p, "delivery_id")?)?;
                if method == "subscription.ack" {
                    ensure!(
                        matches!(d.target, SubscriptionTarget::Consumer),
                        "Workflow deliveries are acknowledged by the engine"
                    );
                    d.state = "completed".into();
                } else {
                    ensure!(d.state == "failed", "Only failed deliveries can be retried");
                    d.state = "pending".into();
                    d.attempts = 0;
                    d.next_attempt_at = 0;
                }
                db.save(&d)?;
                Ok(json!(d))
            }),
            _ => bail!("Unknown event operation {method}"),
        }
    }
}
pub(super) mod github;

impl AppServer {
    pub(super) fn event_deadline(&self) -> Option<u64> {
        self.store
            .lock()
            .ok()?
            .db()
            .list::<EventDelivery>()
            .ok()?
            .into_iter()
            .filter(|d| {
                d.state == "pending" && matches!(d.target, SubscriptionTarget::Workflow { .. })
            })
            .map(|d| d.next_attempt_at)
            .min()
    }
}

fn workflow_ancestor(db: Db<'_>, event: &Event, workflow: &str) -> Result<bool> {
    let mut event = event.clone();
    for _ in 0..16 {
        let Some(id) = event.data["workflow_run_id"].as_str() else {
            return Ok(false);
        };
        let run: WorkflowRun = db.get(id)?;
        if run.workflow_id == workflow {
            return Ok(true);
        }
        let Some(cause) = run.trigger_event_id else {
            return Ok(false);
        };
        event = db.get(&cause)?;
    }
    Ok(true)
}
