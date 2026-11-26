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
                ("CARD_ID".into(), "card-99".into()),
                ("CANBAN_RUN_DIR".into(), "/tmp/路径 with spaces".into()),
                (
                    "CANBAN_CUSTOM".into(),
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
        req.env.insert("CANBAN_BAD.KEY".into(), "value".into());
        assert!(
            prepare_launch(req)
                .unwrap_err()
                .to_string()
                .contains("CANBAN_BAD.KEY")
        );
        assert!(prepare_launch(request(None, &[])).is_err());
    }

    #[test]
    fn codex_does_not_leak_secrets_or_host_env_onto_cli_argv() {
        let p = profile(Some("codex"));
        let mut req = request(Some(&p), &["codex", "resume", "session-id"]);
        req.env
            .insert("OPENAI_API_KEY".into(), "sk-secret-123".into());
        req.env
            .insert("ANTHROPIC_API_KEY".into(), "sk-ant-456".into());
        req.env
            .insert("AWS_SECRET_ACCESS_KEY".into(), "aws-secret".into());
        req.env.insert("GH_TOKEN".into(), "ghp_secret".into());
        req.env
            .insert("CANBAN_RELAY_TOKEN".into(), "relay-secret".into());
        req.env
            .insert("CANBAN_AUTH_SECRET".into(), "auth-secret".into());
        req.env
            .insert("SSH_AUTH_SOCK".into(), "/tmp/ssh-agent.sock".into());
        req.env.insert("HOME".into(), "/Users/demo".into());
        req.env
            .insert("PATH".into(), "/opt/homebrew/bin:/usr/bin".into());
        // Non-allowlisted host keys containing dots must not fail launch.
        req.env
            .insert("npm_config_foo.bar".into(), "host-dotted".into());

        let expected_env = req.env.clone();
        let spec = prepare_launch(req).unwrap();

        // Full env stays in process environment for direct mode and CLI hooks.
        assert_eq!(spec.env, expected_env);

        let joined_argv = spec.argv.join(" ");
        for forbidden in [
            "OPENAI_API_KEY",
            "sk-secret-123",
            "ANTHROPIC_API_KEY",
            "sk-ant-456",
            "AWS_SECRET_ACCESS_KEY",
            "aws-secret",
            "GH_TOKEN",
            "ghp_secret",
            "CANBAN_RELAY_TOKEN",
            "relay-secret",
            "CANBAN_AUTH_SECRET",
            "auth-secret",
            "SSH_AUTH_SOCK",
            "HOME",
            "PATH",
            "npm_config_foo.bar",
        ] {
            assert!(
                !joined_argv.contains(forbidden),
                "Codex CLI argv must not contain {forbidden:?}: {joined_argv}"
            );
        }

        // Allowlisted Canban thread-identity keys still travel on CLI argv.
        for allowed in ["CANBAN_CARD", "CARD_ID", "CANBAN_RUN_DIR", "CANBAN_CUSTOM"] {
            let prefix = format!("shell_environment_policy.set.{allowed}=");
            assert!(
                spec.argv.iter().any(|arg| arg.starts_with(&prefix)),
                "expected {allowed} override in Codex argv: {:?}",
                spec.argv
            );
        }
    }

    #[test]
    fn codex_keeps_custom_agent_and_project_plain_and_secret_vars_out_of_cli_argv() {
        let p = profile(Some("codex"));
        let mut req = request(Some(&p), &["codex", "resume", "session-id"]);
        req.env.insert(
            "ANTHROPIC_BASE_URL".into(),
            "https://proxy.example.com".into(),
        );
        req.env
            .insert("ANTHROPIC_API_KEY".into(), "sk-secret-sentinel-123".into());
        req.env.insert("RUST_LOG".into(), "debug".into());
        let expected_env = req.env.clone();
        let spec = prepare_launch(req).unwrap();
        assert_eq!(spec.env, expected_env);
        let joined_argv = spec.argv.join(" ");
        for k in [
            "ANTHROPIC_BASE_URL",
            "https://proxy.example.com",
            "ANTHROPIC_API_KEY",
            "sk-secret-sentinel-123",
            "RUST_LOG",
        ] {
            assert!(
                !joined_argv.contains(k),
                "user env {k:?} must never travel on Codex CLI argv: {joined_argv}"
            );
        }
    }
}
