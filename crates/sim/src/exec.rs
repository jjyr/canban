//! Deterministic in-process [`canban_exec::CommandRunner`] for DST.
//!
//! Eliminates login-shell probes (`/bin/zsh -ilc`), host `PATH` discovery, host `hostname` calls,
//! OS `git` forks, and OS `/bin/sh` forks for `uses: script` and `merge_batch` `test_command`,
//! while failing closed on any unexpected OS command spawn.
use anyhow::{Context, Result, bail};
use canban_exec::{CommandRunner, CommandSpec, ExecOutput, ShellEnvSnapshot};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Scripted in-process outcome for a simulated `/bin/sh` (`uses: script` or `merge_batch` test).
#[derive(Clone, Debug, Default)]
pub struct SimShellOutcome {
    pub status_code: i32,
    pub stdout: String,
    pub stderr: String,
    /// Key-value lines written to `$CANBAN_ENV` when present in `spec.env_set`.
    pub write_env: BTreeMap<String, String>,
    /// Key-value lines written to `$CANBAN_OUTPUT` when present in `spec.env_set`.
    pub write_output: BTreeMap<String, String>,
}

impl SimShellOutcome {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            status_code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
            write_env: BTreeMap::new(),
            write_output: BTreeMap::new(),
        }
    }

    pub fn fail(status_code: i32, stdout: impl Into<String>, stderr: impl Into<String>) -> Self {
        Self {
            status_code,
            stdout: stdout.into(),
            stderr: stderr.into(),
            write_env: BTreeMap::new(),
            write_output: BTreeMap::new(),
        }
    }

    pub fn with_env(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.write_env.insert(key.into(), val.into());
        self
    }

    pub fn with_output(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.write_output.insert(key.into(), val.into());
        self
    }
}

const EMPTY_TREE_SHA: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Deterministic in-memory worktree checkout state belonging to a [`SimGitRepo`].
#[derive(Clone, Debug)]
struct SimWorktree {
    path: PathBuf,
    git_dir: PathBuf,
    branch: Option<String>,
    head_sha: String,
    dirty: bool,
    staged: bool,
    staged_tree: Option<String>,
    staged_files: BTreeMap<String, String>,
    unstaged_files: BTreeMap<String, String>,
    untracked_files: BTreeMap<String, String>,
    in_merge: bool,
    merge_head: Option<String>,
    in_cherry_pick: bool,
    in_rebase: bool,
    rebase_onto: Option<String>,
    conflict_files: Vec<String>,
}

/// Scripted deterministic git repository fixture for [`SimExec`].
#[derive(Clone, Debug)]
pub struct SimGitRepo {
    pub default_branch: String,
    pub current_branch: String,
    pub head_sha: String,
    pub remotes: BTreeMap<String, String>,
    pub branches: BTreeSet<String>,
    pub dirty: bool,
    branch_heads: BTreeMap<String, String>,
    branch_upstreams: BTreeMap<String, String>,
    remote_refs: BTreeMap<String, String>,
    custom_refs: BTreeMap<String, String>,
    last_fetch_head: Option<String>,
    /// Commit graph (`sha -> parent SHAs`) supporting `merge-base`, `--is-ancestor`, and `rev-list`.
    commits: BTreeMap<String, Vec<String>>,
    /// Deterministic commit messages recorded per commit SHA for `git log` and `git show`.
    commit_messages: BTreeMap<String, String>,
    /// Deterministic touched file paths recorded per commit SHA for `git diff`.
    commit_files: BTreeMap<String, Vec<String>>,
    /// Snapshot of tracked file contents per commit SHA for `git show <rev>:<path>` and `git diff`.
    commit_contents: BTreeMap<String, BTreeMap<String, String>>,
    /// Optional explicit tree SHA per commit (preserves `write-tree` / `commit --amend` tree identity).
    commit_trees: BTreeMap<String, String>,
    next_commit_seq: u64,
    next_worktree_seq: u64,
    worktrees: BTreeMap<PathBuf, SimWorktree>,
    staged: bool,
    staged_tree: Option<String>,
    staged_files: BTreeMap<String, String>,
    unstaged_files: BTreeMap<String, String>,
    untracked_files: BTreeMap<String, String>,
    in_merge: bool,
    merge_head: Option<String>,
    in_cherry_pick: bool,
    in_rebase: bool,
    rebase_onto: Option<String>,
    conflict_files: Vec<String>,
    /// FIFO queue of injected merge/cherry-pick/rebase conflict file lists.
    inject_conflicts: VecDeque<Vec<String>>,
    /// Targeted injected conflict file lists keyed by commit SHA or ref name.
    inject_conflicts_by_ref: BTreeMap<String, Vec<String>>,
    user_name: String,
    user_email: String,
}

impl Default for SimGitRepo {
    fn default() -> Self {
        let mut remotes = BTreeMap::new();
        remotes.insert(
            "origin".into(),
            "https://github.com/acme/canban-sim.git".into(),
        );
        let mut branches = BTreeSet::new();
        branches.insert("main".into());
        let head_sha = "0123456789abcdef0123456789abcdef01234567".to_owned();
        let mut branch_heads = BTreeMap::new();
        branch_heads.insert("main".into(), head_sha.clone());
        let mut remote_refs = BTreeMap::new();
        remote_refs.insert("refs/remotes/origin/main".into(), head_sha.clone());
        let mut commits = BTreeMap::new();
        commits.insert(head_sha.clone(), Vec::new());
        let mut commit_messages = BTreeMap::new();
        commit_messages.insert(head_sha.clone(), "initial".into());
        Self {
            default_branch: "main".into(),
            current_branch: "main".into(),
            head_sha,
            remotes,
            branches,
            dirty: false,
            branch_heads,
            branch_upstreams: BTreeMap::new(),
            remote_refs,
            custom_refs: BTreeMap::new(),
            last_fetch_head: None,
            commits,
            commit_messages,
            commit_files: BTreeMap::new(),
            commit_contents: BTreeMap::new(),
            commit_trees: BTreeMap::new(),
            next_commit_seq: 1,
            next_worktree_seq: 0,
            worktrees: BTreeMap::new(),
            staged: false,
            staged_tree: None,
            staged_files: BTreeMap::new(),
            unstaged_files: BTreeMap::new(),
            untracked_files: BTreeMap::new(),
            in_merge: false,
            merge_head: None,
            in_cherry_pick: false,
            in_rebase: false,
            rebase_onto: None,
            conflict_files: Vec::new(),
            inject_conflicts: VecDeque::new(),
            inject_conflicts_by_ref: BTreeMap::new(),
            user_name: "Sim".into(),
            user_email: "sim@canban.local".into(),
        }
    }
}

impl SimGitRepo {
    fn sync_initial_refs(&mut self) {
        self.commits.entry(self.head_sha.clone()).or_default();
        self.commit_messages
            .entry(self.head_sha.clone())
            .or_insert_with(|| "initial".into());
        self.branches.insert(self.current_branch.clone());
        self.branch_heads
            .entry(self.current_branch.clone())
            .or_insert_with(|| self.head_sha.clone());
        for b in &self.branches {
            self.branch_heads
                .entry(b.clone())
                .or_insert_with(|| self.head_sha.clone());
        }
        for remote in self.remotes.keys() {
            let key = format!("refs/remotes/{remote}/{}", self.default_branch);
            self.remote_refs
                .entry(key)
                .or_insert_with(|| self.head_sha.clone());
        }
    }

    fn alloc_commit(&mut self, parents: Vec<String>) -> String {
        let seq = self.next_commit_seq;
        self.next_commit_seq += 1;
        let sha = format!("{seq:040x}");
        self.commits.insert(sha.clone(), parents);
        sha
    }

    fn commit_subject(&self, sha: &str) -> String {
        self.commit_messages
            .get(sha)
            .and_then(|m| m.lines().next())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("sim commit")
            .to_owned()
    }

    fn commit_body(&self, sha: &str) -> String {
        self.commit_messages
            .get(sha)
            .and_then(|m| {
                m.split_once('\n')
                    .map(|(_, rest)| rest.trim_start_matches('\n'))
            })
            .unwrap_or("")
            .trim_end()
            .to_owned()
    }

    fn format_commit(&self, sha: &str, fmt: &str) -> String {
        let short = &sha[..sha.len().min(7)];
        let parents = self
            .commits
            .get(sha)
            .map(|p| p.join(" "))
            .unwrap_or_default();
        let subj = self.commit_subject(sha);
        let body = self.commit_body(sha);
        fmt.replace("%x09", "\t")
            .replace("%x00", "\0")
            .replace("%H", sha)
            .replace("%h", short)
            .replace("%P", &parents)
            .replace("%an", &self.user_name)
            .replace("%ae", &self.user_email)
            .replace("%cI", "2026-06-01T00:00:00Z")
            .replace("%cr", "1 minute ago")
            .replace("%s", &subj)
            .replace("%b", &body)
    }

    fn resolve_ref(&self, wt_path: Option<&Path>, raw: &str) -> Option<String> {
        let want_tree = raw.ends_with("^{tree}");
        let stripped = raw
            .strip_suffix("^{commit}")
            .or_else(|| raw.strip_suffix("^{tree}"))
            .unwrap_or(raw);

        // Support parent selectors (`<rev>~1`, `<rev>^`, `<rev>^1`, `<rev>^2`, ...).
        let sha = if let Some(stem) = stripped
            .strip_suffix("~1")
            .or_else(|| stripped.strip_suffix("^1"))
            .or_else(|| stripped.strip_suffix('^'))
        {
            let parent_of = self.resolve_ref(wt_path, stem)?;
            self.commits.get(&parent_of)?.first()?.clone()
        } else if let Some((stem, idx_str)) = stripped.rsplit_once('^')
            && !stem.is_empty()
            && let Ok(p_num) = idx_str.parse::<usize>()
            && p_num >= 1
        {
            let parent_of = self.resolve_ref(wt_path, stem)?;
            self.commits.get(&parent_of)?.get(p_num - 1)?.clone()
        } else if stripped == "HEAD" {
            if let Some(wt) = wt_path.and_then(|p| self.worktrees.get(p)) {
                wt.head_sha.clone()
            } else {
                self.head_sha.clone()
            }
        } else if stripped == "FETCH_HEAD" {
            self.last_fetch_head.clone()?
        } else if stripped == "@{upstream}" {
            let cur_b = wt_path
                .and_then(|p| self.worktrees.get(p))
                .and_then(|w| w.branch.as_deref())
                .or((!self.current_branch.is_empty()).then_some(self.current_branch.as_str()))?;
            let up = self.branch_upstreams.get(cur_b)?;
            self.remote_refs.get(&format!("refs/remotes/{up}"))?.clone()
        } else if let Some(b) = stripped.strip_prefix("refs/heads/") {
            if let Some(s) = self.branch_heads.get(b) {
                s.clone()
            } else if self.branches.contains(b) || b == self.current_branch {
                self.head_sha.clone()
            } else {
                return None;
            }
        } else if stripped.starts_with("refs/remotes/") {
            self.remote_refs.get(stripped)?.clone()
        } else if stripped.starts_with("refs/") {
            self.custom_refs.get(stripped)?.clone()
        } else if let Some(s) = self.branch_heads.get(stripped) {
            s.clone()
        } else if self.branches.contains(stripped) {
            self.head_sha.clone()
        } else if let Some(s) = self.remote_refs.get(&format!("refs/remotes/{stripped}")) {
            s.clone()
        } else if self.commits.contains_key(stripped) || is_hex_sha(stripped) {
            stripped.to_owned()
        } else if (4..40).contains(&stripped.len())
            && stripped.chars().all(|c| c.is_ascii_hexdigit())
        {
            let matches: Vec<&String> = self
                .commits
                .keys()
                .filter(|k| k.starts_with(stripped))
                .collect();
            if matches.len() == 1 {
                matches[0].clone()
            } else {
                return None;
            }
        } else {
            return None;
        };

        if want_tree {
            Some(
                self.commit_trees
                    .get(&sha)
                    .cloned()
                    .unwrap_or_else(|| tree_sha_for_commit(&sha)),
            )
        } else {
            Some(sha)
        }
    }

    fn walk_commits_from(&self, start: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(start.to_owned());
        while let Some(cur) = queue.pop_front() {
            if !visited.insert(cur.clone()) {
                continue;
            }
            out.push(cur.clone());
            if let Some(parents) = self.commits.get(&cur) {
                for p in parents {
                    queue.push_back(p.clone());
                }
            }
        }
        out
    }

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> bool {
        if ancestor == descendant {
            return true;
        }
        let mut visited = BTreeSet::new();
        let mut stack = vec![descendant.to_owned()];
        while let Some(cur) = stack.pop() {
            if !visited.insert(cur.clone()) {
                continue;
            }
            if cur == ancestor {
                return true;
            }
            if let Some(parents) = self.commits.get(&cur) {
                for p in parents {
                    stack.push(p.clone());
                }
            }
        }
        false
    }

    fn ancestors_with_depth(&self, start: &str) -> BTreeMap<String, usize> {
        let mut dist = BTreeMap::new();
        let mut queue = VecDeque::new();
        queue.push_back((start.to_owned(), 0usize));
        while let Some((cur, d)) = queue.pop_front() {
            if dist.contains_key(&cur) {
                continue;
            }
            dist.insert(cur.clone(), d);
            if let Some(parents) = self.commits.get(&cur) {
                for p in parents {
                    queue.push_back((p.clone(), d + 1));
                }
            }
        }
        dist
    }

    fn merge_base(&self, a: &str, b: &str) -> Option<String> {
        let da = self.ancestors_with_depth(a);
        let db = self.ancestors_with_depth(b);
        da.into_iter()
            .filter_map(|(sha, cost_a)| db.get(&sha).map(|&cost_b| (cost_a + cost_b, sha)))
            .min()
            .map(|(_, sha)| sha)
    }

    fn rev_list_range(&self, base: &str, head: &str, no_merges: bool) -> Vec<String> {
        let base_anc = self.ancestors_with_depth(base);
        let mut out = Vec::new();
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(head.to_owned());
        while let Some(cur) = queue.pop_front() {
            if base_anc.contains_key(&cur) || !visited.insert(cur.clone()) {
                continue;
            }
            let parents = self.commits.get(&cur).cloned().unwrap_or_default();
            if !(no_merges && parents.len() > 1) {
                out.push(cur);
            }
            for p in parents {
                queue.push_back(p);
            }
        }
        out
    }

    fn set_checkout_head(&mut self, wt_path: Option<&Path>, new_sha: String) {
        self.commits.entry(new_sha.clone()).or_default();
        if let Some(wt) = wt_path.and_then(|p| self.worktrees.get_mut(p)) {
            wt.head_sha = new_sha.clone();
            if let Some(branch) = wt.branch.clone() {
                self.branches.insert(branch.clone());
                self.branch_heads.insert(branch, new_sha);
            }
        } else {
            self.head_sha = new_sha.clone();
            if !self.current_branch.is_empty() {
                self.branches.insert(self.current_branch.clone());
                self.branch_heads
                    .insert(self.current_branch.clone(), new_sha);
            }
        }
    }

    fn pop_injected_conflict(&mut self, keys: &[Option<&str>]) -> Option<Vec<String>> {
        for key in keys.iter().flatten() {
            if let Some(files) = self.inject_conflicts_by_ref.remove(*key) {
                return Some(files);
            }
        }
        self.inject_conflicts.pop_front()
    }

    fn write_rebase_sentinel(git_dir: &Path, onto_sha: &str, branch: Option<&str>) {
        let dir = git_dir.join("rebase-merge");
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = std::fs::write(dir.join("onto"), format!("{onto_sha}\n"));
            if let Some(b) = branch.filter(|b| !b.is_empty()) {
                let _ = std::fs::write(dir.join("head-name"), format!("refs/heads/{b}\n"));
            }
        }
    }

    fn clear_rebase_sentinel(git_dir: &Path) {
        let _ = std::fs::remove_dir_all(git_dir.join("rebase-merge"));
        let _ = std::fs::remove_dir_all(git_dir.join("rebase-apply"));
    }
}

struct State {
    host_name: String,
    shell_env: ShellEnvSnapshot,
    executables: BTreeMap<String, PathBuf>,
    git_repos: BTreeMap<PathBuf, SimGitRepo>,
    /// Reverse index mapping normalized worktree path -> owning normalized repo root.
    worktree_to_repo: BTreeMap<PathBuf, PathBuf>,
    /// Script step outcomes keyed by `CANBAN_STEP_ID` (FIFO queue, falls back to `script_defaults`).
    script_outcomes: BTreeMap<String, VecDeque<SimShellOutcome>>,
    script_defaults: BTreeMap<String, SimShellOutcome>,
    /// Shell `-lc <cmd>` outcomes keyed by command substring (FIFO queue, falls back to `shell_defaults`).
    shell_outcomes: BTreeMap<String, VecDeque<SimShellOutcome>>,
    shell_defaults: BTreeMap<String, SimShellOutcome>,
    recorded: Vec<CommandSpec>,
    blocked: Vec<CommandSpec>,
}

/// Per-host deterministic [`CommandRunner`] used inside [`crate::world::SimWorld`].
pub struct SimExec {
    state: Mutex<State>,
}

impl SimExec {
    pub fn new(host_name: impl Into<String>) -> Arc<Self> {
        let host_name = host_name.into();
        let mut vars = BTreeMap::new();
        vars.insert("PATH".into(), "/sim/bin:/usr/bin:/bin".into());
        vars.insert("HOME".into(), format!("/sim/{host_name}"));
        vars.insert("USER".into(), "sim".into());
        vars.insert("LANG".into(), "C.UTF-8".into());
        let shell_env = ShellEnvSnapshot {
            shell: "/bin/sh".into(),
            vars,
            path_entries: vec![
                PathBuf::from("/sim/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
            ],
            full_env_captured: true,
        };
        let mut executables = BTreeMap::new();
        for (name, path) in [
            ("echo", "/bin/echo"),
            ("/bin/echo", "/bin/echo"),
            ("git", "/sim/bin/git"),
            ("/sim/bin/git", "/sim/bin/git"),
            ("/usr/bin/git", "/sim/bin/git"),
            ("sh", "/bin/sh"),
            ("/bin/sh", "/bin/sh"),
        ] {
            executables.insert(name.into(), PathBuf::from(path));
        }
        Arc::new(Self {
            state: Mutex::new(State {
                host_name,
                shell_env,
                executables,
                git_repos: BTreeMap::new(),
                worktree_to_repo: BTreeMap::new(),
                script_outcomes: BTreeMap::new(),
                script_defaults: BTreeMap::new(),
                shell_outcomes: BTreeMap::new(),
                shell_defaults: BTreeMap::new(),
                recorded: Vec::new(),
                blocked: Vec::new(),
            }),
        })
    }

    pub fn host_name(&self) -> String {
        self.state.lock().unwrap().host_name.clone()
    }

    /// Registers a virtual binary resolvable via `find_executable` without touching the host FS.
    pub fn register_executable(&self, name: impl Into<String>, path: impl Into<PathBuf>) {
        let name = name.into();
        let path = path.into();
        let mut st = self.state.lock().unwrap();
        st.executables
            .insert(path.to_string_lossy().into_owned(), path.clone());
        st.executables.insert(name, path);
    }

    /// Overrides the static [`ShellEnvSnapshot`] returned by this host's `shell_env()`.
    pub fn set_shell_env(&self, snapshot: ShellEnvSnapshot) {
        self.state.lock().unwrap().shell_env = snapshot;
    }

    /// Registers a deterministic git repository fixture for `dir` (matched by exact or ancestor path).
    pub fn register_git_repo(&self, dir: impl AsRef<Path>, mut repo: SimGitRepo) {
        repo.sync_initial_refs();
        let path = normalize_path(dir.as_ref());
        self.state.lock().unwrap().git_repos.insert(path, repo);
    }

    /// Creates a deterministic commit inside `dir` (main repo or managed worktree), advancing
    /// its HEAD and checked-out branch ref. Useful for simulating Agent edits before `deliver`.
    pub fn commit_in_worktree(&self, dir: impl AsRef<Path>, message: &str) -> Result<String> {
        let mut st = self.state.lock().unwrap();
        let (repo_root, wt_key) = Self::resolve_repo_and_worktree(&st, Some(dir.as_ref()))
            .with_context(|| format!("No SimGitRepo found for {}", dir.as_ref().display()))?;
        let repo = st
            .git_repos
            .get_mut(&repo_root)
            .context("owning repo disappeared")?;
        let parent = wt_key
            .as_deref()
            .and_then(|p| repo.worktrees.get(p))
            .map(|w| w.head_sha.clone())
            .unwrap_or_else(|| repo.head_sha.clone());
        let seq_label = repo.next_commit_seq;
        let new_sha = repo.alloc_commit(vec![parent.clone()]);
        repo.commit_messages
            .insert(new_sha.clone(), message.to_owned());
        let file_name = format!("src/change_{seq_label}.rs");
        let mut contents = repo
            .commit_contents
            .get(&parent)
            .cloned()
            .unwrap_or_default();
        contents.insert(
            file_name.clone(),
            format!("// sim commit {new_sha}\n// {message}\n"),
        );
        repo.commit_contents.insert(new_sha.clone(), contents);
        repo.commit_files.insert(new_sha.clone(), vec![file_name]);
        repo.set_checkout_head(wt_key.as_deref(), new_sha.clone());
        if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
            wt.dirty = false;
            wt.staged = false;
            wt.staged_tree = None;
            wt.staged_files.clear();
            wt.unstaged_files.clear();
            wt.untracked_files.clear();
            wt.in_merge = false;
            wt.merge_head = None;
            wt.in_cherry_pick = false;
            wt.in_rebase = false;
            wt.rebase_onto = None;
            wt.conflict_files.clear();
            SimGitRepo::clear_rebase_sentinel(&wt.git_dir);
        } else {
            repo.dirty = false;
            repo.staged = false;
            repo.staged_tree = None;
            repo.staged_files.clear();
            repo.unstaged_files.clear();
            repo.untracked_files.clear();
            repo.in_merge = false;
            repo.merge_head = None;
            repo.in_cherry_pick = false;
            repo.in_rebase = false;
            repo.rebase_onto = None;
            repo.conflict_files.clear();
            SimGitRepo::clear_rebase_sentinel(&repo_root.join(".git"));
        }
        Ok(new_sha)
    }

    /// Writes a deterministic file into a simulated repo/worktree on disk and marks it modified
    /// or untracked in [`SimGitRepo`] so `git.{status,stage,unstage,commit,compare,file_lines}`
    /// observe real file contents with zero host `git` spawns.
    pub fn write_worktree_file(
        &self,
        dir: impl AsRef<Path>,
        rel_path: &str,
        content: &str,
    ) -> Result<()> {
        let target = dir.as_ref().join(rel_path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, content)?;
        let mut st = self.state.lock().unwrap();
        let (repo_root, wt_key) = Self::resolve_repo_and_worktree(&st, Some(dir.as_ref()))
            .with_context(|| format!("No SimGitRepo found for {}", dir.as_ref().display()))?;
        let repo = st
            .git_repos
            .get_mut(&repo_root)
            .context("owning repo disappeared")?;
        let head_sha = wt_key
            .as_deref()
            .and_then(|p| repo.worktrees.get(p))
            .map(|w| w.head_sha.clone())
            .unwrap_or_else(|| repo.head_sha.clone());
        let is_tracked = repo
            .commit_contents
            .get(&head_sha)
            .is_some_and(|m| m.contains_key(rel_path));
        if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
            if is_tracked || wt.staged_files.contains_key(rel_path) {
                wt.untracked_files.remove(rel_path);
                wt.unstaged_files
                    .insert(rel_path.to_owned(), content.to_owned());
            } else {
                wt.untracked_files
                    .insert(rel_path.to_owned(), content.to_owned());
            }
            wt.dirty = true;
        } else {
            if is_tracked || repo.staged_files.contains_key(rel_path) {
                repo.untracked_files.remove(rel_path);
                repo.unstaged_files
                    .insert(rel_path.to_owned(), content.to_owned());
            } else {
                repo.untracked_files
                    .insert(rel_path.to_owned(), content.to_owned());
            }
            repo.dirty = true;
        }
        Ok(())
    }

    /// Configures an explicit upstream tracking branch (e.g. `main -> origin/main`) in [`SimGitRepo`].
    pub fn set_branch_upstream(
        &self,
        repo_dir: impl AsRef<Path>,
        branch: &str,
        upstream: &str,
    ) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        let (repo_root, _) = Self::resolve_repo_and_worktree(&st, Some(repo_dir.as_ref()))
            .with_context(|| format!("No SimGitRepo found for {}", repo_dir.as_ref().display()))?;
        let repo = st
            .git_repos
            .get_mut(&repo_root)
            .context("owning repo disappeared")?;
        repo.branch_upstreams
            .insert(branch.to_owned(), upstream.to_owned());
        Ok(())
    }

    /// Registers an external (outside-Canban-DB) git worktree in [`SimGitRepo`] via in-process
    /// `git worktree add -b <branch> <wt_dir>` with zero OS spawns.
    pub fn add_external_worktree(
        &self,
        repo_dir: impl AsRef<Path>,
        branch: &str,
        wt_dir: impl AsRef<Path>,
    ) -> Result<()> {
        let out = self.run(&CommandSpec {
            program: "git".into(),
            args: vec![
                "-C".into(),
                repo_dir.as_ref().to_string_lossy().into_owned(),
                "worktree".into(),
                "add".into(),
                "-b".into(),
                branch.into(),
                wt_dir.as_ref().to_string_lossy().into_owned(),
            ],
            cwd: Some(repo_dir.as_ref().to_path_buf()),
            env_set: BTreeMap::new(),
            env_remove: Vec::new(),
            stdin: None,
        })?;
        if out.status_code != 0 {
            bail!(
                "sim external worktree add failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    /// Sets dirty status on a main repo or registered worktree directory.
    pub fn set_worktree_dirty(&self, dir: impl AsRef<Path>, dirty: bool) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        let (repo_root, wt_key) = Self::resolve_repo_and_worktree(&st, Some(dir.as_ref()))
            .with_context(|| format!("No SimGitRepo found for {}", dir.as_ref().display()))?;
        let repo = st
            .git_repos
            .get_mut(&repo_root)
            .context("owning repo disappeared")?;
        if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
            wt.dirty = dirty;
        } else {
            repo.dirty = dirty;
        }
        Ok(())
    }

    /// Queues a merge/cherry-pick/rebase conflict for the next integration operation on `repo_dir`.
    pub fn inject_merge_conflict(&self, repo_dir: impl AsRef<Path>, files: Vec<String>) {
        let mut st = self.state.lock().unwrap();
        if let Some((root, _)) = Self::resolve_repo_and_worktree(&st, Some(repo_dir.as_ref()))
            && let Some(repo) = st.git_repos.get_mut(&root)
        {
            repo.inject_conflicts.push_back(files);
        }
    }

    /// Queues a merge/cherry-pick/rebase conflict triggered specifically when integrating
    /// `commit_or_ref` (exact commit SHA or ref name), leaving earlier cards in a multi-card
    /// batch unaffected.
    pub fn inject_merge_conflict_for_commit(
        &self,
        repo_dir: impl AsRef<Path>,
        commit_or_ref: impl Into<String>,
        files: Vec<String>,
    ) {
        let mut st = self.state.lock().unwrap();
        if let Some((root, _)) = Self::resolve_repo_and_worktree(&st, Some(repo_dir.as_ref()))
            && let Some(repo) = st.git_repos.get_mut(&root)
        {
            repo.inject_conflicts_by_ref
                .insert(commit_or_ref.into(), files);
        }
    }

    /// Aborts any in-progress cherry-pick, merge, or rebase conflict inside `dir` (restoring a
    /// clean checkout at the last integrated commit without creating an extra commit; simulates
    /// `@merger` running `git cherry-pick --abort` before evicting a conflicting Card).
    pub fn abort_worktree_conflict(&self, dir: impl AsRef<Path>) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        let (repo_root, wt_key) = Self::resolve_repo_and_worktree(&st, Some(dir.as_ref()))
            .with_context(|| format!("No SimGitRepo found for {}", dir.as_ref().display()))?;
        let repo = st
            .git_repos
            .get_mut(&repo_root)
            .context("owning repo disappeared")?;
        if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
            wt.dirty = false;
            wt.staged = false;
            wt.in_merge = false;
            wt.merge_head = None;
            wt.in_cherry_pick = false;
            wt.in_rebase = false;
            wt.rebase_onto = None;
            wt.conflict_files.clear();
            SimGitRepo::clear_rebase_sentinel(&wt.git_dir);
        } else {
            repo.dirty = false;
            repo.staged = false;
            repo.in_merge = false;
            repo.merge_head = None;
            repo.in_cherry_pick = false;
            repo.in_rebase = false;
            repo.rebase_onto = None;
            repo.conflict_files.clear();
            SimGitRepo::clear_rebase_sentinel(&repo_root.join(".git"));
        }
        Ok(())
    }

    /// Clears any unresolved conflict state in `dir` and creates a resolution commit (simulates
    /// `@merger` fixing conflicts or test failures inside `_merge_queue`).
    pub fn resolve_worktree_and_commit(&self, dir: impl AsRef<Path>) -> Result<String> {
        self.commit_in_worktree(dir, "resolve batch conflict")
    }

    /// Returns the current HEAD SHA for a main repo or worktree directory.
    pub fn head_sha(&self, dir: impl AsRef<Path>) -> Option<String> {
        let st = self.state.lock().unwrap();
        let (root, wt_key) = Self::resolve_repo_and_worktree(&st, Some(dir.as_ref()))?;
        let repo = st.git_repos.get(&root)?;
        Some(
            wt_key
                .as_deref()
                .and_then(|p| repo.worktrees.get(p))
                .map(|w| w.head_sha.clone())
                .unwrap_or_else(|| repo.head_sha.clone()),
        )
    }

    /// Returns the recorded local branch HEAD SHA (`refs/heads/<branch>`).
    pub fn branch_head_sha(&self, repo_dir: impl AsRef<Path>, branch: &str) -> Option<String> {
        let st = self.state.lock().unwrap();
        let (root, _) = Self::resolve_repo_and_worktree(&st, Some(repo_dir.as_ref()))?;
        let repo = st.git_repos.get(&root)?;
        repo.branch_heads.get(branch).cloned()
    }

    /// Returns the recorded remote tracking ref SHA (`refs/remotes/<remote>/<branch>`).
    pub fn remote_ref_sha(
        &self,
        repo_dir: impl AsRef<Path>,
        remote: &str,
        branch: &str,
    ) -> Option<String> {
        let st = self.state.lock().unwrap();
        let (root, _) = Self::resolve_repo_and_worktree(&st, Some(repo_dir.as_ref()))?;
        let repo = st.git_repos.get(&root)?;
        repo.remote_refs
            .get(&format!("refs/remotes/{remote}/{branch}"))
            .cloned()
    }

    /// Advances `refs/remotes/<remote>/<branch>` by one deterministic upstream commit (useful for
    /// verifying `sync_upstream: true` `sync_target` fast-forwards local `main` before merging).
    pub fn advance_remote_ref(
        &self,
        repo_dir: impl AsRef<Path>,
        remote: &str,
        branch: &str,
    ) -> Result<String> {
        let mut st = self.state.lock().unwrap();
        let (root, _) = Self::resolve_repo_and_worktree(&st, Some(repo_dir.as_ref()))
            .with_context(|| format!("No SimGitRepo found for {}", repo_dir.as_ref().display()))?;
        let repo = st
            .git_repos
            .get_mut(&root)
            .context("owning repo disappeared")?;
        let key = format!("refs/remotes/{remote}/{branch}");
        let parent = repo
            .remote_refs
            .get(&key)
            .cloned()
            .or_else(|| repo.branch_heads.get(branch).cloned())
            .unwrap_or_else(|| repo.head_sha.clone());
        let new_sha = repo.alloc_commit(vec![parent]);
        repo.remote_refs.insert(key, new_sha.clone());
        Ok(new_sha)
    }

    /// Returns the recorded parent commit SHAs for `sha` in `repo_dir`'s commit DAG.
    pub fn commit_parents(&self, repo_dir: impl AsRef<Path>, sha: &str) -> Option<Vec<String>> {
        let st = self.state.lock().unwrap();
        let (root, _) = Self::resolve_repo_and_worktree(&st, Some(repo_dir.as_ref()))?;
        let repo = st.git_repos.get(&root)?;
        repo.commits.get(sha).cloned()
    }

    /// Returns all currently registered worktree paths for `repo_dir`.
    pub fn worktree_paths(&self, repo_dir: impl AsRef<Path>) -> Vec<PathBuf> {
        let st = self.state.lock().unwrap();
        let Some((root, _)) = Self::resolve_repo_and_worktree(&st, Some(repo_dir.as_ref())) else {
            return Vec::new();
        };
        st.git_repos
            .get(&root)
            .map(|r| r.worktrees.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Queues scripted outcomes (consumed FIFO) for a `uses: script` step identified by `CANBAN_STEP_ID`.
    pub fn enqueue_script_outcomes(
        &self,
        step_id: impl Into<String>,
        outcomes: impl IntoIterator<Item = SimShellOutcome>,
    ) {
        let mut st = self.state.lock().unwrap();
        st.script_outcomes
            .entry(step_id.into())
            .or_default()
            .extend(outcomes);
    }

    /// Sets a sticky default outcome for a `uses: script` step when its FIFO queue is empty.
    pub fn set_script_default(&self, step_id: impl Into<String>, outcome: SimShellOutcome) {
        self.state
            .lock()
            .unwrap()
            .script_defaults
            .insert(step_id.into(), outcome);
    }

    /// Queues scripted outcomes (consumed FIFO) for `/bin/sh -lc <cmd>` matching `cmd_pattern`.
    pub fn enqueue_shell_outcomes(
        &self,
        cmd_pattern: impl Into<String>,
        outcomes: impl IntoIterator<Item = SimShellOutcome>,
    ) {
        let mut st = self.state.lock().unwrap();
        st.shell_outcomes
            .entry(cmd_pattern.into())
            .or_default()
            .extend(outcomes);
    }

    /// Sets a sticky default outcome for `/bin/sh -lc <cmd>` matching `cmd_pattern`.
    pub fn set_shell_default(&self, cmd_pattern: impl Into<String>, outcome: SimShellOutcome) {
        self.state
            .lock()
            .unwrap()
            .shell_defaults
            .insert(cmd_pattern.into(), outcome);
    }

    /// Returns every [`CommandSpec`] executed through this runner (virtual and blocked).
    pub fn recorded_commands(&self) -> Vec<CommandSpec> {
        self.state.lock().unwrap().recorded.clone()
    }

    /// Returns every unexpected OS command rejected by fail-closed enforcement.
    pub fn blocked_commands(&self) -> Vec<CommandSpec> {
        self.state.lock().unwrap().blocked.clone()
    }

    pub fn command_count(&self) -> usize {
        self.state.lock().unwrap().recorded.len()
    }

    pub fn clear_recorded_commands(&self) {
        let mut st = self.state.lock().unwrap();
        st.recorded.clear();
        st.blocked.clear();
    }

    fn resolve_repo_and_worktree(
        st: &State,
        dir: Option<&Path>,
    ) -> Option<(PathBuf, Option<PathBuf>)> {
        let raw = dir?;
        let norm = normalize_path(raw);

        // 1. Check registered worktrees first (longest prefix match).
        if let Some((wt_root, repo_root)) = st
            .worktree_to_repo
            .iter()
            .filter(|(wt, _)| norm.starts_with(wt) || raw.starts_with(wt))
            .max_by_key(|(wt, _)| wt.components().count())
        {
            return Some((repo_root.clone(), Some(wt_root.clone())));
        }

        // 2. Fall back to main repo roots (longest prefix match).
        st.git_repos
            .keys()
            .filter(|root| norm.starts_with(root) || raw.starts_with(root))
            .max_by_key(|root| root.components().count())
            .map(|root| (root.clone(), None))
    }

    fn run_shell(st: &mut State, spec: &CommandSpec) -> Option<Result<ExecOutput>> {
        // 1. `uses: script` steps set `CANBAN_STEP_ID` in `spec.env_set`.
        if let Some(step_id) = spec.env_set.get("CANBAN_STEP_ID") {
            let outcome = st
                .script_outcomes
                .get_mut(step_id)
                .and_then(VecDeque::pop_front)
                .or_else(|| st.script_defaults.get(step_id).cloned());
            if let Some(out) = outcome {
                if let Some(env_file) = spec.env_set.get("CANBAN_ENV")
                    && !out.write_env.is_empty()
                {
                    let mut body = String::new();
                    for (k, v) in &out.write_env {
                        body.push_str(&format!("{k}={v}\n"));
                    }
                    if let Err(e) = std::fs::write(env_file, body) {
                        return Some(Err(e.into()));
                    }
                }
                if let Some(out_file) = spec.env_set.get("CANBAN_OUTPUT")
                    && !out.write_output.is_empty()
                {
                    let mut body = String::new();
                    for (k, v) in &out.write_output {
                        body.push_str(&format!("{k}={v}\n"));
                    }
                    if let Err(e) = std::fs::write(out_file, body) {
                        return Some(Err(e.into()));
                    }
                }
                crate::trace::record_coverage_hit("exec.script_executed");
                return Some(Ok(ExecOutput {
                    status_code: out.status_code,
                    stdout: out.stdout.into_bytes(),
                    stderr: out.stderr.into_bytes(),
                }));
            }
        }

        // 2. `/bin/sh -lc <cmd>` or `-c <cmd>` (used by `merge_batch::run_batch_tests`).
        let cmd_str = spec
            .args
            .iter()
            .position(|a| a == "-lc" || a == "-c")
            .and_then(|pos| spec.args.get(pos + 1))
            .map(String::as_str)
            .unwrap_or("");
        if !cmd_str.is_empty() {
            let queued_key = st
                .shell_outcomes
                .iter()
                .filter(|(pat, q)| !q.is_empty() && cmd_str.contains(pat.as_str()))
                .max_by_key(|(pat, _)| pat.len())
                .map(|(pat, _)| pat.clone());
            let outcome = if let Some(key) = queued_key {
                st.shell_outcomes
                    .get_mut(&key)
                    .and_then(VecDeque::pop_front)
            } else {
                st.shell_defaults
                    .iter()
                    .filter(|(pat, _)| cmd_str.contains(pat.as_str()))
                    .max_by_key(|(pat, _)| pat.len())
                    .map(|(_, o)| o.clone())
            };
            if let Some(out) = outcome {
                crate::trace::record_coverage_hit("exec.shell_executed");
                return Some(Ok(ExecOutput {
                    status_code: out.status_code,
                    stdout: out.stdout.into_bytes(),
                    stderr: out.stderr.into_bytes(),
                }));
            }
        }

        None
    }

    fn run_git(st: &mut State, spec: &CommandSpec) -> ExecOutput {
        let mut dir = spec.cwd.clone();
        let mut idx = 0usize;
        while idx < spec.args.len() {
            match spec.args[idx].as_str() {
                "--no-optional-locks" => idx += 1,
                "-C" => {
                    if let Some(p) = spec.args.get(idx + 1) {
                        dir = Some(PathBuf::from(p));
                        idx += 2;
                    } else {
                        break;
                    }
                }
                "-c" => {
                    idx += 2;
                }
                _ => break,
            }
        }
        let sub_args = &spec.args[idx.min(spec.args.len())..];
        let Some(sub) = sub_args.first().map(String::as_str) else {
            return err_out(1, "sim git: missing subcommand\n");
        };

        if sub == "--version" {
            return ok_out("git version 2.45.0 (canban-sim)\n");
        }

        if sub == "check-ref-format" {
            let branch = sub_args.last().map(String::as_str).unwrap_or("");
            if is_valid_git_branch_name(branch) {
                return ok_out(&format!("{branch}\n"));
            }
            return err_out(
                1,
                &format!("fatal: '{branch}' is not a valid branch name\n"),
            );
        }

        let Some((root, wt_key)) = Self::resolve_repo_and_worktree(st, dir.as_deref()) else {
            return err_out(
                128,
                "fatal: not a git repository (or any of the parent directories): .git\n",
            );
        };

        // Handle worktree subcommands that also update `st.worktree_to_repo`.
        if sub == "worktree" {
            return Self::run_git_worktree(st, &root, &sub_args[1..]);
        }

        let Some(repo) = st.git_repos.get_mut(&root) else {
            return err_out(128, "fatal: not a git repository\n");
        };

        let checkout_root = wt_key.as_deref().unwrap_or(root.as_path());
        let checkout_git_dir = wt_key
            .as_deref()
            .and_then(|p| repo.worktrees.get(p))
            .map(|w| w.git_dir.clone())
            .unwrap_or_else(|| root.join(".git"));
        let (
            cur_branch,
            cur_head,
            cur_dirty,
            cur_staged,
            cur_staged_files,
            cur_unstaged_files,
            cur_untracked_files,
            cur_in_merge,
            cur_merge_head,
            cur_in_cp,
            cur_conflicts,
        ) = if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get(p)) {
            (
                wt.branch.clone(),
                wt.head_sha.clone(),
                wt.dirty,
                wt.staged,
                wt.staged_files.clone(),
                wt.unstaged_files.clone(),
                wt.untracked_files.clone(),
                wt.in_merge,
                wt.merge_head.clone(),
                wt.in_cherry_pick,
                wt.conflict_files.clone(),
            )
        } else {
            (
                (!repo.current_branch.is_empty()).then(|| repo.current_branch.clone()),
                repo.head_sha.clone(),
                repo.dirty,
                repo.staged,
                repo.staged_files.clone(),
                repo.unstaged_files.clone(),
                repo.untracked_files.clone(),
                repo.in_merge,
                repo.merge_head.clone(),
                repo.in_cherry_pick,
                repo.conflict_files.clone(),
            )
        };

        match sub {
            "rev-parse" => {
                let flags = &sub_args[1..];
                let short_sha = flags.iter().any(|f| f == "--short");
                if flags.iter().any(|f| f == "--is-inside-work-tree") {
                    ok_out("true\n")
                } else if flags.iter().any(|f| f == "--git-common-dir")
                    && flags.iter().any(|f| f == "--git-dir")
                    && flags.iter().any(|f| f == "--show-toplevel")
                {
                    ok_out(&format!(
                        "{}\n{}\n{}\n",
                        root.join(".git").display(),
                        checkout_git_dir.display(),
                        checkout_root.display()
                    ))
                } else if flags.iter().any(|f| f == "--show-toplevel") {
                    ok_out(&format!("{}\n", checkout_root.display()))
                } else if flags.iter().any(|f| f == "--git-common-dir") {
                    ok_out(&format!("{}\n", root.join(".git").display()))
                } else if flags.iter().any(|f| f == "--git-dir") {
                    ok_out(&format!("{}\n", checkout_git_dir.display()))
                } else if let Some(pos) = flags.iter().position(|f| f == "--git-path") {
                    let rel = flags.get(pos + 1).map(String::as_str).unwrap_or("");
                    ok_out(&format!("{}\n", checkout_git_dir.join(rel).display()))
                } else if flags.iter().any(|f| f == "@{upstream}") {
                    if let Some(b) = cur_branch.as_deref()
                        && let Some(up) = repo.branch_upstreams.get(b)
                    {
                        if flags.iter().any(|f| f == "--abbrev-ref") {
                            ok_out(&format!("{up}\n"))
                        } else {
                            ok_out(&format!("refs/remotes/{up}\n"))
                        }
                    } else {
                        err_out(128, "fatal: no upstream configured\n")
                    }
                } else if flags.iter().any(|f| f == "--abbrev-ref") {
                    let name = cur_branch.as_deref().unwrap_or("HEAD");
                    ok_out(&format!("{name}\n"))
                } else if flags.iter().any(|f| f == "MERGE_HEAD") {
                    if cur_in_merge {
                        let mh = cur_merge_head.as_deref().unwrap_or(&repo.head_sha);
                        ok_out(&format!("{mh}\n"))
                    } else {
                        err_out(128, "fatal: Needed a single revision (MERGE_HEAD)\n")
                    }
                } else if flags.iter().any(|f| f == "CHERRY_PICK_HEAD") {
                    if cur_in_cp {
                        ok_out(&format!("{}\n", cur_head))
                    } else {
                        err_out(128, "fatal: Needed a single revision (CHERRY_PICK_HEAD)\n")
                    }
                } else if let Some(target) = flags.iter().rev().find(|f| !f.starts_with('-')) {
                    if let Some(sha) = repo.resolve_ref(wt_key.as_deref(), target) {
                        if short_sha {
                            ok_out(&format!("{}\n", &sha[..sha.len().min(7)]))
                        } else {
                            ok_out(&format!("{sha}\n"))
                        }
                    } else {
                        err_out(
                            128,
                            &format!("fatal: Needed a single revision ({target})\n"),
                        )
                    }
                } else if short_sha {
                    ok_out(&format!("{}\n", &cur_head[..cur_head.len().min(7)]))
                } else {
                    ok_out(&format!("{cur_head}\n"))
                }
            }
            "symbolic-ref" => {
                let short = sub_args.iter().any(|a| a == "--short");
                let target = sub_args.last().map(String::as_str).unwrap_or("");
                if target == "HEAD" {
                    if let Some(b) = cur_branch {
                        if short {
                            ok_out(&format!("{b}\n"))
                        } else {
                            ok_out(&format!("refs/heads/{b}\n"))
                        }
                    } else {
                        err_out(1, "fatal: ref HEAD is not a symbolic ref\n")
                    }
                } else if let Some(rest) = target.strip_prefix("refs/remotes/")
                    && let Some(remote) = rest.strip_suffix("/HEAD")
                {
                    if repo.remotes.contains_key(remote) {
                        ok_out(&format!("refs/remotes/{remote}/{}\n", repo.default_branch))
                    } else {
                        err_out(1, &format!("fatal: ref {target} is not a symbolic ref\n"))
                    }
                } else {
                    err_out(1, &format!("fatal: ref {target} is not a symbolic ref\n"))
                }
            }
            "remote" => {
                if sub_args.len() == 1 {
                    let mut out = String::new();
                    for name in repo.remotes.keys() {
                        out.push_str(name);
                        out.push('\n');
                    }
                    ok_out(&out)
                } else if sub_args.get(1).map(String::as_str) == Some("get-url") {
                    let remote = sub_args
                        .iter()
                        .skip(2)
                        .find(|a| !a.starts_with('-'))
                        .map(String::as_str)
                        .unwrap_or("origin");
                    if let Some(url) = repo.remotes.get(remote) {
                        ok_out(&format!("{url}\n"))
                    } else {
                        err_out(2, &format!("error: No such remote '{remote}'\n"))
                    }
                } else {
                    err_out(1, "sim git: unsupported remote subcommand\n")
                }
            }
            "config" => {
                let positional: Vec<&str> = sub_args[1..]
                    .iter()
                    .filter(|a| !a.starts_with('-'))
                    .map(String::as_str)
                    .collect();
                let is_get = sub_args.iter().any(|a| a == "--get") || positional.len() == 1;
                if let Some(&key) = positional.first() {
                    if is_get {
                        if key == "user.name" {
                            return ok_out(&format!("{}\n", repo.user_name));
                        }
                        if key == "user.email" {
                            return ok_out(&format!("{}\n", repo.user_email));
                        }
                        if let Some(rest) = key.strip_prefix("remote.")
                            && let Some(remote) = rest.strip_suffix(".url")
                            && let Some(url) = repo.remotes.get(remote)
                        {
                            return ok_out(&format!("{url}\n"));
                        }
                        return err_out(1, "");
                    }
                    if let Some(&val) = positional.get(1) {
                        if key == "user.name" {
                            repo.user_name = val.to_owned();
                        } else if key == "user.email" {
                            repo.user_email = val.to_owned();
                        }
                        return ok_out("");
                    }
                }
                err_out(1, "")
            }
            "branch" => {
                if sub_args.iter().any(|a| a == "--show-current") {
                    match cur_branch {
                        Some(b) => ok_out(&format!("{b}\n")),
                        None => ok_out(""),
                    }
                } else if sub_args.iter().any(|a| a == "-d" || a == "-D") {
                    if let Some(name) = sub_args.last() {
                        repo.branches.remove(name);
                        repo.branch_heads.remove(name);
                    }
                    ok_out("")
                } else {
                    let pos: Vec<&str> = sub_args[1..]
                        .iter()
                        .filter(|a| !a.starts_with('-'))
                        .map(String::as_str)
                        .collect();
                    if let Some(&new_branch) = pos.first() {
                        let start = pos
                            .get(1)
                            .and_then(|r| repo.resolve_ref(wt_key.as_deref(), r))
                            .unwrap_or_else(|| cur_head.clone());
                        repo.branches.insert(new_branch.to_owned());
                        repo.branch_heads.insert(new_branch.to_owned(), start);
                        ok_out("")
                    } else {
                        let mut out = String::new();
                        for b in &repo.branches {
                            out.push_str(b);
                            out.push('\n');
                        }
                        ok_out(&out)
                    }
                }
            }
            "switch" | "checkout" => {
                let args = &sub_args[1..];
                if let Some(sep_pos) = args.iter().position(|a| a == "--") {
                    // Path checkout (`git checkout HEAD -- <paths>`): restore tracked files.
                    let paths = &args[sep_pos + 1..];
                    let head_contents = repo
                        .commit_contents
                        .get(&cur_head)
                        .cloned()
                        .unwrap_or_default();
                    for p in paths {
                        if let Some(content) = head_contents.get(p) {
                            let target = checkout_root.join(p);
                            if let Some(parent) = target.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            let _ = std::fs::write(target, content);
                        }
                        if let Some(wt) = wt_key.as_deref().and_then(|k| repo.worktrees.get_mut(k))
                        {
                            wt.unstaged_files.remove(p);
                            wt.dirty =
                                !wt.unstaged_files.is_empty() || !wt.untracked_files.is_empty();
                        } else {
                            repo.unstaged_files.remove(p);
                            repo.dirty =
                                !repo.unstaged_files.is_empty() || !repo.untracked_files.is_empty();
                        }
                    }
                    ok_out("")
                } else if args.iter().any(|a| a == "--detach") {
                    let target = args
                        .iter()
                        .rev()
                        .find(|a| !a.starts_with('-'))
                        .map(String::as_str)
                        .unwrap_or("HEAD");
                    let Some(sha) = repo.resolve_ref(wt_key.as_deref(), target) else {
                        return err_out(128, &format!("fatal: invalid reference: {target}\n"));
                    };
                    if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                        wt.branch = None;
                        wt.head_sha = sha;
                    } else {
                        repo.current_branch.clear();
                        repo.head_sha = sha;
                    }
                    ok_out("")
                } else if let Some(pos) = args.iter().position(|a| a == "-b" || a == "-c") {
                    let Some(new_branch) = args.get(pos + 1) else {
                        return err_out(128, "fatal: missing branch name\n");
                    };
                    let start = args
                        .get(pos + 2)
                        .and_then(|r| repo.resolve_ref(wt_key.as_deref(), r))
                        .unwrap_or_else(|| cur_head.clone());
                    repo.branches.insert(new_branch.clone());
                    repo.branch_heads.insert(new_branch.clone(), start.clone());
                    if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                        wt.branch = Some(new_branch.clone());
                        wt.head_sha = start;
                    } else {
                        repo.current_branch = new_branch.clone();
                        repo.head_sha = start;
                    }
                    ok_out("")
                } else if let Some(target) = args.iter().rev().find(|a| !a.starts_with('-')) {
                    if let Some(sha) = repo.resolve_ref(wt_key.as_deref(), target) {
                        let is_branch = repo.branches.contains(target);
                        if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p))
                        {
                            wt.branch = is_branch.then(|| target.clone());
                            wt.head_sha = sha;
                        } else {
                            repo.current_branch = if is_branch {
                                target.clone()
                            } else {
                                String::new()
                            };
                            repo.head_sha = sha;
                        }
                        ok_out("")
                    } else {
                        err_out(1, &format!("error: pathspec '{target}' did not match\n"))
                    }
                } else {
                    ok_out("")
                }
            }
            "status" => {
                let nul = sub_args.iter().any(|a| a == "-z");
                let show_branch = sub_args.iter().any(|a| a == "--branch");
                let mut out = String::new();
                if show_branch {
                    let header = match &cur_branch {
                        Some(b) => format!("## {b}"),
                        None => "## HEAD (no branch)".into(),
                    };
                    out.push_str(&header);
                    out.push(if nul { '\0' } else { '\n' });
                }
                if !cur_conflicts.is_empty() {
                    for f in &cur_conflicts {
                        out.push_str("UU ");
                        out.push_str(f);
                        out.push(if nul { '\0' } else { '\n' });
                    }
                } else if !cur_staged_files.is_empty()
                    || !cur_unstaged_files.is_empty()
                    || !cur_untracked_files.is_empty()
                {
                    let head_tracked = repo.commit_contents.get(&cur_head);
                    let mut all_paths = BTreeSet::new();
                    all_paths.extend(cur_staged_files.keys().cloned());
                    all_paths.extend(cur_unstaged_files.keys().cloned());
                    all_paths.extend(cur_untracked_files.keys().cloned());
                    for path in all_paths {
                        let in_staged = cur_staged_files.contains_key(&path);
                        let in_unstaged = cur_unstaged_files.contains_key(&path);
                        let in_untracked = cur_untracked_files.contains_key(&path);
                        let code = if in_untracked && !in_staged {
                            "??"
                        } else {
                            let tracked = head_tracked.is_some_and(|m| m.contains_key(&path));
                            let x = if in_staged {
                                if tracked { 'M' } else { 'A' }
                            } else {
                                ' '
                            };
                            let y = if in_unstaged { 'M' } else { ' ' };
                            match (x, y) {
                                ('A', ' ') => "A ",
                                ('A', 'M') => "AM",
                                ('M', ' ') => "M ",
                                ('M', 'M') => "MM",
                                (' ', 'M') => " M",
                                _ => " M",
                            }
                        };
                        out.push_str(code);
                        out.push(' ');
                        out.push_str(&path);
                        out.push(if nul { '\0' } else { '\n' });
                    }
                } else if cur_dirty {
                    out.push_str(" M dirty.txt");
                    out.push(if nul { '\0' } else { '\n' });
                }
                ok_out(&out)
            }
            "show-ref" => {
                if let Some(ref_name) = sub_args.last().map(String::as_str)
                    && let Some(sha) = repo.resolve_ref(wt_key.as_deref(), ref_name)
                {
                    if sub_args.iter().any(|a| a == "--quiet") {
                        ok_out("")
                    } else {
                        ok_out(&format!("{sha} {ref_name}\n"))
                    }
                } else {
                    err_out(1, "")
                }
            }
            "for-each-ref" => {
                let format_opt = sub_args.iter().find_map(|a| a.strip_prefix("--format="));
                let strip_heads = sub_args.iter().any(|a| a.contains("%(refname:strip=2)"));
                let prefix = sub_args
                    .iter()
                    .skip(1)
                    .find(|a| !a.starts_with('-'))
                    .map(String::as_str)
                    .unwrap_or("");
                if let Some(fmt) = format_opt
                    && fmt.contains("%09")
                {
                    let mut out = String::new();
                    if prefix == "refs/heads"
                        || prefix.starts_with("refs/heads/")
                        || prefix.is_empty()
                    {
                        for b in &repo.branches {
                            let full = format!("refs/heads/{b}");
                            if !prefix.is_empty() && !full.starts_with(prefix) {
                                continue;
                            }
                            let sha = repo
                                .branch_heads
                                .get(b)
                                .cloned()
                                .unwrap_or_else(|| repo.head_sha.clone());
                            let short = &sha[..sha.len().min(7)];
                            let subj = repo.commit_subject(&sha);
                            let up = repo.branch_upstreams.get(b).cloned().unwrap_or_default();
                            let track = if !up.is_empty()
                                && let Some(up_sha) =
                                    repo.remote_refs.get(&format!("refs/remotes/{up}"))
                            {
                                let mb = repo
                                    .merge_base(up_sha, &sha)
                                    .unwrap_or_else(|| up_sha.clone());
                                let behind = repo.rev_list_range(&mb, up_sha, false).len();
                                let ahead = repo.rev_list_range(&mb, &sha, false).len();
                                match (ahead, behind) {
                                    (0, 0) => String::new(),
                                    (a, 0) => format!("[ahead {a}]"),
                                    (0, b) => format!("[behind {b}]"),
                                    (a, b) => format!("[ahead {a}, behind {b}]"),
                                }
                            } else {
                                String::new()
                            };
                            let line = fmt
                                .replace("%09", "\t")
                                .replace("%(refname:short)", b)
                                .replace("%(objectname:short)", short)
                                .replace("%(objectname)", &sha)
                                .replace("%(committerdate:relative)", "1 minute ago")
                                .replace("%(creatordate:relative)", "1 minute ago")
                                .replace("%(upstream:short)", &up)
                                .replace("%(upstream:track)", &track)
                                .replace("%(contents:subject)", &subj);
                            out.push_str(&line);
                            out.push('\n');
                        }
                    }
                    if prefix == "refs/remotes"
                        || prefix.starts_with("refs/remotes/")
                        || prefix.is_empty()
                    {
                        for (full, sha) in &repo.remote_refs {
                            if !prefix.is_empty() && !full.starts_with(prefix) {
                                continue;
                            }
                            let short_ref = full.strip_prefix("refs/remotes/").unwrap_or(full);
                            let short = &sha[..sha.len().min(7)];
                            let subj = repo.commit_subject(sha);
                            let line = fmt
                                .replace("%09", "\t")
                                .replace("%(refname:short)", short_ref)
                                .replace("%(objectname:short)", short)
                                .replace("%(objectname)", sha)
                                .replace("%(committerdate:relative)", "1 minute ago")
                                .replace("%(creatordate:relative)", "1 minute ago")
                                .replace("%(upstream:short)", "")
                                .replace("%(upstream:track)", "")
                                .replace("%(contents:subject)", &subj);
                            out.push_str(&line);
                            out.push('\n');
                        }
                    }
                    return ok_out(&out);
                }
                let mut refs: Vec<String> = Vec::new();
                for b in &repo.branches {
                    let full = format!("refs/heads/{b}");
                    if prefix.is_empty() || full.starts_with(prefix) {
                        refs.push(if strip_heads { b.clone() } else { full });
                    }
                }
                for r in repo.remote_refs.keys() {
                    if prefix.is_empty() || r.starts_with(prefix) {
                        refs.push(r.clone());
                    }
                }
                for r in repo.custom_refs.keys() {
                    if prefix.is_empty() || r.starts_with(prefix) {
                        refs.push(r.clone());
                    }
                }
                refs.sort();
                let mut out = String::new();
                for r in refs {
                    out.push_str(&r);
                    out.push('\n');
                }
                ok_out(&out)
            }
            "update-ref" => {
                if sub_args.get(1).map(String::as_str) == Some("-d") {
                    if let Some(ref_name) = sub_args.get(2) {
                        if let Some(b) = ref_name.strip_prefix("refs/heads/") {
                            repo.branches.remove(b);
                            repo.branch_heads.remove(b);
                        } else if ref_name.starts_with("refs/remotes/") {
                            repo.remote_refs.remove(ref_name);
                        } else {
                            repo.custom_refs.remove(ref_name);
                        }
                    }
                    ok_out("")
                } else if let (Some(ref_name), Some(new_val)) = (sub_args.get(1), sub_args.get(2)) {
                    let sha = repo
                        .resolve_ref(wt_key.as_deref(), new_val)
                        .unwrap_or_else(|| new_val.clone());
                    repo.commits.entry(sha.clone()).or_default();
                    if let Some(b) = ref_name.strip_prefix("refs/heads/") {
                        repo.branches.insert(b.to_owned());
                        repo.branch_heads.insert(b.to_owned(), sha.clone());
                        if repo.current_branch == b {
                            repo.head_sha = sha;
                        }
                    } else if ref_name.starts_with("refs/remotes/") {
                        repo.remote_refs.insert(ref_name.clone(), sha);
                    } else {
                        repo.custom_refs.insert(ref_name.clone(), sha);
                    }
                    ok_out("")
                } else {
                    err_out(1, "usage: git update-ref <ref> <newvalue>\n")
                }
            }
            "merge-base" => {
                let is_ancestor = sub_args.iter().any(|a| a == "--is-ancestor");
                let pos: Vec<&str> = sub_args[1..]
                    .iter()
                    .filter(|a| !a.starts_with('-'))
                    .map(String::as_str)
                    .collect();
                if pos.len() < 2 {
                    return err_out(128, "fatal: merge-base requires two commits\n");
                }
                let Some(a) = repo.resolve_ref(wt_key.as_deref(), pos[0]) else {
                    return err_out(128, &format!("fatal: Not a valid commit {}\n", pos[0]));
                };
                let Some(b) = repo.resolve_ref(wt_key.as_deref(), pos[1]) else {
                    return err_out(128, &format!("fatal: Not a valid commit {}\n", pos[1]));
                };
                if is_ancestor {
                    if repo.is_ancestor(&a, &b) {
                        ok_out("")
                    } else {
                        err_out(1, "")
                    }
                } else if let Some(base) = repo.merge_base(&a, &b) {
                    ok_out(&format!("{base}\n"))
                } else {
                    err_out(1, "")
                }
            }
            "rev-list" => {
                let reverse = sub_args.iter().any(|a| a == "--reverse");
                let no_merges = sub_args.iter().any(|a| a == "--no-merges");
                let count_only = sub_args.iter().any(|a| a == "--count");
                let left_right = sub_args.iter().any(|a| a == "--left-right");
                let range = sub_args
                    .iter()
                    .skip(1)
                    .find(|a| !a.starts_with('-'))
                    .map(String::as_str)
                    .unwrap_or("HEAD");
                if left_right
                    && count_only
                    && let Some((left_ref, right_ref)) = range.split_once("...")
                {
                    let Some(left_sha) = repo.resolve_ref(wt_key.as_deref(), left_ref) else {
                        return err_out(128, "fatal: bad revision\n");
                    };
                    let Some(right_sha) = repo.resolve_ref(wt_key.as_deref(), right_ref) else {
                        return err_out(128, "fatal: bad revision\n");
                    };
                    let mb = repo
                        .merge_base(&left_sha, &right_sha)
                        .unwrap_or_else(|| left_sha.clone());
                    let left_cnt = repo.rev_list_range(&mb, &left_sha, no_merges).len();
                    let right_cnt = repo.rev_list_range(&mb, &right_sha, no_merges).len();
                    return ok_out(&format!("{left_cnt}\t{right_cnt}\n"));
                }
                let mut commits = if let Some((base_ref, head_ref)) = range.split_once("..") {
                    let Some(base) = repo.resolve_ref(wt_key.as_deref(), base_ref) else {
                        return err_out(128, "fatal: bad revision\n");
                    };
                    let Some(head) = repo.resolve_ref(wt_key.as_deref(), head_ref) else {
                        return err_out(128, "fatal: bad revision\n");
                    };
                    repo.rev_list_range(&base, &head, no_merges)
                } else if let Some(head) = repo.resolve_ref(wt_key.as_deref(), range) {
                    vec![head]
                } else {
                    return err_out(128, "fatal: bad revision\n");
                };
                if reverse {
                    commits.reverse();
                }
                if count_only {
                    ok_out(&format!("{}\n", commits.len()))
                } else {
                    let mut out = String::new();
                    for c in commits {
                        out.push_str(&c);
                        out.push('\n');
                    }
                    ok_out(&out)
                }
            }
            "diff" => {
                let nul = sub_args.iter().any(|a| a == "-z");
                let is_cached = sub_args.iter().any(|a| a == "--cached");
                if sub_args.iter().any(|a| a == "--no-index") {
                    let target_rel = sub_args.last().map(String::as_str).unwrap_or("");
                    let content = cur_untracked_files
                        .get(target_rel)
                        .cloned()
                        .or_else(|| std::fs::read_to_string(checkout_root.join(target_rel)).ok())
                        .unwrap_or_default();
                    let patch = format_unified_diff(target_rel, None, Some(&content), &cur_head);
                    return ExecOutput {
                        status_code: if patch.is_empty() { 0 } else { 1 },
                        stdout: patch.into_bytes(),
                        stderr: Vec::new(),
                    };
                }
                if sub_args.iter().any(|a| a == "--diff-filter=U") {
                    let mut out = String::new();
                    for f in &cur_conflicts {
                        out.push_str(f);
                        out.push(if nul { '\0' } else { '\n' });
                    }
                    return ok_out(&out);
                }
                if is_cached && sub_args.iter().any(|a| a == "--quiet") {
                    return if cur_staged || !cur_staged_files.is_empty() {
                        err_out(1, "")
                    } else {
                        ok_out("")
                    };
                }
                let pos_refs: Vec<&str> = sub_args[1..]
                    .iter()
                    .take_while(|a| a.as_str() != "--")
                    .filter(|a| !a.starts_with('-'))
                    .map(String::as_str)
                    .collect();

                // Cached diff (`git diff --cached [base] --`)
                if is_cached {
                    let mut out = String::new();
                    let head_map = repo.commit_contents.get(&cur_head);
                    if !cur_staged_files.is_empty() {
                        for (path, content) in &cur_staged_files {
                            let old = head_map.and_then(|m| m.get(path)).map(String::as_str);
                            out.push_str(&format_unified_diff(
                                path,
                                old,
                                Some(content.as_str()),
                                &cur_head,
                            ));
                        }
                    } else if cur_staged {
                        out.push_str(&format_unified_diff("dirty.txt", None, None, &cur_head));
                    }
                    return ok_out(&out);
                }

                // Two-ref commit/branch diff (`git diff ... <base> <compare> --`)
                if pos_refs.len() >= 2 {
                    let base_raw = pos_refs[0];
                    let cmp_raw = pos_refs[1];
                    let Some(cmp_sha) = repo.resolve_ref(wt_key.as_deref(), cmp_raw) else {
                        return err_out(128, &format!("fatal: bad revision '{cmp_raw}'\n"));
                    };
                    let base_sha = if base_raw == EMPTY_TREE_SHA {
                        EMPTY_TREE_SHA.to_owned()
                    } else {
                        match repo.resolve_ref(wt_key.as_deref(), base_raw) {
                            Some(s) => s,
                            None => {
                                return err_out(
                                    128,
                                    &format!("fatal: bad revision '{base_raw}'\n"),
                                );
                            }
                        }
                    };
                    let mut files: Vec<String> = Vec::new();
                    if base_sha == EMPTY_TREE_SHA {
                        if let Some(cf) = repo.commit_files.get(&cmp_sha) {
                            files.extend(cf.iter().cloned());
                        }
                        if let Some(cc) = repo.commit_contents.get(&cmp_sha) {
                            files.extend(cc.keys().cloned());
                        }
                        if files.is_empty() {
                            let tail = &cmp_sha[cmp_sha.len().saturating_sub(8)..];
                            files.push(format!("src/commit_{tail}.rs"));
                        }
                    } else if base_sha != cmp_sha {
                        let commits = repo.rev_list_range(&base_sha, &cmp_sha, false);
                        for sha in &commits {
                            if let Some(cf) = repo.commit_files.get(sha) {
                                files.extend(cf.iter().cloned());
                            }
                        }
                        if files.is_empty() {
                            let tail = &cmp_sha[cmp_sha.len().saturating_sub(8)..];
                            files.push(format!("src/commit_{tail}.rs"));
                        }
                    }
                    files.sort();
                    files.dedup();
                    if sub_args.iter().any(|a| a == "--name-only") {
                        let mut out = String::new();
                        for f in &files {
                            out.push_str(f);
                            out.push(if nul { '\0' } else { '\n' });
                        }
                        return ok_out(&out);
                    }
                    let base_map = repo.commit_contents.get(&base_sha);
                    let cmp_map = repo.commit_contents.get(&cmp_sha);
                    let mut out = String::new();
                    for f in &files {
                        let old = base_map.and_then(|m| m.get(f)).map(String::as_str);
                        let new_c = cmp_map.and_then(|m| m.get(f)).map(String::as_str);
                        out.push_str(&format_unified_diff(f, old, new_c, &cmp_sha));
                    }
                    return ok_out(&out);
                }

                // Zero-ref unstaged diff (`git diff --no-ext-diff ... --`)
                if pos_refs.is_empty() {
                    let mut out = String::new();
                    let head_map = repo.commit_contents.get(&cur_head);
                    if !cur_unstaged_files.is_empty() {
                        for (path, content) in &cur_unstaged_files {
                            let old = cur_staged_files
                                .get(path)
                                .or_else(|| head_map.and_then(|m| m.get(path)))
                                .map(String::as_str);
                            out.push_str(&format_unified_diff(
                                path,
                                old,
                                Some(content.as_str()),
                                &cur_head,
                            ));
                        }
                    } else if cur_dirty
                        && cur_untracked_files.is_empty()
                        && cur_staged_files.is_empty()
                    {
                        out.push_str(&format_unified_diff("dirty.txt", None, None, &cur_head));
                    }
                    return ok_out(&out);
                }

                // Single-ref diff (`git diff <base> --` / `--name-only`)
                let base_ref = pos_refs.first().copied();
                let mut range_files: Vec<String> = Vec::new();
                if let Some(base) = base_ref
                    && let Some(base_sha) = repo.resolve_ref(wt_key.as_deref(), base)
                    && base_sha != cur_head
                {
                    let commits = repo.rev_list_range(&base_sha, &cur_head, false);
                    for sha in &commits {
                        if let Some(files) = repo.commit_files.get(sha) {
                            range_files.extend(files.iter().cloned());
                        }
                    }
                    if range_files.is_empty() {
                        let tail = &cur_head[cur_head.len().saturating_sub(8)..];
                        range_files.push(format!("src/commit_{tail}.rs"));
                    }
                    range_files.sort();
                    range_files.dedup();
                }
                if sub_args.iter().any(|a| a == "--name-only") {
                    let mut files = Vec::new();
                    if !cur_conflicts.is_empty() {
                        files.extend(cur_conflicts.iter().cloned());
                    } else {
                        files.extend(range_files);
                        files.extend(cur_staged_files.keys().cloned());
                        files.extend(cur_unstaged_files.keys().cloned());
                        if (cur_dirty || cur_staged)
                            && cur_staged_files.is_empty()
                            && cur_unstaged_files.is_empty()
                            && cur_untracked_files.is_empty()
                        {
                            files.push("dirty.txt".into());
                        }
                    }
                    files.sort();
                    files.dedup();
                    let mut out = String::new();
                    for f in &files {
                        out.push_str(f);
                        out.push(if nul { '\0' } else { '\n' });
                    }
                    ok_out(&out)
                } else {
                    let mut files = range_files;
                    files.extend(cur_staged_files.keys().cloned());
                    files.extend(cur_unstaged_files.keys().cloned());
                    if (cur_dirty || cur_staged)
                        && cur_staged_files.is_empty()
                        && cur_unstaged_files.is_empty()
                        && cur_untracked_files.is_empty()
                    {
                        files.push("dirty.txt".into());
                    }
                    files.sort();
                    files.dedup();
                    let head_map = repo.commit_contents.get(&cur_head);
                    let mut out = String::new();
                    for f in &files {
                        let new_c = cur_unstaged_files
                            .get(f)
                            .or_else(|| cur_staged_files.get(f))
                            .or_else(|| head_map.and_then(|m| m.get(f)))
                            .map(String::as_str);
                        out.push_str(&format_unified_diff(f, None, new_c, &cur_head));
                    }
                    ok_out(&out)
                }
            }
            "add" => {
                let explicit_paths: Vec<String> =
                    if let Some(pos) = sub_args.iter().position(|a| a == "--") {
                        sub_args[pos + 1..].to_vec()
                    } else {
                        sub_args[1..]
                            .iter()
                            .filter(|a| !a.starts_with('-'))
                            .cloned()
                            .collect()
                    };
                let stage_all = explicit_paths.is_empty()
                    || explicit_paths.iter().any(|p| p == "." || p == "./");
                let stage_into = |staged_map: &mut BTreeMap<String, String>,
                                  unstaged_map: &mut BTreeMap<String, String>,
                                  untracked_map: &mut BTreeMap<String, String>,
                                  dirty_flag: &mut bool,
                                  staged_flag: &mut bool,
                                  conflicts: &mut Vec<String>| {
                    if stage_all {
                        let had_tracked = !unstaged_map.is_empty() || !untracked_map.is_empty();
                        for (k, v) in std::mem::take(unstaged_map) {
                            staged_map.insert(k, v);
                        }
                        for (k, v) in std::mem::take(untracked_map) {
                            staged_map.insert(k, v);
                        }
                        *staged_flag = had_tracked
                            || !staged_map.is_empty()
                            || *dirty_flag
                            || !conflicts.is_empty()
                            || *staged_flag;
                        if had_tracked {
                            *dirty_flag = false;
                        }
                        conflicts.clear();
                    } else {
                        for p in &explicit_paths {
                            let content = unstaged_map
                                .remove(p)
                                .or_else(|| untracked_map.remove(p))
                                .or_else(|| std::fs::read_to_string(checkout_root.join(p)).ok())
                                .unwrap_or_else(|| format!("// staged {p}\n"));
                            staged_map.insert(p.clone(), content);
                        }
                        *staged_flag = !staged_map.is_empty() || *staged_flag;
                        *dirty_flag = !unstaged_map.is_empty() || !untracked_map.is_empty();
                        conflicts.clear();
                    }
                };
                if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                    stage_into(
                        &mut wt.staged_files,
                        &mut wt.unstaged_files,
                        &mut wt.untracked_files,
                        &mut wt.dirty,
                        &mut wt.staged,
                        &mut wt.conflict_files,
                    );
                } else {
                    stage_into(
                        &mut repo.staged_files,
                        &mut repo.unstaged_files,
                        &mut repo.untracked_files,
                        &mut repo.dirty,
                        &mut repo.staged,
                        &mut repo.conflict_files,
                    );
                }
                ok_out("")
            }
            "commit" => {
                let is_amend = sub_args.iter().any(|a| a == "--amend");
                let commit_msg = sub_args
                    .iter()
                    .position(|a| a == "-m")
                    .and_then(|pos| sub_args.get(pos + 1))
                    .cloned()
                    .unwrap_or_else(|| "sim commit".into());
                let prev_tree = repo
                    .commit_trees
                    .get(&cur_head)
                    .cloned()
                    .unwrap_or_else(|| tree_sha_for_commit(&cur_head));
                let staged_tree = wt_key
                    .as_deref()
                    .and_then(|p| repo.worktrees.get(p))
                    .and_then(|w| w.staged_tree.clone())
                    .or_else(|| repo.staged_tree.clone());
                let parents = if is_amend {
                    repo.commits.get(&cur_head).cloned().unwrap_or_default()
                } else if cur_in_merge && let Some(mh) = cur_merge_head {
                    vec![cur_head.clone(), mh]
                } else {
                    vec![cur_head.clone()]
                };
                let seq_label = repo.next_commit_seq;
                let new_sha = repo.alloc_commit(parents);
                repo.commit_messages.insert(new_sha.clone(), commit_msg);
                let mut new_contents = repo
                    .commit_contents
                    .get(&cur_head)
                    .cloned()
                    .unwrap_or_default();
                let had_file_tracking = !cur_staged_files.is_empty()
                    || !cur_unstaged_files.is_empty()
                    || !cur_untracked_files.is_empty();
                if is_amend {
                    repo.commit_trees.insert(new_sha.clone(), prev_tree);
                    let mut files = repo
                        .commit_files
                        .get(&cur_head)
                        .cloned()
                        .unwrap_or_default();
                    for (k, v) in &cur_staged_files {
                        new_contents.insert(k.clone(), v.clone());
                        if !files.contains(k) {
                            files.push(k.clone());
                        }
                    }
                    repo.commit_files.insert(new_sha.clone(), files);
                    repo.commit_contents.insert(new_sha.clone(), new_contents);
                } else {
                    if let Some(tree) = staged_tree {
                        repo.commit_trees.insert(new_sha.clone(), tree);
                    }
                    if !cur_staged_files.is_empty() {
                        let files: Vec<String> = cur_staged_files.keys().cloned().collect();
                        for (k, v) in &cur_staged_files {
                            new_contents.insert(k.clone(), v.clone());
                        }
                        repo.commit_files.insert(new_sha.clone(), files);
                        repo.commit_contents.insert(new_sha.clone(), new_contents);
                    } else {
                        let fname = format!("src/change_{seq_label}.rs");
                        new_contents.insert(fname.clone(), format!("// sim commit {new_sha}\n"));
                        repo.commit_files.insert(new_sha.clone(), vec![fname]);
                        repo.commit_contents.insert(new_sha.clone(), new_contents);
                    }
                }
                repo.set_checkout_head(wt_key.as_deref(), new_sha);
                if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                    wt.staged_files.clear();
                    wt.dirty = if had_file_tracking {
                        !wt.unstaged_files.is_empty() || !wt.untracked_files.is_empty()
                    } else {
                        false
                    };
                    wt.staged = false;
                    wt.staged_tree = None;
                    wt.in_merge = false;
                    wt.merge_head = None;
                    wt.in_cherry_pick = false;
                    wt.conflict_files.clear();
                } else {
                    repo.staged_files.clear();
                    repo.dirty = if had_file_tracking {
                        !repo.unstaged_files.is_empty() || !repo.untracked_files.is_empty()
                    } else {
                        false
                    };
                    repo.staged = false;
                    repo.staged_tree = None;
                    repo.in_merge = false;
                    repo.merge_head = None;
                    repo.in_cherry_pick = false;
                    repo.conflict_files.clear();
                }
                ok_out("")
            }
            "merge" => {
                if sub_args.iter().any(|a| a == "--abort") {
                    if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                        wt.in_merge = false;
                        wt.merge_head = None;
                        wt.staged = false;
                        wt.conflict_files.clear();
                    } else {
                        repo.in_merge = false;
                        repo.merge_head = None;
                        repo.staged = false;
                        repo.conflict_files.clear();
                    }
                    return ok_out("");
                }
                let target = sub_args
                    .iter()
                    .rev()
                    .find(|a| !a.starts_with('-'))
                    .map(String::as_str)
                    .unwrap_or("HEAD");
                let Some(target_sha) = repo.resolve_ref(wt_key.as_deref(), target) else {
                    return err_out(
                        128,
                        &format!("merge: {target} - not something we can merge\n"),
                    );
                };
                if sub_args.iter().any(|a| a == "--ff-only") {
                    if repo.is_ancestor(&cur_head, &target_sha) {
                        repo.set_checkout_head(wt_key.as_deref(), target_sha);
                        ok_out("")
                    } else {
                        err_out(128, "fatal: Not possible to fast-forward, aborting.\n")
                    }
                } else if let Some(conflict_files) =
                    repo.pop_injected_conflict(&[Some(target_sha.as_str()), Some(target)])
                {
                    if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                        wt.in_merge = true;
                        wt.merge_head = Some(target_sha);
                        wt.conflict_files = conflict_files.clone();
                    } else {
                        repo.in_merge = true;
                        repo.merge_head = Some(target_sha);
                        repo.conflict_files = conflict_files.clone();
                    }
                    err_out(
                        1,
                        &format!(
                            "CONFLICT (content): Merge conflict in {}\n",
                            conflict_files.join(", ")
                        ),
                    )
                } else {
                    let is_squash = sub_args.iter().any(|a| a == "--squash");
                    let no_commit = sub_args.iter().any(|a| a == "--no-commit");
                    if is_squash || no_commit {
                        if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p))
                        {
                            wt.staged = true;
                            wt.in_merge = !is_squash;
                            wt.merge_head = (!is_squash).then_some(target_sha);
                        } else {
                            repo.staged = true;
                            repo.in_merge = !is_squash;
                            repo.merge_head = (!is_squash).then_some(target_sha);
                        }
                    } else {
                        let merged = repo.alloc_commit(vec![cur_head, target_sha]);
                        repo.set_checkout_head(wt_key.as_deref(), merged);
                    }
                    ok_out("")
                }
            }
            "cherry-pick" => {
                if sub_args.iter().any(|a| a == "--abort" || a == "--skip") {
                    if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                        wt.in_cherry_pick = false;
                        wt.staged = false;
                        wt.conflict_files.clear();
                    } else {
                        repo.in_cherry_pick = false;
                        repo.staged = false;
                        repo.conflict_files.clear();
                    }
                    return ok_out("");
                }
                if sub_args.iter().any(|a| a == "--continue") {
                    let new_sha = repo.alloc_commit(vec![cur_head]);
                    repo.set_checkout_head(wt_key.as_deref(), new_sha);
                    if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                        wt.in_cherry_pick = false;
                        wt.staged = false;
                        wt.dirty = false;
                        wt.conflict_files.clear();
                    } else {
                        repo.in_cherry_pick = false;
                        repo.staged = false;
                        repo.dirty = false;
                        repo.conflict_files.clear();
                    }
                    return ok_out("");
                }
                let target_ref = sub_args
                    .iter()
                    .skip(1)
                    .find(|a| !a.starts_with('-'))
                    .map(String::as_str);
                let target_sha = target_ref.and_then(|r| repo.resolve_ref(wt_key.as_deref(), r));
                if let Some(conflict_files) =
                    repo.pop_injected_conflict(&[target_sha.as_deref(), target_ref])
                {
                    if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                        wt.in_cherry_pick = true;
                        wt.conflict_files = conflict_files.clone();
                    } else {
                        repo.in_cherry_pick = true;
                        repo.conflict_files = conflict_files.clone();
                    }
                    err_out(
                        1,
                        &format!(
                            "CONFLICT (content): Merge conflict in {}\n",
                            conflict_files.join(", ")
                        ),
                    )
                } else {
                    let new_sha = repo.alloc_commit(vec![cur_head]);
                    repo.set_checkout_head(wt_key.as_deref(), new_sha);
                    ok_out("")
                }
            }
            "rebase" => {
                if sub_args.iter().any(|a| a == "--abort" || a == "--skip") {
                    if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                        wt.in_rebase = false;
                        wt.rebase_onto = None;
                        wt.staged = false;
                        wt.conflict_files.clear();
                    } else {
                        repo.in_rebase = false;
                        repo.rebase_onto = None;
                        repo.staged = false;
                        repo.conflict_files.clear();
                    }
                    SimGitRepo::clear_rebase_sentinel(&checkout_git_dir);
                    return ok_out("");
                }
                if sub_args.iter().any(|a| a == "--continue") {
                    let new_sha = repo.alloc_commit(vec![cur_head]);
                    repo.set_checkout_head(wt_key.as_deref(), new_sha);
                    if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                        wt.dirty = false;
                        wt.staged = false;
                        wt.in_rebase = false;
                        wt.rebase_onto = None;
                        wt.conflict_files.clear();
                    } else {
                        repo.dirty = false;
                        repo.staged = false;
                        repo.in_rebase = false;
                        repo.rebase_onto = None;
                        repo.conflict_files.clear();
                    }
                    SimGitRepo::clear_rebase_sentinel(&checkout_git_dir);
                    return ok_out("");
                }
                let onto = sub_args
                    .iter()
                    .rev()
                    .find(|a| !a.starts_with('-'))
                    .map(String::as_str)
                    .unwrap_or("HEAD");
                let Some(onto_sha) = repo.resolve_ref(wt_key.as_deref(), onto) else {
                    return err_out(128, &format!("fatal: invalid upstream '{onto}'\n"));
                };
                if let Some(conflict_files) = repo.pop_injected_conflict(&[
                    Some(cur_head.as_str()),
                    Some(onto_sha.as_str()),
                    Some(onto),
                ]) {
                    if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                        wt.in_rebase = true;
                        wt.rebase_onto = Some(onto_sha.clone());
                        wt.conflict_files = conflict_files.clone();
                    } else {
                        repo.in_rebase = true;
                        repo.rebase_onto = Some(onto_sha.clone());
                        repo.conflict_files = conflict_files.clone();
                    }
                    SimGitRepo::write_rebase_sentinel(
                        &checkout_git_dir,
                        &onto_sha,
                        cur_branch.as_deref(),
                    );
                    err_out(
                        1,
                        &format!(
                            "CONFLICT: rebase conflict in {}\n",
                            conflict_files.join(", ")
                        ),
                    )
                } else {
                    let new_sha = repo.alloc_commit(vec![onto_sha]);
                    repo.set_checkout_head(wt_key.as_deref(), new_sha);
                    ok_out("")
                }
            }
            "reset" => {
                if let Some(sep_pos) = sub_args.iter().position(|a| a == "--") {
                    // Pathspec unstage (`git reset -q HEAD -- <paths>`): unstage files without moving HEAD!
                    let paths = &sub_args[sep_pos + 1..];
                    let unstage_all =
                        paths.is_empty() || paths.iter().any(|p| p == "." || p == "./");
                    let head_tracked = repo.commit_contents.get(&cur_head).cloned();
                    let unstage_from =
                        |staged_map: &mut BTreeMap<String, String>,
                         unstaged_map: &mut BTreeMap<String, String>,
                         untracked_map: &mut BTreeMap<String, String>,
                         dirty_flag: &mut bool,
                         staged_flag: &mut bool| {
                            let keys: Vec<String> = if unstage_all {
                                staged_map.keys().cloned().collect()
                            } else {
                                paths.to_vec()
                            };
                            for k in keys {
                                if let Some(v) = staged_map.remove(&k) {
                                    if head_tracked.as_ref().is_some_and(|m| m.contains_key(&k)) {
                                        unstaged_map.insert(k, v);
                                    } else {
                                        untracked_map.insert(k, v);
                                    }
                                }
                            }
                            *staged_flag = !staged_map.is_empty();
                            *dirty_flag = !unstaged_map.is_empty() || !untracked_map.is_empty();
                        };
                    if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                        unstage_from(
                            &mut wt.staged_files,
                            &mut wt.unstaged_files,
                            &mut wt.untracked_files,
                            &mut wt.dirty,
                            &mut wt.staged,
                        );
                    } else {
                        unstage_from(
                            &mut repo.staged_files,
                            &mut repo.unstaged_files,
                            &mut repo.untracked_files,
                            &mut repo.dirty,
                            &mut repo.staged,
                        );
                    }
                    return ok_out("");
                }
                let target = sub_args
                    .iter()
                    .rev()
                    .find(|a| !a.starts_with('-'))
                    .map(String::as_str)
                    .unwrap_or("HEAD");
                let Some(sha) = repo.resolve_ref(wt_key.as_deref(), target) else {
                    return err_out(128, &format!("fatal: ambiguous argument '{target}'\n"));
                };
                repo.set_checkout_head(wt_key.as_deref(), sha);
                if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                    wt.dirty = false;
                    wt.staged = false;
                    wt.staged_files.clear();
                    wt.unstaged_files.clear();
                    wt.untracked_files.clear();
                    wt.in_merge = false;
                    wt.merge_head = None;
                    wt.in_cherry_pick = false;
                    wt.in_rebase = false;
                    wt.rebase_onto = None;
                    wt.conflict_files.clear();
                } else {
                    repo.dirty = false;
                    repo.staged = false;
                    repo.staged_files.clear();
                    repo.unstaged_files.clear();
                    repo.untracked_files.clear();
                    repo.in_merge = false;
                    repo.merge_head = None;
                    repo.in_cherry_pick = false;
                    repo.in_rebase = false;
                    repo.rebase_onto = None;
                    repo.conflict_files.clear();
                }
                SimGitRepo::clear_rebase_sentinel(&checkout_git_dir);
                ok_out("")
            }
            "rm" => {
                let paths: Vec<String> = sub_args
                    .iter()
                    .position(|a| a == "--")
                    .map(|pos| sub_args[pos + 1..].to_vec())
                    .unwrap_or_default();
                let all = paths.is_empty() || paths.iter().any(|p| p == "." || p == "./");
                if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                    if all {
                        for (k, v) in std::mem::take(&mut wt.staged_files) {
                            wt.untracked_files.insert(k, v);
                        }
                    } else {
                        for p in &paths {
                            if let Some(v) = wt.staged_files.remove(p) {
                                wt.untracked_files.insert(p.clone(), v);
                            }
                        }
                    }
                    wt.staged = !wt.staged_files.is_empty();
                    wt.dirty = !wt.unstaged_files.is_empty() || !wt.untracked_files.is_empty();
                } else {
                    if all {
                        for (k, v) in std::mem::take(&mut repo.staged_files) {
                            repo.untracked_files.insert(k, v);
                        }
                    } else {
                        for p in &paths {
                            if let Some(v) = repo.staged_files.remove(p) {
                                repo.untracked_files.insert(p.clone(), v);
                            }
                        }
                    }
                    repo.staged = !repo.staged_files.is_empty();
                    repo.dirty =
                        !repo.unstaged_files.is_empty() || !repo.untracked_files.is_empty();
                }
                ok_out("")
            }
            "clean" => {
                if let Some(sep_pos) = sub_args.iter().position(|a| a == "--") {
                    for p in &sub_args[sep_pos + 1..] {
                        let _ = std::fs::remove_file(checkout_root.join(p));
                        if let Some(wt) = wt_key.as_deref().and_then(|k| repo.worktrees.get_mut(k))
                        {
                            wt.untracked_files.remove(p);
                            wt.dirty =
                                !wt.unstaged_files.is_empty() || !wt.untracked_files.is_empty();
                        } else {
                            repo.untracked_files.remove(p);
                            repo.dirty =
                                !repo.unstaged_files.is_empty() || !repo.untracked_files.is_empty();
                        }
                    }
                } else if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                    wt.untracked_files.clear();
                    wt.dirty = !wt.unstaged_files.is_empty();
                } else {
                    repo.untracked_files.clear();
                    repo.dirty = !repo.unstaged_files.is_empty();
                }
                ok_out("")
            }
            "fetch" => {
                let pos: Vec<&str> = sub_args[1..]
                    .iter()
                    .filter(|a| !a.starts_with('-'))
                    .map(String::as_str)
                    .collect();
                let remote = pos.first().copied().unwrap_or("origin");
                if !repo.remotes.contains_key(remote) {
                    return err_out(
                        128,
                        &format!("fatal: '{remote}' does not appear to be a git repository\n"),
                    );
                }
                if let Some(refspec) = pos.get(1)
                    && let Some((src, dst)) = refspec.trim_start_matches('+').split_once(':')
                {
                    let sha = repo
                        .remote_refs
                        .get(dst)
                        .cloned()
                        .or_else(|| repo.resolve_ref(wt_key.as_deref(), src))
                        .unwrap_or_else(|| repo.head_sha.clone());
                    repo.remote_refs.insert(dst.to_owned(), sha.clone());
                    repo.last_fetch_head = Some(sha);
                } else if let Some(b) = cur_branch.as_deref()
                    && let Some(sha) = repo.remote_refs.get(&format!("refs/remotes/{remote}/{b}"))
                {
                    repo.last_fetch_head = Some(sha.clone());
                }
                ok_out("")
            }
            "ls-remote" => {
                let pos: Vec<&str> = sub_args[1..]
                    .iter()
                    .filter(|a| !a.starts_with('-'))
                    .map(String::as_str)
                    .collect();
                let remote = pos.first().copied().unwrap_or("origin");
                if !repo.remotes.contains_key(remote) {
                    return err_out(
                        128,
                        &format!("fatal: '{remote}' does not appear to be a git repository\n"),
                    );
                }
                let pattern = pos.get(1).copied();
                let mut out = String::new();
                let prefix = format!("refs/remotes/{remote}/");
                for (r_key, sha) in &repo.remote_refs {
                    if let Some(branch) = r_key.strip_prefix(&prefix) {
                        let ref_full = format!("refs/heads/{branch}");
                        if pattern.is_none_or(|p| p == ref_full || p == branch) {
                            out.push_str(&format!("{sha}\t{ref_full}\n"));
                        }
                    }
                }
                ok_out(&out)
            }
            "pull" => {
                let pos: Vec<&str> = sub_args[1..]
                    .iter()
                    .filter(|a| !a.starts_with('-'))
                    .map(String::as_str)
                    .collect();
                let remote = pos.first().copied().unwrap_or("origin");
                if !repo.remotes.contains_key(remote) {
                    return err_out(
                        128,
                        &format!("fatal: '{remote}' does not appear to be a git repository\n"),
                    );
                }
                let branch = pos
                    .get(1)
                    .copied()
                    .or(cur_branch.as_deref())
                    .unwrap_or(&repo.default_branch)
                    .to_owned();
                let remote_key = format!("refs/remotes/{remote}/{branch}");
                let Some(remote_sha) = repo.remote_refs.get(&remote_key).cloned() else {
                    return err_out(128, &format!("fatal: couldn't find remote ref {branch}\n"));
                };
                repo.last_fetch_head = Some(remote_sha.clone());
                if cur_head == remote_sha || repo.is_ancestor(&remote_sha, &cur_head) {
                    ok_out("Already up to date.\n")
                } else if repo.is_ancestor(&cur_head, &remote_sha) {
                    repo.set_checkout_head(wt_key.as_deref(), remote_sha);
                    ok_out("Fast-forward\n")
                } else {
                    err_out(128, "fatal: Not possible to fast-forward, aborting.\n")
                }
            }
            "push" => {
                let set_upstream = sub_args.iter().any(|a| a == "--set-upstream" || a == "-u");
                let pos: Vec<&str> = sub_args[1..]
                    .iter()
                    .filter(|a| !a.starts_with('-'))
                    .map(String::as_str)
                    .collect();
                let remote = pos.first().copied().unwrap_or("origin");
                if !repo.remotes.contains_key(remote) {
                    return err_out(
                        128,
                        &format!("fatal: '{remote}' does not appear to be a git repository\n"),
                    );
                }
                if let Some(refspec) = pos.get(1) {
                    let (src, dst) = refspec.split_once(':').unwrap_or((refspec, refspec));
                    let sha = repo
                        .resolve_ref(wt_key.as_deref(), src)
                        .unwrap_or_else(|| cur_head.clone());
                    let branch = dst.strip_prefix("refs/heads/").unwrap_or(dst);
                    if !branch.is_empty() {
                        repo.remote_refs
                            .insert(format!("refs/remotes/{remote}/{branch}"), sha);
                        if set_upstream && let Some(local_b) = cur_branch.as_deref() {
                            repo.branch_upstreams
                                .insert(local_b.to_owned(), format!("{remote}/{branch}"));
                        }
                        crate::trace::record_coverage_hit("exec.git_pushed");
                    }
                }
                ok_out("")
            }
            "log" => {
                let mut skip = 0usize;
                let mut limit = usize::MAX;
                let mut format_str = "%H%x09%h%x09%P%x09%cr%x09%an%x09%s".to_owned();
                let mut grep_query: Option<String> = None;
                let mut target_ref: Option<String> = None;
                let mut i = 1usize;
                while i < sub_args.len() {
                    let a = sub_args[i].as_str();
                    if a == "--" {
                        break;
                    } else if a == "-n" {
                        if let Some(n) = sub_args.get(i + 1).and_then(|v| v.parse::<usize>().ok()) {
                            limit = n;
                        }
                        i += 2;
                    } else if let Some(s) = a.strip_prefix("--skip=") {
                        skip = s.parse().unwrap_or(0);
                        i += 1;
                    } else if let Some(f) = a.strip_prefix("--format=") {
                        format_str = f.to_owned();
                        i += 1;
                    } else if let Some(g) = a.strip_prefix("--grep=") {
                        grep_query = Some(g.to_owned());
                        i += 1;
                    } else if a.starts_with('-') {
                        i += 1;
                    } else {
                        target_ref = Some(a.to_owned());
                        i += 1;
                    }
                }
                let rev = target_ref.as_deref().unwrap_or("HEAD");
                let commits = if let Some((from_ref, to_ref)) = rev.split_once("..") {
                    let Some(from_sha) = repo.resolve_ref(wt_key.as_deref(), from_ref) else {
                        return err_out(128, "fatal: bad revision\n");
                    };
                    let Some(to_sha) = repo.resolve_ref(wt_key.as_deref(), to_ref) else {
                        return err_out(128, "fatal: bad revision\n");
                    };
                    repo.rev_list_range(&from_sha, &to_sha, false)
                } else if let Some(head_sha) = repo.resolve_ref(wt_key.as_deref(), rev) {
                    repo.walk_commits_from(&head_sha)
                } else {
                    return err_out(128, "fatal: bad revision\n");
                };
                let filtered: Vec<String> = if let Some(q) = grep_query.as_deref() {
                    let q_lower = q.to_ascii_lowercase();
                    commits
                        .into_iter()
                        .filter(|sha| {
                            repo.commit_messages
                                .get(sha)
                                .map(|m| m.to_ascii_lowercase().contains(&q_lower))
                                .unwrap_or(false)
                        })
                        .collect()
                } else {
                    commits
                };
                let mut out = String::new();
                for sha in filtered.into_iter().skip(skip).take(limit) {
                    out.push_str(&repo.format_commit(&sha, &format_str));
                    out.push('\n');
                }
                ok_out(&out)
            }
            "show" => {
                let is_summary = sub_args.iter().any(|a| a == "-s");
                if is_summary {
                    let fmt = sub_args
                        .iter()
                        .find_map(|a| a.strip_prefix("--format="))
                        .unwrap_or("%H");
                    let target = sub_args
                        .iter()
                        .rev()
                        .find(|a| !a.starts_with('-'))
                        .map(String::as_str)
                        .unwrap_or("HEAD");
                    let Some(sha) = repo.resolve_ref(wt_key.as_deref(), target) else {
                        return err_out(128, &format!("fatal: bad object {target}\n"));
                    };
                    return ok_out(&format!("{}\n", repo.format_commit(&sha, fmt)));
                }
                let spec = sub_args
                    .iter()
                    .rev()
                    .find(|a| !a.starts_with('-'))
                    .map(String::as_str)
                    .unwrap_or("");
                if let Some((rev_part, path_part)) = spec.split_once(':') {
                    if rev_part.is_empty() {
                        // Index lookup (`git show :<path>`)
                        if let Some(c) = cur_staged_files.get(path_part) {
                            return ok_out(c);
                        }
                        if let Some(c) = repo
                            .commit_contents
                            .get(&cur_head)
                            .and_then(|m| m.get(path_part))
                        {
                            return ok_out(c);
                        }
                        return err_out(
                            128,
                            &format!("fatal: path '{path_part}' does not exist in index\n"),
                        );
                    }
                    let Some(sha) = repo.resolve_ref(wt_key.as_deref(), rev_part) else {
                        return err_out(128, &format!("fatal: invalid object name '{rev_part}'\n"));
                    };
                    if let Some(c) = repo
                        .commit_contents
                        .get(&sha)
                        .and_then(|m| m.get(path_part))
                    {
                        return ok_out(c);
                    }
                    if repo
                        .commit_files
                        .get(&sha)
                        .is_some_and(|fs| fs.iter().any(|f| f == path_part))
                    {
                        return ok_out(&format!("// sim file {path_part} at {sha}\n"));
                    }
                    return err_out(
                        128,
                        &format!(
                            "fatal: path '{path_part}' exists on disk, but not in '{rev_part}'\n"
                        ),
                    );
                }
                err_out(128, &format!("fatal: bad object {spec}\n"))
            }
            "cat-file" => {
                let target = sub_args.last().map(String::as_str).unwrap_or("");
                if let Some((rev_part, path_part)) = target.split_once(':')
                    && let Some(sha) = repo.resolve_ref(wt_key.as_deref(), rev_part)
                    && repo
                        .commit_contents
                        .get(&sha)
                        .is_some_and(|m| m.contains_key(path_part))
                {
                    ok_out("")
                } else {
                    err_out(1, "")
                }
            }
            "write-tree" => {
                let tree = if cur_dirty || cur_staged {
                    tree_sha_for_commit(&format!("staged:{cur_head}:{}", repo.next_commit_seq))
                } else {
                    repo.commit_trees
                        .get(&cur_head)
                        .cloned()
                        .unwrap_or_else(|| tree_sha_for_commit(&cur_head))
                };
                if let Some(wt) = wt_key.as_deref().and_then(|p| repo.worktrees.get_mut(p)) {
                    wt.staged_tree = Some(tree.clone());
                } else {
                    repo.staged_tree = Some(tree.clone());
                }
                ok_out(&format!("{tree}\n"))
            }
            "commit-tree" => {
                let mut parents = Vec::new();
                let mut i = 1;
                while i < sub_args.len() {
                    if sub_args[i] == "-p"
                        && let Some(p) = sub_args.get(i + 1)
                    {
                        parents.push(
                            repo.resolve_ref(wt_key.as_deref(), p)
                                .unwrap_or_else(|| p.clone()),
                        );
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                let sha = repo.alloc_commit(parents);
                ok_out(&format!("{sha}\n"))
            }
            "ls-files" => {
                let nul = sub_args.iter().any(|a| a == "-z");
                let sep = if nul { '\0' } else { '\n' };
                let mut out = String::new();
                if sub_args.iter().any(|a| a == "--others") {
                    for f in cur_untracked_files.keys() {
                        out.push_str(f);
                        out.push(sep);
                    }
                } else if sub_args.iter().any(|a| a == "--cached") {
                    let mut keys = BTreeSet::new();
                    keys.extend(cur_staged_files.keys().cloned());
                    if let Some(head_map) = repo.commit_contents.get(&cur_head) {
                        keys.extend(head_map.keys().cloned());
                    }
                    for f in keys {
                        out.push_str(&f);
                        out.push(sep);
                    }
                }
                ok_out(&out)
            }
            "merge-tree" => {
                if let Some(conflict_files) = repo.inject_conflicts.front() {
                    let mut out = String::new();
                    out.push('\0');
                    for f in conflict_files {
                        out.push_str(f);
                        out.push('\0');
                    }
                    out.push('\0');
                    ExecOutput {
                        status_code: 1,
                        stdout: out.into_bytes(),
                        stderr: Vec::new(),
                    }
                } else {
                    ok_out(&format!("{}\n", tree_sha_for_commit(&cur_head)))
                }
            }
            _ => err_out(1, &format!("sim git: unsupported subcommand '{sub}'\n")),
        }
    }

    fn run_git_worktree(st: &mut State, root: &Path, args: &[String]) -> ExecOutput {
        let Some(action) = args.first().map(String::as_str) else {
            return err_out(1, "sim git worktree: missing subcommand\n");
        };
        match action {
            "prune" => {
                let mut removed = Vec::new();
                if let Some(repo) = st.git_repos.get_mut(root) {
                    repo.worktrees.retain(|norm_p, wt| {
                        let keep = wt.path.exists() || norm_p.exists();
                        if !keep {
                            removed.push(norm_p.clone());
                        }
                        keep
                    });
                }
                for p in removed {
                    st.worktree_to_repo.remove(&p);
                }
                ok_out("")
            }
            "list" => {
                let nul = args.iter().any(|a| a == "-z");
                let sep = if nul { '\0' } else { '\n' };
                let Some(repo) = st.git_repos.get(root) else {
                    return err_out(128, "fatal: not a git repository\n");
                };
                let mut out = String::new();
                // Main checkout first.
                out.push_str(&format!("worktree {}{sep}", root.display()));
                out.push_str(&format!("HEAD {}{sep}", repo.head_sha));
                if repo.current_branch.is_empty() {
                    out.push_str(&format!("detached{sep}"));
                } else {
                    out.push_str(&format!("branch refs/heads/{}{sep}", repo.current_branch));
                }
                out.push(sep);
                for wt in repo.worktrees.values() {
                    out.push_str(&format!("worktree {}{sep}", wt.path.display()));
                    out.push_str(&format!("HEAD {}{sep}", wt.head_sha));
                    if let Some(b) = &wt.branch {
                        out.push_str(&format!("branch refs/heads/{b}{sep}"));
                    } else {
                        out.push_str(&format!("detached{sep}"));
                    }
                    out.push(sep);
                }
                ok_out(&out)
            }
            "add" => {
                let mut detach = false;
                let mut new_branch: Option<String> = None;
                let mut pos: Vec<String> = Vec::new();
                let mut i = 1usize;
                while i < args.len() {
                    match args[i].as_str() {
                        "--detach" => {
                            detach = true;
                            i += 1;
                        }
                        "-b" | "-B" => {
                            if let Some(b) = args.get(i + 1) {
                                new_branch = Some(b.clone());
                                i += 2;
                            } else {
                                return err_out(128, "fatal: missing branch name for -b\n");
                            }
                        }
                        flag if flag.starts_with('-') => {
                            i += 1;
                        }
                        val => {
                            pos.push(val.to_owned());
                            i += 1;
                        }
                    }
                }
                let Some(raw_path) = pos.first() else {
                    return err_out(128, "fatal: worktree add missing path\n");
                };
                let wt_path = PathBuf::from(raw_path);
                let start_ref = pos.get(1).map(String::as_str);

                let Some(repo) = st.git_repos.get_mut(root) else {
                    return err_out(128, "fatal: not a git repository\n");
                };
                let (branch_opt, head_sha) = if detach {
                    let sha = start_ref
                        .and_then(|r| repo.resolve_ref(None, r))
                        .unwrap_or_else(|| repo.head_sha.clone());
                    (None, sha)
                } else if let Some(b) = new_branch {
                    let sha = start_ref
                        .and_then(|r| repo.resolve_ref(None, r))
                        .unwrap_or_else(|| repo.head_sha.clone());
                    repo.branches.insert(b.clone());
                    repo.branch_heads.insert(b.clone(), sha.clone());
                    (Some(b), sha)
                } else if let Some(existing_branch) = start_ref {
                    let Some(sha) = repo.resolve_ref(None, existing_branch) else {
                        return err_out(
                            128,
                            &format!("fatal: invalid reference: {existing_branch}\n"),
                        );
                    };
                    let b = existing_branch
                        .strip_prefix("refs/heads/")
                        .unwrap_or(existing_branch)
                        .to_owned();
                    repo.branches.insert(b.clone());
                    repo.branch_heads
                        .entry(b.clone())
                        .or_insert_with(|| sha.clone());
                    (Some(b), sha)
                } else {
                    (None, repo.head_sha.clone())
                };

                if let Err(e) = std::fs::create_dir_all(&wt_path) {
                    return err_out(
                        128,
                        &format!(
                            "fatal: could not create worktree directory {}: {e}\n",
                            wt_path.display()
                        ),
                    );
                }
                let norm_wt = normalize_path(&wt_path);
                repo.next_worktree_seq += 1;
                let wt_git_dir = root
                    .join(".git")
                    .join("worktrees")
                    .join(format!("wt-{}", repo.next_worktree_seq));
                repo.worktrees.insert(
                    norm_wt.clone(),
                    SimWorktree {
                        path: wt_path,
                        git_dir: wt_git_dir,
                        branch: branch_opt,
                        head_sha,
                        dirty: false,
                        staged: false,
                        staged_tree: None,
                        staged_files: BTreeMap::new(),
                        unstaged_files: BTreeMap::new(),
                        untracked_files: BTreeMap::new(),
                        in_merge: false,
                        merge_head: None,
                        in_cherry_pick: false,
                        in_rebase: false,
                        rebase_onto: None,
                        conflict_files: Vec::new(),
                    },
                );
                st.worktree_to_repo.insert(norm_wt, root.to_path_buf());
                crate::trace::record_coverage_hit("exec.worktree_added");
                ok_out("")
            }
            "remove" => {
                let target_raw = args
                    .iter()
                    .skip(1)
                    .find(|a| !a.starts_with('-'))
                    .map(String::as_str);
                let Some(raw_path) = target_raw else {
                    return err_out(128, "fatal: worktree remove missing path\n");
                };
                let wt_path = PathBuf::from(raw_path);
                let norm_wt = normalize_path(&wt_path);
                if wt_path.exists() {
                    let _ = std::fs::remove_dir_all(&wt_path);
                }
                if let Some(repo) = st.git_repos.get_mut(root)
                    && let Some(removed) = repo.worktrees.remove(&norm_wt)
                {
                    let _ = std::fs::remove_dir_all(&removed.git_dir);
                }
                st.worktree_to_repo.remove(&norm_wt);
                crate::trace::record_coverage_hit("exec.worktree_removed");
                ok_out("")
            }
            other => err_out(
                1,
                &format!("sim git worktree: unsupported action '{other}'\n"),
            ),
        }
    }
}

/// Normalizes `path` by canonicalizing the longest existing ancestor and re-appending remaining
/// components, eliminating macOS `/var` -> `/private/var` mismatches even for not-yet-created dirs.
fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(path) {
        return c;
    }
    let mut cur = path.to_path_buf();
    let mut tail = Vec::new();
    while !cur.exists() {
        let Some(name) = cur.file_name().map(std::ffi::OsStr::to_os_string) else {
            return path.to_path_buf();
        };
        tail.push(name);
        if !cur.pop() {
            return path.to_path_buf();
        }
    }
    let mut resolved = std::fs::canonicalize(&cur).unwrap_or(cur);
    for comp in tail.into_iter().rev() {
        resolved.push(comp);
    }
    resolved
}

fn format_unified_diff(
    path: &str,
    old: Option<&str>,
    new: Option<&str>,
    fallback_head: &str,
) -> String {
    match (old, new) {
        (None, Some(content)) => {
            let lines: Vec<&str> = content.lines().collect();
            let count = lines.len().max(1);
            let mut out = format!(
                "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{count} @@\n"
            );
            for line in lines {
                out.push('+');
                out.push_str(line);
                out.push('\n');
            }
            out
        }
        (Some(prev), Some(next)) => {
            let prev_lines: Vec<&str> = prev.lines().collect();
            let next_lines: Vec<&str> = next.lines().collect();
            let p_cnt = prev_lines.len().max(1);
            let n_cnt = next_lines.len().max(1);
            let mut out = format!(
                "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1,{p_cnt} +1,{n_cnt} @@\n"
            );
            for line in prev_lines {
                out.push('-');
                out.push_str(line);
                out.push('\n');
            }
            for line in next_lines {
                out.push('+');
                out.push_str(line);
                out.push('\n');
            }
            out
        }
        _ => format!(
            "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1,2 @@\n+// sim diff head={fallback_head}\n"
        ),
    }
}

fn is_hex_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn tree_sha_for_commit(commit_sha: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in commit_sha.as_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("111111111111111111111111{h:016x}")
}

fn ok_out(stdout: &str) -> ExecOutput {
    ExecOutput {
        status_code: 0,
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

fn err_out(code: i32, stderr: &str) -> ExecOutput {
    ExecOutput {
        status_code: code,
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

fn is_valid_git_branch_name(branch: &str) -> bool {
    if branch.is_empty()
        || branch == "@"
        || branch.starts_with('-')
        || branch.starts_with('/')
        || branch.ends_with('/')
        || branch.ends_with('.')
        || branch.ends_with(".lock")
        || branch.contains("..")
        || branch.contains("//")
        || branch.contains("@{")
    {
        return false;
    }
    !branch.chars().any(|c| {
        c.is_ascii_control() || matches!(c, ' ' | '\t' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    })
}

impl CommandRunner for SimExec {
    fn run(&self, spec: &CommandSpec) -> Result<ExecOutput> {
        let mut st = self.state.lock().unwrap();
        st.recorded.push(spec.clone());

        let base = Path::new(&spec.program)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&spec.program);

        match base {
            "echo" => {
                if spec.args.first().map(String::as_str) == Some("--version") {
                    Ok(ok_out("sim-echo 1.0.0\n"))
                } else {
                    Ok(ok_out(&format!("{}\n", spec.args.join(" "))))
                }
            }
            "git" => Ok(Self::run_git(&mut st, spec)),
            "sh" | "bash" | "zsh" => {
                if let Some(res) = Self::run_shell(&mut st, spec) {
                    return res;
                }
                st.blocked.push(spec.clone());
                let host = st.host_name.clone();
                drop(st);
                crate::trace::record_coverage_hit("exec.blocked_command");
                bail!(
                    "SimExec({host}) blocked unconfigured shell command: {} {:?}",
                    spec.program,
                    spec.args
                );
            }
            _ => {
                st.blocked.push(spec.clone());
                let host = st.host_name.clone();
                drop(st);
                crate::trace::record_coverage_hit("exec.blocked_command");
                bail!(
                    "SimExec({host}) blocked unexpected OS command: {} {:?}",
                    spec.program,
                    spec.args
                );
            }
        }
    }

    fn find_executable(&self, name: &str) -> Option<PathBuf> {
        let st = self.state.lock().unwrap();
        if let Some(p) = st.executables.get(name) {
            return Some(p.clone());
        }
        let base = Path::new(name).file_name().and_then(|s| s.to_str())?;
        if !name.contains('/') {
            return st.executables.get(base).cloned();
        }
        None
    }

    fn find_executable_with_env(
        &self,
        name: &str,
        _extra_env: &BTreeMap<String, String>,
    ) -> Option<PathBuf> {
        self.find_executable(name)
    }

    fn shell_env(&self) -> ShellEnvSnapshot {
        self.state.lock().unwrap().shell_env.clone()
    }

    fn hostname(&self) -> String {
        self.state.lock().unwrap().host_name.clone()
    }

    fn is_system(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(exec: &SimExec, dir: &Path, args: &[&str]) -> ExecOutput {
        let mut full = vec!["-C".to_owned(), dir.to_string_lossy().into_owned()];
        full.extend(args.iter().map(|s| (*s).to_owned()));
        exec.run(&CommandSpec {
            program: "git".into(),
            args: full,
            cwd: Some(PathBuf::from("/")),
            env_set: BTreeMap::new(),
            env_remove: Vec::new(),
            stdin: None,
        })
        .unwrap()
    }

    fn git_ok(exec: &SimExec, dir: &Path, args: &[&str]) -> String {
        let out = git(exec, dir, args);
        assert_eq!(
            out.status_code,
            0,
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    #[test]
    fn targeted_conflict_targets_second_commit_and_abort_preserves_first() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let wt1 = tmp.path().join("wt1");
        let wt2 = tmp.path().join("wt2");
        let mq = tmp.path().join("mq");
        std::fs::create_dir_all(&repo).unwrap();

        let exec = SimExec::new("test-host");
        exec.register_git_repo(&repo, SimGitRepo::default());
        let base_head = exec.head_sha(&repo).unwrap();

        git_ok(
            &exec,
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "card/1",
                wt1.to_str().unwrap(),
                &base_head,
            ],
        );
        git_ok(
            &exec,
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "card/2",
                wt2.to_str().unwrap(),
                &base_head,
            ],
        );
        git_ok(
            &exec,
            &repo,
            &[
                "worktree",
                "add",
                "--detach",
                mq.to_str().unwrap(),
                &base_head,
            ],
        );

        let c1_sha = exec.commit_in_worktree(&wt1, "card 1").unwrap();
        let c2_sha = exec.commit_in_worktree(&wt2, "card 2").unwrap();

        // Inject conflict specifically for c2_sha: c1_sha must cherry-pick cleanly!
        exec.inject_merge_conflict_for_commit(&repo, &c2_sha, vec!["conflict.rs".into()]);
        git_ok(&exec, &mq, &["cherry-pick", &c1_sha]);
        let after_c1 = git_ok(&exec, &mq, &["rev-parse", "HEAD"]);
        assert_ne!(after_c1, base_head);

        let cp2 = git(&exec, &mq, &["cherry-pick", &c2_sha]);
        assert_eq!(cp2.status_code, 1);
        assert_eq!(
            git_ok(&exec, &mq, &["diff", "--name-only", "--diff-filter=U"]),
            "conflict.rs"
        );
        assert_eq!(
            git(&exec, &mq, &["rev-parse", "--verify", "CHERRY_PICK_HEAD"]).status_code,
            0
        );

        exec.abort_worktree_conflict(&mq).unwrap();
        assert_ne!(
            git(&exec, &mq, &["rev-parse", "--verify", "CHERRY_PICK_HEAD"]).status_code,
            0
        );
        assert_eq!(git_ok(&exec, &mq, &["rev-parse", "HEAD"]), after_c1);
        assert!(exec.blocked_commands().is_empty());
    }

    #[test]
    fn squash_merge_and_sync_upstream_preserve_dag_parents_and_remote_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let wt1 = tmp.path().join("wt1");
        let wt2 = tmp.path().join("wt2");
        let mq = tmp.path().join("mq");
        std::fs::create_dir_all(&repo).unwrap();

        let exec = SimExec::new("test-host");
        exec.register_git_repo(&repo, SimGitRepo::default());
        let base_head = exec.head_sha(&repo).unwrap();

        git_ok(
            &exec,
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "card/1",
                wt1.to_str().unwrap(),
                &base_head,
            ],
        );
        let _ = exec.commit_in_worktree(&wt1, "card 1 part 1").unwrap();
        let c1_head = exec.commit_in_worktree(&wt1, "card 1 part 2").unwrap();

        // Advance remote origin/main -> fetch -> fast-forward local main.
        let upstream_sha = exec.advance_remote_ref(&repo, "origin", "main").unwrap();
        git_ok(
            &exec,
            &repo,
            &[
                "fetch",
                "--",
                "origin",
                "+refs/heads/main:refs/remotes/origin/main",
            ],
        );
        git_ok(&exec, &repo, &["merge", "--ff-only", &upstream_sha]);
        assert_eq!(exec.head_sha(&repo).as_deref(), Some(upstream_sha.as_str()));

        // Squash strategy produces a 1-parent commit on top of upstream_sha.
        git_ok(
            &exec,
            &repo,
            &[
                "worktree",
                "add",
                "--detach",
                mq.to_str().unwrap(),
                &upstream_sha,
            ],
        );
        git_ok(&exec, &mq, &["merge", "--squash", "--no-commit", &c1_head]);
        assert_eq!(
            git(&exec, &mq, &["diff", "--cached", "--quiet"]).status_code,
            1
        );
        assert_ne!(
            git(&exec, &mq, &["rev-parse", "--verify", "MERGE_HEAD"]).status_code,
            0
        );
        git_ok(&exec, &mq, &["commit", "-m", "Canban #1: squash"]);
        let squash_sha = git_ok(&exec, &mq, &["rev-parse", "HEAD"]);
        assert_eq!(
            exec.commit_parents(&repo, &squash_sha),
            Some(vec![upstream_sha])
        );
        git_ok(&exec, &repo, &["merge", "--ff-only", &squash_sha]);
        git_ok(
            &exec,
            &repo,
            &[
                "push",
                "--porcelain",
                "origin",
                "refs/heads/main:refs/heads/main",
            ],
        );
        assert_eq!(
            exec.remote_ref_sha(&repo, "origin", "main"),
            Some(squash_sha.clone())
        );

        // Merge strategy preserves MERGE_HEAD and produces a 2-parent commit [squash_sha, c2_head].
        git_ok(
            &exec,
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "card/2",
                wt2.to_str().unwrap(),
                &squash_sha,
            ],
        );
        let c2_head = exec.commit_in_worktree(&wt2, "card 2").unwrap();
        git_ok(&exec, &mq, &["checkout", "--detach", &squash_sha]);
        git_ok(&exec, &mq, &["merge", "--no-ff", "--no-commit", &c2_head]);
        assert_eq!(
            git_ok(&exec, &mq, &["rev-parse", "--verify", "MERGE_HEAD"]),
            c2_head
        );
        git_ok(&exec, &mq, &["commit", "-m", "Merge card #2"]);
        let merge_sha = git_ok(&exec, &mq, &["rev-parse", "HEAD"]);
        assert_eq!(
            exec.commit_parents(&repo, &merge_sha),
            Some(vec![squash_sha, c2_head])
        );
        assert!(exec.blocked_commands().is_empty());
    }

    #[test]
    fn rebase_conflict_materializes_and_cleans_per_worktree_in_rebase_sentinel() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let wt1 = tmp.path().join("wt1");
        let wt2 = tmp.path().join("wt2");
        std::fs::create_dir_all(&repo).unwrap();

        let exec = SimExec::new("test-host");
        exec.register_git_repo(&repo, SimGitRepo::default());
        let base_head = exec.head_sha(&repo).unwrap();

        git_ok(
            &exec,
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "card/1",
                wt1.to_str().unwrap(),
                &base_head,
            ],
        );
        git_ok(
            &exec,
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "card/2",
                wt2.to_str().unwrap(),
                &base_head,
            ],
        );
        let _ = exec.commit_in_worktree(&wt1, "card 1").unwrap();
        let main_new = exec.commit_in_worktree(&repo, "main advance").unwrap();

        let wt1_sentinel = PathBuf::from(git_ok(
            &exec,
            &wt1,
            &["rev-parse", "--git-path", "rebase-merge"],
        ));
        let wt2_sentinel = PathBuf::from(git_ok(
            &exec,
            &wt2,
            &["rev-parse", "--git-path", "rebase-merge"],
        ));
        assert_ne!(wt1_sentinel, wt2_sentinel);
        assert!(!wt1_sentinel.exists());
        assert!(!wt2_sentinel.exists());

        exec.inject_merge_conflict(&repo, vec!["shared.txt".into()]);
        let rb = git(&exec, &wt1, &["rebase", &main_new]);
        assert_eq!(rb.status_code, 1);
        assert!(
            wt1_sentinel.exists(),
            "rebase conflict must create rebase-merge sentinel"
        );
        assert!(
            !wt2_sentinel.exists(),
            "sibling worktree must NOT appear in rebase"
        );
        assert_eq!(
            std::fs::read_to_string(wt1_sentinel.join("onto"))
                .unwrap()
                .trim(),
            main_new
        );
        assert_eq!(
            std::fs::read_to_string(wt1_sentinel.join("head-name"))
                .unwrap()
                .trim(),
            "refs/heads/card/1"
        );

        git_ok(&exec, &wt1, &["rebase", "--abort"]);
        assert!(
            !wt1_sentinel.exists(),
            "rebase --abort must remove rebase-merge sentinel"
        );
        assert!(exec.blocked_commands().is_empty());
    }

    #[test]
    fn diff_range_write_tree_amend_and_branch_head_sha() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let wt = tmp.path().join("wt1");
        std::fs::create_dir_all(&repo).unwrap();

        let exec = SimExec::new("test-host");
        exec.register_git_repo(&repo, SimGitRepo::default());
        let base_head = exec.head_sha(&repo).unwrap();
        assert_eq!(exec.branch_head_sha(&repo, "main"), Some(base_head.clone()));

        git_ok(
            &exec,
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "feat/diff",
                wt.to_str().unwrap(),
                &base_head,
            ],
        );

        // Dirty worktree -> add + write-tree + commit must preserve tree SHA across HEAD^{tree}.
        exec.set_worktree_dirty(&wt, true).unwrap();
        git_ok(&exec, &wt, &["add", "-A", "--", "."]);
        let written_tree = git_ok(&exec, &wt, &["write-tree"]);
        git_ok(&exec, &wt, &["commit", "-m", "initial change"]);
        assert_eq!(
            git_ok(&exec, &wt, &["rev-parse", "HEAD^{tree}"]),
            written_tree
        );

        // Amending preserves HEAD^{tree} and parent while advancing HEAD SHA.
        let before_amend = git_ok(&exec, &wt, &["rev-parse", "HEAD"]);
        git_ok(&exec, &wt, &["commit", "--amend", "-m", "amended change"]);
        let after_amend = git_ok(&exec, &wt, &["rev-parse", "HEAD"]);
        assert_ne!(after_amend, before_amend);
        assert_eq!(
            git_ok(&exec, &wt, &["rev-parse", "HEAD^{tree}"]),
            written_tree
        );
        assert_eq!(
            exec.commit_parents(&repo, &after_amend),
            Some(vec![base_head.clone()])
        );
        assert_eq!(exec.branch_head_sha(&repo, "feat/diff"), Some(after_amend));

        // Diff against base returns non-empty files and unified patch.
        let names = git_ok(
            &exec,
            &wt,
            &["diff", "--no-ext-diff", "--name-only", &base_head, "--"],
        );
        assert!(!names.is_empty());
        let patch = git_ok(
            &exec,
            &wt,
            &["diff", "--no-ext-diff", "--binary", &base_head, "--"],
        );
        assert!(patch.contains("diff --git"));
        assert!(exec.blocked_commands().is_empty());
    }
}
