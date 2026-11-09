//! Resolve editable presets and explicit overrides; no provider dispatch at runtime.
use anyhow::{Context, Result, bail, ensure};
use canban_types::{Config, ExitAction, Profile};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub fn presets(home: &Path) -> Result<BTreeMap<String, Profile>> {
    let mut presets = BTreeMap::new();
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
    for profile in &mut config.profiles {
        if let Some(name) = &profile.preset {
            let preset = presets
                .get(name)
                .with_context(|| format!("Unknown preset {name} for @{}", profile.id))?;
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

pub fn executable(program: &str) -> String {
    if Path::new(program).components().count() != 1 {
        return program.into();
    }
    let mut paths =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>();
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(std::path::PathBuf::from(home).join(".local/bin"));
    }
    paths.extend(["/opt/homebrew/bin".into(), "/usr/local/bin".into()]);
    paths
        .into_iter()
        .map(|p| p.join(program))
        .find(|p| p.is_file())
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
