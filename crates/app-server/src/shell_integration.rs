//! Lightweight shell integration scripts for plain interactive shells (`zsh`, `bash`, `fish`).
//!
//! Emits namespaced OSC 7501 records (`id=canban-shell`) so command execution state
//! never collides with root AI agent status.

use anyhow::{Context, Result};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const VERSION_DIR: &str = "v1";
const ZSH_ZSENV: &str = include_str!("../shell-integration/zsh/.zshenv");
const BASH_INIT: &str = include_str!("../shell-integration/bash/canban.bash");
const FISH_INIT: &str = include_str!("../shell-integration/fish/canban.fish");

fn write_if_changed(path: &Path, content: &str) -> Result<()> {
    if let Ok(existing) = fs::read_to_string(path)
        && existing == content
    {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create shell integration directory {}", parent.display()))?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&tmp, content.as_bytes())
        .with_context(|| format!("write shell integration script {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("install shell integration script {}", path.display()))?;
    Ok(())
}

/// Materialize bundled shell integration scripts under `$CANBAN_HOME/shell-integration/v1/`
/// and return that versioned root directory.
pub(crate) fn install(home: &Path) -> Result<PathBuf> {
    let dir = home.join("shell-integration").join(VERSION_DIR);
    write_if_changed(&dir.join("zsh").join(".zshenv"), ZSH_ZSENV)?;
    write_if_changed(&dir.join("bash").join("canban.bash"), BASH_INIT)?;
    write_if_changed(&dir.join("fish").join("canban.fish"), FISH_INIT)?;
    Ok(dir)
}

fn single_quote_fish(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let escaped = raw.replace('\\', "\\\\").replace('\'', "\\'");
    format!("'{escaped}'")
}

/// Build `(argv, env_overrides)` for launching an interactive plain shell with hooks.
/// Returns `None` for unsupported shells, allowing callers to fall back to `[shell, "-l"]`.
pub(crate) fn plan(
    shell: &str,
    dir: &Path,
    user_zdotdir: Option<&str>,
) -> Option<(Vec<String>, BTreeMap<String, String>)> {
    let name = Path::new(shell).file_name()?.to_str()?;
    let mut env = BTreeMap::new();
    match name {
        "zsh" => {
            env.insert(
                "ZDOTDIR".into(),
                dir.join("zsh").to_string_lossy().into_owned(),
            );
            if let Some(zdot) = user_zdotdir.filter(|s| !s.is_empty()) {
                env.insert("CANBAN_USER_ZDOTDIR".into(), zdot.to_owned());
            }
            Some((vec![shell.to_owned(), "-l".into()], env))
        }
        "bash" => {
            let init = dir.join("bash").join("canban.bash");
            Some((
                vec![
                    shell.to_owned(),
                    "--init-file".into(),
                    init.to_string_lossy().into_owned(),
                ],
                env,
            ))
        }
        "fish" => {
            let init = dir.join("fish").join("canban.fish");
            Some((
                vec![
                    shell.to_owned(),
                    "-l".into(),
                    "--init-command".into(),
                    format!("source {}", single_quote_fish(&init)),
                ],
                env,
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_covers_zsh_bash_fish_and_unsupported_shells() {
        let dir = Path::new("/tmp/canban-home/shell-integration/v1");

        let (zsh_argv, zsh_env) = plan("/bin/zsh", dir, Some("/Users/dev/.zsh")).unwrap();
        assert_eq!(zsh_argv, vec!["/bin/zsh", "-l"]);
        assert_eq!(
            zsh_env.get("ZDOTDIR").map(String::as_str),
            Some("/tmp/canban-home/shell-integration/v1/zsh")
        );
        assert_eq!(
            zsh_env.get("CANBAN_USER_ZDOTDIR").map(String::as_str),
            Some("/Users/dev/.zsh")
        );

        let (_, zsh_no_user_env) = plan("zsh", dir, None).unwrap();
        assert_eq!(
            zsh_no_user_env.get("ZDOTDIR").map(String::as_str),
            Some("/tmp/canban-home/shell-integration/v1/zsh")
        );
        assert!(!zsh_no_user_env.contains_key("CANBAN_USER_ZDOTDIR"));

        let (bash_argv, bash_env) = plan("/opt/homebrew/bin/bash", dir, None).unwrap();
        assert_eq!(
            bash_argv,
            vec![
                "/opt/homebrew/bin/bash",
                "--init-file",
                "/tmp/canban-home/shell-integration/v1/bash/canban.bash"
            ]
        );
        assert!(bash_env.is_empty());

        let (fish_argv, fish_env) = plan("/opt/homebrew/bin/fish", dir, None).unwrap();
        assert_eq!(
            fish_argv,
            vec![
                "/opt/homebrew/bin/fish",
                "-l",
                "--init-command",
                "source '/tmp/canban-home/shell-integration/v1/fish/canban.fish'"
            ]
        );
        assert!(fish_env.is_empty());

        assert!(plan("/bin/sh", dir, None).is_none());
        assert!(plan("/usr/local/bin/nu", dir, None).is_none());
    }

    #[test]
    fn install_writes_and_updates_idempotently() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install(tmp.path()).unwrap();
        assert!(dir.join("zsh/.zshenv").is_file());
        assert!(dir.join("bash/canban.bash").is_file());
        assert!(dir.join("fish/canban.fish").is_file());
        // Second call succeeds idempotently.
        assert_eq!(install(tmp.path()).unwrap(), dir);
    }
}
