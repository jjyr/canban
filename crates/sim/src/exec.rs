//! Deterministic in-process [`canban_exec::CommandRunner`] for DST.
//!
//! Eliminates login-shell probes (`/bin/zsh -ilc`), host `PATH` discovery, host `hostname` calls,
//! and OS `git` forks, while failing closed on any unexpected OS command spawn.
use anyhow::{Result, bail};
use canban_exec::{CommandRunner, CommandSpec, ExecOutput, ShellEnvSnapshot};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Scripted deterministic git repository fixture for [`SimExec`].
#[derive(Clone, Debug)]
pub struct SimGitRepo {
    pub default_branch: String,
    pub current_branch: String,
    pub head_sha: String,
    pub remotes: BTreeMap<String, String>,
    pub branches: BTreeSet<String>,
    pub dirty: bool,
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
        Self {
            default_branch: "main".into(),
            current_branch: "main".into(),
            head_sha: "0123456789abcdef0123456789abcdef01234567".into(),
            remotes,
            branches,
            dirty: false,
        }
    }
}

struct State {
    host_name: String,
    shell_env: ShellEnvSnapshot,
    executables: BTreeMap<String, PathBuf>,
    git_repos: BTreeMap<PathBuf, SimGitRepo>,
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
        ] {
            executables.insert(name.into(), PathBuf::from(path));
        }
        Arc::new(Self {
            state: Mutex::new(State {
                host_name,
                shell_env,
                executables,
                git_repos: BTreeMap::new(),
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
    pub fn register_git_repo(&self, dir: impl AsRef<Path>, repo: SimGitRepo) {
        let path =
            std::fs::canonicalize(dir.as_ref()).unwrap_or_else(|_| dir.as_ref().to_path_buf());
        self.state.lock().unwrap().git_repos.insert(path, repo);
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

    fn find_repo<'a>(
        repos: &'a BTreeMap<PathBuf, SimGitRepo>,
        dir: Option<&Path>,
    ) -> Option<(&'a PathBuf, &'a SimGitRepo)> {
        let raw = dir?;
        let canonical = std::fs::canonicalize(raw).unwrap_or_else(|_| raw.to_path_buf());
        repos
            .iter()
            .filter(|(root, _)| canonical.starts_with(root) || raw.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
    }

    fn run_git(st: &State, spec: &CommandSpec) -> ExecOutput {
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
            return ExecOutput {
                status_code: 1,
                stdout: Vec::new(),
                stderr: b"sim git: missing subcommand\n".to_vec(),
            };
        };

        if sub == "--version" {
            return ExecOutput {
                status_code: 0,
                stdout: b"git version 2.45.0 (canban-sim)\n".to_vec(),
                stderr: Vec::new(),
            };
        }

        if sub == "check-ref-format" {
            let branch = sub_args.last().map(String::as_str).unwrap_or("");
            if is_valid_git_branch_name(branch) {
                return ExecOutput {
                    status_code: 0,
                    stdout: format!("{branch}\n").into_bytes(),
                    stderr: Vec::new(),
                };
            }
            return ExecOutput {
                status_code: 1,
                stdout: Vec::new(),
                stderr: format!("fatal: '{branch}' is not a valid branch name\n").into_bytes(),
            };
        }

        let Some((root, repo)) = Self::find_repo(&st.git_repos, dir.as_deref()) else {
            return ExecOutput {
                status_code: 128,
                stdout: Vec::new(),
                stderr: b"fatal: not a git repository (or any of the parent directories): .git\n"
                    .to_vec(),
            };
        };

        match sub {
            "rev-parse" => {
                let flags = &sub_args[1..];
                if flags.iter().any(|f| f == "--is-inside-work-tree") {
                    ok_out("true\n")
                } else if flags.iter().any(|f| f == "--show-toplevel") {
                    ok_out(&format!("{}\n", root.display()))
                } else if flags
                    .iter()
                    .any(|f| f == "--git-common-dir" || f == "--git-dir")
                {
                    ok_out(&format!("{}\n", root.join(".git").display()))
                } else if flags.iter().any(|f| f == "--abbrev-ref") {
                    ok_out(&format!("{}\n", repo.current_branch))
                } else if flags.iter().any(|f| f == "HEAD") {
                    ok_out(&format!("{}\n", repo.head_sha))
                } else if let Some(ref_arg) = flags.iter().find(|f| f.starts_with("refs/heads/")) {
                    let b = ref_arg.trim_start_matches("refs/heads/");
                    if repo.branches.contains(b) || b == repo.current_branch {
                        ok_out(&format!("{}\n", repo.head_sha))
                    } else {
                        err_out(
                            128,
                            &format!("fatal: Needed a single revision ({ref_arg})\n"),
                        )
                    }
                } else {
                    ok_out(&format!("{}\n", repo.head_sha))
                }
            }
            "symbolic-ref" => {
                let target = sub_args.last().map(String::as_str).unwrap_or("");
                if target == "HEAD" {
                    ok_out(&format!("{}\n", repo.current_branch))
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
                    let remote = sub_args.get(2).map(String::as_str).unwrap_or("origin");
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
                if let Some(key) = sub_args.last().map(String::as_str)
                    && let Some(rest) = key.strip_prefix("remote.")
                    && let Some(remote) = rest.strip_suffix(".url")
                    && let Some(url) = repo.remotes.get(remote)
                {
                    return ok_out(&format!("{url}\n"));
                }
                err_out(1, "")
            }
            "branch" => {
                if sub_args.iter().any(|a| a == "--show-current") {
                    ok_out(&format!("{}\n", repo.current_branch))
                } else {
                    let mut out = String::new();
                    for b in &repo.branches {
                        out.push_str(b);
                        out.push('\n');
                    }
                    ok_out(&out)
                }
            }
            "status" => {
                if repo.dirty {
                    ok_out(" M dirty.txt\n")
                } else {
                    ok_out("")
                }
            }
            "show-ref" => {
                if let Some(ref_name) = sub_args.last().map(String::as_str)
                    && let Some(b) = ref_name.strip_prefix("refs/heads/")
                    && (repo.branches.contains(b) || b == repo.current_branch)
                {
                    ok_out(&format!("{} {ref_name}\n", repo.head_sha))
                } else {
                    err_out(1, "")
                }
            }
            "merge-base" => err_out(1, ""),
            _ => err_out(1, &format!("sim git: unsupported subcommand '{sub}'\n")),
        }
    }
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
            "git" => Ok(Self::run_git(&st, spec)),
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
