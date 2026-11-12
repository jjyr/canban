//! Herdr-backed Agent catalog probing, Onboarding quick setup, Global Identity &
//! Agent management, and Project-level identity backend overrides.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

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
        "planner" => {
            "You are the Card's planning Agent. Analyze the task and relevant repository files in read-only mode, identify risks, and write a concise implementation plan without modifying source files."
        }
        "developer" => {
            "You are the Card's development Agent. Implement the agreed task, preserve relevant project conventions, and explain your changes and validation."
        }
        "reviewer" => {
            "You are the Card's independent review Agent. Inspect the actual changes critically, identify correctness issues and missing validation, and report findings. Do not implement changes unless explicitly asked."
        }
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

        let mut agents = Vec::new();
        for entry in &catalog {
            let existing = config
                .profiles
                .iter()
                .find(|prof| prof.id == entry.id && !prof.is_identity());
            let installed_path = probed.get(&entry.binary).cloned();
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
            }));
        }

        Ok(json!({
            "machine": machine,
            "probe_error": probe_error,
            "agents": agents,
            "herdr_templates": herdr_templates,
            "config": config,
        }))
    }

    fn save_raw_config(&self, mut raw: Config) -> Result<Config> {
        let resolved = canban_profiles::resolve(raw.clone(), &self.home)?;
        Self::validate_config(&resolved)?;
        self.validate_active_workflow_references(&resolved)?;
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

    fn validate_active_workflow_references(&self, config: &Config) -> Result<()> {
        let workflow = self.default_workflow()?;
        Self::validate_workflow(config, &workflow)?;
        let snapshot = self.snapshot()?;
        for project in &snapshot.projects {
            Self::validate_workflow(config, &project.workflow)
                .with_context(|| format!("Project {} workflow", project.name))?;
        }
        let valid_ids: Vec<String> = config.profiles.iter().map(|p| p.id.clone()).collect();
        for source in &snapshot.workflow_sources {
            let project = snapshot
                .projects
                .iter()
                .find(|p| p.id == source.project_id)
                .context("Workflow Project missing")?;
            let rev = snapshot
                .workflow_revisions
                .iter()
                .find(|r| r.id == source.revision_id)
                .context("Active Workflow revision missing")?;
            canban_workflow_engine::validate_definition(
                &rev.definition,
                &project
                    .workflow
                    .stages
                    .iter()
                    .map(|s| s.id.clone())
                    .collect::<Vec<_>>(),
                &valid_ids,
            )
            .with_context(|| format!("Project '{}' ({})", project.name, source.filename))?;
        }
        Ok(())
    }

    fn load_unmigrated_config(&self) -> Result<Config> {
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

        // 3. Ensure built-in semantic identities (`developer`, `reviewer`, `planner`) exist and use selected engines.
        for role_id in ["developer", "reviewer", "planner"] {
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
                existing.identity = true;
                existing.enabled = true;
                existing.preset = Some(preset_ref);
                existing.argv = None;
                existing.resume_argv = None;
                existing.exit = None;
                existing.session_instruction = None;
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
            if matches!(role_id.as_str(), "developer" | "reviewer" | "planner") {
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
                existing.preset = Some(preset_ref);
                existing.argv = None;
                existing.resume_argv = None;
                existing.exit = None;
                existing.session_instruction = None;
            }
        }

        if let Some(def) = p["default_agent"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            raw.default_agent = def.to_owned();
        } else if !raw
            .profiles
            .iter()
            .any(|prof| prof.id == raw.default_agent && prof.enabled)
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
                let new_prof = Profile {
                    id: id.to_owned(),
                    name: display,
                    enabled: p["enabled"].as_bool().unwrap_or(true),
                    identity: false,
                    preset,
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
                    !matches!(id, "developer" | "reviewer" | "planner"),
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
