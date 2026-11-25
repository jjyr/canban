//! Local process and filesystem operations honoring configured `hc_env` and [`canban_exec::CommandRunner`].
use super::*;
use std::{
    path::{Path, PathBuf},
    process::Output,
    sync::Arc,
};

#[derive(Clone)]
pub struct Host {
    pub hc_env: BTreeMap<String, String>,
    runner: Arc<dyn canban_exec::CommandRunner>,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            hc_env: BTreeMap::new(),
            runner: canban_exec::system_runner(),
        }
    }
}

impl Host {
    pub fn local() -> Self {
        Self::default()
    }

    pub fn local_with_runner(runner: Arc<dyn canban_exec::CommandRunner>) -> Self {
        Self {
            hc_env: BTreeMap::new(),
            runner,
        }
    }

    pub fn new(config: Config) -> Self {
        Self::with_runner(config, canban_exec::system_runner())
    }

    pub fn with_runner(config: Config, runner: Arc<dyn canban_exec::CommandRunner>) -> Self {
        Self {
            hc_env: config.hc_env,
            runner,
        }
    }

    pub fn runner(&self) -> &Arc<dyn canban_exec::CommandRunner> {
        &self.runner
    }

    pub fn run(&self, dir: &Path, args: &[&str], input: Option<&str>) -> Result<Output> {
        self.run_env(dir, &[], args, input)
    }

    /// Execute a local command with explicit environment overrides.
    pub fn run_env(
        &self,
        dir: &Path,
        envs: &[(&str, &str)],
        args: &[&str],
        input: Option<&str>,
    ) -> Result<Output> {
        ensure!(!args.is_empty(), "Empty program");
        let mut env_set = self.hc_env.clone();
        for &(k, v) in envs {
            env_set.insert(k.to_owned(), v.to_owned());
        }
        let spec = canban_exec::CommandSpec {
            program: args[0].to_owned(),
            args: args[1..].iter().map(|s| (*s).to_owned()).collect(),
            cwd: Some(dir.to_path_buf()),
            env_set,
            env_remove: vec![
                "GIT_DIR".into(),
                "GIT_WORK_TREE".into(),
                "GIT_INDEX_FILE".into(),
            ],
            stdin: input.map(|s| s.as_bytes().to_vec()),
        };
        Ok(self.runner.run(&spec)?.into_std_output())
    }

    pub fn python(&self, script: &str, args: &[&str]) -> Result<Value> {
        let mut argv = vec!["python3", "-c", script];
        argv.extend(args);
        let out = self.run(Path::new("/"), &argv, None)?;
        ensure!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(serde_json::from_slice(&out.stdout)?)
    }

    pub fn exists(&self, p: &Path) -> bool {
        p.exists()
    }

    pub fn is_dir(&self, p: &Path) -> bool {
        p.is_dir()
    }

    pub fn canonical(&self, p: &Path) -> Result<PathBuf> {
        Ok(p.canonicalize()?)
    }

    pub fn mkdir(&self, p: &Path) -> Result<()> {
        std::fs::create_dir_all(p)?;
        Ok(())
    }

    pub fn home(&self) -> Result<String> {
        std::env::var("HOME").context("HOME unavailable")
    }

    pub fn write_bytes(&self, p: &str, bytes: &[u8]) -> Result<()> {
        let path = Path::new(p);
        std::fs::create_dir_all(path.parent().context("Missing parent")?)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn write(&self, p: &str, text: &str) -> Result<()> {
        self.write_bytes(p, text.as_bytes())
    }

    /// Batch-probe executable availability on the local host.
    pub fn probe_binaries(&self, binaries: &[&str]) -> Result<BTreeMap<String, String>> {
        let mut found = BTreeMap::new();
        if !self.runner.is_system() {
            for &bin in binaries {
                if let Some(hit) = self.runner.find_executable_with_env(bin, &self.hc_env) {
                    found.insert(bin.to_owned(), hit.to_string_lossy().into_owned());
                }
            }
            return Ok(found);
        }
        let dirs = crate::local_search_paths(&self.hc_env);
        for &bin in binaries {
            if Path::new(bin).components().count() != 1 {
                let p = Path::new(bin);
                if p.is_file() {
                    found.insert(bin.to_owned(), bin.to_owned());
                }
                continue;
            }
            if let Some(hit) = dirs.iter().map(|d| d.join(bin)).find(|p| p.is_file()) {
                found.insert(bin.to_owned(), hit.to_string_lossy().into_owned());
            }
        }
        Ok(found)
    }
}
