//! Multi-layer environment & secret resolution, unified `env.save`, preview, log masking, and Herdr launch helper.
//!
//! Precedence (lowest -> highest):
//! Host Login Shell < Global (`scope="global"`) < CLI Agent (`scope="agent"`)
//!   < `@Identity` (`scope="agent"`) < Project (`scope="project"`)
//!   < Protected `CANBAN_*` context < Workflow root env < `$CANBAN_ENV` < Step env.
use super::*;
use canban_storage::{Db, env as env_store, vault};
use canban_types::{EnvEntryMeta, EnvPreviewRow, EnvWriteEntry, is_valid_env_key};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use zeroize::Zeroizing;

const CORE_PROTECTED_KEYS: &[&str] = &[
    "CANBAN_CLI",
    "CANBAN_ENDPOINT",
    "CANBAN_HOME",
    "CANBAN_PROJECT",
    "CANBAN_CARD",
    "CARD_ID",
    "CANBAN_RUN_DIR",
    "CANBAN_SESSION_ID",
    "CANBAN_PROFILE_ID",
    "CANBAN_SOURCE_TRUST",
];

pub(crate) struct ResolvedEnv {
    pub env: BTreeMap<String, String>,
    pub unset_keys: BTreeSet<String>,
    pub safe_workflow_env: BTreeMap<String, String>,
    pub fingerprint: String,
    pub secret_values: Vec<Zeroizing<String>>,
    pub skipped_untrusted_secrets: Vec<String>,
}

pub(crate) struct HerdrLaunchRequest<'a> {
    pub profile: Option<&'a Profile>,
    pub project: Option<&'a Project>,
    pub protected_context: &'a BTreeMap<String, String>,
    pub workflow_env: Option<&'a BTreeMap<String, String>>,
    pub source_trust: Option<&'a str>,
    pub argv: Vec<String>,
    pub cwd: String,
    pub labels: BTreeMap<String, String>,
    pub size: Option<canban_herdr::TerminalSize>,
}

pub(crate) struct HerdrLaunchOutcome {
    pub receipt: Value,
    #[allow(dead_code)]
    pub resolved: ResolvedEnv,
}

pub(crate) fn is_trusted_source(source_trust: Option<&str>) -> bool {
    matches!(source_trust, None | Some("local" | "collaborator"))
}

pub(crate) fn mask_secrets(text: &str, secrets: &[Zeroizing<String>]) -> String {
    if text.is_empty() || secrets.is_empty() {
        return text.to_owned();
    }
    let mut unique: Vec<&str> = secrets
        .iter()
        .map(|s| s.as_str())
        .filter(|s| s.chars().count() >= 6)
        .collect();
    if unique.is_empty() {
        return text.to_owned();
    }
    unique.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    unique.dedup();
    let mut out = text.to_owned();
    for secret in unique {
        if out.contains(secret) {
            out = out.replace(secret, "***");
        }
    }
    out
}

fn resolve_backing_agent_id(
    resolved_config: &Config,
    project: Option<&Project>,
    profile: Option<&Profile>,
) -> Option<String> {
    let prof = profile?;
    if !prof.is_identity() {
        return Some(prof.id.clone());
    }
    if let Some(proj) = project
        && let Some(ov) = proj
            .agent_overrides
            .get(&prof.id)
            .map(ProjectAgentOverride::normalized)
        && let Some(target) = ov.agent
        && !resolved_config
            .profiles
            .iter()
            .any(|p| p.id == target && !p.is_identity() && !p.enabled)
    {
        return Some(target);
    }
    prof.preset.clone()
}

struct EnvLayer {
    label: String,
    scope: &'static str,
    target_id: String,
    meta: BTreeMap<String, EnvEntryMeta>,
}

fn collect_env_layers(
    db: Db<'_>,
    resolved_config: &Config,
    profile: Option<&Profile>,
    project: Option<&Project>,
) -> Result<Vec<EnvLayer>> {
    let mut layers = vec![EnvLayer {
        label: "Global".into(),
        scope: env_store::SCOPE_GLOBAL,
        target_id: String::new(),
        meta: env_store::list_env_meta(db.conn, env_store::SCOPE_GLOBAL, "")?,
    }];
    if let Some(backing_id) = resolve_backing_agent_id(resolved_config, project, profile)
        && !backing_id.starts_with("herdr:")
    {
        let meta = env_store::list_env_meta(db.conn, env_store::SCOPE_AGENT, &backing_id)?;
        if !meta.is_empty() {
            layers.push(EnvLayer {
                label: format!("Agent (@{backing_id})"),
                scope: env_store::SCOPE_AGENT,
                target_id: backing_id,
                meta,
            });
        }
    }
    if let Some(prof) = profile
        && prof.is_identity()
    {
        let meta = env_store::list_env_meta(db.conn, env_store::SCOPE_AGENT, &prof.id)?;
        if !meta.is_empty() {
            layers.push(EnvLayer {
                label: format!("Identity (@{})", prof.id),
                scope: env_store::SCOPE_AGENT,
                target_id: prof.id.clone(),
                meta,
            });
        }
    }
    if let Some(proj) = project {
        let meta = env_store::list_env_meta(db.conn, env_store::SCOPE_PROJECT, &proj.id)?;
        if !meta.is_empty() {
            layers.push(EnvLayer {
                label: "Project".into(),
                scope: env_store::SCOPE_PROJECT,
                target_id: proj.id.clone(),
                meta,
            });
        }
    }
    Ok(layers)
}

fn fingerprint_for(
    trusted: bool,
    user_effective: &BTreeMap<String, String>,
    unset_keys: &BTreeSet<String>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(if trusted { b"T" } else { b"U" });
    for (k, v) in user_effective {
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(b"--unset--\0");
    for k in unset_keys {
        hasher.update(k.as_bytes());
        hasher.update(b"\0");
    }
    format!("{:x}", hasher.finalize())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_effective_env(
    db: Db<'_>,
    home: &Path,
    resolved_config: &Config,
    profile: Option<&Profile>,
    project: Option<&Project>,
    protected_context: &BTreeMap<String, String>,
    workflow_env: Option<&BTreeMap<String, String>>,
    source_trust: Option<&str>,
) -> Result<ResolvedEnv> {
    let trusted = is_trusted_source(source_trust);
    let mut env = BTreeMap::new();
    let mut user_effective = BTreeMap::new();
    let mut vars = BTreeMap::new();
    let mut secret_map: BTreeMap<String, Zeroizing<String>> = BTreeMap::new();
    let mut unset_keys = BTreeSet::new();
    let mut secret_values: Vec<Zeroizing<String>> = Vec::new();
    let mut skipped_untrusted_secrets: Vec<String> = Vec::new();

    let mut push_skipped = |key: &str| {
        if !skipped_untrusted_secrets.iter().any(|k| k == key) {
            skipped_untrusted_secrets.push(key.to_owned());
        }
    };

    for layer in collect_env_layers(db, resolved_config, profile, project)? {
        let decrypted = if trusted && layer.meta.values().any(EnvEntryMeta::is_secret) {
            env_store::list_decrypted_scope(db.conn, home, layer.scope, &layer.target_id)?
        } else {
            BTreeMap::new()
        };
        for (key, entry) in layer.meta {
            match entry {
                EnvEntryMeta::Plain { value } => {
                    unset_keys.remove(&key);
                    env.insert(key.clone(), value.clone());
                    user_effective.insert(key.clone(), value.clone());
                    vars.insert(key, value);
                }
                EnvEntryMeta::Secret { .. } => {
                    unset_keys.remove(&key);
                    if trusted {
                        if let Some((plain, _)) = decrypted.get(&key) {
                            env.insert(key.clone(), plain.as_str().to_owned());
                            user_effective.insert(key.clone(), plain.as_str().to_owned());
                            secret_map.insert(key, plain.clone());
                            secret_values.push(plain.clone());
                        }
                    } else {
                        push_skipped(&key);
                    }
                }
                EnvEntryMeta::Unset => {
                    env.remove(&key);
                    user_effective.remove(&key);
                    vars.remove(&key);
                    secret_map.remove(&key);
                    unset_keys.insert(key);
                }
            }
        }
    }

    for (k, v) in protected_context {
        unset_keys.remove(k);
        env.insert(k.clone(), v.clone());
    }

    let mut safe_workflow_env = BTreeMap::new();
    if let Some(wf_env) = workflow_env {
        let secret_strings: BTreeMap<String, String> = secret_map
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().to_owned()))
            .collect();
        for (k, raw_val) in wf_env {
            let (expanded, skipped) = canban_workflow_engine::definition::expand_env_templates(
                raw_val,
                &vars,
                &secret_strings,
                trusted,
            );
            for s in skipped {
                push_skipped(&s);
            }
            unset_keys.remove(k);
            env.insert(k.clone(), expanded.clone());
            user_effective.insert(k.clone(), expanded.clone());
            if canban_workflow_engine::definition::env_value_references_secret(raw_val) {
                safe_workflow_env.insert(k.clone(), "<secret>".to_owned());
                if trusted && !expanded.is_empty() {
                    secret_values.push(Zeroizing::new(expanded));
                }
            } else {
                safe_workflow_env.insert(k.clone(), expanded);
            }
        }
    }

    for core_key in CORE_PROTECTED_KEYS {
        if let Some(v) = protected_context.get(*core_key) {
            unset_keys.remove(*core_key);
            env.insert((*core_key).to_owned(), v.clone());
        }
    }

    let fingerprint = fingerprint_for(trusted, &user_effective, &unset_keys);
    Ok(ResolvedEnv {
        env,
        unset_keys,
        safe_workflow_env,
        fingerprint,
        secret_values,
        skipped_untrusted_secrets,
    })
}

fn decrypt_incoming_write_entries(
    home: &Path,
    raw_entries: &Value,
    transport_token: Option<&str>,
) -> Result<BTreeMap<String, EnvWriteEntry>> {
    let obj = raw_entries
        .as_object()
        .context("`entries` must be a JSON object")?;
    let mut parsed = BTreeMap::new();
    let mut server_token: Option<String> = None;

    for (key, val) in obj {
        let key = key.trim();
        ensure!(is_valid_env_key(key), "Invalid environment key '{key}'");
        let mut entry: EnvWriteEntry = serde_json::from_value(val.clone())
            .with_context(|| format!("Invalid environment entry for '{key}'"))?;
        if let EnvWriteEntry::Secret {
            value: Some(raw_val),
            ..
        } = &mut entry
            && raw_val.starts_with(vault::TRANSPORT_SEAL_PREFIX)
        {
            let tok = if let Some(t) = transport_token.map(str::trim).filter(|s| !s.is_empty()) {
                t
            } else {
                if server_token.is_none() {
                    server_token = network::read_server_token(home);
                }
                server_token.as_deref().context(
                    "Cannot unseal transport secret without session auth token or legacy server.token",
                )?
            };
            let plain = vault::open_transport(tok, key, raw_val)?;
            *raw_val = plain.as_str().to_owned();
        }
        parsed.insert(key.to_owned(), entry);
    }
    Ok(parsed)
}

fn build_preview_rows(
    db: Db<'_>,
    resolved_config: &Config,
    profile: Option<&Profile>,
    project: Option<&Project>,
    runner: &dyn canban_exec::CommandRunner,
) -> Result<Vec<EnvPreviewRow>> {
    struct RowAcc {
        display_value: String,
        is_secret: bool,
        is_unset: bool,
        configured: bool,
        source_layer: String,
        overridden_layers: Vec<String>,
    }

    let mut tracked: BTreeMap<String, RowAcc> = BTreeMap::new();
    if resolved_config.terminal_env_mode.is_inherit() {
        let snap = runner.shell_env();
        for (k, v) in &snap.vars {
            if canban_exec::shell_env::is_denied_shell_env_key(k) {
                continue;
            }
            let is_sec = canban_types::is_likely_secret_key(k);
            tracked.insert(
                k.clone(),
                RowAcc {
                    display_value: if is_sec {
                        canban_types::secret_hint(v)
                    } else {
                        v.clone()
                    },
                    is_secret: is_sec,
                    is_unset: false,
                    configured: true,
                    source_layer: "Host Login Shell".into(),
                    overridden_layers: Vec::new(),
                },
            );
        }
    } else if let Some(path_val) = if runner.is_system() {
        canban_exec::shell_env::local_merged_path_string(&resolved_config.hc_env)
    } else {
        canban_exec::merged_path_string_from_snapshot(&runner.shell_env(), &resolved_config.hc_env)
    }
    .filter(|s| !s.is_empty())
    {
        tracked.insert(
            "PATH".into(),
            RowAcc {
                display_value: path_val,
                is_secret: false,
                is_unset: false,
                configured: true,
                source_layer: "Host Minimal (PATH)".into(),
                overridden_layers: Vec::new(),
            },
        );
    }

    for layer in collect_env_layers(db, resolved_config, profile, project)? {
        for (k, entry) in layer.meta {
            let prev = tracked.get(&k).map(|existing| {
                let mut list = existing.overridden_layers.clone();
                list.push(existing.source_layer.clone());
                list
            });
            let row = match entry {
                EnvEntryMeta::Plain { value } => RowAcc {
                    display_value: value,
                    is_secret: false,
                    is_unset: false,
                    configured: true,
                    source_layer: layer.label.clone(),
                    overridden_layers: prev.unwrap_or_default(),
                },
                EnvEntryMeta::Secret { configured, hint } => RowAcc {
                    display_value: if !configured {
                        "Missing on this host".into()
                    } else if hint.is_empty() {
                        "••••".into()
                    } else {
                        hint
                    },
                    is_secret: true,
                    is_unset: false,
                    configured,
                    source_layer: layer.label.clone(),
                    overridden_layers: prev.unwrap_or_default(),
                },
                EnvEntryMeta::Unset => RowAcc {
                    display_value: "(unset)".into(),
                    is_secret: false,
                    is_unset: true,
                    configured: true,
                    source_layer: layer.label.clone(),
                    overridden_layers: prev.unwrap_or_default(),
                },
            };
            tracked.insert(k, row);
        }
    }

    Ok(tracked
        .into_iter()
        .map(|(key, r)| EnvPreviewRow {
            key,
            display_value: r.display_value,
            is_secret: r.is_secret,
            is_unset: r.is_unset,
            configured: r.configured,
            source_layer: r.source_layer,
            overridden_layers: r.overridden_layers,
        })
        .collect())
}

impl AppServer {
    pub(super) fn resolve_runtime_env(
        &self,
        profile: Option<&Profile>,
        project: Option<&Project>,
        protected_context: &BTreeMap<String, String>,
        workflow_env: Option<&BTreeMap<String, String>>,
        source_trust: Option<&str>,
    ) -> Result<ResolvedEnv> {
        let resolved_config = self.config();
        let store = self.store.lock().unwrap();
        resolve_effective_env(
            store.db(),
            &self.home,
            &resolved_config,
            profile,
            project,
            protected_context,
            workflow_env,
            source_trust,
        )
    }

    pub(super) fn expected_agent_env_fingerprint_with_db(
        &self,
        db: Db<'_>,
        snapshot: &Snapshot,
        entry: &TerminalEntry,
    ) -> Option<String> {
        let agent = entry.agent.as_ref()?;
        let pending = agent.pending.as_ref()?;
        let project = snapshot.projects.iter().find(|p| p.id == agent.project_id);
        let card = snapshot.cards.iter().find(|c| c.id == agent.card_id);
        let wf_env = pending
            .workflow_run_id
            .as_deref()
            .and_then(|id| snapshot.workflow_runs.iter().find(|r| r.id == id))
            .map(|run| run.custom_env_for_step(pending.workflow_step.as_deref()));
        let trust = card.map(Card::trust_level);
        let resolved_config = self.config();
        resolve_effective_env(
            db,
            &self.home,
            &resolved_config,
            Some(&pending.profile),
            project,
            &BTreeMap::new(),
            wf_env.as_ref(),
            trust,
        )
        .ok()
        .map(|r| r.fingerprint)
    }

    pub(super) fn launch_herdr_with_env(
        &self,
        req: HerdrLaunchRequest<'_>,
    ) -> Result<HerdrLaunchOutcome> {
        let resolved = self.resolve_runtime_env(
            req.profile,
            req.project,
            req.protected_context,
            req.workflow_env,
            req.source_trust,
        )?;
        let cfg = self.config();
        let spec = canban_profiles::prepare_launch(canban_profiles::LaunchRequest {
            profile: req.profile,
            argv: req.argv,
            env: resolved.env.clone(),
            cwd: req.cwd,
        })?;
        let env_policy = canban_exec::shell_env::LocalEnvPolicy::for_source_trust(
            cfg.terminal_env_mode.is_inherit(),
            req.source_trust,
        );
        let launch_size = self.resolve_launch_terminal_size(req.size);
        let receipt = self
            .herdr_client_with_config(cfg)
            .launch_with_policy_unset_and_size(
                spec.argv,
                &spec.cwd,
                spec.env,
                req.labels,
                env_policy,
                &resolved.unset_keys,
                Some(launch_size),
            )?;
        if let Err(err) = self.check_failpoint(canban_types::FP_HERDR_AFTER_TERMINAL_RUN_BEFORE_DB)
        {
            return Err(crate::LaunchReceiptUncertain(err).into());
        }
        Ok(HerdrLaunchOutcome { receipt, resolved })
    }

    pub(super) fn env_save(&self, p: Value, transport_token: Option<&str>) -> Result<Value> {
        let scope = strp(&p, "scope")?.trim();
        ensure!(
            matches!(
                scope,
                env_store::SCOPE_GLOBAL | env_store::SCOPE_AGENT | env_store::SCOPE_PROJECT
            ),
            "Unsupported env scope '{scope}'"
        );
        let raw_target = p["target_id"].as_str().map(str::trim).unwrap_or("");
        let entries = decrypt_incoming_write_entries(&self.home, &p["entries"], transport_token)?;
        let now_ts = self.now_ms();
        let config = self.config();
        let fs_sync = self.fs_durability == canban_context::FsDurability::Full;
        let vault_id_gen = self.vault_id_gen();
        if entries
            .values()
            .any(|e| matches!(e, EnvWriteEntry::Secret { value: Some(_), .. }))
        {
            let _ = vault::load_or_create_master_key_with_sync_and_id_gen(
                &self.home,
                fs_sync,
                vault_id_gen,
            )?;
        }
        let (target_id, snapshot_key, meta) = self.store.lock().unwrap().write(|db| {
            let (tid, snap_key) = match scope {
                env_store::SCOPE_GLOBAL => (String::new(), "global".to_owned()),
                env_store::SCOPE_AGENT => {
                    let pid = raw_target.trim_start_matches('@');
                    ensure!(
                        !pid.is_empty() && config.profiles.iter().any(|prof| prof.id == pid),
                        "Unknown agent or identity @{pid}"
                    );
                    (pid.to_owned(), Snapshot::agent_env_key("local", pid))
                }
                env_store::SCOPE_PROJECT => {
                    let pid = super::resolve_project_id(db, raw_target)?;
                    (pid.clone(), Snapshot::project_env_key(&pid))
                }
                _ => unreachable!(),
            };
            let meta = env_store::apply_env_writes_with_options(
                db.conn,
                &self.home,
                scope,
                &tid,
                &entries,
                now_ts,
                fs_sync,
                vault_id_gen,
            )?;
            Ok((tid, snap_key, meta))
        })?;
        if scope != env_store::SCOPE_PROJECT {
            self.emit("configuration", None);
        }
        self.emit("application", None);
        Ok(json!({
            "scope": scope,
            "target_id": target_id,
            "snapshot_key": snapshot_key,
            "env": meta,
        }))
    }

    pub(super) fn global_env_sync(&self, p: Value, transport_token: Option<&str>) -> Result<Value> {
        let wire: Vec<env_store::GlobalEnvSyncWireRecord> =
            serde_json::from_value(p["records"].clone())
                .context("Invalid global env sync payload")?;
        let active_tok = transport_token.map(str::trim).filter(|s| !s.is_empty());
        let fallback_tok = active_tok
            .is_none()
            .then(|| network::read_server_token(&self.home))
            .flatten();
        let tok = active_tok.or(fallback_tok.as_deref());
        let mut records = Vec::with_capacity(wire.len());
        for w in wire {
            records.push(env_store::GlobalEnvSyncRecord::from_wire(w, tok)?);
        }
        let fs_sync = self.fs_durability == canban_context::FsDurability::Full;
        let vault_id_gen = self.vault_id_gen();
        if records.iter().any(|r| !r.deleted && r.kind == "secret") {
            let _ = vault::load_or_create_master_key_with_sync_and_id_gen(
                &self.home,
                fs_sync,
                vault_id_gen,
            )?;
        }
        let applied = self.store.lock().unwrap().write(|db| {
            env_store::apply_synced_global_env_with_options(
                db.conn,
                &self.home,
                &records,
                fs_sync,
                vault_id_gen,
            )
        })?;
        let meta = {
            let store = self.store.lock().unwrap();
            env_store::list_env_meta(store.db().conn, env_store::SCOPE_GLOBAL, "")?
        };
        if applied > 0 {
            self.emit("configuration", None);
            self.emit("application", None);
        }
        Ok(json!({ "applied": applied, "global_env": meta }))
    }

    pub(super) fn env_preview(&self, p: Value) -> Result<Value> {
        let resolved_config = self.config();
        let project_id = p["project_id"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let profile_id = p["profile_id"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_start_matches('@'));
        let store = self.store.lock().unwrap();
        let db = store.db();
        let project = if let Some(pid) = project_id {
            let resolved_pid = super::resolve_project_id(db, pid)?;
            Some(db.get::<Project>(&resolved_pid)?)
        } else {
            None
        };
        let profile = profile_id.and_then(|pid| {
            resolved_config
                .profiles
                .iter()
                .find(|prof| prof.id == pid)
                .cloned()
        });
        let rows = build_preview_rows(
            db,
            &resolved_config,
            profile.as_ref(),
            project.as_ref(),
            self.exec_runner.as_ref(),
        )?;
        Ok(json!({ "rows": rows }))
    }

    pub(super) fn host_env_keys(&self) -> Result<Value> {
        let snap = self.exec_runner.shell_env();
        let keys: Vec<Value> = snap
            .vars
            .iter()
            .filter(|(k, _)| !canban_exec::shell_env::is_denied_shell_env_key(k))
            .map(|(k, v)| {
                let is_sec = canban_types::is_likely_secret_key(k);
                json!({
                    "key": k,
                    "is_secret": is_sec,
                    "preview": if is_sec { canban_types::secret_hint(v) } else { v.clone() },
                })
            })
            .collect();
        Ok(json!({
            "shell": snap.shell,
            "keys": keys,
        }))
    }
}
