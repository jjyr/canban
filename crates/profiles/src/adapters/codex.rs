use anyhow::{Result, ensure};

use crate::launch::LaunchSpec;

/// Explicit config travels with the thread even when an existing daemon owns
/// execution. Keep process env as well, for direct mode and CLI-side hooks.
pub(super) fn prepare(spec: &mut LaunchSpec) -> Result<()> {
    let mut overrides = Vec::with_capacity(spec.env.len() * 2);
    for (key, value) in &spec.env {
        // Codex splits override paths on dots literally, without TOML key parsing.
        ensure!(
            !key.is_empty() && !key.contains(['.', '=', '\0']),
            "Codex cannot encode environment key {key:?} as a config override"
        );
        overrides.push("-c".to_owned());
        // Encode the value as a TOML basic string, without involving a shell.
        overrides.push(format!(
            "shell_environment_policy.set.{}={}",
            key,
            serde_json::to_string(value)?.replace('\u{7f}', "\\u007f"),
        ));
    }
    // Append after user config for per-invocation precedence, but before an
    // explicit end-of-options marker. Codex -c is global, including on resume.
    let position = spec
        .argv
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(spec.argv.len());
    spec.argv.splice(position..position, overrides);
    Ok(())
}
