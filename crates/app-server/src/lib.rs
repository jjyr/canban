use anyhow::{Context, Result, bail, ensure};
use canban_storage::{Db, Store};
use canban_types::*;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

pub struct AppServer {
    pub home: PathBuf,
    store: Mutex<Store>,
    config: RwLock<Config>,
    cli: PathBuf,
    listeners: Mutex<Vec<mpsc::Sender<AppEvent>>>,
    stop: AtomicBool,
    review_gate: Mutex<()>,
}
pub struct Running {
    server: Arc<AppServer>,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.server.stop.store(true, Ordering::SeqCst);
    }
}
fn strp<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    p.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Missing string field: {key}"))
}
fn strings(p: &Value, key: &str) -> Vec<String> {
    p[key]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
fn revise(c: &Card, p: &Value) -> Result<()> {
    canban_kanban::check_revision(
        c,
        p["revision"]
            .as_u64()
            .context("Expected card revision is required")?,
    )
}
fn log(
    db: Db<'_>,
    c: &Card,
    kind: &str,
    actor: &str,
    message: &str,
    request: Option<String>,
    data: Value,
) -> Result<()> {
    db.append(canban_timeline::event(
        &c.project_id,
        &c.id,
        kind,
        actor,
        message,
        request,
        data,
    ))?;
    Ok(())
}
fn attention(db: Db<'_>, card: &mut Card, message: &str) -> Result<()> {
    card.attention = Some(message.into());
    db.save(&canban_notifications::attention(
        &card.project_id,
        &card.id,
        message,
    ))?;
    Ok(())
}
#[allow(
    clippy::too_many_arguments,
    reason = "Transaction-scoped composition of routing, stage and optional mention task"
)]
fn enqueue(
    db: Db<'_>,
    config: &Config,
    card: &mut Card,
    project: &Project,
    stage: &Stage,
    trigger: &str,
    explicit: Option<&str>,
    instruction: Option<String>,
) -> Result<Option<Request>> {
    if !stage.execute && explicit.is_none() {
        return Ok(None);
    }
    let profile = match canban_scheduler::select_for_stage(
        config,
        &card.tags,
        stage.profile.as_deref(),
        explicit,
    ) {
        Ok(p) => p,
        Err(e) => {
            attention(db, card, &e.to_string())?;
            log(
                db,
                card,
                "scheduler.blocked",
                "system",
                &e.to_string(),
                None,
                json!({}),
            )?;
            return Ok(None);
        }
    };
    let req = canban_scheduler::request(
        &project.id,
        &card.id,
        &stage.id,
        trigger,
        profile,
        instruction.unwrap_or_else(|| stage.instruction.clone()),
        if explicit.is_some() {
            None
        } else {
            stage.on_completed.clone()
        },
    );
    db.save(&req)?;
    let mut context = crate::sessions::conversation(db, card, &req.profile)?;
    // Queued work does not replace the pointer to an invocation that is still live.
    if context.last_request_id.is_none() {
        context.last_request_id = Some(req.id.clone());
        db.save(&context)?;
    }
    log(
        db,
        card,
        "execution.queued",
        "system",
        &format!("Queued @{}", req.profile.id),
        Some(req.id.clone()),
        json!({"trigger":trigger,"stage":stage.id}),
    )?;
    Ok(Some(req))
}
impl AppServer {
    pub fn open(home: PathBuf, cli: PathBuf) -> Result<Arc<Self>> {
        let store = Store::open(&home)?;
        let path = home.join("config/agents.json");
        if !path.exists() {
            canban_context::atomic(&path, &serde_json::to_vec_pretty(&Config::default())?)?;
        }
        let (config, migration) = Self::read_config(&home)?;
        let flow = home.join("config/workflows/default.json");
        if !flow.exists() {
            let mut initial = Workflow::default();
            // An older/custom registry may not define the new starter roles.
            // Preserve its tag/default routing instead of modifying user profiles.
            for stage in &mut initial.stages {
                if stage
                    .profile
                    .as_ref()
                    .is_some_and(|id| !config.profiles.iter().any(|p| &p.id == id))
                {
                    stage.profile = None;
                }
            }
            canban_context::atomic(&flow, &serde_json::to_vec_pretty(&initial)?)?;
        }
        let default_workflow: Workflow = serde_json::from_slice(&std::fs::read(&flow)?)?;
        Self::validate_workflow(&config, &default_workflow)?;
        for project in store.db().list::<Project>()? {
            Self::validate_workflow(&config, &project.workflow)?;
        }
        Self::publish_migration(&home, migration)?;
        Ok(Arc::new(Self {
            home,
            store: Mutex::new(store),
            config: RwLock::new(config),
            cli,
            listeners: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
            review_gate: Mutex::new(()),
        }))
    }
    fn read_config(home: &Path) -> Result<(Config, Option<Vec<u8>>)> {
        let bytes = std::fs::read(home.join("config/agents.json"))?;
        let mut value: Value = serde_json::from_slice(&bytes)?;
        let changed = canban_profiles::migrate(&mut value)?;
        let raw: Config = serde_json::from_value(value.clone())?;
        let config = canban_profiles::resolve(raw, home)?;
        Self::validate_config(&config)?;
        Ok((
            config,
            if changed {
                Some(serde_json::to_vec_pretty(&value)?)
            } else {
                None
            },
        ))
    }
    fn publish_migration(home: &Path, bytes: Option<Vec<u8>>) -> Result<()> {
        if let Some(bytes) = bytes {
            let path = home.join("config/agents.json");
            let backup = home
                .join("config")
                .join(format!("agents.legacy-{}.json", id()));
            canban_context::atomic(&backup, &std::fs::read(&path)?)?;
            canban_context::atomic(&path, &bytes)?;
        }
        Ok(())
    }
    fn validate_config(c: &Config) -> Result<()> {
        let mut ids = std::collections::HashSet::new();
        for p in &c.profiles {
            ensure!(
                !p.id.is_empty() && ids.insert(&p.id),
                "Profile IDs must be nonempty and unique"
            );
            ensure!(!p.command().is_empty(), "Profile needs a launch command");
        }
        ensure!(
            ids.contains(&c.default_agent),
            "Default agent is not configured"
        );
        Ok(())
    }
    fn validate_workflow(config: &Config, workflow: &Workflow) -> Result<()> {
        canban_workflow_engine::validate(workflow)?;
        for stage in &workflow.stages {
            if let Some(profile) = &stage.profile {
                ensure!(
                    config.profiles.iter().any(|p| &p.id == profile),
                    "Stage {} references unknown Agent profile @{profile}",
                    stage.id
                );
            }
        }
        Ok(())
    }
    fn validate_references(&self, config: &Config) -> Result<()> {
        let workflow: Workflow = serde_json::from_slice(&std::fs::read(
            self.home.join("config/workflows/default.json"),
        )?)?;
        Self::validate_workflow(config, &workflow)?;
        for project in self.snapshot()?.projects {
            Self::validate_workflow(config, &project.workflow)
                .with_context(|| format!("Project {} workflow", project.name))?;
        }
        Ok(())
    }
    pub fn endpoint(&self) -> PathBuf {
        self.home.join("app.sock")
    }
    pub fn subscribe(&self) -> mpsc::Receiver<AppEvent> {
        let (tx, rx) = mpsc::channel();
        self.listeners.lock().unwrap().push(tx);
        rx
    }
    fn emit(&self, topic: &'static str, card: Option<String>) {
        let event = AppEvent::Changed {
            topic,
            card_id: card,
        };
        self.listeners
            .lock()
            .unwrap()
            .retain(|tx| tx.send(event.clone()).is_ok());
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.store.lock().unwrap().snapshot()
    }
    pub fn config(&self) -> Config {
        self.config.read().unwrap().clone()
    }
    fn mutate(
        &self,
        method: &str,
        p: &Value,
        f: impl FnOnce(Db<'_>) -> Result<Value>,
    ) -> Result<Value> {
        let fingerprint = canban_context::digest(&serde_json::to_vec(&json!([method, p]))?);
        let key = p["request_key"].as_str();
        let result = self.store.lock().unwrap().write(|db| {
            if let Some(key) = key
                && let Some((old, result)) = db
                    .conn
                    .query_row(
                        "SELECT fingerprint,result FROM operations WHERE id=?1",
                        [key],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                    )
                    .optional()?
            {
                ensure!(
                    old == fingerprint,
                    "request_key_conflict: same key, different payload"
                );
                return Ok(serde_json::from_str(&result)?);
            }
            let result = f(db)?;
            if let Some(key) = key {
                db.conn.execute(
                    "INSERT INTO operations(id,fingerprint,result) VALUES(?1,?2,?3)",
                    params![key, fingerprint, serde_json::to_string(&result)?],
                )?;
            }
            Ok(result)
        })?;
        self.emit("application", p["card_id"].as_str().map(str::to_owned));
        Ok(result)
    }
}

mod commands;

mod notes;
mod reports;
mod resources;
mod runtime;
mod sessions;

pub fn default_home() -> Result<PathBuf> {
    if let Some(v) = std::env::var_os("CANBAN_HOME") {
        return Ok(v.into());
    }
    Ok(PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".canban"))
}
pub fn rpc(endpoint: &Path, method: &str, params: Value) -> Result<Value> {
    let mut stream = UnixStream::connect(endpoint)
        .with_context(|| format!("Canban is not running at {}", endpoint.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(90)))?;
    writeln!(
        stream,
        "{}",
        json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
    )?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let reply: Value = serde_json::from_str(&line)?;
    ensure!(
        reply.get("error").is_none(),
        "{}",
        reply["error"]["message"]
    );
    reply
        .get("result")
        .cloned()
        .context("Invalid Canban response")
}

#[cfg(test)]
mod tests;

mod review;
