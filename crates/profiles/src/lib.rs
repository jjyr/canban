//! Resolve editable presets and explicit overrides; no provider dispatch at runtime.
pub mod catalog;
pub use catalog::{
    CatalogEntry, ModelOption, builtin_catalog, catalog_entry_for_profile, display_name,
    herdr_template_profile,
};

use anyhow::{Context, Result, bail, ensure};
use canban_types::{Config, ExitAction, Profile, Project};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub fn presets(home: &Path) -> Result<BTreeMap<String, Profile>> {
    let mut presets = BTreeMap::new();
    for entry in builtin_catalog() {
        presets.insert(entry.id.clone(), entry.to_preset_profile());
    }
    for data in [
        include_str!("../presets/codex.json"),
        include_str!("../presets/claude.json"),
    ] {
        let p: Profile = serde_json::from_str(data)?;
        presets.insert(p.id.clone(), p);
    }
    let dir = home.join("config/presets");
    if dir.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let mut p: Profile = serde_json::from_slice(&std::fs::read(&path)?)?;
            let name = path
                .file_stem()
                .and_then(|x| x.to_str())
                .context("Invalid preset filename")?;
            p.id = name.into();
            ensure!(
                p.preset.is_none(),
                "Preset {name} cannot inherit another preset"
            );
            presets.insert(name.into(), p);
        }
    }
    Ok(presets)
}

pub fn resolve(config: Config, home: &Path) -> Result<Config> {
    let presets = presets(home)?;
    let mut config = config;
    config.hc = executable(&config.hc);
    let snapshot_profiles = config.profiles.clone();
    for profile in &mut config.profiles {
        profile.model = profile
            .model
            .take()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        profile.effort = profile
            .effort
            .take()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        if let Some(name) = &profile.preset {
            let peer_agent = snapshot_profiles
                .iter()
                .find(|p| p.id == *name && !p.is_identity());
            if let Some(tpl) = name.strip_prefix("herdr:") {
                ensure!(
                    !tpl.trim().is_empty(),
                    "Empty Herdr template name for @{}",
                    profile.id
                );
                if profile.argv.is_none() {
                    profile.argv = Some(vec![tpl.to_owned(), "{prompt}".into()]);
                }
            } else if let Some(preset) = presets.get(name) {
                if profile.argv.is_none() {
                    profile.argv = preset.argv.clone();
                }
                if profile.resume_argv.is_none() {
                    profile.resume_argv = preset.resume_argv.clone();
                }
                if profile.exit.is_none() {
                    profile.exit = preset.exit.clone();
                }
                if profile.session_instruction.is_none() {
                    profile.session_instruction = preset.session_instruction.clone();
                }
                if profile.model.is_none() {
                    profile.model = peer_agent
                        .and_then(|p| p.model.as_deref().map(str::trim))
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .or_else(|| preset.model.clone());
                }
                if profile.effort.is_none() {
                    profile.effort = peer_agent
                        .and_then(|p| p.effort.as_deref().map(str::trim))
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .or_else(|| preset.effort.clone());
                }
            } else if let Some(peer) = peer_agent {
                let peer_preset = peer.preset.as_deref().and_then(|pn| presets.get(pn));
                if profile.argv.is_none() {
                    profile.argv = peer
                        .argv
                        .clone()
                        .or_else(|| peer_preset.and_then(|pp| pp.argv.clone()));
                }
                if profile.resume_argv.is_none() {
                    profile.resume_argv = peer
                        .resume_argv
                        .clone()
                        .or_else(|| peer_preset.and_then(|pp| pp.resume_argv.clone()));
                }
                if profile.exit.is_none() {
                    profile.exit = peer
                        .exit
                        .clone()
                        .or_else(|| peer_preset.and_then(|pp| pp.exit.clone()));
                }
                if profile.session_instruction.is_none() {
                    profile.session_instruction = peer
                        .session_instruction
                        .clone()
                        .or_else(|| peer_preset.and_then(|pp| pp.session_instruction.clone()));
                }
                if profile.model.is_none() {
                    profile.model = peer
                        .model
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .or_else(|| peer_preset.and_then(|pp| pp.model.clone()));
                }
                if profile.effort.is_none() {
                    profile.effort = peer
                        .effort
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .or_else(|| peer_preset.and_then(|pp| pp.effort.clone()));
                }
            } else {
                bail!("Unknown preset {name} for @{}", profile.id);
            }
        }
        ensure!(
            !profile.command().is_empty(),
            "Profile @{} needs a launch command",
            profile.id
        );
        if let Some(ExitAction::Input { text }) = &profile.exit {
            ensure!(
                !text.trim().is_empty(),
                "Profile @{} has an empty exit input",
                profile.id
            );
        }
        profile.exit.get_or_insert(ExitAction::Manual);
    }
    Ok(config)
}

/// Resolve an execution engine profile by ID from configured non-identity profiles,
/// built-in presets, or `herdr:<template>`.
pub fn resolve_engine(config: &Config, engine_id: &str, home: Option<&Path>) -> Result<Profile> {
    if let Some(peer) = config
        .profiles
        .iter()
        .find(|p| p.id == engine_id && !p.is_identity())
    {
        return Ok(peer.clone());
    }
    if let Some(tpl) = engine_id.strip_prefix("herdr:") {
        ensure!(!tpl.trim().is_empty(), "Empty Herdr template");
        return Ok(herdr_template_profile(tpl));
    }
    if let Some(entry) = builtin_catalog().into_iter().find(|e| e.id == engine_id) {
        return Ok(entry.to_preset_profile());
    }
    if let Some(home) = home
        && let Ok(map) = presets(home)
        && let Some(p) = map.get(engine_id)
    {
        return Ok(p.clone());
    }
    bail!("Unknown Agent backend '{engine_id}'")
}

/// Apply optional Project-level identity backend override (`project.agent_overrides`)
/// while preserving the identity's `id` and `role_instruction`.
pub fn apply_project_override(
    config: &Config,
    project: Option<&Project>,
    profile: &Profile,
) -> Profile {
    let Some(project) = project else {
        return profile.clone();
    };
    let Some(target) = project
        .agent_overrides
        .get(&profile.id)
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty())
    else {
        return profile.clone();
    };
    // Only apply an override if the target backend isn't explicitly disabled in global config.
    if config
        .profiles
        .iter()
        .any(|p| p.id == target && !p.is_identity() && !p.enabled)
    {
        return profile.clone();
    }
    let Ok(engine) = resolve_engine(config, target, None) else {
        return profile.clone();
    };
    let next_preset = engine.preset.clone().or_else(|| Some(target.to_owned()));
    let same_engine = profile.preset.as_deref() == next_preset.as_deref()
        || profile.preset.as_deref() == Some(target)
        || profile.command_name() == engine.command_name();
    let mut out = profile.clone();
    out.preset = next_preset;
    out.argv = engine.argv;
    out.resume_argv = engine.resume_argv;
    out.exit = Some(engine.exit.unwrap_or(ExitAction::Manual));
    out.session_instruction = engine.session_instruction;
    if same_engine {
        if out.model.is_none() {
            out.model = engine.model;
        }
        if out.effort.is_none() {
            out.effort = engine.effort;
        }
    } else {
        out.model = engine.model;
        out.effort = engine.effort;
    }
    out
}

/// Find an executable path on the local host if installed; returns `None` when missing.
pub fn find_local_executable(program: &str) -> Option<PathBuf> {
    if Path::new(program).components().count() != 1 {
        let p = PathBuf::from(program);
        return p.is_file().then_some(p);
    }
    let mut paths =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        paths.push(home.join(".local/bin"));
        paths.push(home.join(".cargo/bin"));
        paths.push(home.join(".bun/bin"));
        paths.push(home.join("bin"));
    }
    paths.extend(["/opt/homebrew/bin".into(), "/usr/local/bin".into()]);
    paths
        .into_iter()
        .map(|p| p.join(program))
        .find(|p| p.is_file())
}

pub fn executable(program: &str) -> String {
    find_local_executable(program)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.into())
}

/// Explicit, one-time old configuration conversion. Historical request snapshots
/// are not rewritten. The caller backs up the original bytes before publication.
pub fn migrate(value: &mut Value) -> Result<bool> {
    let profiles = value["profiles"]
        .as_array_mut()
        .context("profiles must be an array")?;
    let mut changed = false;
    for p in profiles {
        ensure!(p.is_object(), "Profile must be an object");
        if let Some((old_role, new_role)) = match p["id"].as_str() {
            Some("developer") => Some((
                "You are the Card's development Agent. Implement the agreed task, preserve relevant project conventions, and explain your changes and validation.",
                "You are the Card's development Agent. Follow repository conventions and verify changes before finishing.",
            )),
            Some("reviewer") => Some((
                "You are the Card's independent review Agent. Inspect the actual changes critically, identify correctness issues and missing validation, and report findings. Do not implement changes unless explicitly asked.",
                "You are the Card's independent review Agent. Critically inspect changes and test coverage without making unrequested edits.",
            )),
            Some("planner") => Some((
                "You are the Card's planning Agent. Analyze the task and relevant repository files in read-only mode, identify risks, and write a concise implementation plan without modifying source files.",
                "You are the Card's planning Agent. Inspect repository files in read-only mode without modifying source files.",
            )),
            _ => None,
        } && p["role_instruction"].as_str().map(str::trim) == Some(old_role)
        {
            p["role_instruction"] = json!(new_role);
            changed = true;
        }
        let legacy = p.get("provider").is_some() || p.get("template").is_some();
        // Pre-interactive registries have no provider field; recognize their
        // explicit batch syntax only during this migration.
        let program = p["argv"][0]
            .as_str()
            .and_then(|s| Path::new(s).file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let old_batch = p["argv"].as_array().is_some_and(|a| match program {
            "codex" => a.get(1).is_some_and(|s| s == "exec"),
            "claude" => a.iter().any(|s| s == "-p" || s == "--print"),
            _ => false,
        });
        if !legacy && !old_batch {
            continue;
        }
        if p["template"].is_string() {
            bail!(
                "Profile @{} uses a legacy Herdr template. Replace it with explicit argv/resume_argv; the original configuration has not been changed",
                p["id"]
            );
        }
        let program = p["argv"][0]
            .as_str()
            .and_then(|s| Path::new(s).file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let provider = p["provider"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(program)
            .to_owned();
        let known = matches!(provider.as_str(), "codex" | "claude");
        if known {
            p["preset"] = json!(provider);
            let args = p["argv"].as_array_mut().context("argv must be an array")?;
            if provider == "codex" {
                if args.get(1).is_some_and(|a| a == "exec") {
                    args.remove(1);
                }
                args.retain(|a| a != "--skip-git-repo-check");
            } else {
                args.retain(|a| a != "-p" && a != "--print");
            }
            if provider == "claude" && !args.iter().any(|a| a == "--session-id") {
                args.splice(1..1, [json!("--session-id"), json!("{session_id}")]);
            }
            if p["resume_argv"].as_array().is_none_or(|a| a.is_empty()) {
                p.as_object_mut().unwrap().remove("resume_argv");
            }
        }
        p.as_object_mut().unwrap().remove("provider");
        p.as_object_mut().unwrap().remove("template");
        changed = true;
    }
    Ok(changed)
}

fn expand_template(arg: &str, replacements: &[(&str, &str)]) -> String {
    let mut rest = arg;
    let mut out = String::new();
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        if let Some(close) = rest.find('}') {
            let key = &rest[1..close];
            if let Some((_, value)) = replacements.iter().find(|(name, _)| *name == key) {
                out.push_str(value);
            } else {
                out.push_str(&rest[..=close]);
            }
            rest = &rest[close + 1..];
        } else {
            out.push_str(rest);
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

fn push_trusted_dir(dirs: &mut Vec<String>, candidate: &str) {
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        return;
    }
    if !dirs.iter().any(|d| d == trimmed) {
        dirs.push(trimmed.to_owned());
    }
    if let Ok(canon) = std::fs::canonicalize(trimmed) {
        let canon_str = canon.to_string_lossy().into_owned();
        if !dirs.iter().any(|d| d == &canon_str) {
            dirs.push(canon_str);
        }
    }
}

/// Expand arguments as individual values; never interpret a shell command.
pub fn command(
    profile: &Profile,
    session: Option<&str>,
    fresh: bool,
    vars: &[(&str, &str)],
) -> Result<Vec<String>> {
    let base_args: Vec<String> = if !fresh && session.is_some() {
        ensure!(
            !profile.resume_command().is_empty(),
            "Profile @{} does not support Resume; choose Start with retained Card context explicitly",
            profile.id
        );
        profile.resume_command().to_vec()
    } else {
        profile.command().to_vec()
    };
    ensure!(
        !base_args.is_empty(),
        "Profile @{} has no command",
        profile.id
    );

    let lookup = |key: &str| -> Option<&str> {
        vars.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
            .filter(|v| !v.trim().is_empty())
    };
    let home_val = lookup("home").unwrap_or("");
    let cwd_val = lookup("cwd").or(lookup("project_dir")).unwrap_or(home_val);
    let project_dir_val = lookup("project_dir").unwrap_or(cwd_val);

    let mut trusted_dirs = Vec::new();
    push_trusted_dir(&mut trusted_dirs, cwd_val);
    push_trusted_dir(&mut trusted_dirs, project_dir_val);
    push_trusted_dir(&mut trusted_dirs, home_val);
    let agy_trusted_settings = serde_json::to_string(&json!({
        "trustedWorkspaces": trusted_dirs,
    }))?;

    let model_val = profile
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let effort_val = profile
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let is_herdr_template = profile
        .preset
        .as_deref()
        .is_some_and(|s| s.starts_with("herdr:"));
    let uses_model_placeholder = base_args.iter().any(|a| a.contains("{model}"));
    let uses_effort_placeholder = base_args.iter().any(|a| a.contains("{effort}"));
    let catalog_meta = catalog_entry_for_profile(profile);

    let mut args = Vec::with_capacity(base_args.len() + 4);
    args.push(base_args[0].clone());
    if !is_herdr_template {
        if let Some(m) = model_val
            && !uses_model_placeholder
        {
            let tpl = catalog_meta
                .as_ref()
                .filter(|c| !c.model_args.is_empty())
                .map(|c| c.model_args.clone())
                .unwrap_or_else(|| vec!["--model".into(), "{model}".into()]);
            for item in tpl {
                args.push(expand_template(&item, &[("model", m)]));
            }
        }
        if let Some(eff) = effort_val
            && !uses_effort_placeholder
        {
            let tpl = catalog_meta
                .as_ref()
                .filter(|c| !c.effort_args.is_empty())
                .map(|c| c.effort_args.clone())
                .unwrap_or_else(|| vec!["--effort".into(), "{effort}".into()]);
            for item in tpl {
                args.push(expand_template(&item, &[("effort", eff)]));
            }
        }
    }
    args.extend_from_slice(&base_args[1..]);

    // Substitute only the original argument template. Text inserted by a
    // prompt/session value must not be interpreted as another placeholder.
    let mut replacements: Vec<(&str, &str)> = Vec::with_capacity(vars.len() + 6);
    for &(k, v) in vars {
        if !matches!(k, "cwd" | "project_dir") {
            replacements.push((k, v));
        }
    }
    replacements.push(("cwd", cwd_val));
    replacements.push(("project_dir", project_dir_val));
    replacements.push(("agy_trusted_settings", &agy_trusted_settings));
    replacements.push(("model", model_val.unwrap_or("")));
    replacements.push(("effort", effort_val.unwrap_or("")));
    replacements.push(("session_id", session.unwrap_or("")));

    // Deduplicate consecutive or repeated `--add-dir <value>` pairs and omit empty `--add-dir ""` pairs.
    let expanded: Vec<String> = args
        .iter()
        .map(|arg| expand_template(arg, &replacements))
        .collect();
    let mut final_args = Vec::with_capacity(expanded.len());
    let mut seen_add_dirs: Vec<String> = Vec::new();
    let mut i = 0;
    while i < expanded.len() {
        if expanded[i] == "--add-dir" && i + 1 < expanded.len() {
            let dir = expanded[i + 1].trim();
            if !dir.is_empty() && !seen_add_dirs.iter().any(|d| d == dir) {
                seen_add_dirs.push(dir.to_owned());
                final_args.push("--add-dir".into());
                final_args.push(expanded[i + 1].clone());
            }
            i += 2;
            continue;
        }
        final_args.push(expanded[i].clone());
        i += 1;
    }
    Ok(final_args)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overrides_replace_whole_commands_and_presets_do_not_install_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Config::default();
        c.profiles[0].argv = Some(vec!["own-codex-wrapper".into(), "{prompt}".into()]);
        c.profiles[0].resume_argv = Some(vec![]);
        c.profiles[0].exit = Some(ExitAction::Manual);
        let c = resolve(c, dir.path()).unwrap();
        assert_eq!(c.profiles[0].command()[0], "own-codex-wrapper");
        assert!(command(&c.profiles[0], Some("thread"), false, &[]).is_err());
        assert_eq!(c.profiles[0].exit, Some(ExitAction::Manual));
        let argv = command(
            &c.profiles[2],
            Some("exact-thread"),
            false,
            &[("prompt", "task"), ("home", "/tmp/home")],
        )
        .unwrap();
        assert!(argv.windows(2).any(|a| a == ["resume", "exact-thread"]));
        assert!(
            !argv
                .iter()
                .any(|a| a.contains("notify=") || a == "--settings")
        );
    }
    #[test]
    fn migration_preserves_options_and_rejects_opaque_templates() {
        let mut v = json!({"profiles":[{"id":"dev","provider":"codex","argv":["/bin/codex","exec","--sandbox","workspace-write","{prompt}"],"resume_argv":[]}]});
        assert!(migrate(&mut v).unwrap());
        assert_eq!(v["profiles"][0]["preset"], "codex");
        assert_eq!(
            v["profiles"][0]["argv"],
            json!(["/bin/codex", "--sandbox", "workspace-write", "{prompt}"])
        );
        assert!(!migrate(&mut v).unwrap());
        assert!(
            migrate(&mut json!({"profiles":[{"id":"old","template":"foo","argv":[]}]})).is_err()
        );
    }
    #[test]
    fn user_preset_is_resolved_without_a_provider_registry() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("config/presets")).unwrap();
        std::fs::write(dir.path().join("config/presets/local.json"),r#"{"id":"local","argv":["my-agent","{prompt}"],"resume_argv":["my-agent","resume","{session_id}","{prompt}"]}"#).unwrap();
        let c = Config {
            profiles: vec![Profile {
                id: "custom".into(),
                preset: Some("local".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let c = resolve(c, dir.path()).unwrap();
        assert_eq!(c.profiles[0].command()[0], "my-agent");
    }
    #[test]
    fn migration_preserves_codex_profile_option_and_prompt_tokens_are_literal() {
        let mut value = json!({"profiles":[{"id":"dev","provider":"codex","argv":["codex","-p","custom","{prompt}"]}]});
        migrate(&mut value).unwrap();
        assert_eq!(
            value["profiles"][0]["argv"],
            json!(["codex", "-p", "custom", "{prompt}"])
        );
        let profile: Profile = serde_json::from_value(value["profiles"][0].clone()).unwrap();
        let args = command(
            &profile,
            None,
            true,
            &[
                ("prompt", "literal {home} {session_id} {cli}"),
                ("home", "/home"),
                ("cli", "/bin/canban"),
            ],
        )
        .unwrap();
        assert_eq!(args.last().unwrap(), "literal {home} {session_id} {cli}");
        let mut custom = json!({"profiles":[{"id":"custom","argv":["own-command","-p","value"]}]});
        assert!(!migrate(&mut custom).unwrap());
    }

    #[test]
    fn builtin_herdr_catalog_and_project_overrides_resolve_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = builtin_catalog();
        assert_eq!(catalog.len(), 24);
        assert!(catalog.iter().any(|e| e.id == "agy" && e.binary == "agy"));
        assert!(
            catalog
                .iter()
                .any(|e| e.id == "cursor" && e.binary == "cursor-agent")
        );
        assert!(
            catalog
                .iter()
                .any(|e| e.id == "kiro" && e.binary == "kiro-cli")
        );

        let mut c = Config::default();
        c.profiles.push(Profile {
            id: "qa".into(),
            identity: true,
            preset: Some("agy".into()),
            role_instruction: "Verify edge cases.".into(),
            ..Default::default()
        });
        c.profiles.push(Profile {
            id: "agy".into(),
            preset: Some("agy".into()),
            ..Default::default()
        });
        c.profiles.push(Profile {
            id: "gemini".into(),
            preset: Some("gemini".into()),
            enabled: false,
            ..Default::default()
        });
        let c = resolve(c, dir.path()).unwrap();
        let qa = c.profiles.iter().find(|p| p.id == "qa").unwrap();
        assert_eq!(qa.command()[0], "agy");
        assert!(qa.is_identity());

        let mut proj = Project {
            github: None,
            delivery_mode: Default::default(),
            agent_overrides: BTreeMap::from([
                ("developer".into(), "agy".into()),
                ("reviewer".into(), "gemini".into()), // disabled globally -> ignored!
            ]),
            id: "p1".into(),
            name: "Demo".into(),
            directory: "/tmp".into(),
            location: None,
            workflow: Default::default(),
            target_branch: None,
            created_at: 0,
        };
        let dev = c.profiles.iter().find(|p| p.id == "developer").unwrap();
        let overridden = apply_project_override(&c, Some(&proj), dev);
        assert_eq!(overridden.id, "developer");
        assert_eq!(overridden.role_instruction, dev.role_instruction);
        assert_eq!(overridden.command()[0], "agy");

        let rev = c.profiles.iter().find(|p| p.id == "reviewer").unwrap();
        let disabled_ignored = apply_project_override(&c, Some(&proj), rev);
        assert_eq!(disabled_ignored.command()[0], "codex");

        proj.agent_overrides
            .insert("developer".into(), "herdr:my-tpl".into());
        let tpl_override = apply_project_override(&c, Some(&proj), dev);
        assert_eq!(tpl_override.command()[0], "my-tpl");
    }

    #[test]
    fn agy_command_passes_deduplicated_add_dirs_and_session_trusted_workspaces() {
        let dir = tempfile::tempdir().unwrap();
        let home_dir = dir.path().join("home");
        let proj_dir = dir.path().join("repo");
        let worktree_dir = home_dir.join("worktrees/repo/card-1");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mut cfg = Config::default();
        cfg.profiles.push(Profile {
            id: "agy".into(),
            preset: Some("agy".into()),
            model: Some("gemini-4-carbon-high".into()),
            effort: Some("high".into()),
            ..Default::default()
        });
        cfg.profiles.push(Profile {
            id: "agy-dev".into(),
            identity: true,
            preset: Some("agy".into()),
            role_instruction: "Build carefully.".into(),
            ..Default::default()
        });
        let cfg = resolve(cfg, dir.path()).unwrap();
        let agy_dev = cfg.profiles.iter().find(|p| p.id == "agy-dev").unwrap();
        // Identity inherits model & effort from @agy when not explicitly set on the identity
        assert_eq!(agy_dev.model.as_deref(), Some("gemini-4-carbon-high"));
        assert_eq!(agy_dev.effort.as_deref(), Some("high"));

        let home_s = home_dir.to_string_lossy().into_owned();
        let cwd_s = worktree_dir.to_string_lossy().into_owned();
        let proj_s = proj_dir.to_string_lossy().into_owned();
        let argv = command(
            agy_dev,
            None,
            true,
            &[
                ("home", &home_s),
                ("cwd", &cwd_s),
                ("project_dir", &proj_s),
                ("prompt", "hello"),
            ],
        )
        .unwrap();
        assert_eq!(argv[0], "agy");
        assert!(
            argv.windows(2)
                .any(|w| w == ["--model", "gemini-4-carbon-high"])
        );
        assert!(argv.windows(2).any(|w| w == ["--effort", "high"]));
        assert!(argv.windows(2).any(|w| w == ["--add-dir", home_s.as_str()]));
        assert!(argv.windows(2).any(|w| w == ["--add-dir", cwd_s.as_str()]));
        assert!(argv.windows(2).any(|w| w == ["--add-dir", proj_s.as_str()]));

        let settings_idx = argv.iter().position(|a| a == "--settings").unwrap();
        let settings_val: Value = serde_json::from_str(&argv[settings_idx + 1]).unwrap();
        let trusted = settings_val["trustedWorkspaces"].as_array().unwrap();
        let canon_cwd = std::fs::canonicalize(&worktree_dir)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let canon_proj = std::fs::canonicalize(&proj_dir)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            trusted
                .iter()
                .any(|v| v.as_str() == Some(canon_cwd.as_str()))
        );
        assert!(
            trusted
                .iter()
                .any(|v| v.as_str() == Some(canon_proj.as_str()))
        );

        // When cwd == project_dir, --add-dir is deduplicated cleanly
        let dedup_argv = command(
            agy_dev,
            Some("conv-123"),
            false,
            &[
                ("home", &home_s),
                ("cwd", &proj_s),
                ("project_dir", &proj_s),
                ("prompt", "resume task"),
            ],
        )
        .unwrap();
        let add_dir_count = dedup_argv.iter().filter(|a| *a == "--add-dir").count();
        assert_eq!(add_dir_count, 2); // home + proj_s only!
        assert!(
            dedup_argv
                .windows(2)
                .any(|w| w == ["--conversation", "conv-123"])
        );
    }

    #[test]
    fn codex_and_claude_model_and_effort_flags_expand_in_start_and_resume() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.profiles[0].model = Some("gpt-6.1-sol".into());
        cfg.profiles[0].effort = Some("xhigh".into());
        cfg.profiles[1].model = Some("opus".into());
        cfg.profiles[1].effort = Some("max".into());
        // Override @developer explicitly while @reviewer inherits from @codex
        cfg.profiles[2].model = Some("gpt-5.4".into());
        cfg.profiles[2].effort = Some("medium".into());

        let cfg = resolve(cfg, dir.path()).unwrap();
        let dev = cfg.profiles.iter().find(|p| p.id == "developer").unwrap();
        let rev = cfg.profiles.iter().find(|p| p.id == "reviewer").unwrap();
        let claude = cfg.profiles.iter().find(|p| p.id == "claude").unwrap();
        assert_eq!(dev.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(dev.effort.as_deref(), Some("medium"));
        assert_eq!(rev.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(rev.effort.as_deref(), Some("xhigh"));

        let dev_resume = command(
            dev,
            Some("thread-1"),
            false,
            &[("home", "/tmp/h"), ("prompt", "go")],
        )
        .unwrap();
        assert_eq!(dev_resume[0], "codex");
        assert_eq!(
            &dev_resume[1..5],
            &["-m", "gpt-5.4", "-c", "model_reasoning_effort=\"medium\""]
        );
        assert!(dev_resume.windows(2).any(|w| w == ["resume", "thread-1"]));

        let claude_start = command(
            claude,
            Some("sess-1"),
            true,
            &[("home", "/tmp/h"), ("prompt", "go")],
        )
        .unwrap();
        assert_eq!(claude_start[0], "claude");
        assert_eq!(&claude_start[1..5], &["--model", "opus", "--effort", "max"]);

        // Cross-provider project override replaces incompatible model/effort with target engine's
        let mut cfg_with_agy = cfg.clone();
        cfg_with_agy.profiles.push(Profile {
            id: "agy".into(),
            preset: Some("agy".into()),
            model: Some("gemini-4-carbon-high".into()),
            effort: Some("high".into()),
            ..Default::default()
        });
        let proj = Project {
            github: None,
            delivery_mode: Default::default(),
            agent_overrides: BTreeMap::from([("developer".into(), "agy".into())]),
            id: "p1".into(),
            name: "Demo".into(),
            directory: "/tmp".into(),
            location: None,
            workflow: Default::default(),
            target_branch: None,
            created_at: 0,
        };
        let dev_on_agy = apply_project_override(&cfg_with_agy, Some(&proj), dev);
        assert_eq!(dev_on_agy.model.as_deref(), Some("gemini-4-carbon-high"));
        assert_eq!(dev_on_agy.effort.as_deref(), Some("high"));
    }

    #[test]
    fn migrate_upgrades_unmodified_builtin_role_instructions_and_preserves_custom_ones() {
        let mut legacy = json!({
            "profiles": [
                {
                    "id": "developer",
                    "preset": "codex",
                    "identity": true,
                    "role_instruction": "You are the Card's development Agent. Implement the agreed task, preserve relevant project conventions, and explain your changes and validation."
                },
                {
                    "id": "reviewer",
                    "preset": "codex",
                    "identity": true,
                    "role_instruction": "Custom security-focused reviewer prompt."
                }
            ]
        });
        assert!(migrate(&mut legacy).unwrap());
        assert_eq!(
            legacy["profiles"][0]["role_instruction"].as_str(),
            Some(
                Config::default()
                    .profiles
                    .iter()
                    .find(|p| p.id == "developer")
                    .unwrap()
                    .role_instruction
                    .as_str()
            )
        );
        assert_eq!(
            legacy["profiles"][1]["role_instruction"].as_str(),
            Some("Custom security-focused reviewer prompt.")
        );
        assert!(!migrate(&mut legacy).unwrap());
    }
}

#[cfg(test)]
mod machine_tests {
    use super::*;
    #[test]
    fn profile_commands_remain_portable_until_execution_placement_is_known() {
        let home = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.profiles[0].argv = Some(vec!["sh".into(), "-l".into()]);
        let resolved = resolve(config, home.path()).unwrap();
        assert_eq!(resolved.profiles[0].command()[0], "sh");
        assert!(Path::new(&executable("sh")).is_absolute());
    }
}
