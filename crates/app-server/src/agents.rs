//! Herdr-backed Agent catalog probing, Onboarding quick setup, Global Identity &
//! Agent management, and Project-level identity backend overrides.
use super::*;
use canban_profiles::ModelOption;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

pub(super) fn parse_codex_cached_models_json(
    val: &Value,
    defaults: &[ModelOption],
) -> Vec<ModelOption> {
    let mut out = Vec::new();
    if let Some(models) = val["models"].as_array() {
        for m in models {
            if m["visibility"].as_str() == Some("hide") {
                continue;
            }
            let Some(slug) = m["slug"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
                continue;
            };
            let label = m["display_name"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(slug);
            let supported_efforts = m["supported_reasoning_levels"].as_array().map(|arr| {
                let mut levels = Vec::new();
                for item in arr {
                    if let Some(eff) = item["effort"]
                        .as_str()
                        .or_else(|| item.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        && !levels.iter().any(|existing: &String| existing == eff)
                    {
                        levels.push(eff.to_owned());
                    }
                }
                levels
            });
            if !out.iter().any(|x: &ModelOption| x.id == slug) {
                out.push(ModelOption {
                    id: slug.to_owned(),
                    label: label.to_owned(),
                    supported_efforts,
                    effort_labels: BTreeMap::new(),
                });
            }
        }
    }
    if out.is_empty() {
        defaults.to_vec()
    } else {
        out
    }
}

fn probe_codex_cached_models(defaults: &[ModelOption]) -> Vec<ModelOption> {
    if let Some(home) = std::env::var_os("HOME") {
        let cache_path = std::path::PathBuf::from(home).join(".codex/models_cache.json");
        if let Ok(bytes) = std::fs::read(&cache_path)
            && let Ok(val) = serde_json::from_slice::<Value>(&bytes)
        {
            return parse_codex_cached_models_json(&val, defaults);
        }
    }
    defaults.to_vec()
}

pub(super) fn parse_agy_models_output(text: &str) -> Vec<ModelOption> {
    let mut parsed = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("Fetching ") {
            continue;
        }
        if let Some((id, label)) = trimmed.split_once('\t') {
            let id = id.trim();
            let label = label.trim();
            if !id.is_empty() && !parsed.iter().any(|m: &ModelOption| m.id == id) {
                parsed.push(ModelOption {
                    id: id.to_owned(),
                    label: if label.is_empty() {
                        id.to_owned()
                    } else {
                        label.to_owned()
                    },
                    supported_efforts: None,
                    effort_labels: BTreeMap::new(),
                });
            }
        }
    }
    canban_profiles::collapse_agy_models(parsed)
}

pub(super) const AGENT_MODEL_PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const AGENT_MODEL_CACHE_TTL: Duration = Duration::from_secs(300);

type ModelCacheMap = BTreeMap<String, (Instant, Vec<ModelOption>)>;

fn model_probe_cache() -> &'static Mutex<ModelCacheMap> {
    static CACHE: OnceLock<Mutex<ModelCacheMap>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
pub(super) fn probe_cli_models_with_timeout(
    cache_key: &str,
    program: &str,
    args: &[&str],
    timeout: Duration,
    defaults: &[ModelOption],
    parser: fn(&str) -> Vec<ModelOption>,
) -> Vec<ModelOption> {
    probe_cli_models_with_timeout_hook_and_env(
        cache_key,
        program,
        args,
        timeout,
        defaults,
        parser,
        None,
        &BTreeMap::new(),
        &BTreeSet::new(),
    )
}

#[allow(clippy::too_many_arguments)]
fn probe_cli_models_with_timeout_hook_and_env(
    cache_key: &str,
    program: &str,
    args: &[&str],
    timeout: Duration,
    defaults: &[ModelOption],
    parser: fn(&str) -> Vec<ModelOption>,
    on_live: Option<fn(&[ModelOption])>,
    extra_env: &BTreeMap<String, String>,
    unset_keys: &BTreeSet<String>,
) -> Vec<ModelOption> {
    let cache = model_probe_cache();
    if let Ok(guard) = cache.lock()
        && let Some((ts, cached)) = guard.get(cache_key)
        && ts.elapsed() < AGENT_MODEL_CACHE_TTL
        && !cached.is_empty()
    {
        if let Some(hook) = on_live {
            hook(cached);
        }
        return cached.clone();
    }

    let key_owned = cache_key.to_owned();
    let program_owned = program.to_owned();
    let args_owned: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    let extra_env_owned = extra_env.clone();
    let unset_keys_owned = unset_keys.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let run_probe = move || {
        let mut cmd = canban_exec::command(&program_owned);
        for key in &unset_keys_owned {
            cmd.env_remove(key);
        }
        if !extra_env_owned.is_empty() {
            cmd.envs(&extra_env_owned);
        }
        cmd.args(&args_owned)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let parsed = match cmd.output() {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout);
                parser(&text)
            }
            _ => Vec::new(),
        };
        if !parsed.is_empty() {
            if let Some(hook) = on_live {
                hook(&parsed);
            }
            if let Ok(mut guard) = model_probe_cache().lock() {
                guard.insert(key_owned, (Instant::now(), parsed.clone()));
            }
        }
        let _ = tx.send(parsed);
    };
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn_blocking(run_probe);
    } else {
        std::thread::spawn(run_probe);
    }

    if let Ok(parsed) = rx.recv_timeout(timeout)
        && !parsed.is_empty()
    {
        return parsed;
    }

    defaults.to_vec()
}

fn probe_agy_models(
    agy_path: &str,
    defaults: &[ModelOption],
    env_fingerprint: &str,
    extra_env: &BTreeMap<String, String>,
    unset_keys: &BTreeSet<String>,
) -> Vec<ModelOption> {
    let cache_key = format!("agy:{agy_path}:{env_fingerprint}");
    probe_cli_models_with_timeout_hook_and_env(
        &cache_key,
        agy_path,
        &["models"],
        AGENT_MODEL_PROBE_TIMEOUT,
        defaults,
        parse_agy_models_output,
        Some(canban_profiles::update_probed_agy_models),
        extra_env,
        unset_keys,
    )
}

fn models_for_entry(
    entry: &canban_profiles::CatalogEntry,
    installed_path: Option<&str>,
    env_fingerprint: &str,
    extra_env: &BTreeMap<String, String>,
    unset_keys: &BTreeSet<String>,
) -> Vec<ModelOption> {
    match entry.id.as_str() {
        "agy" => {
            if let Some(path) = installed_path {
                return probe_agy_models(
                    path,
                    &entry.default_models,
                    env_fingerprint,
                    extra_env,
                    unset_keys,
                );
            }
        }
        "codex" => {
            return probe_codex_cached_models(&entry.default_models);
        }
        _ => {}
    }
    entry.default_models.clone()
}

fn validate_profile_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 64
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "Agent/Identity ID '{id}' must use 1..64 ASCII letters, digits, '-' or '_'"
    );
    Ok(())
}

fn default_role_instruction(id: &str) -> &'static str {
    match id {
        "planner" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../prompts/profiles/role_planner.txt"
        ))
        .trim_end(),
        "developer" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../prompts/profiles/role_developer.txt"
        ))
        .trim_end(),
        "reviewer" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../prompts/profiles/role_reviewer.txt"
        ))
        .trim_end(),
        "merger" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../prompts/profiles/role_merger.txt"
        ))
        .trim_end(),
        _ => "",
    }
}

fn parse_orchestrator_param(val: &Value) -> Result<Option<canban_types::OrchestratorConfig>> {
    if val.is_null() {
        return Ok(None);
    }
    let parsed: canban_types::OrchestratorConfig =
        serde_json::from_value(val.clone()).context("Invalid orchestrator configuration")?;
    Ok(parsed.into_non_empty())
}

fn profile_supports_orchestrator(profiles: &[Profile], prof: &Profile) -> bool {
    prof.is_agy()
        || prof.preset.as_deref().is_some_and(|preset| {
            profiles
                .iter()
                .any(|p| p.id == preset && !p.is_identity() && p.is_agy())
        })
}

impl AppServer {
    fn probe_models_for_profile(
        &self,
        entry: &canban_profiles::CatalogEntry,
        installed_path: Option<&str>,
        profile: Option<&Profile>,
    ) -> Vec<ModelOption> {
        let resolved_probe_env = self
            .resolve_runtime_env(profile, None, &BTreeMap::new(), None, None)
            .ok();
        let empty_map = BTreeMap::new();
        let empty_set = BTreeSet::new();
        let (probe_fp, probe_env, probe_unset) = match resolved_probe_env.as_ref() {
            Some(r) => (r.fingerprint.as_str(), &r.env, &r.unset_keys),
            None => ("", &empty_map, &empty_set),
        };
        models_for_entry(entry, installed_path, probe_fp, probe_env, probe_unset)
    }

    pub(super) fn agent_catalog(&self, _p: Value) -> Result<Value> {
        let config = self.config();
        let catalog = canban_profiles::builtin_catalog();
        let mut bins: Vec<&str> = catalog.iter().map(|e| e.binary.as_str()).collect();
        for prof in &config.profiles {
            if !prof.is_identity()
                && let Some(cmd) = prof.command().first()
                && !bins.contains(&cmd.as_str())
            {
                bins.push(cmd.as_str());
            }
        }
        canban_herdr::refresh_local_shell_path_if_stale(Duration::from_secs(2));
        let host = canban_herdr::host::Host::new(config.clone());
        let (probed, probe_error) = match host.probe_binaries(&bins) {
            Ok(map) => (map, None),
            Err(e) => (BTreeMap::new(), Some(format!("{e:#}"))),
        };

        let herdr_templates = canban_herdr::Client {
            config: config.clone(),
        }
        .call("templates.list", json!({}))
        .ok()
        .and_then(|v| {
            v["templates"]
                .as_array()
                .cloned()
                .or_else(|| v.as_array().cloned())
        })
        .unwrap_or_default();

        let presets = canban_profiles::presets(&self.home).unwrap_or_default();
        let builtin_ids: BTreeSet<String> = catalog.iter().map(|e| e.id.clone()).collect();
        let raw_config = self
            .load_unmigrated_config()
            .unwrap_or_else(|_| config.clone());

        let mut agents = Vec::new();
        for entry in &catalog {
            let existing = config
                .profiles
                .iter()
                .find(|prof| prof.id == entry.id && !prof.is_identity());
            let effective_bin = existing
                .and_then(|p| p.command().first().map(String::as_str))
                .unwrap_or(entry.binary.as_str());
            let installed_path = probed
                .get(effective_bin)
                .or_else(|| probed.get(&entry.binary))
                .cloned();
            let probe_for_models = probed
                .get(effective_bin)
                .or_else(|| probed.get(&entry.binary))
                .map(String::as_str);
            let models = self.probe_models_for_profile(entry, probe_for_models, existing);
            agents.push(json!({
                "id": entry.id,
                "display_name": existing.and_then(|p| p.name.clone()).unwrap_or_else(|| entry.display_name.clone()),
                "binary": effective_bin,
                "default_argv": entry.argv,
                "kind": "builtin",
                "resumable": entry.resumable,
                "installed": probed.contains_key(effective_bin),
                "path": installed_path,
                "configured": existing.is_some(),
                "enabled": existing.is_some_and(|p| p.enabled),
                "models": models,
                "supported_efforts": entry.supported_efforts,
                "supports_model": !entry.model_args.is_empty(),
                "supports_effort": !entry.effort_args.is_empty(),
                "supports_orchestrator": entry.supports_orchestrator,
            }));
        }

        // User-defined preset files (`config/presets/*.json`) not in builtin 24
        for (preset_id, preset_prof) in &presets {
            if builtin_ids.contains(preset_id) {
                continue;
            }
            let existing = config
                .profiles
                .iter()
                .find(|prof| prof.id == *preset_id && !prof.is_identity());
            let bin = existing
                .and_then(|p| p.command().first().cloned())
                .or_else(|| preset_prof.command().first().cloned())
                .unwrap_or_else(|| preset_id.clone());
            let installed_path = probed.get(&bin).cloned();
            let meta = canban_profiles::catalog_entry_for_profile(preset_prof);
            let models = meta
                .as_ref()
                .map(|m| self.probe_models_for_profile(m, installed_path.as_deref(), existing))
                .unwrap_or_default();
            let supported_efforts = meta
                .as_ref()
                .map(|m| m.supported_efforts.clone())
                .unwrap_or_default();
            let supports_effort = meta.as_ref().is_some_and(|m| !m.effort_args.is_empty())
                || existing
                    .unwrap_or(preset_prof)
                    .command()
                    .iter()
                    .any(|a| a.contains("{effort}"));
            let supports_orchestrator = meta.as_ref().is_some_and(|m| m.supports_orchestrator);
            agents.push(json!({
                "id": preset_id,
                "display_name": existing
                    .and_then(|p| p.name.clone())
                    .or_else(|| preset_prof.name.clone())
                    .unwrap_or_else(|| preset_id.clone()),
                "binary": bin,
                "default_argv": preset_prof.command(),
                "kind": "preset",
                "resumable": !existing.unwrap_or(preset_prof).resume_command().is_empty(),
                "installed": installed_path.is_some(),
                "path": installed_path,
                "configured": existing.is_some(),
                "enabled": existing.is_some_and(|p| p.enabled),
                "models": models,
                "supported_efforts": supported_efforts,
                "supports_model": true,
                "supports_effort": supports_effort,
                "supports_orchestrator": supports_orchestrator,
            }));
        }

        // Custom CLI direct profiles in `agents.json`
        for prof in &config.profiles {
            if prof.is_identity()
                || builtin_ids.contains(&prof.id)
                || presets.contains_key(&prof.id)
            {
                continue;
            }
            let bin = prof
                .command()
                .first()
                .cloned()
                .unwrap_or_else(|| prof.id.clone());
            let is_herdr_tpl = prof
                .preset
                .as_deref()
                .is_some_and(|s| s.starts_with("herdr:"));
            let installed_path = if is_herdr_tpl {
                Some(format!("herdr template ({bin})"))
            } else {
                probed.get(&bin).cloned()
            };
            let meta = canban_profiles::catalog_entry_for_profile(prof);
            let models = meta
                .as_ref()
                .map(|m| self.probe_models_for_profile(m, installed_path.as_deref(), Some(prof)))
                .unwrap_or_default();
            let supported_efforts = meta
                .as_ref()
                .map(|m| m.supported_efforts.clone())
                .unwrap_or_default();
            let supports_effort = !is_herdr_tpl
                && (meta.as_ref().is_some_and(|m| !m.effort_args.is_empty())
                    || prof.command().iter().any(|a| a.contains("{effort}")));
            let supports_orchestrator =
                !is_herdr_tpl && meta.as_ref().is_some_and(|m| m.supports_orchestrator);
            let default_argv = if let Some(tpl) = prof
                .preset
                .as_deref()
                .and_then(|s| s.strip_prefix("herdr:"))
            {
                vec![tpl.to_owned(), "{prompt}".into()]
            } else if let Some(pn) = prof.preset.as_deref()
                && let Some(pp) = presets.get(pn)
            {
                pp.command().to_vec()
            } else {
                Vec::new()
            };
            agents.push(json!({
                "id": prof.id,
                "display_name": prof.name.clone().unwrap_or_else(|| prof.id.clone()),
                "binary": bin,
                "default_argv": default_argv,
                "kind": if is_herdr_tpl { "herdr_template" } else { "custom" },
                "resumable": !prof.resume_command().is_empty(),
                "installed": installed_path.is_some(),
                "path": installed_path,
                "configured": true,
                "enabled": prof.enabled,
                "models": models,
                "supported_efforts": supported_efforts,
                "supports_model": !is_herdr_tpl,
                "supports_effort": supports_effort,
                "supports_orchestrator": supports_orchestrator,
            }));
        }

        Ok(json!({
            "probe_error": probe_error,
            "agents": agents,
            "herdr_templates": herdr_templates,
            "config": config,
            "raw_profiles": raw_config.profiles,
        }))
    }

    fn save_raw_config(&self, mut raw: Config) -> Result<Config> {
        let resolved = canban_profiles::resolve(raw.clone(), &self.home)?;
        Self::validate_config(&resolved)?;
        self.validate_references(&resolved)?;
        // Persist concise `raw` JSON while publishing resolved in-memory `Config`.
        for prof in &mut raw.profiles {
            if prof.preset.is_some()
                && prof.is_identity()
                && prof.argv.is_none()
                && prof.resume_argv.is_none()
            {
                prof.exit = None;
                prof.session_instruction = None;
            }
        }
        let path = self.home.join("config/agents.json");
        canban_context::atomic(&path, &serde_json::to_vec_pretty(&raw)?)?;
        *self.config.write().unwrap() = resolved.clone();
        self.emit("configuration", None);
        Ok(resolved)
    }

    pub(super) fn load_unmigrated_config(&self) -> Result<Config> {
        let path = self.home.join("config/agents.json");
        let bytes = std::fs::read(&path)?;
        let mut value: Value = serde_json::from_slice(&bytes)?;
        let _ = canban_profiles::migrate(&mut value)?;
        Ok(serde_json::from_value(value)?)
    }

    /// Onboarding / Re-run Agent Setup wizard (`agent.setup`).
    /// Atomically updates enabled direct agents, assigns backing engines to
    /// `@planner`, `@developer`, `@reviewer` (creating missing built-in identities),
    /// preserves any user custom identities and custom role instructions, and marks `onboarded = true`.
    pub(super) fn agent_setup(&self, p: Value) -> Result<Value> {
        let mut raw = self.load_unmigrated_config()?;
        let enabled_ids: BTreeSet<String> = strings(&p, "enabled_agents").into_iter().collect();
        ensure!(
            !enabled_ids.is_empty(),
            "Select at least one enabled Agent to continue"
        );
        let default_engine = p["default_engine"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| enabled_ids.iter().next().cloned().unwrap());
        ensure!(
            enabled_ids.contains(&default_engine),
            "Primary Agent '{default_engine}' must be enabled"
        );

        let identity_engines: BTreeMap<String, String> = p["identity_engines"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.trim().to_owned())))
                    .filter(|(_, v)| !v.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // 1. Update existing non-identity profiles' `enabled` flag.
        for prof in &mut raw.profiles {
            if !prof.is_identity() {
                prof.enabled = enabled_ids.contains(&prof.id);
            }
        }

        // 2. Ensure every newly enabled agent exists in `raw.profiles`.
        let catalog = canban_profiles::builtin_catalog();
        let presets = canban_profiles::presets(&self.home).unwrap_or_default();
        for agent_id in &enabled_ids {
            validate_profile_id(agent_id)?;
            if let Some(existing) = raw
                .profiles
                .iter_mut()
                .find(|prof| prof.id == *agent_id && !prof.is_identity())
            {
                existing.enabled = true;
                continue;
            }
            if raw
                .profiles
                .iter()
                .any(|prof| prof.id == *agent_id && prof.is_identity())
            {
                bail!("@{agent_id} is already used as a role identity");
            }
            if let Some(entry) = catalog.iter().find(|e| e.id == *agent_id) {
                raw.profiles.push(Profile {
                    id: entry.id.clone(),
                    name: Some(entry.display_name.clone()),
                    enabled: true,
                    identity: false,
                    preset: Some(entry.id.clone()),
                    ..Default::default()
                });
            } else if presets.contains_key(agent_id) {
                raw.profiles.push(Profile {
                    id: agent_id.clone(),
                    enabled: true,
                    identity: false,
                    preset: Some(agent_id.clone()),
                    ..Default::default()
                });
            } else if let Some(tpl) = agent_id.strip_prefix("herdr-") {
                raw.profiles
                    .push(canban_profiles::herdr_template_profile(tpl));
            } else {
                bail!("Unknown Agent '{agent_id}'");
            }
        }

        // 3. Ensure built-in semantic identities (`developer`, `reviewer`, `planner`, `merger`) exist and use selected engines.
        for role_id in ["developer", "reviewer", "planner", "merger"] {
            let chosen_engine = identity_engines
                .get(role_id)
                .cloned()
                .unwrap_or_else(|| default_engine.clone());
            ensure!(
                enabled_ids.contains(&chosen_engine),
                "Identity @{role_id} cannot use disabled Agent '{chosen_engine}'"
            );
            let preset_ref = raw
                .profiles
                .iter()
                .find(|prof| prof.id == chosen_engine && !prof.is_identity())
                .and_then(|prof| prof.preset.clone())
                .unwrap_or(chosen_engine);
            if let Some(existing) = raw.profiles.iter_mut().find(|prof| prof.id == role_id) {
                let engine_changed = existing.preset.as_deref() != Some(preset_ref.as_str());
                existing.identity = true;
                existing.enabled = true;
                existing.preset = Some(preset_ref);
                existing.argv = None;
                existing.resume_argv = None;
                existing.exit = None;
                existing.session_instruction = None;
                if engine_changed {
                    existing.model = None;
                    existing.effort = None;
                    existing.orchestrator = None;
                }
                if existing.role_instruction.trim().is_empty() {
                    existing.role_instruction = default_role_instruction(role_id).into();
                }
            } else {
                raw.profiles.push(Profile {
                    id: role_id.into(),
                    name: None,
                    enabled: true,
                    identity: true,
                    preset: Some(preset_ref),
                    role_instruction: default_role_instruction(role_id).into(),
                    ..Default::default()
                });
            }
        }

        // Also update any custom identities explicitly listed in `identity_engines`.
        for (role_id, engine_id) in &identity_engines {
            if matches!(
                role_id.as_str(),
                "developer" | "reviewer" | "planner" | "merger"
            ) {
                continue;
            }
            let preset_ref = raw
                .profiles
                .iter()
                .find(|prof| prof.id == *engine_id && !prof.is_identity())
                .and_then(|prof| prof.preset.clone())
                .unwrap_or_else(|| engine_id.clone());
            if let Some(existing) = raw
                .profiles
                .iter_mut()
                .find(|prof| prof.id == *role_id && prof.is_identity())
            {
                ensure!(
                    enabled_ids.contains(engine_id),
                    "Identity @{role_id} cannot use disabled Agent '{engine_id}'"
                );
                let engine_changed = existing.preset.as_deref() != Some(preset_ref.as_str());
                existing.preset = Some(preset_ref);
                existing.argv = None;
                existing.resume_argv = None;
                existing.exit = None;
                existing.session_instruction = None;
                if engine_changed {
                    existing.model = None;
                    existing.effort = None;
                    existing.orchestrator = None;
                }
            }
        }

        if let Some(def) = p["default_agent"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            raw.default_agent = def.to_owned();
        } else if !raw.onboarded
            || !raw
                .profiles
                .iter()
                .any(|prof| prof.id == raw.default_agent && prof.enabled && prof.is_identity())
        {
            raw.default_agent = "developer".into();
        }
        raw.onboarded = true;

        let resolved = self.save_raw_config(raw)?;
        Ok(serde_json::to_value(resolved)?)
    }

    /// Fine-grained Global Agent & Identity mutations (`config.agents.save`).
    pub(super) fn save_agents_config(&self, p: Value) -> Result<Value> {
        let mut raw = self.load_unmigrated_config()?;
        let op = p["op"].as_str().unwrap_or("save");
        match op {
            "set_terminal_env_mode" => {
                let raw_mode = strp(&p, "terminal_env_mode")?.trim();
                let mode = HostTerminalEnvMode::parse(raw_mode)
                    .with_context(|| format!("Invalid terminal_env_mode '{raw_mode}'"))?;
                raw.terminal_env_mode = mode;
            }
            "set_default" => {
                let target = strp(&p, "default_agent")?.trim();
                let prof = raw
                    .profiles
                    .iter()
                    .find(|x| x.id == target)
                    .with_context(|| format!("Unknown profile @{target}"))?;
                ensure!(
                    prof.enabled,
                    "Enable @{target} before setting it as default"
                );
                raw.default_agent = target.to_owned();
            }
            "toggle_agent" => {
                let target = strp(&p, "id")?.trim();
                validate_profile_id(target)?;
                let enabled = p["enabled"].as_bool().unwrap_or(true);
                if let Some(existing) = raw.profiles.iter_mut().find(|x| x.id == target) {
                    existing.enabled = enabled;
                } else if enabled {
                    let catalog = canban_profiles::builtin_catalog();
                    let presets = canban_profiles::presets(&self.home).unwrap_or_default();
                    if let Some(entry) = catalog.iter().find(|e| e.id == target) {
                        raw.profiles.push(Profile {
                            id: entry.id.clone(),
                            name: Some(entry.display_name.clone()),
                            enabled: true,
                            identity: false,
                            preset: Some(entry.id.clone()),
                            ..Default::default()
                        });
                    } else if presets.contains_key(target) {
                        raw.profiles.push(Profile {
                            id: target.to_owned(),
                            enabled: true,
                            identity: false,
                            preset: Some(target.to_owned()),
                            ..Default::default()
                        });
                    } else if let Some(tpl) = target.strip_prefix("herdr-") {
                        raw.profiles
                            .push(canban_profiles::herdr_template_profile(tpl));
                    } else {
                        bail!("Unknown Agent '{target}'");
                    }
                }
                if !raw
                    .profiles
                    .iter()
                    .any(|x| x.id == raw.default_agent && x.enabled)
                    && let Some(fallback) = raw
                        .profiles
                        .iter()
                        .find(|x| x.id == "developer" && x.enabled)
                        .or_else(|| raw.profiles.iter().find(|x| x.enabled))
                {
                    raw.default_agent = fallback.id.clone();
                }
            }
            "upsert_custom_agent" => {
                let id = strp(&p, "id")?.trim();
                validate_profile_id(id)?;
                if raw.profiles.iter().any(|x| x.id == id && x.is_identity()) {
                    bail!("@{id} is already used as a role identity");
                }
                let argv = strings(&p, "argv");
                let template = p["herdr_template"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let (preset, final_argv) = if let Some(tpl) = template {
                    (
                        Some(format!("herdr:{tpl}")),
                        Some(vec![tpl.to_owned(), "{prompt}".into()]),
                    )
                } else {
                    ensure!(!argv.is_empty(), "Launch command (argv) is required");
                    (None, Some(argv))
                };
                let resume_argv = strings(&p, "resume_argv");
                let display = p["name"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                let existing_opts = raw
                    .profiles
                    .iter()
                    .find(|x| x.id == id)
                    .map(|x| (x.model.clone(), x.effort.clone(), x.orchestrator.clone()));
                let next_model = if p.get("model").is_some() {
                    p["model"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                } else {
                    existing_opts.as_ref().and_then(|(m, _, _)| m.clone())
                };
                let next_effort = if p.get("effort").is_some() {
                    p["effort"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                } else {
                    existing_opts.as_ref().and_then(|(_, e, _)| e.clone())
                };
                let parsed_orch = p
                    .get("orchestrator")
                    .map(parse_orchestrator_param)
                    .transpose()?;
                let next_orch = parsed_orch
                    .clone()
                    .unwrap_or_else(|| existing_opts.and_then(|(_, _, o)| o));
                let mut new_prof = Profile {
                    id: id.to_owned(),
                    name: display,
                    enabled: p["enabled"].as_bool().unwrap_or(true),
                    identity: false,
                    preset,
                    model: next_model,
                    effort: next_effort,
                    orchestrator: next_orch,
                    role_instruction: String::new(),
                    argv: final_argv,
                    resume_argv: (!resume_argv.is_empty()).then_some(resume_argv),
                    exit: Some(ExitAction::Manual),
                    session_instruction: None,
                };
                if new_prof.is_agy() {
                    if let Some(m) = new_prof.model.as_deref()
                        && canban_types::is_orchestrator_model_id(m)
                    {
                        if let Some(legacy) = canban_types::legacy_agy_orchestrator_preset(m)
                            && parsed_orch.is_none()
                            && new_prof.orchestrator.is_none()
                        {
                            new_prof.orchestrator = Some(legacy);
                        }
                        new_prof.model = Some(canban_types::ORCHESTRATOR_MODEL_ID.to_owned());
                        new_prof.effort = None;
                    } else if parsed_orch.as_ref().is_some_and(Option::is_some) {
                        new_prof.model = Some(canban_types::ORCHESTRATOR_MODEL_ID.to_owned());
                        new_prof.effort = None;
                    } else {
                        new_prof.orchestrator = None;
                    }
                } else {
                    if new_prof
                        .model
                        .as_deref()
                        .is_some_and(canban_types::is_orchestrator_model_id)
                        || parsed_orch.as_ref().is_some_and(Option::is_some)
                    {
                        bail!("Orchestrator mode is only supported by Antigravity (agy)");
                    }
                    new_prof.orchestrator = None;
                }
                if let Some(existing) = raw.profiles.iter_mut().find(|x| x.id == id) {
                    *existing = new_prof;
                } else {
                    raw.profiles.push(new_prof);
                }
            }
            "set_profile_options" => {
                let id = strp(&p, "id")?.trim().trim_start_matches('@');
                validate_profile_id(id)?;
                let catalog = canban_profiles::builtin_catalog();
                let presets = canban_profiles::presets(&self.home).unwrap_or_default();
                if !raw.profiles.iter().any(|x| x.id == id) {
                    if let Some(entry) = catalog.iter().find(|e| e.id == id) {
                        raw.profiles.push(Profile {
                            id: entry.id.clone(),
                            name: Some(entry.display_name.clone()),
                            enabled: true,
                            identity: false,
                            preset: Some(entry.id.clone()),
                            ..Default::default()
                        });
                    } else if presets.contains_key(id) {
                        raw.profiles.push(Profile {
                            id: id.to_owned(),
                            enabled: true,
                            identity: false,
                            preset: Some(id.to_owned()),
                            ..Default::default()
                        });
                    } else {
                        bail!("Unknown profile @{id}");
                    }
                }
                let supports_orch = raw
                    .profiles
                    .iter()
                    .find(|x| x.id == id)
                    .is_some_and(|x| profile_supports_orchestrator(&raw.profiles, x));
                let parsed_orch_param = p
                    .get("orchestrator")
                    .map(parse_orchestrator_param)
                    .transpose()?;
                let prof = raw
                    .profiles
                    .iter_mut()
                    .find(|x| x.id == id)
                    .with_context(|| format!("Unknown profile @{id}"))?;
                if p.get("model").is_some() {
                    let next_model = p["model"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    if let Some(m) = next_model.as_deref()
                        && canban_types::is_orchestrator_model_id(m)
                    {
                        ensure!(
                            supports_orch,
                            "Orchestrator mode is only supported by Antigravity (agy)"
                        );
                        if let Some(legacy) = canban_types::legacy_agy_orchestrator_preset(m)
                            && parsed_orch_param.is_none()
                            && prof.orchestrator.is_none()
                        {
                            prof.orchestrator = Some(legacy);
                        }
                        prof.model = Some(canban_types::ORCHESTRATOR_MODEL_ID.to_owned());
                        prof.effort = None;
                    } else {
                        prof.model = next_model;
                        if parsed_orch_param.is_none() {
                            prof.orchestrator = None;
                        }
                    }
                }
                if p.get("effort").is_some() {
                    if prof
                        .model
                        .as_deref()
                        .is_some_and(canban_types::is_orchestrator_model_id)
                    {
                        prof.effort = None;
                    } else {
                        prof.effort = p["effort"]
                            .as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned);
                    }
                }
                if let Some(opt_orch) = parsed_orch_param {
                    match opt_orch {
                        Some(orch) => {
                            ensure!(
                                supports_orch,
                                "Orchestrator mode is only supported by Antigravity (agy)"
                            );
                            if !prof.is_identity() && prof.model.is_none() {
                                prof.model = Some(canban_types::ORCHESTRATOR_MODEL_ID.to_owned());
                            }
                            prof.orchestrator = Some(orch);
                            prof.effort = None;
                        }
                        None => {
                            prof.orchestrator = None;
                        }
                    }
                }
                if let Some(argv_val) = p.get("argv") {
                    let preset_default_argv: Option<Vec<String>> =
                        prof.preset.as_deref().and_then(|pn| {
                            if let Some(tpl) = pn.strip_prefix("herdr:") {
                                Some(vec![tpl.to_owned(), "{prompt}".into()])
                            } else {
                                presets.get(pn).and_then(|pp| pp.argv.clone())
                            }
                        });
                    if argv_val.is_null() {
                        ensure!(
                            preset_default_argv.is_some(),
                            "Custom CLI Agent @{id} has no preset default command to reset to"
                        );
                        prof.argv = None;
                    } else {
                        let parsed = strings(&p, "argv");
                        ensure!(
                            !parsed.is_empty() && !parsed[0].trim().is_empty(),
                            "Command cannot be empty"
                        );
                        let mut cleaned = parsed;
                        cleaned[0] = cleaned[0].trim().to_owned();
                        if preset_default_argv.as_ref() == Some(&cleaned) {
                            prof.argv = None;
                        } else {
                            prof.argv = Some(cleaned);
                        }
                    }
                }
            }
            "upsert_identity" => {
                let id = strp(&p, "id")?.trim().trim_start_matches('@');
                validate_profile_id(id)?;
                if raw.profiles.iter().any(|x| x.id == id && !x.is_identity()) {
                    bail!("@{id} is already registered as a direct CLI Agent");
                }
                let engine = strp(&p, "engine")?.trim();
                ensure!(!engine.is_empty(), "Backing Agent is required for @{id}");
                let role_instruction = p["role_instruction"]
                    .as_str()
                    .map(str::trim)
                    .unwrap_or("")
                    .to_owned();
                let enabled = p["enabled"].as_bool().unwrap_or(true);
                let display = p["name"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                let explicit_model = p.get("model").map(|v| {
                    v.as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                });
                let explicit_effort = p.get("effort").map(|v| {
                    v.as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                });
                let explicit_orch = p
                    .get("orchestrator")
                    .map(parse_orchestrator_param)
                    .transpose()?;
                // Ensure the chosen engine is resolved / present if built-in
                let catalog = canban_profiles::builtin_catalog();
                if !raw
                    .profiles
                    .iter()
                    .any(|x| x.id == engine && !x.is_identity())
                    && let Some(entry) = catalog.iter().find(|e| e.id == engine)
                {
                    raw.profiles.push(Profile {
                        id: entry.id.clone(),
                        name: Some(entry.display_name.clone()),
                        enabled: true,
                        identity: false,
                        preset: Some(entry.id.clone()),
                        ..Default::default()
                    });
                }
                let preset_ref = raw
                    .profiles
                    .iter()
                    .find(|x| x.id == engine && !x.is_identity())
                    .and_then(|x| x.preset.clone())
                    .unwrap_or_else(|| engine.to_owned());
                let supports_orch = preset_ref == "agy"
                    || raw
                        .profiles
                        .iter()
                        .any(|x| x.id == engine && !x.is_identity() && x.is_agy());
                if explicit_model
                    .as_ref()
                    .and_then(|m| m.as_deref())
                    .is_some_and(canban_types::is_orchestrator_model_id)
                    || explicit_orch.as_ref().is_some_and(Option::is_some)
                {
                    ensure!(
                        supports_orch,
                        "Orchestrator mode is only supported by Antigravity (agy)"
                    );
                }
                if let Some(existing) = raw.profiles.iter_mut().find(|x| x.id == id) {
                    let engine_changed = existing.preset.as_deref() != Some(preset_ref.as_str());
                    existing.identity = true;
                    existing.enabled = enabled;
                    if display.is_some() {
                        existing.name = display;
                    }
                    existing.preset = Some(preset_ref);
                    existing.argv = None;
                    existing.resume_argv = None;
                    existing.exit = None;
                    existing.session_instruction = None;
                    if let Some(m) = explicit_model {
                        existing.model = m;
                    } else if engine_changed {
                        existing.model = None;
                    }
                    if let Some(e) = explicit_effort {
                        existing.effort = e;
                    } else if engine_changed {
                        existing.effort = None;
                    }
                    if let Some(o) = explicit_orch {
                        existing.orchestrator = o;
                    } else if engine_changed {
                        existing.orchestrator = None;
                    }
                    if let Some(m) = existing.model.as_deref()
                        && canban_types::is_orchestrator_model_id(m)
                    {
                        if let Some(legacy) = canban_types::legacy_agy_orchestrator_preset(m)
                            && existing.orchestrator.is_none()
                        {
                            existing.orchestrator = Some(legacy);
                        }
                        existing.model = Some(canban_types::ORCHESTRATOR_MODEL_ID.to_owned());
                        existing.effort = None;
                    } else if existing.model.is_some() || !supports_orch {
                        existing.orchestrator = None;
                    }
                    if p.get("role_instruction").is_some() {
                        existing.role_instruction = role_instruction;
                    }
                } else {
                    let mut created_model = explicit_model.flatten();
                    let mut created_effort = explicit_effort.flatten();
                    let mut created_orch = explicit_orch.flatten();
                    if let Some(m) = created_model.as_deref()
                        && canban_types::is_orchestrator_model_id(m)
                    {
                        if let Some(legacy) = canban_types::legacy_agy_orchestrator_preset(m)
                            && created_orch.is_none()
                        {
                            created_orch = Some(legacy);
                        }
                        created_model = Some(canban_types::ORCHESTRATOR_MODEL_ID.to_owned());
                        created_effort = None;
                    } else if created_model.is_some() || !supports_orch {
                        created_orch = None;
                    }
                    raw.profiles.push(Profile {
                        id: id.to_owned(),
                        name: display,
                        enabled,
                        identity: true,
                        preset: Some(preset_ref),
                        model: created_model,
                        effort: created_effort,
                        orchestrator: created_orch,
                        role_instruction: if role_instruction.is_empty() {
                            format!("You are the Card's {id} Agent.")
                        } else {
                            role_instruction
                        },
                        ..Default::default()
                    });
                }
            }
            "delete_profile" => {
                let id = strp(&p, "id")?.trim();
                ensure!(
                    !matches!(id, "developer" | "reviewer" | "planner" | "merger"),
                    "Built-in identity @{id} cannot be deleted; disable it or change its backing Agent instead"
                );
                let before = raw.profiles.len();
                raw.profiles.retain(|x| x.id != id);
                ensure!(raw.profiles.len() < before, "Profile @{id} not found");
                {
                    let mut store = self.store.lock().unwrap();
                    store.write(|db| {
                        canban_storage::env::delete_scope(
                            db.conn,
                            canban_storage::env::SCOPE_AGENT,
                            id,
                        )
                    })?;
                }
                if raw.default_agent == id {
                    raw.default_agent = raw
                        .profiles
                        .iter()
                        .find(|x| x.id == "developer" && x.enabled)
                        .or_else(|| raw.profiles.iter().find(|x| x.enabled))
                        .map(|x| x.id.clone())
                        .unwrap_or_else(|| "developer".into());
                }
            }
            "replace_profiles" => {
                let incoming: Vec<Profile> = serde_json::from_value(p["profiles"].clone())
                    .context("Invalid profiles list")?;
                ensure!(!incoming.is_empty(), "At least one profile is required");
                for prof in &incoming {
                    validate_profile_id(&prof.id)?;
                }
                raw.profiles = incoming;
                if let Some(def) = p["default_agent"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    && raw.profiles.iter().any(|x| x.id == def && x.enabled)
                {
                    raw.default_agent = def.to_owned();
                } else if !raw
                    .profiles
                    .iter()
                    .any(|x| x.id == raw.default_agent && x.enabled)
                    && let Some(fallback) = raw
                        .profiles
                        .iter()
                        .find(|x| x.id == "developer" && x.enabled)
                        .or_else(|| raw.profiles.iter().find(|x| x.enabled))
                {
                    raw.default_agent = fallback.id.clone();
                }
            }
            _ => bail!("Unsupported config.agents.save op: {op}"),
        }
        let resolved = self.save_raw_config(raw)?;
        Ok(serde_json::to_value(resolved)?)
    }

    pub(super) fn parse_agent_overrides(
        &self,
        config: &Config,
        value: Option<&Value>,
    ) -> Result<BTreeMap<String, ProjectAgentOverride>> {
        let mut overrides = BTreeMap::new();
        let Some(obj) = value.and_then(Value::as_object) else {
            return Ok(overrides);
        };
        for (role_id, target_val) in obj {
            let role_id = role_id.trim();
            if target_val.is_null() {
                continue;
            }
            let mut parsed: ProjectAgentOverride = if let Some(s) = target_val.as_str() {
                ProjectAgentOverride::from_agent(s).normalized()
            } else if target_val.is_object() {
                serde_json::from_value(target_val.clone())
                    .with_context(|| format!("Invalid Project override for @{role_id}"))?
            } else {
                bail!("Invalid Project override value for @{role_id}");
            };
            if parsed.is_empty() {
                continue;
            }
            let role_prof = config
                .profiles
                .iter()
                .find(|p| p.id == role_id)
                .with_context(|| format!("Unknown identity @{role_id}"))?;
            ensure!(
                role_prof.is_identity(),
                "Project overrides only apply to role @identities (got direct CLI Agent @{role_id})"
            );
            let effective_is_agy = if let Some(target) = parsed.agent.as_deref() {
                let resolved_eng =
                    canban_profiles::resolve_engine(config, target, Some(&self.home))?;
                if let Some(direct) = config
                    .profiles
                    .iter()
                    .find(|p| p.id == target && !p.is_identity())
                {
                    ensure!(
                        direct.enabled,
                        "Agent '{target}' is disabled globally; enable it in Settings first"
                    );
                }
                resolved_eng.is_agy()
            } else {
                role_prof.is_agy()
            };
            if parsed
                .model
                .as_deref()
                .is_some_and(canban_types::is_orchestrator_model_id)
                || parsed.orchestrator.is_some()
            {
                ensure!(
                    effective_is_agy,
                    "Orchestrator mode is only supported by Antigravity (agy)"
                );
            }
            if effective_is_agy
                && let Some(m) = parsed.model.as_deref()
                && canban_types::is_orchestrator_model_id(m)
            {
                if let Some(legacy) = canban_types::legacy_agy_orchestrator_preset(m)
                    && parsed.orchestrator.is_none()
                {
                    parsed.orchestrator = Some(legacy);
                }
                parsed.model = Some(canban_types::ORCHESTRATOR_MODEL_ID.to_owned());
                parsed.effort = None;
            }
            overrides.insert(role_id.to_owned(), parsed);
        }
        Ok(overrides)
    }

    /// Save Project-level identity -> Agent overrides (`project.agents`).
    pub(super) fn update_project_agents(&self, p: Value) -> Result<Value> {
        let config = self.config();
        let overrides = self.parse_agent_overrides(&config, p.get("agent_overrides"))?;
        self.mutate("project.agents", &p, |db| {
            let pid = super::resolve_project_id(db, strp(&p, "project_id")?)?;
            let mut project: Project = db.get(&pid)?;
            project.agent_overrides = overrides;
            db.save(&project)?;
            Ok(serde_json::to_value(project)?)
        })
    }
}

#[cfg(test)]
mod model_probe_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn cli_model_probe_uses_15s_timeout_and_backfills_cache_after_late_completion() {
        assert_eq!(AGENT_MODEL_PROBE_TIMEOUT, Duration::from_secs(15));

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock-agy");
        std::fs::write(
            &script,
            "#!/bin/sh\nsleep 0.15\nprintf 'gemini-4-neon-high\\tGemini 4 Neon (High)\\ngemini-4-neon-xhigh\\tGemini 4 Neon (XHigh)\\ngemini-4-neon-medium\\tGemini 4 Neon (Medium)\\ngemini-4-neon-low\\tGemini 4 Neon (Low)\\ngemini-pro-orchestrator-neon\\tOrchestrator (Neon)\\n'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let defaults = vec![ModelOption {
            id: "fallback-model".into(),
            label: "Fallback".into(),
            supported_efforts: None,
            effort_labels: BTreeMap::new(),
        }];
        let cache_key = format!("test-agy-late-{}", script.display());

        // 1. Short timeout returns defaults without blocking, while the worker finishes in background.
        let first = probe_cli_models_with_timeout(
            &cache_key,
            script.to_str().unwrap(),
            &["models"],
            Duration::from_millis(30),
            &defaults,
            parse_agy_models_output,
        );
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].id, "fallback-model");

        // 2. Wait for the first background probe to populate the cache, then verify the next call hits cache immediately.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if model_probe_cache()
                .lock()
                .ok()
                .is_some_and(|g| g.contains_key(&cache_key))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let second = probe_cli_models_with_timeout(
            &cache_key,
            script.to_str().unwrap(),
            &["models"],
            Duration::from_millis(10),
            &defaults,
            parse_agy_models_output,
        );
        let neon = second
            .iter()
            .find(|m| m.id == "gemini-4-neon")
            .expect("backfilled cache should include collapsed gemini-4-neon");
        assert_eq!(neon.label, "Gemini 4 Neon");
        assert_eq!(
            neon.supported_efforts,
            Some(vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "xhigh".into()
            ])
        );
        assert_eq!(
            neon.effort_labels.get("xhigh").map(String::as_str),
            Some("Gemini 4 Neon (XHigh)")
        );
        let orch = second.iter().find(|m| m.id == "orchestrator").expect(
            "backfilled cache should collapse orchestrator slugs into unified orchestrator",
        );
        assert_eq!(orch.supported_efforts, Some(Vec::new()));
        // Live agent probe overrides defaults completely; removed/fallback models must not linger.
        assert!(
            !second.iter().any(|m| m.id == "fallback-model"),
            "probed agent models must replace defaults instead of merging stale entries"
        );
    }

    #[test]
    fn static_agy_defaults_include_neon_and_codex_cache_overrides_removed_defaults() {
        let agy_defaults = canban_profiles::builtin_catalog()
            .into_iter()
            .find(|e| e.id == "agy")
            .unwrap()
            .default_models;
        let neon = agy_defaults
            .iter()
            .find(|m| m.id == "gemini-4-neon")
            .expect("static agy defaults should include gemini-4-neon");
        assert_eq!(neon.label, "Gemini 4 Neon");
        assert_eq!(
            neon.supported_efforts,
            Some(vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "xhigh".into()
            ])
        );
        let orch = agy_defaults
            .iter()
            .find(|m| m.id == "orchestrator")
            .expect("static agy defaults should include unified orchestrator");
        assert_eq!(orch.supported_efforts, Some(Vec::new()));

        let codex_defaults = vec![
            ModelOption {
                id: "retired-old-model".into(),
                label: "Retired".into(),
                supported_efforts: None,
                effort_labels: BTreeMap::new(),
            },
            ModelOption {
                id: "still-here".into(),
                label: "Still Here".into(),
                supported_efforts: None,
                effort_labels: BTreeMap::new(),
            },
        ];
        let parsed = parse_codex_cached_models_json(
            &json!({
                "models": [
                    { "slug": "still-here", "display_name": "Still Here" },
                    { "slug": "brand-new-model", "display_name": "Brand New" }
                ]
            }),
            &codex_defaults,
        );
        let ids: Vec<&str> = parsed.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["still-here", "brand-new-model"]);

        // Empty live cache still falls back to defaults.
        let fallback = parse_codex_cached_models_json(&json!({ "models": [] }), &codex_defaults);
        assert_eq!(fallback, codex_defaults);
    }
}
