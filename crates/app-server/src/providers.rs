//! Provider command adaptation only. Herdr continues to own every PTY.
use super::*;

pub(super) fn prepare(
    profile: &Profile,
    session: Option<&str>,
    fresh: bool,
    cli: &Path,
    home: &Path,
    run_dir: &Path,
) -> Result<Profile> {
    let mut out = profile.clone();
    let kind = profile.provider_kind();
    if kind == "command" && session.is_none() {
        return Ok(out);
    }
    ensure!(
        profile.template.is_none() || kind == "command",
        "Interactive provider profiles use argv rather than an opaque Herdr template"
    );
    let base = if session.is_some() && !profile.resume_argv.is_empty() {
        profile.resume_argv.clone()
    } else {
        profile.argv.clone()
    };
    ensure!(!base.is_empty(), "Profile has no command");
    let mut args = base;
    match kind {
        "codex" => {
            args.retain(|a| a != "exec" && a != "--skip-git-repo-check");
            if let Some(session) = session
                && profile.resume_argv.is_empty()
            {
                args.insert(1, "resume".into());
                args.insert(2, session.into());
            }
            let notify = json!([cli.to_string_lossy(), "agent-notify", "codex"]);
            args.splice(1..1, ["-c".into(), format!("notify={notify}")]);
        }
        "claude" => {
            args.retain(|a| a != "-p" && a != "--print");
            let command = format!(
                "{} agent-notify claude",
                canban_context::shell_quote(&cli.to_string_lossy())
            );
            let settings = run_dir.join("lifecycle.json");
            let data = json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":command}]}]}});
            canban_context::atomic(&settings, &serde_json::to_vec(&data)?)?;
            let mut options = vec!["--settings".into(), settings.to_string_lossy().into_owned()];
            if let Some(session) = session {
                options.extend([
                    if fresh { "--session-id" } else { "--resume" }.into(),
                    session.into(),
                ]);
            }
            args.splice(1..1, options);
        }
        "custom" => {
            ensure!(
                session.is_none() || !profile.resume_argv.is_empty(),
                "This custom profile has no resume_argv"
            );
        }
        "command" => {}
        _ => bail!("Unsupported provider {kind}; use codex, claude, custom or command"),
    }
    for a in &mut args {
        *a = a
            .replace("{session_id}", session.unwrap_or(""))
            .replace("{home}", &home.to_string_lossy());
    }
    out.argv = args;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_resume_and_interactive_launch_options() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::default();
        let codex = config.profiles.iter().find(|p| p.id == "codex").unwrap();
        let out = prepare(
            codex,
            Some("exact-thread"),
            false,
            Path::new("/bin/canban"),
            dir.path(),
            dir.path(),
        )
        .unwrap();
        let index = out.argv.iter().position(|a| a == "resume").unwrap();
        assert_eq!(out.argv[index + 1], "exact-thread");
        assert!(!out.argv.iter().any(|a| a == "exec"));
        assert!(out.argv.iter().any(|a| a.starts_with("notify=")));
        let claude = config.profiles.iter().find(|p| p.id == "claude").unwrap();
        for (fresh, flag) in [(true, "--session-id"), (false, "--resume")] {
            let out = prepare(
                claude,
                Some("exact-thread"),
                fresh,
                Path::new("/bin/canban"),
                dir.path(),
                dir.path(),
            )
            .unwrap();
            let index = out.argv.iter().position(|a| a == flag).unwrap();
            assert_eq!(out.argv[index + 1], "exact-thread");
            assert!(!out.argv.iter().any(|a| a == "-p"));
            assert!(dir.path().join("lifecycle.json").exists());
        }
    }
}
