//! Project directory resolution, updates, and card workspace bindings.
use super::*;
use canban_herdr::host::Host;

impl AppServer {
    pub(super) fn ops(&self) -> canban_workspaces::Ops {
        canban_workspaces::Ops::with_host(Host::with_runner(
            self.config(),
            Arc::clone(&self.exec_runner),
        ))
    }

    pub(super) fn resolve_project_directory(&self, p: &Value) -> Result<String> {
        let raw_dir = strp(p, "directory")?.trim();
        let host = Host::with_runner(self.config(), Arc::clone(&self.exec_runner));
        let expanded = if raw_dir == "~" {
            host.home()?
        } else if let Some(rest) = raw_dir.strip_prefix("~/") {
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
            "An absolute directory is required"
        );
        host.mkdir(Path::new(&expanded))?;
        Ok(host
            .canonical(Path::new(&expanded))?
            .to_string_lossy()
            .into_owned())
    }

    pub(super) fn update_project(&self, p: Value) -> Result<Value> {
        let next_directory = if p.get("directory").and_then(Value::as_str).is_some() {
            Some(self.resolve_project_directory(&p)?)
        } else {
            None
        };
        let next_name = p
            .get("name")
            .and_then(Value::as_str)
            .map(|s| {
                let trimmed = s.trim();
                ensure!(!trimmed.is_empty(), "Project name is required");
                Ok::<String, anyhow::Error>(trimmed.to_owned())
            })
            .transpose()?;
        let next_target_branch = p
            .get("target_branch")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_owned());
        ensure!(
            next_directory.is_some() || next_name.is_some() || next_target_branch.is_some(),
            "At least one of directory, name, or target_branch is required"
        );
        if let Some(branch) = next_target_branch.as_deref().filter(|b| !b.is_empty()) {
            self.ops().validate_branch_name(branch)?;
        }
        let existing = {
            let store = self.store.lock().unwrap();
            let db = store.db();
            let pid = super::resolve_project_id(db, strp(&p, "project_id")?)?;
            db.get::<Project>(&pid)?
        };
        let detected_github = next_directory
            .as_ref()
            .map(|dir| self.detect_project_github(dir, &existing.workflow.workspace.remote));
        let auto_source = detected_github.as_ref().and_then(|detected| {
            if existing.github.is_none() && !existing.github_disconnected {
                detected
                    .as_ref()
                    .and_then(|d| self.try_auto_link_github(&d.repository))
            } else {
                None
            }
        });
        let account_generation = self.github.generation();
        let linked = auto_source.is_some();
        let result = self.mutate("project.update", &p, |db| {
            let pid = super::resolve_project_id(db, strp(&p, "project_id")?)?;
            let mut project: Project = db.get(&pid)?;
            if let Some(expected) = p["expected_directory"].as_str() {
                ensure!(
                    project.directory == expected,
                    "Project directory changed; reload before saving"
                );
            }
            let mut directory_changed = None;
            if let Some(directory) = next_directory {
                let previous = project.directory.clone();
                if previous != directory {
                    directory_changed = Some((previous, directory.clone()));
                }
                project.directory = directory;
                if let Some(detected) = detected_github {
                    project.detected_github = detected;
                }
                if let Some(source) = auto_source
                    && project.github.is_none()
                    && !project.github_disconnected
                    && self.github.generation() == account_generation
                {
                    project.github = Some(source);
                }
            }
            if let Some(name) = next_name {
                project.name = name;
            }
            if let Some(branch) = next_target_branch {
                project.target_branch = (!branch.is_empty()).then_some(branch);
            }
            db.save(&project)?;
            if let Some((previous, directory)) = directory_changed {
                for c in db
                    .list::<Card>()?
                    .iter()
                    .filter(|c| c.project_id == project.id)
                {
                    log(
                        db,
                        c,
                        "project.updated",
                        "user",
                        "Project work directory updated for future executions",
                        None,
                        json!({"previous_directory": previous, "directory": directory}),
                        self.now_ms(),
                    )?;
                }
            }
            Ok(serde_json::to_value(project)?)
        })?;
        if linked {
            self.trigger_github_sync();
        }
        Ok(result)
    }

    pub(super) fn bound_project(&self, project: &Project, card: &Card) -> Project {
        let mut p = project.clone();
        if let Some(w) = &card.workspace {
            p.directory = w.directory.clone();
            p.target_branch = w.target_branch.clone();
        }
        p
    }
}
