//! User dwell recording, Agent working segment flushing, and Insights activity queries.
use super::*;
use std::collections::HashMap;

const MAX_RECORD_BATCH: usize = 200;
const MAX_DWELL_DURATION_MS: u64 = 12 * 60 * 60 * 1000;
const FUTURE_SLACK_MS: u64 = 60_000;
const MAX_ACTIVITY_RANGE_MS: u64 = 8 * 24 * 60 * 60 * 1000;
const MAX_ACTIVITY_ROWS: usize = 20_000;
const MAX_BUCKET_BOUNDARIES: usize = 400;

fn opt_id(val: &Value, key: &str) -> Result<Option<String>> {
    match val.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                ensure!(trimmed.len() <= 256, "{key} is too long");
                Ok(Some(trimmed.to_owned()))
            }
        }
        Some(_) => bail!("{key} must be a string"),
    }
}

fn parse_kinds(p: &Value) -> Result<Option<Vec<ActivityKind>>> {
    let Some(raw) = p.get("kinds").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let arr = raw.as_array().context("kinds must be an array")?;
    let mut kinds = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item.as_str().context("kinds entries must be strings")?;
        let kind = match s {
            "dwell" => ActivityKind::Dwell,
            "agent" => ActivityKind::Agent,
            other => bail!("Unknown activity kind '{other}'"),
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    Ok(Some(kinds))
}

fn find_terminal<'a>(
    snapshot: &'a Snapshot,
    core_id: &str,
    address: &str,
) -> Option<&'a TerminalEntry> {
    snapshot.terminals.iter().find(|entry| {
        AppServer::terminal_binding(snapshot, entry).is_some_and(|(bound_core, bound_addr)| {
            bound_addr == address && (core_id.is_empty() || bound_core == core_id)
        })
    })
}

fn special_profile_id<'a>(snapshot: &'a Snapshot, entry_id: &str) -> Option<&'a str> {
    if let Some(cfg) = snapshot
        .configuration_sessions
        .iter()
        .find(|c| c.id == entry_id)
    {
        return Some(cfg.profile_id.as_str());
    }
    if let Some(batch) = snapshot.merge_batches.iter().find(|b| b.id == entry_id) {
        return Some(batch.profile_id.as_str());
    }
    if let Some(srun) = snapshot
        .scheduled_runs
        .iter()
        .find(|r| r.terminal_id.as_deref() == Some(entry_id) || r.id == entry_id)
    {
        return srun.profile_id.as_deref();
    }
    None
}

struct AgentWorkSpan<'a> {
    core_id: &'a str,
    address: &'a str,
    start_ms: u64,
    end_ms: u64,
    hint_engine: Option<&'a str>,
    open: bool,
}

fn build_agent_segment(
    snapshot: &Snapshot,
    config: &Config,
    span: AgentWorkSpan<'_>,
) -> Option<ActivitySegment> {
    let AgentWorkSpan {
        core_id,
        address,
        start_ms,
        end_ms,
        hint_engine,
        open,
    } = span;
    if end_ms <= start_ms {
        return None;
    }
    let entry = find_terminal(snapshot, core_id, address)?;
    if let Some(run) = &entry.agent {
        return Some(ActivitySegment {
            id: None,
            kind: ActivityKind::Agent,
            start_ms,
            end_ms,
            project_id: Some(run.project_id.clone()),
            card_id: Some(run.card_id.clone()),
            terminal_id: Some(entry.id.clone()),
            open,
            data: ActivityDetail::Agent(AgentWorkData {
                profile_id: run.profile.id.clone(),
                provider: run.profile.provider_name(),
                model: run.profile.effective_model(),
                engine: hint_engine
                    .map(str::to_owned)
                    .or_else(|| Some(run.profile.command_name())),
                stage: Some(run.stage.clone()),
                run_number: Some(run.number),
                trigger: Some(run.trigger.to_string()),
                manual: false,
            }),
        });
    }
    let runtime = entry.runtime.as_ref()?;
    let project_id = snapshot.terminal_project_id(entry).map(str::to_owned);
    let card_id = snapshot
        .terminal_contexts
        .iter()
        .find(|c| c.terminal_id == entry.id)
        .and_then(|c| c.card_id.clone())
        .or_else(|| entry.origin_card_id.clone());

    if let Some(pid) = special_profile_id(snapshot, &entry.id)
        && let Some(profile) = config.profiles.iter().find(|p| p.id == pid)
    {
        return Some(ActivitySegment {
            id: None,
            kind: ActivityKind::Agent,
            start_ms,
            end_ms,
            project_id,
            card_id,
            terminal_id: Some(entry.id.clone()),
            open,
            data: ActivityDetail::Agent(AgentWorkData {
                profile_id: profile.id.clone(),
                provider: profile.provider_name(),
                model: profile.effective_model(),
                engine: hint_engine
                    .map(str::to_owned)
                    .or_else(|| Some(profile.command_name())),
                stage: None,
                run_number: None,
                trigger: None,
                manual: false,
            }),
        });
    }

    let raw_engine = entry
        .agent_engine
        .as_deref()
        .or(runtime.observed_agent.as_deref())
        .or(hint_engine)?;
    let normalized = raw_engine.strip_prefix("herdr:").unwrap_or(raw_engine);
    if normalized.is_empty() {
        return None;
    }
    Some(ActivitySegment {
        id: None,
        kind: ActivityKind::Agent,
        start_ms,
        end_ms,
        project_id,
        card_id,
        terminal_id: Some(entry.id.clone()),
        open,
        data: ActivityDetail::Agent(AgentWorkData {
            profile_id: "manual".into(),
            provider: normalized.to_owned(),
            model: "unknown".into(),
            engine: Some(normalized.to_owned()),
            stage: None,
            run_number: None,
            trigger: None,
            manual: true,
        }),
    })
}

fn clip_into_buckets(
    boundaries: &[u64],
    buckets: &mut [ActivityBucket],
    agent_maps: &mut [HashMap<(String, String, String), u64>],
    seg: &ActivitySegment,
) {
    if seg.end_ms <= seg.start_ms {
        return;
    }
    let first = boundaries[0];
    let last = *boundaries.last().unwrap();
    if seg.end_ms <= first || seg.start_ms >= last {
        return;
    }
    let start_idx = boundaries
        .partition_point(|&b| b <= seg.start_ms)
        .saturating_sub(1);
    let end_idx = boundaries
        .partition_point(|&b| b < seg.end_ms)
        .min(buckets.len());
    for i in start_idx..end_idx {
        let bucket = &mut buckets[i];
        let overlap = seg
            .end_ms
            .min(bucket.end_ms)
            .saturating_sub(seg.start_ms.max(bucket.start_ms));
        if overlap == 0 {
            continue;
        }
        let pid = seg.project_id.as_deref().filter(|s| !s.is_empty());
        match (&seg.kind, &seg.data) {
            (ActivityKind::Dwell, ActivityDetail::Dwell(dwell)) => {
                match dwell.lane {
                    DwellLane::Card => bucket.dwell.card += overlap,
                    DwellLane::Terminal => bucket.dwell.terminal += overlap,
                    DwellLane::Other => bucket.dwell.other += overlap,
                }
                if let Some(pid) = pid {
                    bucket
                        .by_project
                        .entry(pid.to_owned())
                        .or_default()
                        .dwell_ms += overlap;
                }
            }
            (ActivityKind::Agent, ActivityDetail::Agent(agent)) => {
                bucket.agent_ms += overlap;
                if let Some(pid) = pid {
                    bucket
                        .by_project
                        .entry(pid.to_owned())
                        .or_default()
                        .agent_ms += overlap;
                }
                *agent_maps[i]
                    .entry((
                        agent.profile_id.clone(),
                        agent.provider.clone(),
                        agent.model.clone(),
                    ))
                    .or_default() += overlap;
            }
            _ => {}
        }
    }
}

impl AppServer {
    pub(super) fn flush_agent_work(&self) -> Result<()> {
        let closed = {
            let mut inv = self.inventory.lock().unwrap();
            if inv.closed_work.is_empty() {
                return Ok(());
            }
            std::mem::take(&mut inv.closed_work)
        };
        self.persist_closed_agent_work(&closed)
    }

    pub(super) fn finish_agent_work(&self) -> Result<()> {
        let closed = {
            let mut inv = self.inventory.lock().unwrap();
            inv.close_all_working(self.now_ms());
            if inv.closed_work.is_empty() {
                return Ok(());
            }
            std::mem::take(&mut inv.closed_work)
        };
        self.persist_closed_agent_work(&closed)
    }

    fn persist_closed_agent_work(&self, closed: &[observation::ClosedAgentWork]) -> Result<()> {
        if closed.is_empty() {
            return Ok(());
        }
        let mut snapshot = self.store.lock().unwrap().db().terminal_snapshot()?;
        self.populate_terminal_contexts(&mut snapshot);
        let config = self.config();
        let rows: Vec<ActivitySegment> = closed
            .iter()
            .filter_map(|work| {
                build_agent_segment(
                    &snapshot,
                    &config,
                    AgentWorkSpan {
                        core_id: &work.core,
                        address: &work.address,
                        start_ms: work.start_ms,
                        end_ms: work.end_ms,
                        hint_engine: work.engine.as_deref(),
                        open: false,
                    },
                )
            })
            .collect();
        if rows.is_empty() {
            return Ok(());
        }
        self.store
            .lock()
            .unwrap()
            .write(|db| db.insert_activity_segments(&rows).map(|_| ()))
    }

    fn open_agent_work_segments(
        &self,
        snapshot: &Snapshot,
        config: &Config,
        current_now: u64,
    ) -> Vec<ActivitySegment> {
        let open_items: Vec<(String, observation::OpenAgentWork)> = {
            let inv = self.inventory.lock().unwrap();
            inv.working
                .iter()
                .map(|(addr, w)| (addr.clone(), w.clone()))
                .collect()
        };
        open_items
            .into_iter()
            .filter_map(|(addr, w)| {
                build_agent_segment(
                    snapshot,
                    config,
                    AgentWorkSpan {
                        core_id: &w.core,
                        address: &addr,
                        start_ms: w.since,
                        end_ms: current_now,
                        hint_engine: w.engine.as_deref(),
                        open: true,
                    },
                )
            })
            .collect()
    }

    pub(super) fn activity_record(&self, p: &Value) -> Result<Value> {
        let arr = p
            .get("segments")
            .and_then(Value::as_array)
            .context("segments array is required")?;
        ensure!(
            arr.len() <= MAX_RECORD_BATCH,
            "At most {MAX_RECORD_BATCH} segments may be recorded per call"
        );
        if arr.is_empty() {
            return Ok(json!({"recorded": 0}));
        }
        let current_now = self.now_ms();
        let mut rows = Vec::with_capacity(arr.len());
        for seg in arr {
            ensure!(seg.is_object(), "Each segment must be an object");
            let start_ms = seg
                .get("start_ms")
                .and_then(Value::as_u64)
                .context("start_ms is required")?;
            let end_ms = seg
                .get("end_ms")
                .and_then(Value::as_u64)
                .context("end_ms is required")?;
            ensure!(end_ms > start_ms, "Segment end_ms must exceed start_ms");
            ensure!(
                end_ms - start_ms <= MAX_DWELL_DURATION_MS,
                "Segment duration exceeds 12h maximum"
            );
            ensure!(
                end_ms <= current_now.saturating_add(FUTURE_SLACK_MS),
                "Segment end_ms cannot be in the future"
            );
            let lane_raw = seg
                .get("lane")
                .and_then(Value::as_str)
                .context("lane is required")?;
            let lane = match lane_raw {
                "card" => DwellLane::Card,
                "terminal" => DwellLane::Terminal,
                "other" => DwellLane::Other,
                other => bail!("Invalid dwell lane '{other}'"),
            };
            let page = seg
                .get("page")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .context("page is required")?;
            ensure!(page.len() <= 64, "page name is too long");
            rows.push(ActivitySegment {
                id: None,
                kind: ActivityKind::Dwell,
                start_ms,
                end_ms,
                project_id: opt_id(seg, "project_id")?,
                card_id: opt_id(seg, "card_id")?,
                terminal_id: opt_id(seg, "terminal_id")?,
                open: false,
                data: ActivityDetail::Dwell(DwellData {
                    lane,
                    page: page.to_owned(),
                }),
            });
        }
        let recorded = self
            .store
            .lock()
            .unwrap()
            .write(|db| db.insert_activity_segments(&rows))?;
        Ok(json!({ "recorded": recorded }))
    }

    pub(super) fn insights_activity(&self, p: &Value) -> Result<Value> {
        let from_ms = p
            .get("from_ms")
            .and_then(Value::as_u64)
            .context("from_ms is required")?;
        let to_ms = p
            .get("to_ms")
            .and_then(Value::as_u64)
            .context("to_ms is required")?;
        ensure!(to_ms > from_ms, "to_ms must be greater than from_ms");
        ensure!(
            to_ms - from_ms <= MAX_ACTIVITY_RANGE_MS,
            "insights.activity range cannot exceed 8 days"
        );
        let kinds = parse_kinds(p)?;
        let project_raw = opt_id(p, "project_id")?;

        let (project, mut snapshot, mut segments, mut truncated) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let project = project_raw
                .as_deref()
                .map(|pid| super::resolve_project_id(db, pid).unwrap_or_else(|_| pid.to_owned()));
            let (rows, truncated) = db.query_activity_segments(
                from_ms,
                to_ms,
                project.as_deref(),
                kinds.as_deref(),
                MAX_ACTIVITY_ROWS,
            )?;
            let snapshot = db.terminal_snapshot()?;
            (project, snapshot, rows, truncated)
        };
        for seg in &mut segments {
            seg.start_ms = seg.start_ms.max(from_ms);
            seg.end_ms = seg.end_ms.min(to_ms);
        }

        let include_agent = kinds
            .as_ref()
            .is_none_or(|k| k.contains(&ActivityKind::Agent));
        if include_agent {
            self.populate_terminal_contexts(&mut snapshot);
            let config = self.config();
            let current_now = self.now_ms();
            for mut seg in self.open_agent_work_segments(&snapshot, &config, current_now) {
                if let Some(pid) = project.as_deref()
                    && seg.project_id.as_deref() != Some(pid)
                {
                    continue;
                }
                if seg.end_ms <= from_ms || seg.start_ms >= to_ms {
                    continue;
                }
                seg.start_ms = seg.start_ms.max(from_ms);
                seg.end_ms = seg.end_ms.min(to_ms);
                if seg.end_ms <= seg.start_ms {
                    continue;
                }
                if segments.len() >= MAX_ACTIVITY_ROWS {
                    truncated = true;
                    break;
                }
                segments.push(seg);
            }
        }
        segments.sort_by_key(|s| (s.start_ms, s.end_ms, s.id.unwrap_or(i64::MAX)));
        Ok(json!({ "segments": segments, "truncated": truncated }))
    }

    pub(super) fn insights_activity_buckets(&self, p: &Value) -> Result<Value> {
        let raw_bounds = p
            .get("boundaries")
            .and_then(Value::as_array)
            .context("boundaries array is required")?;
        ensure!(
            (2..=MAX_BUCKET_BOUNDARIES).contains(&raw_bounds.len()),
            "boundaries must contain between 2 and {MAX_BUCKET_BOUNDARIES} timestamps"
        );
        let mut boundaries = Vec::with_capacity(raw_bounds.len());
        for b in raw_bounds {
            let ms = b.as_u64().context("boundaries entries must be integers")?;
            if let Some(&prev) = boundaries.last() {
                ensure!(ms > prev, "boundaries must be strictly increasing");
            }
            boundaries.push(ms);
        }
        let kinds = parse_kinds(p)?;
        let project_raw = opt_id(p, "project_id")?;

        let first = boundaries[0];
        let last = *boundaries.last().unwrap();
        let mut buckets: Vec<ActivityBucket> = boundaries
            .windows(2)
            .map(|w| ActivityBucket {
                start_ms: w[0],
                end_ms: w[1],
                ..Default::default()
            })
            .collect();
        let mut agent_maps: Vec<HashMap<(String, String, String), u64>> =
            vec![HashMap::new(); buckets.len()];

        let (project, mut snapshot) = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let project = project_raw
                .as_deref()
                .map(|pid| super::resolve_project_id(db, pid).unwrap_or_else(|_| pid.to_owned()));
            db.for_each_activity_segment(
                first,
                last,
                project.as_deref(),
                kinds.as_deref(),
                |seg| {
                    clip_into_buckets(&boundaries, &mut buckets, &mut agent_maps, &seg);
                    Ok(())
                },
            )?;
            let snapshot = db.terminal_snapshot()?;
            (project, snapshot)
        };

        let include_agent = kinds
            .as_ref()
            .is_none_or(|k| k.contains(&ActivityKind::Agent));
        if include_agent {
            self.populate_terminal_contexts(&mut snapshot);
            let config = self.config();
            let current_now = self.now_ms();
            for seg in self.open_agent_work_segments(&snapshot, &config, current_now) {
                if let Some(pid) = project.as_deref()
                    && seg.project_id.as_deref() != Some(pid)
                {
                    continue;
                }
                clip_into_buckets(&boundaries, &mut buckets, &mut agent_maps, &seg);
            }
        }

        for (bucket, map) in buckets.iter_mut().zip(agent_maps) {
            let mut stats: Vec<AgentBucketStat> = map
                .into_iter()
                .map(|((profile_id, provider, model), ms)| AgentBucketStat {
                    profile_id,
                    provider,
                    model,
                    ms,
                })
                .collect();
            stats.sort_by(|a, b| {
                b.ms.cmp(&a.ms)
                    .then_with(|| a.profile_id.cmp(&b.profile_id))
                    .then_with(|| a.provider.cmp(&b.provider))
                    .then_with(|| a.model.cmp(&b.model))
            });
            bucket.by_agent = stats;
        }
        Ok(json!({ "buckets": buckets }))
    }
}
