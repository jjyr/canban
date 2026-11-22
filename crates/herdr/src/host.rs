//! Local process and filesystem operations honoring configured `hc_env`.
use super::*;
use std::{
    path::{Path, PathBuf},
    process::Output,
};

#[derive(Clone, Default)]
pub struct Host {
    pub hc_env: BTreeMap<String, String>,
}

impl Host {
    pub fn local() -> Self {
        Self::default()
    }

    pub fn new(config: Config) -> Self {
        Self {
            hc_env: config.hc_env,
        }
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
        let mut cmd = if envs.iter().any(|(k, _)| *k == "PATH") {
            canban_exec::raw_command(args[0])
        } else {
            canban_exec::command_with_env(args[0], &self.hc_env)
        };
        cmd.args(&args[1..])
            .current_dir(dir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        cmd.envs(envs.iter().copied());
        if let Some(input) = input {
            let mut child = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            child.stdin.take().unwrap().write_all(input.as_bytes())?;
            Ok(child.wait_with_output()?)
        } else {
            Ok(cmd.output()?)
        }
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
