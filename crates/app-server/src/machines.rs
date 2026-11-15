//! Machine catalog comes from Core. Product records reference immutable Core IDs.
use super::*;
use canban_herdr::host::Host;
impl AppServer {
    pub(super) fn machines(&self) -> Result<Value> {
        let _permit = self.check("local");
        let client = canban_herdr::Client {
            config: self.config(),
        };
        let known = self.inventory.lock().unwrap().core.clone();
        let status = if known.is_empty() {
            client.status()?
        } else {
            json!({"core":{"id":known}})
        };
        let mut value = client.call("machine.list", json!({}))?;
        value["local"] = json!({"id":"local","name":"AppServer host","state":"connected","core_id":status["core"]["id"]});
        {
            let mut inv = self.inventory.lock().unwrap();
            inv.machines = value.clone();
            inv.remote_error = None;
            let records = std::mem::take(&mut inv.records);
            for (key, mut record) in records {
                let key = inv.address(&key);
                record["address"] = json!(key);
                inv.records.insert(key, record);
            }
        }
        Ok(value)
    }
    pub(super) fn resolve_machine(&self, machine: &str, connected: bool) -> Result<String> {
        if machine == "local" {
            return Ok(machine.into());
        }
        let list = self.machines()?;
        let row = list["machines"]
            .as_array()
            .context("No machine catalog")?
            .iter()
            .find(|v| v["id"] == machine || v["name"] == machine)
            .context("Machine is not registered with AppServer's Core")?;
        if connected {
            ensure!(
                row["state"] == "connected",
                "Machine is not connected: {}",
                row["state"]
            );
        }
        Ok(strp(row, "id")?.into())
    }
    pub(super) fn ops(&self, machine: &str) -> canban_workspaces::Ops {
        canban_workspaces::Ops {
            host: Host::new(self.config(), machine),
        }
    }
    pub(super) fn execution_home(&self, machine: &str) -> Result<PathBuf> {
        if machine == "local" {
            return Ok(self.home.clone());
        }
        // Per-authority namespace prevents two independent Canban servers sharing workspaces.
        Ok(PathBuf::from(Host::new(self.config(), machine).home()?)
            .join(".canban/executors")
            .join(&self.identity))
    }

    pub(super) fn work_location(&self, p: &Value) -> Result<WorkLocation> {
        let raw = p.get("location").filter(|v| v.is_object()).unwrap_or(p);
        let machine = self.resolve_machine(raw["machine"].as_str().unwrap_or("local"), false)?;
        let raw_dir = strp(raw, "directory")?.trim();
        let host = Host::new(self.config(), &machine);
        // An offline configuration is valid. Actual execution always preflights.
        let available = self.resolve_machine(&machine, true).is_ok();
        let expanded = if raw_dir == "~" {
            ensure!(available, "Machine must be connected to expand '~'");
            host.home()?
        } else if let Some(rest) = raw_dir.strip_prefix("~/") {
            ensure!(available, "Machine must be connected to expand '~/'");
            let home = host.home()?;
            if home == "/" {
                format!("/{rest}")
            } else {
                format!("{home}/{rest}")
            }
        } else {
            raw_dir.to_owned()
        };
        ensure!(
            !expanded.is_empty() && Path::new(&expanded).is_absolute(),
            "An absolute directory on the selected machine is required"
        );
        let directory = if available {
            host.mkdir(Path::new(&expanded))?;
            host.canonical(Path::new(&expanded))?
                .to_string_lossy()
                .into_owned()
        } else {
            expanded
        };
        Ok(WorkLocation { machine, directory })
    }
    pub(super) fn update_location(&self, p: Value) -> Result<Value> {
        let location = self.work_location(&p)?;
        let existing = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let pid = super::resolve_project_id(db, strp(&p, "project_id")?)?;
            db.get::<Project>(&pid)?
        };
        let detected_github = self.detect_project_github(
            &location.machine,
            &location.directory,
            &existing.workflow.workspace.remote,
        );
        let auto_source = if existing.github.is_none() && !existing.github_disconnected {
            detected_github
                .as_ref()
                .and_then(|d| self.try_auto_link_github(&d.repository))
        } else {
            None
        };
        let account_generation = self.github.generation();
        let linked = auto_source.is_some();
        let result = self.mutate("project.location", &p, |db| {
            let pid = super::resolve_project_id(db, strp(&p, "project_id")?)?;
            let mut project: Project = db.get(&pid)?;
            if let Some(expected) = p.get("expected_location") {
                ensure!(
                    serde_json::to_value(project.work_location())? == *expected,
                    "Project location changed; reload before saving"
                );
            }
            let previous = project.work_location();
            project.directory = location.directory.clone();
            project.location = Some(location.clone());
            project.detected_github = detected_github;
            if let Some(source) = auto_source
                && project.github.is_none()
                && !project.github_disconnected
                && self.github.generation() == account_generation
            {
                project.github = Some(source);
            }
            if let Some(branch) = p["target_branch"].as_str() {
                project.target_branch = Some(branch.into())
            }
            db.save(&project)?;
            for c in db
                .list::<Card>()?
                .iter()
                .filter(|c| c.project_id == project.id)
            {
                log(
                    db,
                    c,
                    "project.location_changed",
                    "user",
                    "Project work location changed for future executions",
                    None,
                    json!({"previous":previous,"location":location}),
                )?;
            }
            Ok(serde_json::to_value(project)?)
        })?;
        if linked {
            self.github_sync_due.store(true, Ordering::SeqCst);
        }
        Ok(result)
    }
    pub(super) fn browse_directory(&self, p: Value) -> Result<Value> {
        let machine = self.resolve_machine(p["machine"].as_str().unwrap_or("local"), true)?;
        let host = Host::new(self.config(), &machine);
        let raw = p["path"].as_str().map(str::trim).filter(|v| !v.is_empty());
        let path = match raw {
            Some("~") => host.home()?,
            Some(rest) if rest.starts_with("~/") => {
                let home = host.home()?;
                if home == "/" {
                    format!("/{}", &rest[2..])
                } else {
                    format!("{home}/{}", &rest[2..])
                }
            }
            Some(other) => other.to_owned(),
            None => host.home()?,
        };
        ensure!(
            Path::new(&path).is_absolute(),
            "An absolute directory is required"
        );
        if !host.remote() {
            let home = host.home().unwrap_or_default();
            let target = Path::new(&path);
            if !target.is_dir() {
                return Ok(json!({
                    "path": path,
                    "parent": target.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into()),
                    "home": home,
                    "exists": false,
                    "directories": [],
                }));
            }
            let real = target
                .canonicalize()
                .unwrap_or_else(|_| target.to_path_buf());
            let mut directories = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&real) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    if entry_path.is_dir()
                        && let Some(name) = entry.file_name().to_str()
                    {
                        directories
                            .push((name.to_owned(), entry_path.to_string_lossy().into_owned()));
                    }
                }
            }
            directories.sort_by(|a, b| a.0.cmp(&b.0));
            let parent = real
                .parent()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into());
            return Ok(json!({
                "path": real.to_string_lossy(),
                "parent": parent,
                "home": home,
                "exists": true,
                "directories": directories
                    .into_iter()
                    .map(|(name, p)| json!({"name": name, "path": p}))
                    .collect::<Vec<_>>(),
            }));
        }
        host.python(
            r#"import os,sys,json
raw=sys.argv[1]
home=os.path.expanduser('~')
if not os.path.isdir(raw):
    print(json.dumps({'path':raw,'parent':os.path.dirname(raw) or '/','home':home,'exists':False,'directories':[]}))
else:
    p=os.path.realpath(raw)
    try:
        names=sorted(os.listdir(p))
    except OSError:
        names=[]
    dirs=[]
    for n in names:
        full=os.path.join(p,n)
        try:
            if os.path.isdir(full):
                dirs.append({'name':n,'path':full})
        except OSError:
            pass
    print(json.dumps({'path':p,'parent':os.path.dirname(p) or '/','home':home,'exists':True,'directories':dirs}))"#,
            &[&path],
        )
    }
    pub(super) fn add_machine(&self, p: Value) -> Result<Value> {
        // Interactive CLI handles host keys, login and installation on AppServer host.
        let target = strp(&p, "target")?.trim();
        ensure!(
            !target.is_empty() && !target.starts_with('-'),
            "SSH target required"
        );
        let mut argv = vec![
            self.config().hc,
            "machine".into(),
            "add".into(),
            target.into(),
        ];
        if let Some(name) = p["name"].as_str().filter(|s| !s.trim().is_empty()) {
            argv.extend(["--name".into(), name.into()]);
        }
        if let Some(session) = p["session"].as_str().filter(|s| !s.trim().is_empty()) {
            argv.extend(["--session".into(), session.into()]);
        }
        let mut snap = self.snapshot()?;
        snap.terminal_contexts = self.terminal_contexts.lock().unwrap().clone();
        let title = snap.next_unique_terminal_title("Add machine");
        self.create_terminal(
            json!({"machine":"local","name":title,"setup_argv":argv,"request_key":p["request_key"]}),
        )
    }
    pub(super) fn remove_machine(&self, p: Value) -> Result<Value> {
        let machine = self.resolve_machine(strp(&p, "machine")?, false)?;
        ensure!(machine != "local", "Cannot remove the AppServer host");
        let s = self.snapshot()?;
        let affected = s
            .projects
            .iter()
            .filter(|v| v.work_location().machine == machine)
            .count()
            + s.terminals
                .iter()
                .filter(|v| {
                    v.agent
                        .as_ref()
                        .and_then(|a| a.location.as_ref())
                        .is_some_and(|l| l.machine == machine)
                        || v.runtime.as_ref().is_some_and(|r| r.machine == machine)
                })
                .count();
        ensure!(
            p["confirmed"] == true || affected == 0,
            "Connection has {affected} saved references; confirm removal. Runtime programs and history remain."
        );
        let catalog = self.machines()?;
        let name = catalog["machines"]
            .as_array()
            .and_then(|ms| ms.iter().find(|m| m["id"] == machine))
            .and_then(|m| m["name"].as_str())
            .context("Machine connection changed; refresh")?;
        let result = canban_herdr::Client {
            config: self.config(),
        }
        .call("machine.rm", json!({"machine":name}))?;
        self.emit("machines", None);
        Ok(result)
    }
    pub(super) fn bound_project(&self, project: &Project, card: &Card) -> Project {
        let mut p = project.clone();
        if let Some(w) = &card.workspace {
            p.directory = w.location.directory.clone();
            p.location = Some(w.location.clone());
            p.target_branch = w.target_branch.clone();
        }
        p
    }
}
