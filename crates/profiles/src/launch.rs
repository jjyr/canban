//! Provider adaptation between business orchestration and PTY transport.
use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use canban_types::Profile;

/// A rendered command and explicit per-invocation environment overrides.
pub struct LaunchRequest<'a> {
    pub profile: Option<&'a Profile>,
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct LaunchSpec {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: String,
}

/// Called after command rendering and host path resolution, before transport.
/// A missing profile denotes an ordinary shell/setup process.
pub fn prepare_launch(request: LaunchRequest<'_>) -> Result<LaunchSpec> {
    ensure!(!request.argv.is_empty(), "Launch command is empty");
    let mut spec = LaunchSpec {
        argv: request.argv,
        env: request.env,
        cwd: request.cwd,
    };
    crate::adapters::prepare(request.profile, &mut spec)?;
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(preset: Option<&str>) -> Profile {
        Profile {
            id: "developer".into(),
            preset: preset.map(str::to_owned),
            ..Profile::default()
        }
    }

    fn request<'a>(profile: Option<&'a Profile>, argv: &[&str]) -> LaunchRequest<'a> {
        LaunchRequest {
            profile,
            argv: argv.iter().map(|s| (*s).into()).collect(),
            env: BTreeMap::from([
                ("CANBAN_CARD".into(), "99".into()),
                ("CANBAN_RUN_DIR".into(), "/tmp/路径 with spaces".into()),
                (
                    "CUSTOM".into(),
                    "quotes \" slash \\ newline\n$(touch nope)\u{7f}".into(),
                ),
            ]),
            cwd: "/worktree".into(),
        }
    }

    #[test]
    fn codex_start_resume_and_interactive_keep_process_env_and_exact_values() {
        let p = profile(Some("codex"));
        for argv in [
            vec!["/custom/codex-wrapper", "prompt"],
            vec!["codex", "resume", "session-id", "prompt"],
            vec!["codex", "--no-daemon"],
        ] {
            let req = request(Some(&p), &argv);
            let env = req.env.clone();
            let spec = prepare_launch(req).unwrap();
            assert_eq!(spec.env, env);
            assert_eq!(spec.cwd, "/worktree");
            assert_eq!(&spec.argv[..argv.len()], argv.as_slice());
            for pair in spec.argv[argv.len()..].as_chunks::<2>().0 {
                assert_eq!(pair[0], "-c");
                let (key, value) = pair[1].split_once('=').unwrap();
                let key = key.strip_prefix("shell_environment_policy.set.").unwrap();
                assert_eq!(serde_json::from_str::<String>(value).unwrap(), env[key]);
            }
        }
    }

    #[test]
    fn explicit_overrides_follow_user_config_but_precede_option_terminator() {
        let p = profile(Some("codex"));
        let spec = prepare_launch(request(
            Some(&p),
            &[
                "codex",
                "-c",
                "shell_environment_policy.set.CANBAN_CARD=\"old\"",
                "resume",
                "id",
                "--",
                "-prompt",
            ],
        ))
        .unwrap();
        let marker = spec.argv.iter().position(|a| a == "--").unwrap();
        assert_eq!(&spec.argv[marker..], ["--", "-prompt"]);
        let injected = spec
            .argv
            .iter()
            .position(|a| a == "shell_environment_policy.set.CANBAN_CARD=\"99\"")
            .unwrap();
        assert!(injected > 4 && injected < marker);
    }

    #[test]
    fn other_agents_templates_untyped_commands_and_shells_are_unchanged() {
        for preset in [None, Some("claude"), Some("agy"), Some("herdr:codex")] {
            let p = profile(preset);
            let req = request(Some(&p), &["codex", "resume", "prompt"]);
            let expected = LaunchSpec {
                argv: req.argv.clone(),
                env: req.env.clone(),
                cwd: req.cwd.clone(),
            };
            assert_eq!(prepare_launch(req).unwrap(), expected);
        }
        let req = request(None, &["/bin/zsh", "-l"]);
        let expected = LaunchSpec {
            argv: req.argv.clone(),
            env: req.env.clone(),
            cwd: req.cwd.clone(),
        };
        assert_eq!(prepare_launch(req).unwrap(), expected);
    }

    #[test]
    fn rejects_ambiguous_codex_keys_and_empty_commands() {
        let p = profile(Some("codex"));
        let mut req = request(Some(&p), &["codex"]);
        req.env.insert("a.b".into(), "value".into());
        assert!(prepare_launch(req).unwrap_err().to_string().contains("a.b"));
        assert!(prepare_launch(request(None, &[])).is_err());
    }
}

#[cfg(test)]
mod acceptance {
    use super::*;
    use std::io::Write;
    use std::process::Stdio;

    /// No model calls: real Codex parses our argv, then executes shell commands
    /// in concurrent threads of one isolated app-server and a resumed thread.
    #[test]
    #[ignore = "requires installed Codex and Python; run mise run acceptance-agent-launch"]
    fn real_codex_thread_environment_isolation() {
        let binary = std::env::var("CANBAN_CODEX_BIN").unwrap_or_else(|_| "codex".into());
        let profile = Profile {
            preset: Some("codex".into()),
            ..Profile::default()
        };
        let mut requests = Vec::new();
        for (card, version) in [
            ("99", "first"),
            ("100", "second"),
            ("99", "resumed"),
            ("", "cardless"),
        ] {
            let mut env = BTreeMap::from([
                (
                    "CANBAN_RUN_DIR".into(),
                    format!("/tmp/acceptance {version}"),
                ),
                (
                    "CANBAN_ENDPOINT".into(),
                    format!("spool:/tmp/{version}/rpc"),
                ),
                (
                    "CUSTOM_VALUE".into(),
                    format!("{version}: \"quote\" \\ newline\n世界\u{7f}"),
                ),
            ]);
            if !card.is_empty() {
                env.insert("CANBAN_CARD".into(), card.into());
            }
            let spec = prepare_launch(LaunchRequest {
                profile: Some(&profile),
                argv: vec![binary.clone(), "app-server".into(), "--stdio".into()],
                env,
                cwd: "/tmp".into(),
            })
            .unwrap();
            requests.push(serde_json::json!({"argv": spec.argv, "env": spec.env}));
        }
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/acceptance-agent-launch.py");
        let mut child = canban_exec::command("python3")
            .arg(script)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(serde_json::to_string(&requests).unwrap().as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
    }
}
