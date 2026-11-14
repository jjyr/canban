//! Herdr-backed Agent catalog probing, Onboarding quick setup, Global Identity &
//! Agent management, and Project-level identity backend overrides.
use super::*;
use canban_profiles::ModelOption;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

fn probe_codex_cached_models(defaults: &[ModelOption]) -> Vec<ModelOption> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let cache_path = std::path::PathBuf::from(home).join(".codex/models_cache.json");
        if let Ok(bytes) = std::fs::read(&cache_path)
            && let Ok(val) = serde_json::from_slice::<Value>(&bytes)
            && let Some(models) = val["models"].as_array()
        {
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
                if !out.iter().any(|x: &ModelOption| x.id == slug) {
                    out.push(ModelOption {
                        id: slug.to_owned(),
                        label: label.to_owned(),
                    });
                }
            }
        }
    }
    for def in defaults {
        if !out.iter().any(|x| x.id == def.id) {
            out.push(def.clone());
        }
    }
    out
}

fn probe_agy_models(agy_path: &str, defaults: &[ModelOption]) -> Vec<ModelOption> {
    type CacheMap = BTreeMap<String, (Instant, Vec<ModelOption>)>;
    static CACHE: OnceLock<Mutex<CacheMap>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Ok(guard) = cache.lock()
        && let Some((ts, cached)) = guard.get(agy_path)
        && ts.elapsed() < Duration::from_secs(300)
        && !cached.is_empty()
    {
        return cached.clone();
    }

    let path_owned = agy_path.to_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let res = std::process::Command::new(&path_owned)
            .arg("models")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();
        let _ = tx.send(res);
    });

    let mut parsed = Vec::new();
    if let Ok(Ok(output)) = rx.recv_timeout(Duration::from_millis(2500))
        && output.status.success()
    {
        let text = String::from_utf8_lossy(&output.stdout);
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
                    });
                }
            }
        }
    }

    if parsed.is_empty() {
        return defaults.to_vec();
    }
    if let Ok(mut guard) = cache.lock() {
        guard.insert(agy_path.to_owned(), (Instant::now(), parsed.clone()));
    }
    parsed
}

fn models_for_entry(
    entry: &canban_profiles::CatalogEntry,
    machine: &str,
    installed_path: Option<&str>,
) -> Vec<ModelOption> {
    if machine == "local" {
        match entry.id.as_str() {
            "agy" => {
                if let Some(path) = installed_path {
                    return probe_agy_models(path, &entry.default_models);
                }
            }
            "codex" => {
                return probe_codex_cached_models(&entry.default_models);
            }
            _ => {}
        }
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

impl AppServer {
    pub(super) fn agent_catalog(&self, p: Value) -> Result<Value> {
        let machine = match p["machine"].as_str().filter(|s| !s.trim().is_empty()) {
            Some(m) => self
                .resolve_machine(m, false)
                .unwrap_or_else(|_| "local".into()),
            None => "local".into(),
        };
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
        let host = canban_herdr::host::Host::new(config.clone(), &machine);
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
            let installed_path = probed.get(&entry.binary).cloned();
            let models = models_for_entry(entry, &machine, installed_path.as_deref());
            agents.push(json!({
                "id": entry.id,
                "display_name": existing.and_then(|p| p.name.clone()).unwrap_or_else(|| entry.display_name.clone()),
                "binary": entry.binary,
                "kind": "builtin",
                "resumable": entry.resumable,
                "installed": installed_path.is_some(),
                "path": installed_path,
                "configured": existing.is_some(),
                "enabled": existing.is_some_and(|p| p.enabled),
                "models": models,
                "supported_efforts": entry.supported_efforts,
                "supports_model": !entry.model_args.is_empty(),
                "supports_effort": !entry.effort_args.is_empty(),
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
            let bin = preset_prof
                .command()
                .first()
                .cloned()
                .unwrap_or_else(|| preset_id.clone());
            let installed_path = probed.get(&bin).cloned();
            let meta = canban_profiles::catalog_entry_for_profile(preset_prof);
            let models = meta
                .as_ref()
                .map(|m| models_for_entry(m, &machine, installed_path.as_deref()))
                .unwrap_or_default();
            let supported_efforts = meta
                .as_ref()
                .map(|m| m.supported_efforts.clone())
                .unwrap_or_default();
            let supports_effort = meta.as_ref().is_some_and(|m| !m.effort_args.is_empty())
                || preset_prof.command().iter().any(|a| a.contains("{effort}"));
            agents.push(json!({
                "id": preset_id,
                "display_name": preset_prof.name.clone().unwrap_or_else(|| preset_id.clone()),
                "binary": bin,
                "kind": "preset",
                "resumable": !preset_prof.resume_command().is_empty(),
                "installed": installed_path.is_some(),
                "path": installed_path,
                "configured": existing.is_some(),
                "enabled": existing.is_some_and(|p| p.enabled),
                "models": models,
                "supported_efforts": supported_efforts,
                "supports_model": true,
                "supports_effort": supports_effort,
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
                .map(|m| models_for_entry(m, &machine, installed_path.as_deref()))
                .unwrap_or_default();
            let supported_efforts = meta
                .as_ref()
                .map(|m| m.supported_efforts.clone())
                .unwrap_or_default();
            let supports_effort = !is_herdr_tpl
                && (meta.as_ref().is_some_and(|m| !m.effort_args.is_empty())
                    || prof.command().iter().any(|a| a.contains("{effort}")));
            agents.push(json!({
                "id": prof.id,
                "display_name": prof.name.clone().unwrap_or_else(|| prof.id.clone()),
                "binary": bin,
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
            }));
        }

        Ok(json!({
            "machine": machine,
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
                    .map(|x| (x.model.clone(), x.effort.clone()));
                let next_model = if p.get("model").is_some() {
                    p["model"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                } else {
                    existing_opts.as_ref().and_then(|(m, _)| m.clone())
                };
                let next_effort = if p.get("effort").is_some() {
                    p["effort"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                } else {
                    existing_opts.as_ref().and_then(|(_, e)| e.clone())
                };
                let new_prof = Profile {
                    id: id.to_owned(),
                    name: display,
                    enabled: p["enabled"].as_bool().unwrap_or(true),
                    identity: false,
                    preset,
                    model: next_model,
                    effort: next_effort,
                    role_instruction: String::new(),
                    argv: final_argv,
                    resume_argv: (!resume_argv.is_empty()).then_some(resume_argv),
                    exit: Some(ExitAction::Manual),
                    session_instruction: None,
                };
                if let Some(existing) = raw.profiles.iter_mut().find(|x| x.id == id) {
                    *existing = new_prof;
                } else {
                    raw.profiles.push(new_prof);
                }
            }
            "set_profile_options" => {
                let id = strp(&p, "id")?.trim().trim_start_matches('@');
                validate_profile_id(id)?;
                if !raw.profiles.iter().any(|x| x.id == id) {
                    let catalog = canban_profiles::builtin_catalog();
                    let presets = canban_profiles::presets(&self.home).unwrap_or_default();
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
                let prof = raw
                    .profiles
                    .iter_mut()
                    .find(|x| x.id == id)
                    .with_context(|| format!("Unknown profile @{id}"))?;
                if p.get("model").is_some() {
                    prof.model = p["model"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                }
                if p.get("effort").is_some() {
                    prof.effort = p["effort"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
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
                    if p.get("role_instruction").is_some() {
                        existing.role_instruction = role_instruction;
                    }
                } else {
                    raw.profiles.push(Profile {
                        id: id.to_owned(),
                        name: display,
                        enabled,
                        identity: true,
                        preset: Some(preset_ref),
                        model: explicit_model.flatten(),
                        effort: explicit_effort.flatten(),
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
            _ => bail!("Unsupported config.agents.save op: {op}"),
        }
        let resolved = self.save_raw_config(raw)?;
        Ok(serde_json::to_value(resolved)?)
    }

    pub(super) fn parse_agent_overrides(
        &self,
        config: &Config,
        value: Option<&Value>,
    ) -> Result<BTreeMap<String, String>> {
        let mut overrides = BTreeMap::new();
        let Some(obj) = value.and_then(Value::as_object) else {
            return Ok(overrides);
        };
        for (role_id, target_val) in obj {
            let role_id = role_id.trim();
            let Some(target) = target_val
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "inherit")
            else {
                continue;
            };
            ensure!(
                config.profiles.iter().any(|p| p.id == role_id),
                "Unknown identity @{role_id}"
            );
            let _ = canban_profiles::resolve_engine(config, target, Some(&self.home))?;
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
            overrides.insert(role_id.to_owned(), target.to_owned());
        }
        Ok(overrides)
    }

    /// Save Project-level identity -> Agent overrides (`project.agents`).
    pub(super) fn update_project_agents(&self, p: Value) -> Result<Value> {
        let config = self.config();
        let overrides = self.parse_agent_overrides(&config, p.get("agent_overrides"))?;
        self.mutate("project.agents", &p, |db| {
            let mut project: Project = db.get(strp(&p, "project_id")?)?;
            project.agent_overrides = overrides;
            db.save(&project)?;
            Ok(serde_json::to_value(project)?)
        })
    }
}
