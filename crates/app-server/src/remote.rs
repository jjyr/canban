//! Remote CLI calls reuse Core's authenticated connection, without a second DB,
//! exposed TCP listener or per-machine SSH reverse tunnel.
use super::*;
use canban_herdr::host::Host;
impl AppServer {
    pub(super) fn start_remote_workers(self: &Arc<Self>) {
        let Ok(snapshot) = self.snapshot() else {
            return;
        };
        let machines: std::collections::HashSet<_> = snapshot
            .terminals
            .iter()
            .filter_map(|t| t.agent.as_ref())
            .filter(|a| !a.finished())
            .filter_map(|a| a.location.as_ref().map(|l| l.machine.clone()))
            .filter(|m| m != "local")
            .collect();
        for machine in machines {
            if !self.remote_workers.lock().unwrap().insert(machine.clone()) {
                continue;
            }
            let weak = Arc::downgrade(self);
            std::thread::spawn(move || {
                while let Some(server) = weak.upgrade() {
                    if server.stopped() {
                        break;
                    }
                    let active = server.snapshot().is_ok_and(|s| {
                        s.terminals
                            .iter()
                            .filter_map(|t| t.agent.as_ref())
                            .any(|a| {
                                !a.finished()
                                    && a.location.as_ref().is_some_and(|l| l.machine == machine)
                            })
                    });
                    if !active {
                        break;
                    }
                    let _ = server.poll_remote(&machine);
                    drop(server);
                    std::thread::sleep(Duration::from_millis(750));
                }
                if let Some(server) = weak.upgrade() {
                    server.remote_workers.lock().unwrap().remove(&machine);
                    server.wake_runtime();
                }
            });
        }
    }
    fn poll_remote(&self, machine: &str) -> Result<()> {
        ensure!(
            self.remote_connected(&format!("{machine}/bridge")),
            "Remote reporting connection is unavailable"
        );
        let host = Host::new(self.config(), machine);
        let root = self.execution_home(machine)?.join("rpc");
        let result = host.python(
            r#"import pathlib,json,sys
p=pathlib.Path(sys.argv[1]);p.mkdir(parents=True,exist_ok=True,mode=0o700)
rows=[]
for f in sorted(p.glob('*.req')):
 if len(f.stem)!=32 or not all(c in '0123456789abcdef' for c in f.stem):continue
 if f.with_suffix('.res').exists():continue
 try:
  assert f.stat().st_size<=4*1024*1024,'Request exceeds 4 MiB'
  rows.append({'id':f.stem,'call':json.loads(f.read_text())})
 except Exception as e:rows.append({'id':f.stem,'error':str(e)})
 break
print(json.dumps(rows))"#,
            &[&root.to_string_lossy()],
        )?;
        for row in result.as_array().context("Invalid remote queue")? {
            let key = strp(row, "id")?;
            let call = &row["call"];
            let result = (|| {
                ensure!(
                    row.get("error").is_none(),
                    "Invalid request: {}",
                    row["error"]
                );
                let method = strp(call, "method")?;
                self.remote_call(method, call["params"].clone())
            })();
            let reply = match result {
                Ok(None) => continue,
                Ok(Some(v)) => json!({"result":v}),
                Err(e) => json!({"error":format!("{e:#}")}),
            };
            // Publish complete replies atomically. Lost publication retries reuse operation keys.
            let tmp = root.join(format!("{key}.reply"));
            host.write(&tmp.to_string_lossy(), &serde_json::to_string(&reply)?)?;
            host.python(
                "import os,sys;os.replace(sys.argv[1],sys.argv[2]);print('true')",
                &[
                    &tmp.to_string_lossy(),
                    &root.join(format!("{key}.res")).to_string_lossy(),
                ],
            )?;
        }
        Ok(())
    }
    pub(super) fn remote_call(&self, method: &str, params: Value) -> Result<Option<Value>> {
        ensure!(
            !matches!(
                method,
                "app.shutdown"
                    | "herdr.connect"
                    | "machine.add"
                    | "machine.remove"
                    | "update.install"
            ),
            "Operation is not available through the Agent reporting bridge"
        );
        let result = self.call(method, params);
        if matches!(method, "execution.report" | "agent.reply")
            && (canban_updater::handoff::active(&self.home)
                || result
                    .as_ref()
                    .is_err_and(|e| e.to_string().starts_with("Canban is upgrading;")))
        {
            // Leave the original remote .req unanswered for the replacement
            // service; publishing a handoff error would consume a durable report.
            return Ok(None);
        }
        result.map(Some)
    }
    pub(super) fn stage_remote(&self, host: &Host, home: &Path, card: &Card) -> Result<PathBuf> {
        let cli = home.join("bin/canban");
        host.write(&cli.to_string_lossy(), include_str!("remote_cli.py"))?;
        host.python(
            "import os,sys;os.chmod(sys.argv[1],0o700);print('true')",
            &[&cli.to_string_lossy()],
        )?;
        let skill = canban_context::skill_path(home);
        host.write(&skill.to_string_lossy(), canban_context::SKILL_MD)?;
        let local = canban_context::card_dir(&self.home, &card.project_id, &card.id);
        fn copy(host: &Host, root: &Path, target: &Path, path: &Path) -> Result<()> {
            if !path.exists() {
                return Ok(());
            }
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let p = entry.path();
                if entry.file_type()?.is_dir() {
                    copy(host, root, target, &p)?
                } else if entry.file_type()?.is_file() {
                    host.write_bytes(
                        &target.join(p.strip_prefix(root)?).to_string_lossy(),
                        &std::fs::read(p)?,
                    )?;
                }
            }
            Ok(())
        }
        copy(host, &self.home, home, &local)?;
        Ok(cli)
    }
}
