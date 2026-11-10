//! Readable initial names; saved Card bindings remain authoritative.
use super::*;
use std::io::ErrorKind;

/// Keep a conservative, Unicode-safe ref/path component. Bound bytes rather
/// than chars so a long multilingual directory still fits filesystem limits.
fn component(value: &str, fallback: &str) -> String {
    let mut result = String::new();
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' {
            if ch == '-' && (result.is_empty() || result.ends_with('-')) {
                continue;
            }
            if result.len() + ch.len_utf8() > 80 {
                break;
            }
            result.push(ch);
        } else if !result.is_empty() && !result.ends_with('-') {
            result.push('-');
        }
    }
    let result = result.trim_matches('-');
    if result.is_empty() {
        fallback.into()
    } else {
        result.into()
    }
}
pub(super) fn namespace(project: &Project) -> String {
    let directory = Path::new(&project.directory)
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("");
    let short_id = project.id.chars().take(4).collect::<String>();
    format!(
        "{}-{}",
        component(directory, "project"),
        component(&short_id, "id")
    )
}
pub(super) fn managed_path(home: &Path, card: &Card, path: &Path) -> bool {
    let Some(branch) = card.branch.as_deref() else {
        return false;
    };
    let parts = Path::new(branch).components().collect::<Vec<_>>();
    parts.len() == 2
        && parts
            .iter()
            .all(|p| matches!(p, std::path::Component::Normal(_)))
        && path == home.join("worktrees").join(branch)
}
fn suffixed(base: &str, n: usize) -> String {
    if n == 1 {
        base.into()
    } else {
        format!("{base}-{n}")
    }
}
fn conflicts(heads: &[String], candidate: &str) -> bool {
    // Conservative on case-sensitive hosts too: the same data may later be
    // restored on macOS's usual case-insensitive filesystem.
    let candidate = candidate.to_lowercase();
    heads.iter().any(|head| {
        let head = head.to_lowercase();
        head == candidate
            || head.starts_with(&format!("{candidate}/"))
            || candidate.starts_with(&format!("{head}/"))
    })
}
impl Ops {
    fn occupied(&self, path: &Path) -> Result<bool> {
        if self.host.remote() {
            return self
                .host
                .python(
                    "import os,json,sys;print(json.dumps(os.path.lexists(sys.argv[1])))",
                    &[path.to_str().context("Invalid worktree path")?],
                )?
                .as_bool()
                .context("Invalid path availability response");
        }
        match std::fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
    fn reserve_directory(&self, path: &Path) -> Result<bool> {
        if self.host.remote() {
            return self.host.python(
                "import os,json,sys\ntry:\n os.mkdir(sys.argv[1]);print('true')\nexcept FileExistsError:\n print('false')",
                &[path.to_str().context("Invalid worktree path")?],
            )?.as_bool().context("Invalid directory reservation response");
        }
        match std::fs::create_dir(path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
    fn release_empty_directory(&self, path: &Path) -> bool {
        if self.host.remote() {
            return self.host.python(
                "import os,json,sys\ntry:\n os.rmdir(sys.argv[1]);print('true')\nexcept OSError:\n print('false')",
                &[&path.to_string_lossy()],
            ).ok() == Some(serde_json::json!(true));
        }
        std::fs::remove_dir(path).is_ok()
    }
    fn heads(&self, source: &Path) -> Result<Vec<String>> {
        Ok(self
            .checked(
                source,
                &["for-each-ref", "--format=%(refname:strip=2)", "refs/heads/"],
            )?
            .lines()
            .map(str::to_owned)
            .collect())
    }
    pub(super) fn create_named(
        &self,
        home: &Path,
        project: &Project,
        card: &Card,
        start: &str,
    ) -> Result<PathBuf> {
        let source = Path::new(&project.directory);
        let root = home.join("worktrees");
        self.host.mkdir(&root)?;
        let namespace = namespace(project);
        let mut heads = self.heads(source)?;
        // Registered but missing/locked checkouts still own their paths.
        let registered = self.checked(source, &["worktree", "list", "--porcelain", "-z"])?;
        let registered = registered
            .split('\0')
            .filter_map(|v| v.strip_prefix("worktree "))
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let mut group_suffix = 1;
        let mut leaf_suffix = 1;
        for _ in 0..1024 {
            let group = suffixed(&namespace, group_suffix);
            let parent = root.join(&group);
            if heads
                .iter()
                .any(|h| h.to_lowercase() == group.to_lowercase())
                || (self.occupied(&parent)? && !self.host.is_dir(&parent))
            {
                group_suffix += 1;
                leaf_suffix = 1;
                continue;
            }
            let leaf = suffixed(&card.number.to_string(), leaf_suffix);
            let branch = format!("{group}/{leaf}");
            let target = parent.join(&leaf);
            if conflicts(&heads, &branch)
                || registered.contains(&target)
                || self.occupied(&target)?
            {
                leaf_suffix += 1;
                continue;
            }
            self.checked(source, &["check-ref-format", "--branch", &branch])?;
            self.host.mkdir(&parent)?;
            // mkdir reserves the path across concurrent creators; Git's ref
            // transaction reserves the branch across different Canban homes.
            if !self.reserve_directory(&target)? {
                leaf_suffix += 1;
                continue;
            }
            let out = self.git(
                source,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &branch,
                    target.to_str().context("Invalid worktree path")?,
                    start,
                ],
            )?;
            if out.status.success() {
                return Ok(target);
            }
            let released = self.release_empty_directory(&target);
            heads = self.heads(source)?;
            if released && conflicts(&heads, &branch) {
                // Another creator won the ref race; never delete their branch.
                continue;
            }
            anyhow::bail!(
                "git worktree add: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        anyhow::bail!("Could not allocate a Card workspace after 1024 name conflicts")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multilingual_directory_components_produce_valid_refs_and_ignore_display_names() {
        let mut project: Project = serde_json::from_value(serde_json::json!({
            "id":"4e5r-long-id", "name":"完全不同", "directory":"/tmp/source", "created_at":0,
            "workflow":canban_types::Workflow::default()
        }))
        .unwrap();
        for (directory, expected) in [
            ("/tmp/Canban", "canban-4e5r"),
            ("/tmp/工程 Repo.. @{bad}.lock/", "工程-repo-bad-lock-4e5r"),
            ("/tmp/???", "project-4e5r"),
            ("/", "project-4e5r"),
            ("/tmp/Репозиторий", "репозиторий-4e5r"),
        ] {
            project.directory = directory.into();
            assert_eq!(namespace(&project), expected);
            let output = Ops::local()
                .host
                .run(
                    Path::new("/"),
                    &[
                        "git",
                        "check-ref-format",
                        "--branch",
                        &format!("{expected}/42"),
                    ],
                    None,
                )
                .unwrap();
            assert!(output.status.success());
        }
        project.directory = format!("/tmp/{}", "界".repeat(200));
        let group = namespace(&project);
        assert!(group.len() <= 85);
        assert!(group.ends_with("-4e5r"));
    }
    #[test]
    fn saved_readable_paths_do_not_depend_on_the_current_directory_name() {
        let card: Card = serde_json::from_value(serde_json::json!({
            "id":"c", "project_id":"p", "number":42, "title":"Task", "body":"", "stage":"develop", "revision":1,"created_at":0,
            "branch":"old-directory-4e5r/42-2"
        })).unwrap();
        let home = Path::new("/managed");
        assert!(managed_path(
            home,
            &card,
            Path::new("/managed/worktrees/old-directory-4e5r/42-2")
        ));
        assert!(!managed_path(
            home,
            &card,
            Path::new("/elsewhere/old-directory-4e5r/42-2")
        ));
        let mut traversal = card;
        traversal.branch = Some("../outside".into());
        assert!(!managed_path(
            home,
            &traversal,
            Path::new("/managed/worktrees/../outside")
        ));
    }
}
