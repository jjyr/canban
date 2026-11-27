//! Last opened Project is a desktop preference, never an execution command.
use crate::presenter::*;
use canban_types::{HostSummary, Project};
use nsh_core::prelude::*;
use serde_json::json;

pub fn restore(projects: &[Project], saved: Option<&str>) -> Option<String> {
    saved
        .and_then(|id| projects.iter().find(|p| p.id == id))
        .or_else(|| projects.first())
        .map(|p| p.id.clone())
}

/// Tracks a saved remote `last_project` (`host::<host_id>::<raw_id>`) across startup until
/// its remote host connects and merges its projects into the desktop snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingRemoteRestore {
    pub saved: String,
    pub fallback: String,
}

impl PendingRemoteRestore {
    pub fn from_bootstrap(
        projects: &[Project],
        saved: Option<&str>,
        restored: Option<&str>,
    ) -> Option<Self> {
        let saved = saved.filter(|id| !id.is_empty())?;
        if projects.iter().any(|p| p.id == saved) {
            return None;
        }
        Some(Self {
            saved: saved.to_owned(),
            fallback: restored.unwrap_or_default().to_owned(),
        })
    }

    pub fn resolve(
        slot: &mut Option<Self>,
        current_project: &str,
        projects: &[Project],
        hosts: &[HostSummary],
    ) -> Option<String> {
        let pending = slot.as_ref()?;
        if current_project != pending.fallback {
            *slot = None;
            return None;
        }
        if projects.iter().any(|p| p.id == pending.saved) {
            let saved = pending.saved.clone();
            *slot = None;
            return Some(saved);
        }
        if !hosts.is_empty() {
            if let Some((host_id, _)) = canban_types::split_qualified_host_id(&pending.saved) {
                match hosts.iter().find(|h| h.id == host_id) {
                    None => *slot = None,
                    Some(h) if h.connection.is_connected() => *slot = None,
                    Some(_) => {}
                }
            } else if hosts
                .iter()
                .filter(|h| !h.is_local)
                .all(|h| h.connection.is_connected())
            {
                *slot = None;
            }
        }
        None
    }
}

pub fn remember(ui: Ui) {
    let Some(backend) = crate::backend::get() else {
        return;
    };
    let pending = signal(None::<String>);
    let saving = signal(false);
    let queued = stored(ui.project.get_untracked());
    effect(move || {
        let _ = ui.update.get();
        #[cfg(feature = "local-host")]
        if crate::backend::local_home().is_some_and(|home| canban_updater::handoff::active(&home)) {
            return;
        }
        let id = ui.project.get();
        let is_pending_fallback = ui
            .pending_remote_project
            .with(|slot| slot.as_ref().is_some_and(|p| p.fallback == id));
        if !id.is_empty()
            && !is_pending_fallback
            && ui.snapshot.with(|s| s.projects.iter().any(|p| p.id == id))
            && queued.get() != id
        {
            queued.replace(id.clone());
            pending.set(Some(id));
        }
    });
    // One in-flight preference write. A newer selection replaces the pending
    // value, so a slower old save cannot become the final startup selection.
    effect(move || {
        if saving.get() {
            return;
        }
        let Some(id) = pending.get() else {
            return;
        };
        saving.set(true);
        pending.set(None);
        let backend = backend.clone();
        let target = id.clone();
        ui.spawn(async move {
            let result = blocking(move || {
                backend.call_local("utilities.configure", json!({"last_project":id}))
            })
            .await;
            let error = match result {
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(format!("{e:#}")),
                Err(e) => Some(e.to_string()),
            };
            if ui.project.try_get().as_ref() == Some(&target)
                && let Some(error) = error
            {
                ui.operation_error
                    .try_set(format!("Could not save the last opened Project: {error}"));
            }
            saving.try_set(false);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restore_uses_identity_and_falls_back_without_replaying_a_page_or_execution() {
        let projects = ["first", "recent"].map(|id| serde_json::from_value::<Project>(json!({
            "id":id,"name":"Same name","directory":"/example","workflow":canban_types::Workflow::default(),"created_at":1
        })).unwrap());
        assert_eq!(restore(&projects, Some("recent")), Some("recent".into()));
        assert_eq!(restore(&projects, Some("removed")), Some("first".into()));
        assert_eq!(restore(&projects, None), Some("first".into()));
        assert_eq!(restore(&[], Some("recent")), None);
    }
}
