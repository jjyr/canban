//! Directory context is a presentation hint, never a task assignment.
use super::*;
use canban_types::Snapshot;

fn canonical(path: &str) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}
fn repository(path: &Path) -> Option<PathBuf> {
    let dir = checked(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    std::fs::canonicalize(dir).ok()
}
pub fn contains(root: &str, cwd: &str) -> bool {
    canonical(root)
        .zip(canonical(cwd))
        .is_some_and(|(root, cwd)| cwd.starts_with(root))
}
/// Match registered directories first, then other worktrees of a known repo.
pub fn resolve(snapshot: &Snapshot, cwd: &str) -> (Option<String>, Option<String>, Option<String>) {
    let Some(path) = canonical(cwd) else {
        return (None, None, None);
    };
    let branch = review::current_branch(cwd);
    let mut roots: Vec<_> = snapshot
        .projects
        .iter()
        .filter_map(|p| canonical(&p.directory).map(|r| (r, p.id.clone())))
        .collect();
    roots.extend(
        snapshot
            .cards
            .iter()
            .filter_map(|c| canonical(c.worktree.as_deref()?).map(|r| (r, c.project_id.clone()))),
    );
    let mut matches: Vec<_> = roots.iter().filter(|(r, _)| path.starts_with(r)).collect();
    matches.sort_by_key(|(r, _)| std::cmp::Reverse(r.components().count()));
    let repo = repository(&path);
    let direct = matches.first().and_then(|(root, project)| {
        matches
            .iter()
            .filter(|(r, _)| r.components().count() == root.components().count())
            .all(|(_, p)| p == project)
            .then(|| project.clone())
    });
    let project = direct.or_else(|| {
        let repo = repo.as_ref()?;
        let mut ps = snapshot
            .projects
            .iter()
            .filter(|p| repository(Path::new(&p.directory)).as_ref() == Some(repo));
        let first = ps.next()?;
        ps.next().is_none().then(|| first.id.clone())
    });
    let Some(project) = project else {
        return (None, None, branch);
    };
    let cards: Vec<_> = snapshot
        .cards
        .iter()
        .filter(|c| c.project_id == project && c.archived_at.is_none())
        .collect();
    let worktree: Vec<_> = cards
        .iter()
        .filter(|c| c.worktree.as_ref().is_some_and(|w| contains(w, cwd)))
        .collect();
    let card = if worktree.len() == 1 {
        Some(worktree[0].id.clone())
    } else if worktree.is_empty() && repo.is_some() {
        // A branch name alone is never enough: verify the repository identity.
        let candidates: Vec<_> = cards
            .iter()
            .filter(|c| {
                c.branch.is_some()
                    && c.branch == branch
                    && snapshot
                        .projects
                        .iter()
                        .find(|p| p.id == project)
                        .is_some_and(|p| repository(Path::new(&p.directory)) == repo)
            })
            .collect();
        (candidates.len() == 1).then(|| candidates[0].id.clone())
    } else {
        None
    };
    (Some(project), card, branch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{Card, Workflow};
    #[test]
    fn directory_matching_handles_worktrees_nested_projects_symlinks_and_ambiguous_branches() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("repo");
        std::fs::create_dir(&source).unwrap();
        checked(&source, &["init", "-b", "main"]).unwrap();
        checked(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "Seed",
            ],
        )
        .unwrap();
        let project = Project {
            id: "p".into(),
            name: "Project".into(),
            directory: source.to_str().unwrap().into(),
            workflow: Workflow::default(),
            target_branch: None,
            created_at: 0,
        };
        let worktree = dir.path().join("separate checkout");
        checked(
            &source,
            &["worktree", "add", "-b", "task", worktree.to_str().unwrap()],
        )
        .unwrap();
        let child = worktree.join("subdir");
        std::fs::create_dir(&child).unwrap();
        let card:Card=serde_json::from_value(serde_json::json!({"id":"c","project_id":"p","number":1,"title":"Task","body":"","tags":[],"stage":"develop","revision":1,"created_at":0,"worktree":worktree,"branch":"task"})).unwrap();
        let mut s = Snapshot {
            projects: vec![project],
            cards: vec![card.clone()],
            ..Default::default()
        };
        let matched = resolve(&s, child.to_str().unwrap());
        assert_eq!(
            matched,
            (Some("p".into()), Some("c".into()), Some("task".into()))
        );
        let link = dir.path().join("alias");
        std::os::unix::fs::symlink(&child, &link).unwrap();
        assert_eq!(resolve(&s, link.to_str().unwrap()), matched);
        let sibling = dir.path().join("repo-other");
        std::fs::create_dir(&sibling).unwrap();
        assert!(resolve(&s, sibling.to_str().unwrap()).0.is_none());
        s.cards[0].worktree = None;
        assert_eq!(resolve(&s, child.to_str().unwrap()).1, Some("c".into()));
        let mut duplicate = card.clone();
        duplicate.id = "duplicate".into();
        duplicate.worktree = None;
        s.cards.push(duplicate);
        assert!(resolve(&s, child.to_str().unwrap()).1.is_none());
        let unrelated = dir.path().join("unrelated");
        std::fs::create_dir(&unrelated).unwrap();
        checked(&unrelated, &["init", "-b", "task"]).unwrap();
        assert!(resolve(&s, unrelated.to_str().unwrap()).0.is_none());
        let nested = source.join("nested");
        std::fs::create_dir(&nested).unwrap();
        let mut p = s.projects[0].clone();
        p.id = "nested".into();
        p.directory = nested.to_str().unwrap().into();
        s.projects.push(p);
        assert_eq!(
            resolve(&s, nested.to_str().unwrap()).0,
            Some("nested".into())
        );
        s.cards[0].worktree = card.worktree;
        s.cards[0].archived_at = Some(2);
        s.cards.pop();
        assert!(resolve(&s, child.to_str().unwrap()).1.is_none());
    }
}
