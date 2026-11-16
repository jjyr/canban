//! Resolve editable presets and prepare provider-adapted process launches.
mod adapters;
pub mod launch;
pub use launch::{LaunchRequest, LaunchSpec, prepare_launch};
pub mod catalog;
pub use catalog::{
    CatalogEntry, ModelOption, annotate_agy_model_efforts, builtin_catalog,
    catalog_entry_for_profile, display_name, herdr_template_profile, identify_agent_process,
    identify_herdr_agent, model_supported_efforts, split_agy_effort_suffix,
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

/// Split a POSIX-style argument string into tokens, honoring single quotes (`'...'`),
/// double quotes (`"..."`), and `\` escapes without invoking a shell.
pub fn split_posix_words(input: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            Some('"') => match ch {
                '"' => quote = None,
                '\\' => {
                    if let Some(&next) = chars.peek() {
                        if matches!(next, '"' | '\\' | '$' | '`' | '\n') {
                            let escaped = chars.next().unwrap();
                            if escaped != '\n' {
                                current.push(escaped);
                            }
                        } else {
                            current.push('\\');
                        }
                    } else {
                        bail!("Trailing backslash in double-quoted argument");
                    }
                }
                _ => current.push(ch),
            },
            Some(_) => unreachable!(),
            None => match ch {
                c if c.is_whitespace() => {
                    if in_word {
                        words.push(std::mem::take(&mut current));
                        in_word = false;
                    }
                }
                '\'' | '"' => {
                    in_word = true;
                    quote = Some(ch);
                }
                '\\' => {
                    in_word = true;
                    match chars.next() {
                        Some('\n') => {}
                        Some(escaped) => current.push(escaped),
                        None => bail!("Trailing backslash at end of arguments"),
                    }
                }
                _ => {
                    in_word = true;
                    current.push(ch);
                }
            },
        }
    }

    if let Some(q) = quote {
        bail!("Unclosed quote ({q}) in arguments");
    }
    if in_word {
        words.push(current);
    }
    Ok(words)
}

/// Format an argument slice back into a human-editable POSIX string that round-trips
/// through [`split_posix_words`]. Plain tokens (including `{prompt}` placeholders) stay unquoted.
pub fn join_posix_words(words: &[String]) -> String {
    words
        .iter()
        .map(|w| {
            if w.is_empty() {
                "''".to_owned()
            } else if !w
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '\\'))
            {
                w.clone()
            } else if !w.contains('\'') {
                format!("'{w}'")
            } else {
                let mut out = String::with_capacity(w.len() + 2);
                out.push('"');
                for c in w.chars() {
                    if matches!(c, '"' | '\\' | '$' | '`') {
                        out.push('\\');
                    }
                    out.push(c);
                }
                out.push('"');
                out
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn sync_inherited_resume_binary(
    resume_argv: Option<Vec<String>>,
    custom_argv: Option<&[String]>,
    default_binary: Option<&str>,
) -> Option<Vec<String>> {
    let mut resume = resume_argv?;
    if let Some(custom_bin) = custom_argv.and_then(|a| a.first())
        && let Some(first) = resume.first_mut()
        && default_binary.is_none_or(|def| first == def)
    {
        *first = custom_bin.clone();
    }
    Some(resume)
}

pub fn resolve(config: Config, home: &Path) -> Result<Config> {
    let presets = presets(home)?;
    let mut config = config;
    config.hc = executable_with_env(&config.hc, &config.hc_env);
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
                let preset_default_bin = preset
                    .argv
                    .as_ref()
                    .and_then(|a| a.first())
                    .map(String::as_str);
                if profile.argv.is_none() {
                    profile.argv = peer_agent
                        .and_then(|p| p.argv.clone())
                        .or_else(|| preset.argv.clone());
                }
                if profile.resume_argv.is_none() {
                    let inherited_resume = peer_agent
                        .and_then(|p| p.resume_argv.clone())
                        .or_else(|| preset.resume_argv.clone());
                    profile.resume_argv = sync_inherited_resume_binary(
                        inherited_resume,
                        profile.argv.as_deref(),
                        preset_default_bin,
                    );
                }
                if profile.exit.is_none() {
                    profile.exit = peer_agent
                        .and_then(|p| p.exit.clone())
                        .or_else(|| preset.exit.clone());
                }
                if profile.session_instruction.is_none() {
                    profile.session_instruction = peer_agent
                        .and_then(|p| p.session_instruction.clone())
                        .or_else(|| preset.session_instruction.clone());
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
                let preset_default_bin = peer_preset
                    .and_then(|pp| pp.argv.as_ref())
                    .and_then(|a| a.first())
                    .map(String::as_str);
                if profile.argv.is_none() {
                    profile.argv = peer
                        .argv
                        .clone()
                        .or_else(|| peer_preset.and_then(|pp| pp.argv.clone()));
                }
                if profile.resume_argv.is_none() {
                    let inherited_resume = peer
                        .resume_argv
                        .clone()
                        .or_else(|| peer_preset.and_then(|pp| pp.resume_argv.clone()));
                    profile.resume_argv = sync_inherited_resume_binary(
                        inherited_resume,
                        profile.argv.as_deref(),
                        preset_default_bin,
                    );
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
        let mut resolved = peer.clone();
        if let Some(preset_name) = resolved.preset.as_deref() {
            if let Some(tpl) = preset_name.strip_prefix("herdr:") {
                ensure!(!tpl.trim().is_empty(), "Empty Herdr template");
                let p = herdr_template_profile(tpl);
                if resolved.argv.is_none() {
                    resolved.argv = p.argv;
                }
            } else if let Some(entry) = builtin_catalog().into_iter().find(|e| e.id == preset_name)
            {
                let p = entry.to_preset_profile();
                let def_bin = p.argv.as_ref().and_then(|a| a.first()).map(String::as_str);
                if resolved.argv.is_none() {
                    resolved.argv = p.argv.clone();
                }
                if resolved.resume_argv.is_none() {
                    resolved.resume_argv = sync_inherited_resume_binary(
                        p.resume_argv,
                        resolved.argv.as_deref(),
                        def_bin,
                    );
                }
                if resolved.exit.is_none() {
                    resolved.exit = p.exit;
                }
                if resolved.session_instruction.is_none() {
                    resolved.session_instruction = p.session_instruction;
                }
            } else if let Some(home) = home
                && let Ok(map) = presets(home)
                && let Some(p) = map.get(preset_name)
            {
                let def_bin = p.argv.as_ref().and_then(|a| a.first()).map(String::as_str);
                if resolved.argv.is_none() {
                    resolved.argv = p.argv.clone();
                }
                if resolved.resume_argv.is_none() {
                    resolved.resume_argv = sync_inherited_resume_binary(
                        p.resume_argv.clone(),
                        resolved.argv.as_deref(),
                        def_bin,
                    );
                }
                if resolved.exit.is_none() {
                    resolved.exit = p.exit.clone();
                }
                if resolved.session_instruction.is_none() {
                    resolved.session_instruction = p.session_instruction.clone();
                }
            }
        }
        return Ok(resolved);
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

/// Apply optional Project-level identity override (`project.agent_overrides`)
/// while preserving the identity's `id` and `role_instruction`.
pub fn apply_project_override(
    config: &Config,
    project: Option<&Project>,
    profile: &Profile,
) -> Profile {
    if !profile.is_identity() {
        return profile.clone();
    }
    let Some(project) = project else {
        return profile.clone();
    };
    let Some(ov) = project
        .agent_overrides
        .get(&profile.id)
        .map(canban_types::ProjectAgentOverride::normalized)
        .filter(|ov| !ov.is_empty())
    else {
        return profile.clone();
    };
    let mut out = profile.clone();
    if let Some(target) = ov.agent.as_deref() {
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
    }
    if let Some(m) = ov.model {
        out.model = Some(m);
    }
    if let Some(e) = ov.effort {
        out.effort = Some(e);
    }
    out
}

/// Find an executable path on the local host if installed; returns `None` when missing.
pub fn find_local_executable(program: &str) -> Option<PathBuf> {
    find_local_executable_with_env(program, &BTreeMap::new())
}

/// Find a local executable, honoring optional overrides (`CANBAN_HERDR_SEARCH_PATHS`,
/// `HC_INSTALL_DIR`) from `env` or process environment, and falling back from default
/// `hc` to `herdr-core` when another tool did not claim `hc`.
pub fn find_local_executable_with_env(
    program: &str,
    env: &BTreeMap<String, String>,
) -> Option<PathBuf> {
    let program = program.trim();
    if program.is_empty() {
        return None;
    }
    if Path::new(program).components().count() != 1 {
        let p = PathBuf::from(program);
        return p.is_file().then_some(p);
    }
    let search_override = env
        .get("CANBAN_HERDR_SEARCH_PATHS")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("CANBAN_HERDR_SEARCH_PATHS"));
    let paths: Vec<PathBuf> = if let Some(override_paths) = search_override {
        std::env::split_paths(&override_paths).collect()
    } else {
        let mut paths = Vec::new();
        if let Some(install_dir) = env
            .get("HC_INSTALL_DIR")
            .map(std::ffi::OsString::from)
            .or_else(|| std::env::var_os("HC_INSTALL_DIR"))
        {
            let dir = PathBuf::from(install_dir);
            if !dir.as_os_str().is_empty() {
                paths.push(dir);
            }
        }
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        if let Some(home) = env
            .get("HOME")
            .map(std::ffi::OsString::from)
            .or_else(|| std::env::var_os("HOME"))
        {
            let home = PathBuf::from(home);
            paths.push(home.join(".local/bin"));
            paths.push(home.join(".cargo/bin"));
            paths.push(home.join(".bun/bin"));
            paths.push(home.join("bin"));
        }
        paths.extend(["/opt/homebrew/bin".into(), "/usr/local/bin".into()]);
        paths
    };
    let candidates: &[&str] = if program == "hc" {
        &["hc", "herdr-core"]
    } else {
        std::slice::from_ref(&program)
    };
    for name in candidates {
        if let Some(found) = paths.iter().map(|p| p.join(name)).find(|p| p.is_file()) {
            return Some(found);
        }
    }
    None
}

pub fn executable(program: &str) -> String {
    executable_with_env(program, &BTreeMap::new())
}

pub fn executable_with_env(program: &str, env: &BTreeMap<String, String>) -> String {
    find_local_executable_with_env(program, env)
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
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../prompts/profiles/role_developer.txt"
                ))
                .trim_end(),
            )),
            Some("reviewer") => Some((
                "You are the Card's independent review Agent. Inspect the actual changes critically, identify correctness issues and missing validation, and report findings. Do not implement changes unless explicitly asked.",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../prompts/profiles/role_reviewer.txt"
                ))
                .trim_end(),
            )),
            Some("planner") => Some((
                "You are the Card's planning Agent. Analyze the task and relevant repository files in read-only mode, identify risks, and write a concise implementation plan without modifying source files.",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../prompts/profiles/role_planner.txt"
                ))
                .trim_end(),
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

    // Upgrade pre-@merger configs that already have the three built-in role identities
    // (`developer`, `reviewer`, `planner`) so `@merger` appears automatically in Agent settings,
    // Project Agent overrides, and Queue Merge conflict resolution without losing the user's
    // configured engine.
    let profiles = value["profiles"]
        .as_array_mut()
        .context("profiles must be an array")?;
    let has_merger = profiles.iter().any(|p| p["id"].as_str() == Some("merger"));
    let has_legacy_builtin_roles = ["developer", "reviewer", "planner"]
        .iter()
        .all(|id| profiles.iter().any(|p| p["id"].as_str() == Some(*id)));
    if !has_merger && has_legacy_builtin_roles {
        let dev = profiles
            .iter()
            .find(|p| p["id"].as_str() == Some("developer"));
        let preset = dev
            .and_then(|p| p["preset"].as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("codex")
            .to_owned();
        let enabled = dev.and_then(|p| p["enabled"].as_bool()).unwrap_or(true);
        let mut merger = json!({
            "id": "merger",
            "identity": true,
            "preset": preset,
            "role_instruction": include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../prompts/profiles/role_merger.txt"
            ))
            .trim_end(),
        });
        if !enabled {
            merger["enabled"] = json!(false);
        }
        profiles.push(merger);
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
    let extra_dirs: Vec<&str> = lookup("extra_dirs")
        .map(|raw| {
            raw.split(['\0', '\n'])
                .map(str::trim)
                .filter(|dir| !dir.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let mut trusted_dirs = Vec::new();
    push_trusted_dir(&mut trusted_dirs, cwd_val);
    push_trusted_dir(&mut trusted_dirs, project_dir_val);
    push_trusted_dir(&mut trusted_dirs, home_val);
    for dir in &extra_dirs {
        push_trusted_dir(&mut trusted_dirs, dir);
    }
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

    // Filter out unsupported or incompatible effort levels for the effective model so
    // inherited Agent/Role effort (e.g. `medium`) is omitted when a role overrides to an
    // effort-less model (such as `gemini-pro-orchestrator` or `gpt-oss-120b-medium`).
    let effective_effort = effort_val.filter(|eff| match &catalog_meta {
        Some(meta) if !uses_effort_placeholder => {
            if meta.effort_args.is_empty() {
                false
            } else {
                match model_supported_efforts(meta, &meta.default_models, model_val) {
                    None => false,
                    Some(levels) if levels.is_empty() => false,
                    Some(levels) => levels.iter().any(|l| l == eff),
                }
            }
        }
        _ => true,
    });

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
        if let Some(eff) = effective_effort
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
    replacements.push(("effort", effective_effort.unwrap_or("")));
    replacements.push(("session_id", session.unwrap_or("")));

    // Deduplicate consecutive or repeated `--add-dir <value>` pairs and omit empty `--add-dir ""` pairs.
    let expanded: Vec<String> = args
        .iter()
        .map(|arg| expand_template(arg, &replacements))
        .collect();
    let mut final_args = Vec::with_capacity(expanded.len() + extra_dirs.len() * 2);
    let mut seen_add_dirs: Vec<String> = Vec::new();
    let mut last_add_dir_end: Option<usize> = None;
    let mut i = 0;
    while i < expanded.len() {
        if expanded[i] == "--add-dir" && i + 1 < expanded.len() {
            let dir = expanded[i + 1].trim();
            if !dir.is_empty() && !seen_add_dirs.iter().any(|d| d == dir) {
                seen_add_dirs.push(dir.to_owned());
                final_args.push("--add-dir".into());
                final_args.push(expanded[i + 1].clone());
                last_add_dir_end = Some(final_args.len());
            }
            i += 2;
            continue;
        }
        final_args.push(expanded[i].clone());
        i += 1;
    }
    if let Some(mut insert_at) = last_add_dir_end {
        for dir in extra_dirs {
            if !seen_add_dirs.iter().any(|d| d == dir) {
                seen_add_dirs.push(dir.to_owned());
                final_args.splice(insert_at..insert_at, ["--add-dir".into(), dir.to_owned()]);
                insert_at += 2;
            }
        }
    }
    Ok(final_args)
}

fn strip_prompt_args(raw: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if i > 0 {
            if matches!(
                raw[i].as_str(),
                "-i" | "--prompt" | "--prompt-interactive" | "-p" | "--print"
            ) && i + 1 < raw.len()
                && raw[i + 1].contains("{prompt}")
            {
                i += 2;
                continue;
            }
            if raw[i].contains("{prompt}") {
                i += 1;
                continue;
            }
        }
        out.push(raw[i].clone());
        i += 1;
    }
    out
}

/// Expand an interactive CLI Agent command with no initial prompt and no Resume binding.
pub fn interactive_command(
    profile: &Profile,
    session: Option<&str>,
    vars: &[(&str, &str)],
) -> Result<Vec<String>> {
    let mut clean = profile.clone();
    clean.argv = Some(strip_prompt_args(profile.command()));
    clean.resume_argv = None;
    command(&clean, session, true, vars)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overrides_replace_whole_commands_and_presets_do_not_install_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Config::default();
        c.profiles[2].argv = Some(vec!["own-codex-wrapper".into(), "{prompt}".into()]);
        c.profiles[2].resume_argv = Some(vec![]);
        c.profiles[2].exit = Some(ExitAction::Manual);
        let c = resolve(c, dir.path()).unwrap();
        assert_eq!(c.profiles[2].command()[0], "own-codex-wrapper");
        assert!(command(&c.profiles[2], Some("thread"), false, &[]).is_err());
        assert_eq!(c.profiles[2].exit, Some(ExitAction::Manual));
        let argv = command(
            &c.profiles[0],
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
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: BTreeMap::from([
                (
                    "developer".into(),
                    canban_types::ProjectAgentOverride::from_agent("agy"),
                ),
                (
                    "reviewer".into(),
                    canban_types::ProjectAgentOverride::from_agent("gemini"),
                ), // disabled globally -> ignored!
            ]),
            tags: Default::default(),
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

        proj.agent_overrides.insert(
            "developer".into(),
            canban_types::ProjectAgentOverride::from_agent("herdr:my-tpl"),
        );
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
    fn per_model_effort_capabilities_omit_unsupported_flags_for_orchestrator_and_fixed_tier_models()
    {
        let dir = tempfile::tempdir().unwrap();
        let agy_entry = builtin_catalog()
            .into_iter()
            .find(|e| e.id == "agy")
            .unwrap();
        let orch = agy_entry
            .default_models
            .iter()
            .find(|m| m.id == "gemini-pro-orchestrator")
            .unwrap();
        assert_eq!(orch.supported_efforts, Some(Vec::new()));
        let orch_argon = agy_entry
            .default_models
            .iter()
            .find(|m| m.id == "gemini-pro-orchestrator-argon")
            .unwrap();
        assert_eq!(orch_argon.supported_efforts, Some(Vec::new()));
        let fixed_oss = agy_entry
            .default_models
            .iter()
            .find(|m| m.id == "gpt-oss-120b-medium")
            .unwrap();
        assert_eq!(fixed_oss.supported_efforts, Some(Vec::new()));

        let carbon = agy_entry
            .default_models
            .iter()
            .find(|m| m.id == "gemini-4-carbon-high")
            .unwrap();
        assert_eq!(
            carbon.supported_efforts,
            Some(vec!["low".into(), "medium".into(), "high".into()])
        );
        let hydrogen = agy_entry
            .default_models
            .iter()
            .find(|m| m.id == "gemini-4-hydrogen-high")
            .unwrap();
        assert_eq!(
            hydrogen.supported_efforts,
            Some(vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "xhigh".into()
            ])
        );

        // Inherited effort (`medium` from `@agy`) is omitted when `@merger` overrides to
        // `gemini-pro-orchestrator` or when `xhigh` is paired with `gemini-4-carbon-high`.
        let mut cfg = Config::default();
        cfg.profiles.push(Profile {
            id: "agy".into(),
            preset: Some("agy".into()),
            model: Some("gemini-4-carbon-medium".into()),
            effort: Some("medium".into()),
            ..Default::default()
        });
        if let Some(merger) = cfg.profiles.iter_mut().find(|p| p.id == "merger") {
            merger.preset = Some("agy".into());
            merger.model = Some("gemini-pro-orchestrator".into());
        }
        let cfg = resolve(cfg, dir.path()).unwrap();
        let merger = cfg.profiles.iter().find(|p| p.id == "merger").unwrap();
        assert_eq!(merger.model.as_deref(), Some("gemini-pro-orchestrator"));
        assert_eq!(merger.effort.as_deref(), Some("medium"));

        let merger_argv = command(
            merger,
            None,
            true,
            &[("home", "/tmp/h"), ("cwd", "/tmp/wt"), ("prompt", "merge")],
        )
        .unwrap();
        assert!(
            merger_argv
                .windows(2)
                .any(|w| w == ["--model", "gemini-pro-orchestrator"])
        );
        assert!(
            !merger_argv.iter().any(|a| a == "--effort"),
            "--effort must be omitted for gemini-pro-orchestrator: {merger_argv:?}"
        );

        // Unsupported effort tier (`xhigh`) on a `low/medium/high` model (`gemini-4-carbon-high`)
        // is also omitted, while supported `xhigh` on `gemini-4-hydrogen-high` is passed through.
        let mut carbon_xhigh = merger.clone();
        carbon_xhigh.model = Some("gemini-4-carbon-high".into());
        carbon_xhigh.effort = Some("xhigh".into());
        let carbon_argv = command(&carbon_xhigh, None, true, &[("home", "/tmp/h")]).unwrap();
        assert!(!carbon_argv.iter().any(|a| a == "--effort"));

        let mut hydrogen_xhigh = merger.clone();
        hydrogen_xhigh.model = Some("gemini-4-hydrogen-high".into());
        hydrogen_xhigh.effort = Some("xhigh".into());
        let hydrogen_argv = command(&hydrogen_xhigh, None, true, &[("home", "/tmp/h")]).unwrap();
        assert!(hydrogen_argv.windows(2).any(|w| w == ["--effort", "xhigh"]));

        // Claude Haiku also omits `--effort` even when global `@claude` sets an effort.
        let claude = cfg.profiles.iter().find(|p| p.id == "claude").unwrap();
        let mut claude_haiku = claude.clone();
        claude_haiku.model = Some("haiku".into());
        claude_haiku.effort = Some("high".into());
        let haiku_argv = command(
            &claude_haiku,
            Some("sess-1"),
            true,
            &[("home", "/tmp/h"), ("prompt", "hi")],
        )
        .unwrap();
        assert!(haiku_argv.windows(2).any(|w| w == ["--model", "haiku"]));
        assert!(!haiku_argv.iter().any(|a| a == "--effort"));
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
        let mut proj = Project {
            github: None,
            detected_github: None,
            github_disconnected: false,
            delivery_mode: Default::default(),
            agent_overrides: BTreeMap::from([(
                "developer".into(),
                canban_types::ProjectAgentOverride::from_agent("agy"),
            )]),
            tags: Default::default(),
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

        // Explicit Project model & effort overrides take precedence over global identity/engine defaults
        proj.agent_overrides.insert(
            "developer".into(),
            canban_types::ProjectAgentOverride {
                agent: Some("agy".into()),
                model: Some("gemini-4-carbon-low".into()),
                effort: Some("low".into()),
            },
        );
        let dev_custom_agy = apply_project_override(&cfg_with_agy, Some(&proj), dev);
        assert_eq!(dev_custom_agy.command()[0], "agy");
        assert_eq!(dev_custom_agy.model.as_deref(), Some("gemini-4-carbon-low"));
        assert_eq!(dev_custom_agy.effort.as_deref(), Some("low"));

        // Model/effort-only Project override keeps global backing Agent (codex)
        proj.agent_overrides.insert(
            "reviewer".into(),
            canban_types::ProjectAgentOverride {
                agent: None,
                model: Some("gpt-5.4-mini".into()),
                effort: Some("low".into()),
            },
        );
        let rev_model_only = apply_project_override(&cfg_with_agy, Some(&proj), rev);
        assert_eq!(rev_model_only.command()[0], "codex");
        assert_eq!(rev_model_only.model.as_deref(), Some("gpt-5.4-mini"));
        assert_eq!(rev_model_only.effort.as_deref(), Some("low"));

        // Legacy string format in stored Project JSON still deserializes cleanly
        let legacy_proj: Project = serde_json::from_value(json!({
            "id": "p-legacy",
            "name": "Legacy",
            "directory": "/tmp",
            "workflow": canban_types::Workflow::default(),
            "created_at": 0,
            "agent_overrides": {
                "developer": "agy"
            }
        }))
        .unwrap();
        assert_eq!(
            legacy_proj.agent_overrides["developer"].agent.as_deref(),
            Some("agy")
        );
        assert!(legacy_proj.agent_overrides["developer"].model.is_none());
    }
    #[test]
    fn interactive_command_strips_prompt_flags_and_never_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.profiles.push(Profile {
            id: "agy".into(),
            preset: Some("agy".into()),
            model: Some("gemini-4-carbon-high".into()),
            effort: Some("high".into()),
            ..Default::default()
        });
        cfg.profiles.push(Profile {
            id: "gemini".into(),
            preset: Some("gemini".into()),
            ..Default::default()
        });
        cfg.profiles.push(Profile {
            id: "opencode".into(),
            preset: Some("opencode".into()),
            ..Default::default()
        });
        cfg.profiles.push(Profile {
            id: "copilot".into(),
            preset: Some("copilot".into()),
            ..Default::default()
        });
        cfg.profiles.push(Profile {
            id: "tpl".into(),
            preset: Some("herdr:my-tpl".into()),
            ..Default::default()
        });
        let cfg = resolve(cfg, dir.path()).unwrap();

        let dev = cfg.profiles.iter().find(|p| p.id == "developer").unwrap();
        let codex_args = interactive_command(
            dev,
            Some("ignored-resume-id"),
            &[("home", "/tmp/home"), ("cwd", "/tmp/worktree")],
        )
        .unwrap();
        assert_eq!(codex_args[0], "codex");
        assert!(
            !codex_args
                .iter()
                .any(|a| a == "resume" || a.is_empty() || a.contains("{prompt}"))
        );

        let claude = cfg.profiles.iter().find(|p| p.id == "claude").unwrap();
        let claude_args = interactive_command(
            claude,
            Some("11111111-2222-3333-4444-555555555555"),
            &[("home", "/tmp/home")],
        )
        .unwrap();
        assert_eq!(
            claude_args,
            vec![
                "claude",
                "--session-id",
                "11111111-2222-3333-4444-555555555555"
            ]
        );

        let agy = cfg.profiles.iter().find(|p| p.id == "agy").unwrap();
        let agy_args = interactive_command(
            agy,
            Some("conv-1"),
            &[
                ("home", "/tmp/home"),
                ("cwd", "/tmp/wt"),
                ("project_dir", "/tmp/repo"),
            ],
        )
        .unwrap();
        assert_eq!(agy_args[0], "agy");
        assert!(
            !agy_args
                .iter()
                .any(|a| a == "-i" || a == "--conversation" || a.contains("{prompt}"))
        );
        assert!(agy_args.iter().any(|a| a == "--settings"));
        assert!(
            agy_args
                .windows(2)
                .any(|w| w == ["--model", "gemini-4-carbon-high"])
        );

        for (id, forbidden) in [
            ("gemini", "-i"),
            ("opencode", "--prompt"),
            ("copilot", "-i"),
        ] {
            let p = cfg.profiles.iter().find(|p| p.id == id).unwrap();
            let args = interactive_command(p, Some("sess-1"), &[("home", "/tmp/home")]).unwrap();
            assert!(!args.iter().any(|a| a == forbidden
                || a == "--resume"
                || a == "--session"
                || a.contains("{prompt}")));
        }

        let tpl = cfg.profiles.iter().find(|p| p.id == id_or("tpl")).unwrap();
        let tpl_args = interactive_command(tpl, None, &[]).unwrap();
        assert_eq!(tpl_args, vec!["my-tpl"]);
    }

    fn id_or(s: &str) -> &str {
        s
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

    #[test]
    fn migrate_adds_builtin_merger_role_inheriting_developer_engine_for_existing_configs() {
        let mut onboarded = json!({
            "onboarded": true,
            "default_agent": "developer",
            "profiles": [
                { "id": "agy", "preset": "agy", "enabled": true },
                { "id": "developer", "preset": "agy", "identity": true, "role_instruction": "Dev" },
                { "id": "reviewer", "preset": "agy", "identity": true, "role_instruction": "Rev" },
                { "id": "planner", "preset": "agy", "identity": true, "role_instruction": "Plan" }
            ]
        });
        assert!(migrate(&mut onboarded).unwrap());
        let profiles = onboarded["profiles"].as_array().unwrap();
        let merger = profiles
            .iter()
            .find(|p| p["id"].as_str() == Some("merger"))
            .expect("@merger added by migration");
        assert_eq!(merger["identity"], json!(true));
        assert_eq!(merger["preset"], json!("agy"));
        assert!(
            merger["role_instruction"]
                .as_str()
                .unwrap()
                .contains("merge queue Agent")
        );
        // Idempotent on second pass.
        assert!(!migrate(&mut onboarded).unwrap());
    }

    #[test]
    fn agy_extra_dirs_adds_trusted_workspaces_and_deduplicated_add_dir_flags() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("project");
        let merge_queue = dir.path().join("worktrees/_merge_queue");
        let card1_wt = dir.path().join("worktrees/c1");
        let card2_wt = dir.path().join("worktrees/c2");
        for path in [&home, &project, &merge_queue, &card1_wt, &card2_wt] {
            std::fs::create_dir_all(path).unwrap();
        }
        let mut cfg = Config::default();
        cfg.profiles.push(Profile {
            id: "agy".into(),
            preset: Some("agy".into()),
            ..Default::default()
        });
        let c = resolve(cfg, &home).unwrap();
        let agy = c.profiles.iter().find(|p| p.id == "agy").unwrap();
        let extra = format!(
            "{}\0{}\0{}",
            card1_wt.display(),
            card2_wt.display(),
            merge_queue.display()
        );
        let argv = command(
            agy,
            None,
            true,
            &[
                ("home", home.to_str().unwrap()),
                ("cwd", merge_queue.to_str().unwrap()),
                ("project_dir", project.to_str().unwrap()),
                ("extra_dirs", &extra),
                ("prompt", "Resolve batch conflict"),
            ],
        )
        .unwrap();

        let settings_idx = argv.iter().position(|a| a == "--settings").unwrap();
        let settings: Value = serde_json::from_str(&argv[settings_idx + 1]).unwrap();
        let trusted: Vec<&str> = settings["trustedWorkspaces"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for expected in [&home, &project, &merge_queue, &card1_wt, &card2_wt] {
            let raw = expected.to_str().unwrap();
            let canon = std::fs::canonicalize(expected)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            assert!(trusted.contains(&raw), "missing raw trusted dir {raw}");
            assert!(
                trusted.contains(&canon.as_str()),
                "missing canonical trusted dir {canon}"
            );
        }

        let add_dirs: Vec<&str> = argv
            .windows(2)
            .filter_map(|w| (w[0] == "--add-dir").then_some(w[1].as_str()))
            .collect();
        assert!(add_dirs.contains(&card1_wt.to_str().unwrap()));
        assert!(add_dirs.contains(&card2_wt.to_str().unwrap()));
        // `_merge_queue` was already passed as `cwd`, so `--add-dir` must not duplicate it.
        assert_eq!(
            add_dirs
                .iter()
                .filter(|d| **d == merge_queue.to_str().unwrap())
                .count(),
            1
        );
        assert_eq!(
            argv.last().map(String::as_str),
            Some("Resolve batch conflict")
        );
    }

    #[test]
    fn posix_words_roundtrip_and_custom_agent_argv_syncs_resume_and_identities() {
        let parsed = split_posix_words(
            r#"--add-dir {home} --settings '{"trusted":["/my repo"]}' --label "hello \"world\"" -i {prompt}"#,
        )
        .unwrap();
        assert_eq!(
            parsed,
            vec![
                "--add-dir",
                "{home}",
                "--settings",
                r#"{"trusted":["/my repo"]}"#,
                "--label",
                r#"hello "world""#,
                "-i",
                "{prompt}",
            ]
        );
        let joined = join_posix_words(&parsed);
        assert_eq!(split_posix_words(&joined).unwrap(), parsed);
        assert!(split_posix_words("--flag 'unterminated").is_err());
        assert!(split_posix_words("--flag trailing\\").is_err());

        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.profiles.push(Profile {
            id: "agy".into(),
            preset: Some("agy".into()),
            argv: Some(vec![
                "/bin/agy-modified".into(),
                "--custom".into(),
                "-i".into(),
                "{prompt}".into(),
            ]),
            ..Default::default()
        });
        cfg.profiles.push(Profile {
            id: "agy-dev".into(),
            identity: true,
            preset: Some("agy".into()),
            role_instruction: "Dev".into(),
            ..Default::default()
        });
        let resolved = resolve(cfg, dir.path()).unwrap();
        let agy = resolved.profiles.iter().find(|p| p.id == "agy").unwrap();
        assert_eq!(
            agy.command(),
            &["/bin/agy-modified", "--custom", "-i", "{prompt}"]
        );
        assert_eq!(
            agy.resume_command().first().map(String::as_str),
            Some("/bin/agy-modified"),
            "Inherited preset resume_argv[0] must follow custom argv[0]"
        );

        let agy_dev = resolved
            .profiles
            .iter()
            .find(|p| p.id == "agy-dev")
            .unwrap();
        assert_eq!(
            agy_dev.command(),
            &["/bin/agy-modified", "--custom", "-i", "{prompt}"],
            "Identities referencing @agy inherit its customized argv"
        );
        assert_eq!(
            agy_dev.resume_command().first().map(String::as_str),
            Some("/bin/agy-modified")
        );
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

    #[test]
    fn find_local_executable_honors_search_paths_override_and_herdr_core_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let mut env = BTreeMap::new();
        env.insert(
            "CANBAN_HERDR_SEARCH_PATHS".into(),
            bin.display().to_string(),
        );
        assert!(find_local_executable_with_env("hc", &env).is_none());

        // Fallback to `herdr-core` when `hc` is absent in search paths.
        let herdr_core = bin.join("herdr-core");
        std::fs::write(&herdr_core, "#!/bin/sh\n").unwrap();
        assert_eq!(
            find_local_executable_with_env("hc", &env),
            Some(herdr_core.clone())
        );

        // Prefer `hc` when both exist.
        let hc = bin.join("hc");
        std::fs::write(&hc, "#!/bin/sh\n").unwrap();
        assert_eq!(find_local_executable_with_env("hc", &env), Some(hc.clone()));

        // Explicit custom path checks exact file existence.
        let missing = dir.path().join("custom/hc");
        assert!(find_local_executable_with_env(&missing.display().to_string(), &env).is_none());
        std::fs::create_dir_all(missing.parent().unwrap()).unwrap();
        std::fs::write(&missing, "#!/bin/sh\n").unwrap();
        assert_eq!(
            find_local_executable_with_env(&missing.display().to_string(), &env),
            Some(missing)
        );
    }
}
