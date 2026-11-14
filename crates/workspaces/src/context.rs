//! Directory context is a presentation hint, never a task assignment.
use super::*;
use canban_types::Snapshot;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatus {
    pub branch: Option<String>,
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedContext {
    pub project_id: Option<String>,
    pub card_id: Option<String>,
    pub worktree_id: Option<String>,
    pub branch: Option<String>,
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
}

pub fn parse_porcelain_v2_branch(stdout: &str) -> GitStatus {
    let mut status = GitStatus::default();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            if let Some(head) = rest.strip_prefix("branch.head ") {
                let head = head.trim();
                if !head.is_empty() && head != "(detached)" {
                    status.branch = Some(head.to_owned());
                }
            } else if let Some(ab) = rest.strip_prefix("branch.ab ") {
                for token in ab.split_whitespace() {
                    if let Some(a) = token.strip_prefix('+') {
                        status.ahead = a.parse::<u32>().unwrap_or(0);
                    } else if let Some(b) = token.strip_prefix('-') {
                        status.behind = b.parse::<u32>().unwrap_or(0);
                    }
                }
            }
        } else if !line.is_empty() {
            status.dirty = true;
            break;
        }
    }
    status
}

#[derive(Default)]
pub struct ContextCache {
    paths: BTreeMap<String, Value>,
    statuses: BTreeMap<String, GitStatus>,
    pub probes: u64,
}
impl ContextCache {
    pub fn invalidate_branches(&mut self) {
        self.statuses.clear();
    }
    pub fn invalidate_statuses(&mut self) {
        self.statuses.clear();
    }
    pub fn watch_paths(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .paths
            .values()
            .flat_map(|v| {
                [
                    v["git_dir"].as_str(),
                    v["repo"].as_str(),
                    v["path"].as_str(),
                ]
                .into_iter()
                .flatten()
                .map(PathBuf::from)
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

impl Ops {
    pub fn c_contains(&self, root: &str, cwd: &str) -> bool {
        self.host
            .canonical(Path::new(root))
            .ok()
            .zip(self.host.canonical(Path::new(cwd)).ok())
            .is_some_and(|(root, cwd)| cwd.starts_with(root))
    }
    fn probe_git_status(&self, cwd: &str) -> GitStatus {
        match self.checked(
            Path::new(cwd),
            &[
                "--no-optional-locks",
                "status",
                "--porcelain=v2",
                "--branch",
                "--untracked-files=normal",
            ],
        ) {
            Ok(out) => {
                let mut status = parse_porcelain_v2_branch(&out);
                if status.branch.is_none() {
                    status.branch = self.rebase_branch(Path::new(cwd));
                }
                status
            }
            Err(_) => GitStatus {
                branch: self
                    .current_branch(cwd)
                    .or_else(|| self.rebase_branch(Path::new(cwd))),
                ..Default::default()
            },
        }
    }
    /// Probe all candidate paths in one remote operation; then use one matching
    /// algorithm for both hosts. Never apply this machine's filesystem to another.
    pub fn directory_context(
        &self,
        s: &Snapshot,
        cwd: &str,
    ) -> (Option<String>, Option<String>, Option<String>) {
        self.directory_context_cached(s, cwd, &mut ContextCache::default())
            .unwrap_or_default()
    }
    pub fn directory_context_cached(
        &self,
        s: &Snapshot,
        cwd: &str,
        cache: &mut ContextCache,
    ) -> Result<(Option<String>, Option<String>, Option<String>)> {
        let full = self.directory_context_full_cached(s, cwd, cache)?;
        Ok((full.project_id, full.card_id, full.branch))
    }
    pub fn directory_context_full(&self, s: &Snapshot, cwd: &str) -> ResolvedContext {
        self.directory_context_full_cached(s, cwd, &mut ContextCache::default())
            .unwrap_or_default()
    }
    pub fn directory_context_full_cached(
        &self,
        s: &Snapshot,
        cwd: &str,
        cache: &mut ContextCache,
    ) -> Result<ResolvedContext> {
        let machine = &self.host.machine;
        let mut roots: Vec<(String, String)> = s
            .projects
            .iter()
            .filter(|p| p.work_location().machine == *machine)
            .map(|p| (p.directory.clone(), p.id.clone()))
            .collect();
        let mut checkouts: Vec<(String, String)> = Vec::new();
        let mut managed_checkouts: Vec<(String, String)> = Vec::new();
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
        for wt in s
            .worktrees
            .iter()
            .filter(|wt| wt.location.machine == *machine)
        {
            roots.push((wt.path.clone(), wt.project_id.clone()));
            roots.push((wt.location.directory.clone(), wt.project_id.clone()));
            managed_checkouts.push((wt.path.clone(), wt.id.clone()));
        }
        let mut paths = roots
            .iter()
            .map(|(p, _)| p.clone())
            .chain(std::iter::once(cwd.to_owned()))
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        let missing = paths
            .iter()
            .filter(|p| !cache.paths.contains_key(*p))
            .cloned()
            .collect::<Vec<_>>();
        let cached_git_dir = cache
            .paths
            .get(cwd)
            .and_then(|v| v["git_dir"].as_str())
            .map(str::to_owned);
        let status_missing = match &cached_git_dir {
            Some(gd) => !cache.statuses.contains_key(gd),
            None => !cache.paths.contains_key(cwd),
        };
        if !missing.is_empty() || status_missing {
            cache.probes += 1;
            if self.host.remote() {
                let known_statuses: Vec<&String> = cache.statuses.keys().collect();
                let probe = self.host.python(
                    r#"import os,sys,json,subprocess
rows={}
for p in json.loads(sys.argv[1]):
 if not os.path.exists(p):rows[p]=None;continue
 def git(args):
  r=subprocess.run(['git','--no-optional-locks','-C',p]+args,capture_output=True,text=True)
  return r.stdout.strip() if r.returncode==0 else None
 dirs=git(['rev-parse','--path-format=absolute','--git-common-dir','--git-dir'])
 parts=dirs.splitlines() if dirs else []
 rows[p]={'path':os.path.realpath(p),'repo':os.path.realpath(parts[0]) if parts else None,'git_dir':os.path.realpath(parts[1]) if len(parts)>1 else None}
cwd=sys.argv[2]
gd=sys.argv[3] or ((rows.get(cwd) or {}).get('git_dir') if cwd in rows else '')
known=set(json.loads(sys.argv[4]))
def rebase_br():
 for rel in ('rebase-merge/head-name','rebase-apply/head-name'):
  hp=os.path.join(gd,rel)
  if os.path.isfile(hp):
   try:
    raw=open(hp).read().strip()
    if raw.startswith('refs/heads/') and len(raw)>11:return raw[11:]
   except OSError:pass
 return None
st=None
if gd and gd not in known:
 r=subprocess.run(['git','--no-optional-locks','-C',cwd,'status','--porcelain=v2','--branch','--untracked-files=normal'],capture_output=True,text=True)
 if r.returncode==0:
  br=None;dirty=False;ahead=0;behind=0
  for line in r.stdout.splitlines():
   if line.startswith('# '):
    rest=line[2:]
    if rest.startswith('branch.head '):
     h=rest[len('branch.head '):].strip()
     if h and h!='(detached)':br=h
    elif rest.startswith('branch.ab '):
     for tok in rest[len('branch.ab '):].split():
      if tok.startswith('+'):
       try:ahead=int(tok[1:])
       except ValueError:pass
      elif tok.startswith('-'):
       try:behind=int(tok[1:])
       except ValueError:pass
   elif line:
    dirty=True;break
  st={'branch':br or rebase_br(),'dirty':dirty,'ahead':ahead,'behind':behind}
 else:
  r2=subprocess.run(['git','--no-optional-locks','-C',cwd,'symbolic-ref','--quiet','--short','HEAD'],capture_output=True,text=True)
  st={'branch':(r2.stdout.strip() if r2.returncode==0 else None) or rebase_br(),'dirty':False,'ahead':0,'behind':0}
print(json.dumps({'paths':rows,'git_dir':gd or None,'status':st}))"#,
                    &[
                        &serde_json::to_string(&missing)?,
                        cwd,
                        cached_git_dir.as_deref().unwrap_or(""),
                        &serde_json::to_string(&known_statuses)?,
                    ],
                )?;
                for p in missing {
                    cache.paths.insert(p.clone(), probe["paths"][&p].clone());
                }
                if let Some(gd) = probe["git_dir"].as_str()
                    && !probe["status"].is_null()
                    && let Ok(st) = serde_json::from_value::<GitStatus>(probe["status"].clone())
                {
                    cache.statuses.insert(gd.to_owned(), st);
                }
            } else {
                for p in &missing {
                    let row = if let Ok(path) = self.host.canonical(Path::new(p)) {
                        let dirs = self
                            .checked(
                                Path::new(p),
                                &[
                                    "--no-optional-locks",
                                    "rev-parse",
                                    "--path-format=absolute",
                                    "--git-common-dir",
                                    "--git-dir",
                                ],
                            )
                            .ok();
                        let parts = dirs
                            .as_deref()
                            .map(|d| d.lines().collect::<Vec<_>>())
                            .unwrap_or_default();
                        json!({
                            "path": path,
                            "repo": parts.first().and_then(|p| self.host.canonical(Path::new(p)).ok()),
                            "git_dir": parts.get(1).and_then(|p| self.host.canonical(Path::new(p)).ok())
                        })
                    } else {
                        Value::Null
                    };
                    cache.paths.insert(p.clone(), row);
                }
                if let Some(gd) = cache
                    .paths
                    .get(cwd)
                    .and_then(|v| v["git_dir"].as_str())
                    .map(str::to_owned)
                    && !cache.statuses.contains_key(&gd)
                {
                    let st = self.probe_git_status(cwd);
                    cache.statuses.insert(gd, st);
                }
            }
        }
        let rows = &cache.paths;
        let canonical = |p: &str| {
            rows.get(p)
                .and_then(|v| v["path"].as_str())
                .map(PathBuf::from)
        };
        let repo = |p: &str| {
            rows.get(p)
                .and_then(|v| v["repo"].as_str())
                .map(str::to_owned)
        };
        let git_status = rows
            .get(cwd)
            .and_then(|v| v["git_dir"].as_str())
            .and_then(|gd| cache.statuses.get(gd))
            .cloned()
            .unwrap_or_default();
        let branch = git_status.branch.clone();
        let Some(path) = canonical(cwd) else {
            return Ok(ResolvedContext {
                branch,
                dirty: git_status.dirty,
                ahead: git_status.ahead,
                behind: git_status.behind,
                ..Default::default()
            });
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
            return Ok(ResolvedContext {
                branch,
                dirty: git_status.dirty,
                ahead: git_status.ahead,
                behind: git_status.behind,
                ..Default::default()
            });
        };
        let mut exact_wt = managed_checkouts
            .iter()
            .filter(|(p, _)| canonical(p).is_some_and(|p| path.starts_with(p)))
            .map(|(_, id)| id.clone())
            .collect::<Vec<_>>();
        exact_wt.sort();
        exact_wt.dedup();
        let worktree_id = (exact_wt.len() == 1).then(|| exact_wt[0].clone());
        let card = if worktree_id.is_some() {
            None
        } else {
            let mut exact = checkouts
                .iter()
                .filter(|(p, _)| canonical(p).is_some_and(|p| path.starts_with(p)))
                .map(|(_, id)| id.clone())
                .collect::<Vec<_>>();
            exact.sort();
            exact.dedup();
            if exact.len() == 1 {
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
            }
        };
        Ok(ResolvedContext {
            project_id: Some(project),
            card_id: card,
            worktree_id,
            branch,
            dirty: git_status.dirty,
            ahead: git_status.ahead,
            behind: git_status.behind,
        })
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
            github: None,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: Default::default(),
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
        let card:Card=serde_json::from_value(serde_json::json!({"id":"c","project_id":"p","number":1,"title":"Task","body":"","stage":"develop","revision":1,"created_at":0,"worktree":worktree,"branch":"task"})).unwrap();
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

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[test]
    fn unchanged_directory_reuses_metadata_but_head_and_mapping_changes_are_independent() {
        let dir = tempfile::tempdir().unwrap();
        checked(dir.path(), &["init", "-b", "main"]).unwrap();
        let cwd = dir.path().to_str().unwrap();
        let ops = Ops::local();
        let mut cache = ContextCache::default();
        let mut s = Snapshot::default();
        assert_eq!(
            ops.directory_context_cached(&s, cwd, &mut cache).unwrap().2,
            Some("main".into())
        );
        let probes = cache.probes;
        ops.directory_context_cached(&s, cwd, &mut cache).unwrap();
        assert_eq!(cache.probes, probes);
        s.projects.push(Project {
            github: None,
            delivery_mode: Default::default(),
            agent_overrides: Default::default(),
            tags: Default::default(),
            location: None,
            id: "p".into(),
            name: "p".into(),
            directory: cwd.into(),
            workflow: Default::default(),
            target_branch: None,
            created_at: 0,
        });
        assert_eq!(
            ops.directory_context_cached(&s, cwd, &mut cache).unwrap().0,
            Some("p".into())
        );
        assert_eq!(
            cache.probes, probes,
            "mapping edits reuse known filesystem metadata"
        );
        checked(dir.path(), &["symbolic-ref", "HEAD", "refs/heads/other"]).unwrap();
        let known_paths = cache.paths.clone();
        cache.invalidate_branches();
        assert_eq!(
            ops.directory_context_cached(&s, cwd, &mut cache).unwrap().2,
            Some("other".into())
        );
        assert_eq!(
            cache.paths, known_paths,
            "HEAD refresh preserves repository discovery"
        );
        assert_eq!(cache.probes, probes + 1);
        let outside_dir = tempfile::tempdir().unwrap();
        let outside = outside_dir.path();
        ops.directory_context_cached(&s, outside.to_str().unwrap(), &mut cache)
            .unwrap();
        let probes = cache.probes;
        ops.directory_context_cached(&s, outside.to_str().unwrap(), &mut cache)
            .unwrap();
        assert_eq!(cache.probes, probes);
    }

    #[test]
    fn porcelain_git_status_reports_dirty_upstream_divergence_managed_worktree_and_shares_git_dir_cache()
     {
        let dir = tempfile::tempdir().unwrap();
        let remote = dir.path().join("remote.git");
        let source = dir.path().join("repo");
        std::fs::create_dir(&remote).unwrap();
        std::fs::create_dir(&source).unwrap();
        checked(&remote, &["init", "--bare", "-b", "main"]).unwrap();
        checked(&source, &["init", "-b", "main"]).unwrap();
        let commit_env = [
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ];
        let commit = |repo: &Path, msg: &str| {
            let mut args = commit_env.to_vec();
            args.extend(["commit", "--allow-empty", "-m", msg]);
            checked(repo, &args).unwrap();
        };
        commit(&source, "Initial");
        checked(
            &source,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        checked(&source, &["push", "-u", "origin", "main"]).unwrap();

        let sub = source.join("crates/sub");
        std::fs::create_dir_all(&sub).unwrap();
        let wt_path = dir.path().join("wt-feature");
        checked(
            &source,
            &[
                "worktree",
                "add",
                "-b",
                "feat/spike",
                wt_path.to_str().unwrap(),
            ],
        )
        .unwrap();

        let s = Snapshot {
            projects: vec![Project {
                github: None,
                delivery_mode: Default::default(),
                agent_overrides: Default::default(),
                tags: Default::default(),
                location: None,
                id: "p1".into(),
                name: "Project 1".into(),
                directory: source.to_str().unwrap().into(),
                workflow: Default::default(),
                target_branch: None,
                created_at: 0,
            }],
            worktrees: vec![canban_types::ManagedWorktree {
                id: "wt-1".into(),
                project_id: "p1".into(),
                branch: "feat/spike".into(),
                base_branch: "main".into(),
                base_commit: None,
                path: wt_path.to_str().unwrap().into(),
                location: canban_types::WorkLocation {
                    machine: "local".into(),
                    directory: wt_path.to_str().unwrap().into(),
                },
                created_at: 0,
            }],
            ..Default::default()
        };

        let ops = Ops::local();
        let mut cache = ContextCache::default();

        // Clean synced main branch
        let ctx = ops
            .directory_context_full_cached(&s, source.to_str().unwrap(), &mut cache)
            .unwrap();
        assert_eq!(ctx.project_id.as_deref(), Some("p1"));
        assert_eq!(ctx.worktree_id, None);
        assert_eq!(ctx.branch.as_deref(), Some("main"));
        assert!(!ctx.dirty);
        assert_eq!((ctx.ahead, ctx.behind), (0, 0));

        // Subdirectory in the same worktree reuses the cached git_dir status without running git status again!
        let statuses_before = cache.statuses.len();
        let sub_ctx = ops
            .directory_context_full_cached(&s, sub.to_str().unwrap(), &mut cache)
            .unwrap();
        assert_eq!(sub_ctx.branch.as_deref(), Some("main"));
        assert_eq!(cache.statuses.len(), statuses_before);

        // Untracked file marks worktree dirty after status invalidation
        std::fs::write(source.join("notes.txt"), "draft").unwrap();
        cache.invalidate_statuses();
        let dirty_ctx = ops
            .directory_context_full_cached(&s, source.to_str().unwrap(), &mut cache)
            .unwrap();
        assert!(dirty_ctx.dirty);

        // Stage and commit 2 local commits -> clean, ahead 2
        checked(&source, &["add", "notes.txt"]).unwrap();
        let mut args = commit_env.to_vec();
        args.extend(["commit", "-m", "Add notes"]);
        checked(&source, &args).unwrap();
        commit(&source, "Second local");
        cache.invalidate_statuses();
        let ahead_ctx = ops
            .directory_context_full_cached(&s, source.to_str().unwrap(), &mut cache)
            .unwrap();
        assert!(!ahead_ctx.dirty);
        assert_eq!((ahead_ctx.ahead, ahead_ctx.behind), (2, 0));

        // Push another commit from a separate clone to advance origin/main, then fetch locally -> ahead 2, behind 1
        let other = dir.path().join("other-clone");
        checked(
            dir.path(),
            &["clone", remote.to_str().unwrap(), other.to_str().unwrap()],
        )
        .unwrap();
        commit(&other, "Upstream advance");
        checked(&other, &["push", "origin", "main"]).unwrap();
        checked(&source, &["fetch", "origin"]).unwrap();
        cache.invalidate_statuses();
        let diverged_ctx = ops
            .directory_context_full_cached(&s, source.to_str().unwrap(), &mut cache)
            .unwrap();
        assert_eq!((diverged_ctx.ahead, diverged_ctx.behind), (2, 1));

        // ManagedWorktree resolves `worktree_id` and has no upstream tracking numbers unless configured
        let wt_ctx = ops
            .directory_context_full_cached(&s, wt_path.to_str().unwrap(), &mut cache)
            .unwrap();
        assert_eq!(wt_ctx.project_id.as_deref(), Some("p1"));
        assert_eq!(wt_ctx.worktree_id.as_deref(), Some("wt-1"));
        assert_eq!(wt_ctx.card_id, None);
        assert_eq!(wt_ctx.branch.as_deref(), Some("feat/spike"));
        assert!(!wt_ctx.dirty);
        assert_eq!((wt_ctx.ahead, wt_ctx.behind), (0, 0));

        // Mid-rebase detached conflict preserves the rebasing branch identity and dirty status
        std::fs::write(source.join("conflict.txt"), "main\n").unwrap();
        checked(&source, &["add", "conflict.txt"]).unwrap();
        let mut args = commit_env.to_vec();
        args.extend(["commit", "-m", "Main conflict"]);
        checked(&source, &args).unwrap();
        std::fs::write(wt_path.join("conflict.txt"), "spike\n").unwrap();
        checked(&wt_path, &["add", "conflict.txt"]).unwrap();
        let mut args = commit_env.to_vec();
        args.extend(["commit", "-m", "Spike conflict"]);
        checked(&wt_path, &args).unwrap();
        let rebase = git(&wt_path, &["rebase", "main"]).unwrap();
        assert!(!rebase.status.success());
        cache.invalidate_statuses();
        let rebase_ctx = ops
            .directory_context_full_cached(&s, wt_path.to_str().unwrap(), &mut cache)
            .unwrap();
        assert_eq!(rebase_ctx.branch.as_deref(), Some("feat/spike"));
        assert!(rebase_ctx.dirty);
    }
}
