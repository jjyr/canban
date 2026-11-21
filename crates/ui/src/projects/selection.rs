//! Last opened Project is a desktop preference, never an execution command.
use crate::presenter::*;
use canban_types::Project;
use nsh_core::prelude::*;
use serde_json::json;

pub fn restore(projects: &[Project], saved: Option<&str>) -> Option<String> {
    saved
        .and_then(|id| projects.iter().find(|p| p.id == id))
        .or_else(|| projects.first())
        .map(|p| p.id.clone())
}

pub fn remember(ui: Ui) {
    let Some(backend) = crate::backend::get() else {
        return;
    };
    let pending = signal(None::<String>);
    let saving = signal(false);
    let queued = stored(String::new());
    effect(move || {
        let _ = ui.update.get();
        #[cfg(feature = "local-host")]
        if crate::backend::local_home().is_some_and(|home| canban_updater::handoff::active(&home)) {
            return;
        }
        let id = ui.project.get();
        if !id.is_empty()
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
