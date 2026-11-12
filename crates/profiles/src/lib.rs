//! Resolve editable presets and explicit overrides; no provider dispatch at runtime.
pub mod catalog;
pub use catalog::{CatalogEntry, builtin_catalog, display_name, herdr_template_profile};

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
        if let Some(name) = &profile.preset {
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
            } else if let Some(peer) = snapshot_profiles
                .iter()
                .find(|p| p.id == *name && !p.is_identity())
            {
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
    let mut out = profile.clone();
    out.preset = engine.preset.or_else(|| Some(target.to_owned()));
    out.argv = engine.argv;
    out.resume_argv = engine.resume_argv;
    out.exit = Some(engine.exit.unwrap_or(ExitAction::Manual));
    out.session_instruction = engine.session_instruction;
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

/// Expand arguments as individual values; never interpret a shell command.
pub fn command(
    profile: &Profile,
    session: Option<&str>,
    fresh: bool,
    vars: &[(&str, &str)],
) -> Result<Vec<String>> {
    let args = if !fresh && session.is_some() {
        ensure!(
            !profile.resume_command().is_empty(),
            "Profile @{} does not support Resume; choose Start with retained Card context explicitly",
            profile.id
        );
        profile.resume_command()
    } else {
        profile.command()
    };
    ensure!(!args.is_empty(), "Profile @{} has no command", profile.id);
    // Substitute only the original argument template. Text inserted by a
    // prompt/session value must not be interpreted as another placeholder.
    let mut replacements = vars.to_vec();
    replacements.push(("session_id", session.unwrap_or("")));
    Ok(args
        .iter()
        .map(|arg| {
            let mut rest = arg.as_str();
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
        })
        .collect())
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
