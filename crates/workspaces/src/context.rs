//! Directory context is a presentation hint, never a task assignment.
use super::*;
use canban_types::Snapshot;
use serde_json::{Value, json};
impl Ops {
    pub fn c_contains(&self, root: &str, cwd: &str) -> bool {
        self.host
            .canonical(Path::new(root))
            .ok()
            .zip(self.host.canonical(Path::new(cwd)).ok())
            .is_some_and(|(root, cwd)| cwd.starts_with(root))
    }
    /// Probe all candidate paths in one remote operation; then use one matching
    /// algorithm for both hosts. Never apply this machine's filesystem to another.
    pub fn directory_context(
        &self,
        s: &Snapshot,
        cwd: &str,
    ) -> (Option<String>, Option<String>, Option<String>) {
        let machine = &self.host.machine;
        let mut roots: Vec<(String, String)> = s
            .projects
            .iter()
            .filter(|p| p.work_location().machine == *machine)
            .map(|p| (p.directory.clone(), p.id.clone()))
            .collect();
        let mut checkouts: Vec<(String, String)> = Vec::new();
        let mut branches: Vec<(String, String, String, String)> = Vec::new();
        for c in s.cards.iter().filter(|c| c.archived_at.is_none()) {
            if c.workspace.is_none() && machine == "local" {
                if let Some(path) = &c.worktree {
                    roots.push((path.clone(), c.project_id.clone()));
                    checkouts.push((path.clone(), c.id.clone()));
                }
                if let Some(branch) = &c.branch
                    && let Some(p) = s
                        .projects
                        .iter()
                        .find(|p| p.id == c.project_id && p.work_location().machine == *machine)
                {
                    branches.push((
                        c.id.clone(),
                        c.project_id.clone(),
                        p.directory.clone(),
                        branch.clone(),
                    ));
                }
            }
            for w in c
                .workspace
                .iter()
                .chain(c.workspace_history.iter())
                .filter(|w| w.location.machine == *machine)
            {
                roots.push((w.path.clone(), c.project_id.clone()));
                roots.push((w.location.directory.clone(), c.project_id.clone()));
                checkouts.push((w.path.clone(), c.id.clone()));
                if let Some(branch) = &w.branch {
                    branches.push((
                        c.id.clone(),
                        c.project_id.clone(),
                        w.location.directory.clone(),
                        branch.clone(),
                    ));
                }
            }
        }
        let mut paths = roots
            .iter()
            .map(|(p, _)| p.clone())
            .chain(std::iter::once(cwd.to_owned()))
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        let probe = if self.host.remote() {
            self.host.python(r#"import os,sys,json,subprocess
rows={}
for p in json.loads(sys.argv[1]):
 if not os.path.exists(p):continue
 canonical=os.path.realpath(p)
 def git(args):
  r=subprocess.run(['git','-C',p]+args,capture_output=True,text=True)
  return r.stdout.strip() if r.returncode==0 else None
 common=git(['rev-parse','--path-format=absolute','--git-common-dir'])
 rows[p]={'path':canonical,'repo':os.path.realpath(common) if common else None}
branch=subprocess.run(['git','-C',sys.argv[2],'symbolic-ref','--quiet','--short','HEAD'],capture_output=True,text=True)
print(json.dumps({'paths':rows,'branch':branch.stdout.strip() if branch.returncode==0 else None}))"#,&[&serde_json::to_string(&paths).unwrap_or_default(),cwd]).ok()
        } else {
            let mut rows = serde_json::Map::new();
            for p in paths {
                if let Ok(path) = self.host.canonical(Path::new(&p)) {
                    let repo = self
                        .checked(
                            Path::new(&p),
                            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                        )
                        .ok()
                        .and_then(|p| self.host.canonical(Path::new(&p)).ok());
                    rows.insert(p, json!({"path":path,"repo":repo}));
                }
            }
            Some(json!({"paths":rows,"branch":self.current_branch(cwd)}))
        };
        let probe = probe.unwrap_or(Value::Null);
        let rows = &probe["paths"];
        let canonical = |p: &str| rows[p]["path"].as_str().map(PathBuf::from);
        let repo = |p: &str| rows[p]["repo"].as_str().map(str::to_owned);
        let branch = probe["branch"].as_str().map(str::to_owned);
        let Some(path) = canonical(cwd) else {
            return (None, None, branch);
        };
        let mut direct = roots
            .iter()
            .filter_map(|(p, id)| canonical(p).map(|p| (p, id.clone())))
            .filter(|(p, _)| path.starts_with(p))
            .collect::<Vec<_>>();
        direct.sort_by_key(|(p, _)| std::cmp::Reverse(p.components().count()));
        let project = direct
            .first()
            .and_then(|(first, id)| {
                direct
                    .iter()
                    .filter(|(p, _)| p.components().count() == first.components().count())
                    .all(|(_, other)| other == id)
                    .then(|| id.clone())
            })
            .or_else(|| {
                let common = repo(cwd)?;
                let mut matches = roots
                    .iter()
                    .filter(|(p, _)| repo(p).as_ref() == Some(&common))
                    .map(|(_, id)| id.clone())
                    .collect::<Vec<_>>();
                matches.sort();
                matches.dedup();
                (matches.len() == 1).then(|| matches[0].clone())
            });
        let Some(project) = project else {
            return (None, None, branch);
        };
        let mut exact = checkouts
            .iter()
            .filter(|(p, _)| canonical(p).is_some_and(|p| path.starts_with(p)))
            .map(|(_, id)| id.clone())
            .collect::<Vec<_>>();
        exact.sort();
        exact.dedup();
        let card = if exact.len() == 1 {
            Some(exact[0].clone())
        } else if exact.is_empty() {
            let mut matches = branches
                .iter()
                .filter(|(_, id, p, b)| {
                    id == &project
                        && Some(b) == branch.as_ref()
                        && repo(p).is_some()
                        && repo(p) == repo(cwd)
                })
                .map(|(id, _, _, _)| id.clone())
                .collect::<Vec<_>>();
            matches.sort();
            matches.dedup();
            (matches.len() == 1).then(|| matches[0].clone())
        } else {
            None
        };
        (Some(project), card, branch)
    }
}
pub fn contains(root: &str, cwd: &str) -> bool {
    Ops::local().c_contains(root, cwd)
}
pub fn resolve(snapshot: &Snapshot, cwd: &str) -> (Option<String>, Option<String>, Option<String>) {
    Ops::local().directory_context(snapshot, cwd)
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
            location: None,
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
