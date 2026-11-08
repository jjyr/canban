//! Machine-aware process/filesystem operations. Remote effects use Core's API.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::os::unix::process::ExitStatusExt;
use std::{
    path::{Path, PathBuf},
    process::{ExitStatus, Output},
};
#[derive(Clone)]
pub struct Host {
    pub client: Client,
    pub machine: String,
}
impl Host {
    pub fn local() -> Self {
        Self {
            client: Client {
                config: Config::default(),
            },
            machine: "local".into(),
        }
    }
    pub fn new(config: Config, machine: &str) -> Self {
        Self {
            client: Client { config },
            machine: machine.into(),
        }
    }
    pub fn remote(&self) -> bool {
        self.machine != "local"
    }
    pub fn run(&self, dir: &Path, args: &[&str], input: Option<&str>) -> Result<Output> {
        ensure!(!args.is_empty(), "Empty program");
        if self.remote() {
            let mut argv = vec![
                "env",
                "-u",
                "GIT_DIR",
                "-u",
                "GIT_WORK_TREE",
                "-u",
                "GIT_INDEX_FILE",
            ];
            argv.extend(args);
            let value=self.client.call("machine.exec",json!({"on":self.machine,"argv":argv,"cwd":dir,"input":input.map(|s|STANDARD.encode(s)),"timeout":60.0}))?;
            ensure!(
                value["timed_out"] != true && value["truncated"] != true,
                "Remote command timed out or output was truncated"
            );
            let code = value["code"]
                .as_i64()
                .context("Remote command ended without an exit code")?;
            return Ok(Output {
                status: ExitStatus::from_raw(code as i32 * 256),
                stdout: STANDARD.decode(value["stdout"].as_str().unwrap_or(""))?,
                stderr: STANDARD.decode(value["stderr"].as_str().unwrap_or(""))?,
            });
        }
        let mut cmd = Command::new(args[0]);
        cmd.args(&args[1..])
            .current_dir(dir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
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
        if !self.remote() {
            return p.exists();
        }
        self.python(
            "import os,json,sys;print(json.dumps(os.path.exists(sys.argv[1])))",
            &[&p.to_string_lossy()],
        )
        .ok()
            == Some(json!(true))
    }
    pub fn is_dir(&self, p: &Path) -> bool {
        if !self.remote() {
            return p.is_dir();
        }
        self.python(
            "import os,json,sys;print(json.dumps(os.path.isdir(sys.argv[1])))",
            &[&p.to_string_lossy()],
        )
        .ok()
            == Some(json!(true))
    }
    pub fn canonical(&self, p: &Path) -> Result<PathBuf> {
        if !self.remote() {
            return Ok(p.canonicalize()?);
        }
        let v=self.python("import os,json,sys;assert os.path.exists(sys.argv[1]), 'Directory no longer exists';print(json.dumps(os.path.realpath(sys.argv[1])))",&[&p.to_string_lossy()])?;
        Ok(PathBuf::from(v.as_str().context("Invalid path")?))
    }
    pub fn mkdir(&self, p: &Path) -> Result<()> {
        if !self.remote() {
            std::fs::create_dir_all(p)?;
            return Ok(());
        }
        self.python(
            "import os,json,sys;os.makedirs(sys.argv[1],exist_ok=True);print('true')",
            &[&p.to_string_lossy()],
        )?;
        Ok(())
    }
    pub fn home(&self) -> Result<String> {
        if !self.remote() {
            return std::env::var("HOME").context("HOME unavailable");
        }
        Ok(self
            .python(
                "import os,json;print(json.dumps(os.path.expanduser('~')))",
                &[],
            )?
            .as_str()
            .context("No home")?
            .into())
    }
    pub fn write_bytes(&self, p: &str, bytes: &[u8]) -> Result<()> {
        if !self.remote() {
            let path = Path::new(p);
            std::fs::create_dir_all(path.parent().context("Missing parent")?)?;
            std::fs::write(path, bytes)?;
            return Ok(());
        }
        if bytes.len() > 8 * 1024 * 1024 {
            self.client
                .call("file.write", json!({"on":self.machine,"path":p,"data":""}))?;
            for chunk in bytes.chunks(1024 * 1024) {
                let out=self.run(Path::new("/"),&["python3","-c","import base64,sys;open(sys.argv[1],'ab').write(base64.b64decode(sys.stdin.buffer.read()))",p],Some(&STANDARD.encode(chunk)))?;
                ensure!(
                    out.status.success(),
                    "Remote file transfer failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            return Ok(());
        }
        self.client.call(
            "file.write",
            json!({"on":self.machine,"path":p,"data":STANDARD.encode(bytes)}),
        )?;
        Ok(())
    }
    pub fn write(&self, p: &str, text: &str) -> Result<()> {
        self.write_bytes(p, text.as_bytes())
    }
}
