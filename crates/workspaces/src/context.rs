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
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
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
    pub additions: u32,
    pub deletions: u32,
    pub ahead: u32,
    pub behind: u32,
}

const MAX_UNTRACKED_FILES: usize = 512;
const MAX_UNTRACKED_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_UNTRACKED_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
const BINARY_SNIFF_BYTES: usize = 8192;

fn accumulate_numstat(stdout: &str, additions: &mut u32, deletions: &mut u32) {
    for line in stdout.lines() {
        let mut cols = line.split('\t');
        if let (Some(add), Some(del)) = (cols.next(), cols.next()) {
            if let Ok(a) = add.parse::<u32>() {
                *additions = additions.saturating_add(a);
            }
            if let Ok(d) = del.parse::<u32>() {
                *deletions = deletions.saturating_add(d);
            }
        }
    }
}

fn untracked_file_lines(path: &Path, total_bytes: &mut u64) -> u32 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.file_type().is_file() {
        return 0;
    }
    let len = meta.len();
    if len == 0 || len > MAX_UNTRACKED_FILE_BYTES {
        return 0;
    }
    if total_bytes.saturating_add(len) > MAX_UNTRACKED_TOTAL_BYTES {
        return 0;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return 0;
    };
    *total_bytes = total_bytes.saturating_add(bytes.len() as u64);
    if bytes.is_empty() {
        return 0;
    }
    let sniff = bytes.len().min(BINARY_SNIFF_BYTES);
    if bytes[..sniff].contains(&0) {
        return 0;
    }
    let lines =
        bytes.iter().filter(|&&b| b == b'\n').count() + usize::from(!bytes.ends_with(b"\n"));
    u32::try_from(lines).unwrap_or(u32::MAX)
}

fn parse_porcelain_v2_z(raw: &[u8]) -> (GitStatus, Vec<PathBuf>) {
    let mut status = GitStatus::default();
    let mut untracked = Vec::new();
    let mut records = raw.split(|&b| b == 0);
    while let Some(rec) = records.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(rest) = rec.strip_prefix(b"# ") {
            if let Ok(rest) = std::str::from_utf8(rest) {
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
            }
            continue;
        }
        if rec.starts_with(b"! ") {
            continue;
        }
        status.dirty = true;
        if rec.starts_with(b"2 ") {
            // In `-z` format, renamed/copied entries (`2 ...`) consume a following NUL-terminated origPath.
            let _ = records.next();
        } else if let Some(rel) = rec.strip_prefix(b"? ")
            && untracked.len() < MAX_UNTRACKED_FILES
        {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                untracked.push(PathBuf::from(std::ffi::OsStr::from_bytes(rel)));
            }
            #[cfg(not(unix))]
            if let Ok(s) = std::str::from_utf8(rel) {
                untracked.push(PathBuf::from(s));
            }
        }
    }
    (status, untracked)
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
    fn probe_git_status(&self, cwd: &str, top_hint: Option<&str>) -> GitStatus {
        let cwd_path = Path::new(cwd);
        match self.git(
            cwd_path,
            &[
                "--no-optional-locks",
                "status",
                "--porcelain=v2",
                "-z",
                "--branch",
                "--untracked-files=all",
            ],
        ) {
            Ok(out) if out.status.success() => {
                let (mut status, untracked) = parse_porcelain_v2_z(&out.stdout);
                if status.branch.is_none() {
                    status.branch = self.rebase_branch(cwd_path);
                }
                if status.dirty {
                    if let Ok(numstat) = self.checked(
                        cwd_path,
                        &["--no-optional-locks", "diff", "HEAD", "--numstat"],
                    ) {
                        accumulate_numstat(&numstat, &mut status.additions, &mut status.deletions);
                    } else {
                        if let Ok(cached) = self.checked(
                            cwd_path,
                            &["--no-optional-locks", "diff", "--cached", "--numstat"],
                        ) {
                            accumulate_numstat(
                                &cached,
                                &mut status.additions,
                                &mut status.deletions,
                            );
                        }
                        if let Ok(unstaged) =
                            self.checked(cwd_path, &["--no-optional-locks", "diff", "--numstat"])
                        {
                            accumulate_numstat(
                                &unstaged,
                                &mut status.additions,
                                &mut status.deletions,
                            );
                        }
                    }
                    if !untracked.is_empty() {
                        let root = top_hint.map(PathBuf::from).or_else(|| {
                            self.checked(
                                cwd_path,
                                &[
                                    "--no-optional-locks",
                                    "rev-parse",
                                    "--path-format=absolute",
                                    "--show-toplevel",
                                ],
                            )
                            .ok()
                            .map(PathBuf::from)
                        });
                        if let Some(root) = root {
                            let mut total_bytes = 0u64;
                            for rel in untracked {
                                status.additions = status.additions.saturating_add(
                                    untracked_file_lines(&root.join(rel), &mut total_bytes),
                                );
                            }
                        }
                    }
                }
                status
            }
            _ => GitStatus {
                branch: self
                    .current_branch(cwd)
                    .or_else(|| self.rebase_branch(cwd_path)),
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
        if machine == "local" {
            for batch in s.merge_batches.iter().filter(|b| b.active()) {
                roots.push((batch.worktree.clone(), batch.project_id.clone()));
            }
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
 dirs=git(['rev-parse','--path-format=absolute','--git-common-dir','--git-dir','--show-toplevel'])
 parts=dirs.splitlines() if dirs else []
 rows[p]={'path':os.path.realpath(p),'repo':os.path.realpath(parts[0]) if parts else None,'git_dir':os.path.realpath(parts[1]) if len(parts)>1 else None,'top':os.path.realpath(parts[2]) if len(parts)>2 else None}
cwd=sys.argv[2]
gd=sys.argv[3] or ((rows.get(cwd) or {}).get('git_dir') if cwd in rows else '')
top=sys.argv[4] or ((rows.get(cwd) or {}).get('top') if cwd in rows else '')
known=set(json.loads(sys.argv[5]))
def rebase_br():
 for rel in ('rebase-merge/head-name','rebase-apply/head-name'):
  hp=os.path.join(gd,rel)
  if os.path.isfile(hp):
   try:
    raw=open(hp).read().strip()
    if raw.startswith('refs/heads/') and len(raw)>11:return raw[11:]
   except OSError:pass
 return None
def sum_numstat(txt):
 a=0;d=0
 for ln in txt.splitlines():
  cols=ln.split('\t')
  if len(cols)>=2:
   try:a+=int(cols[0])
   except ValueError:pass
   try:d+=int(cols[1])
   except ValueError:pass
 return a,d
st=None
if gd and gd not in known:
 r=subprocess.run(['git','--no-optional-locks','-C',cwd,'status','--porcelain=v2','-z','--branch','--untracked-files=all'],capture_output=True)
 if r.returncode==0:
  br=None;dirty=False;ahead=0;behind=0;untracked=[]
  recs=r.stdout.split(b'\x00')
  i=0
  while i<len(recs):
   rec=recs[i];i+=1
   if not rec:continue
   if rec.startswith(b'# '):
    rest=rec[2:].decode('utf-8','replace')
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
   elif rec.startswith(b'! '):
    continue
   else:
    dirty=True
    if rec.startswith(b'2 '):i+=1
    elif rec.startswith(b'? ') and len(untracked)<512:untracked.append(rec[2:])
  adds=0;dels=0
  if dirty:
   nd=subprocess.run(['git','--no-optional-locks','-C',cwd,'diff','HEAD','--numstat'],capture_output=True,text=True)
   if nd.returncode==0:
    adds,dels=sum_numstat(nd.stdout)
   else:
    for ex in (['diff','--cached','--numstat'],['diff','--numstat']):
     nx=subprocess.run(['git','--no-optional-locks','-C',cwd]+ex,capture_output=True,text=True)
     if nx.returncode==0:
      ca,cd=sum_numstat(nx.stdout);adds+=ca;dels+=cd
   if untracked:
    if not top:
     rt=subprocess.run(['git','--no-optional-locks','-C',cwd,'rev-parse','--path-format=absolute','--show-toplevel'],capture_output=True,text=True)
     if rt.returncode==0:top=rt.stdout.strip()
    if top:
     tb=os.fsencode(top);tot=0
     for rel in untracked:
      fp=os.path.join(tb,rel)
      try:
       if os.path.islink(fp) or not os.path.isfile(fp):continue
       sz=os.path.getsize(fp)
       if sz<=0 or sz>2097152 or tot+sz>16777216:continue
       data=open(fp,'rb').read();tot+=len(data)
       if data and b'\x00' not in data[:8192]:
        adds+=data.count(b'\n')+(0 if data.endswith(b'\n') else 1)
      except OSError:pass
  st={'branch':br or rebase_br(),'dirty':dirty,'additions':adds,'deletions':dels,'ahead':ahead,'behind':behind}
 else:
  r2=subprocess.run(['git','--no-optional-locks','-C',cwd,'symbolic-ref','--quiet','--short','HEAD'],capture_output=True,text=True)
  st={'branch':(r2.stdout.strip() if r2.returncode==0 else None) or rebase_br(),'dirty':False,'additions':0,'deletions':0,'ahead':0,'behind':0}
print(json.dumps({'paths':rows,'git_dir':gd or None,'status':st}))"#,
                    &[
                        &serde_json::to_string(&missing)?,
                        cwd,
                        cached_git_dir.as_deref().unwrap_or(""),
                        cache
                            .paths
                            .get(cwd)
                            .and_then(|v| v["top"].as_str())
                            .unwrap_or(""),
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
                                    "--show-toplevel",
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
                            "git_dir": parts.get(1).and_then(|p| self.host.canonical(Path::new(p)).ok()),
                            "top": parts.get(2).and_then(|p| self.host.canonical(Path::new(p)).ok())
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
                    let top_hint = cache
                        .paths
                        .get(cwd)
                        .and_then(|v| v["top"].as_str())
                        .map(str::to_owned);
                    let st = self.probe_git_status(cwd, top_hint.as_deref());
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
                additions: git_status.additions,
                deletions: git_status.deletions,
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
                additions: git_status.additions,
                deletions: git_status.deletions,
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
            additions: git_status.additions,
            deletions: git_status.deletions,
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
            detected_github: None,
            github_disconnected: false,
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
            detected_github: None,
            github_disconnected: false,
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
                detected_github: None,
                github_disconnected: false,
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

    #[test]
    fn diff_line_counts_include_staged_unstaged_untracked_subdir_unborn_and_zero_line_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let ops = Ops::local();

        // 1. Unborn HEAD repository with staged + unstaged + nested untracked files
        let unborn = dir.path().join("unborn");
        std::fs::create_dir(&unborn).unwrap();
        checked(&unborn, &["init", "-b", "main"]).unwrap();
        std::fs::write(unborn.join("staged.txt"), "a\nb\n").unwrap();
        checked(&unborn, &["add", "staged.txt"]).unwrap();
        // Modify staged file in working tree (+1 line) and add nested untracked file (+3 lines, no trailing newline)
        std::fs::write(unborn.join("staged.txt"), "a\nb\nc\n").unwrap();
        std::fs::create_dir_all(unborn.join("nested/dir")).unwrap();
        std::fs::write(unborn.join("nested/dir/new.txt"), "one\ntwo\nthree").unwrap();

        let unborn_ctx = ops.directory_context_full(&Snapshot::default(), unborn.to_str().unwrap());
        assert_eq!(unborn_ctx.branch.as_deref(), Some("main"));
        assert!(unborn_ctx.dirty);
        assert_eq!((unborn_ctx.additions, unborn_ctx.deletions), (6, 0));

        // 2. Committed repo probed from a subdirectory: tracked staged + unstaged + untracked + binary
        let repo = dir.path().join("committed");
        std::fs::create_dir(&repo).unwrap();
        checked(&repo, &["init", "-b", "main"]).unwrap();
        std::fs::write(repo.join("tracked.txt"), "l1\nl2\nl3\nl4\n").unwrap();
        checked(&repo, &["add", "tracked.txt"]).unwrap();
        checked(
            &repo,
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
                "-m",
                "Init",
            ],
        )
        .unwrap();

        // Replace 2 lines with 3 lines in tracked.txt (+3 -2), stage a new 2-line file (+2),
        // add an untracked 4-line file at root while probing from `sub/` (+4), plus a binary untracked file (+0).
        std::fs::write(repo.join("tracked.txt"), "l1\nmod2\nmod3\nadd4\nl4\n").unwrap();
        std::fs::write(repo.join("staged_new.txt"), "s1\ns2\n").unwrap();
        checked(&repo, &["add", "staged_new.txt"]).unwrap();
        std::fs::write(repo.join("root_untracked.rs"), "u1\nu2\nu3\nu4\n").unwrap();
        std::fs::write(repo.join("blob.bin"), b"\x00\x01\x02\n\n\n").unwrap();
        let sub = repo.join("sub");
        std::fs::create_dir(&sub).unwrap();

        let sub_ctx = ops.directory_context_full(&Snapshot::default(), sub.to_str().unwrap());
        assert!(sub_ctx.dirty);
        assert_eq!((sub_ctx.additions, sub_ctx.deletions), (9, 2));

        // 3. Zero-line dirty state (empty untracked file + binary file only) -> dirty: true, +0 -0
        checked(&repo, &["reset", "--hard", "HEAD"]).unwrap();
        std::fs::remove_file(repo.join("root_untracked.rs")).unwrap();
        std::fs::write(repo.join("empty.txt"), "").unwrap();
        let zero_ctx = ops.directory_context_full(&Snapshot::default(), repo.to_str().unwrap());
        assert!(zero_ctx.dirty);
        assert_eq!((zero_ctx.additions, zero_ctx.deletions), (0, 0));
    }
}
