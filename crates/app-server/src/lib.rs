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
    let profile = match canban_scheduler::select(config, &card.tags, explicit) {
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
            let mut initial = Config::default();
            let resolve = |program: &str| {
                let mut paths =
                    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                        .collect::<Vec<_>>();
                if let Some(home) = std::env::var_os("HOME") {
                    paths.push(PathBuf::from(home).join(".local/bin"));
                }
                paths.extend([
                    PathBuf::from("/opt/homebrew/bin"),
                    PathBuf::from("/usr/local/bin"),
                ]);
                paths
                    .into_iter()
                    .map(|p| p.join(program))
                    .find(|p| p.is_file())
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| program.into())
            };
            initial.hc = resolve(&initial.hc);
            for profile in &mut initial.profiles {
                if let Some(program) = profile.argv.first_mut() {
                    *program = resolve(program);
                }
            }
            canban_context::atomic(&path, &serde_json::to_vec_pretty(&initial)?)?;
        }
        let flow = home.join("config/workflows/default.json");
        if !flow.exists() {
            canban_context::atomic(&flow, &serde_json::to_vec_pretty(&Workflow::default())?)?;
        }
        let config: Config = serde_json::from_slice(&std::fs::read(path)?)?;
        Self::validate_config(&config)?;
        Ok(Arc::new(Self {
            home,
            store: Mutex::new(store),
            config: RwLock::new(config),
            cli,
            listeners: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        }))
    }
    fn validate_config(c: &Config) -> Result<()> {
        ensure!(c.capacity > 0, "Global capacity must be positive");
        let mut ids = std::collections::HashSet::new();
        for p in &c.profiles {
            ensure!(
                !p.id.is_empty() && ids.insert(&p.id),
                "Profile IDs must be nonempty and unique"
            );
            ensure!(
                p.capacity > 0 && (!p.argv.is_empty() || p.template.is_some()),
                "Profile needs a command/template and positive capacity"
            );
        }
        ensure!(
            ids.contains(&c.default_agent),
            "Default agent is not configured"
        );
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
mod reports;
mod resources;
mod runtime;

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
