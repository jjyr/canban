use super::*;
pub fn sync_hint(project: &Project, status: Option<&GitHubSyncStatus>, time: u64) -> String {
    let Some(g) = &project.github else {
        return "Connect a GitHub repository".into();
    };
    let status = status.filter(|s| s.generation == g.generation);
    let phase = status.map(|s| s.phase.as_str()).unwrap_or("");
    let message = match phase {
        "queued" => "Waiting to synchronize".into(),
        "issues" => "Synchronizing Issues…".into(),
        "prs" => "Synchronizing pull requests…".into(),
        "observations" => "Refreshing tracked activity…".into(),
        _ => {
            let mut text = g
                .synced_at
                .map(|t| {
                    format!(
                        "Last successful sync: {} minutes ago",
                        time.saturating_sub(t) / 60_000
                    )
                })
                .unwrap_or("Not synchronized yet".into());
            if let Some(error) = status.and_then(|s| s.error.as_ref()).or(g.error.as_ref()) {
                text.push_str(&format!(
                    "\nSynchronization failed or partially completed: {error}\nClick to retry"
                ));
            } else if let Some(s) = status.filter(|s| s.phase == "success") {
                if s.issues_added == 0 && s.prs_added == 0 {
                    text.push_str("\nNo new matching Issues or PRs to add");
                }
                text.push_str(&format!(
                    "\nAdded {} Issues · {} PRs\nClick to synchronize",
                    s.issues_added, s.prs_added
                ));
            } else {
                text.push_str("\nClick to synchronize");
            }
            if g.paused {
                text.push_str("\nAutomatic synchronization paused; manual sync remains available");
            }
            text
        }
    };
    format!("{}\n{message}", g.repository)
}
pub fn sync_button(ui: Ui, project_id: String) -> impl IntoElement {
    dynamic(move || {
        let state = ui.snapshot.get();
        let Some(project) = state.projects.iter().find(|p| p.id == project_id) else {
            return view().into_element();
        };
        let Some(g) = &project.github else {
            return view().into_element();
        };
        let status = state
            .github_sync
            .get(&project_id)
            .filter(|s| s.generation == g.generation);
        let phase = status.map(|s| s.phase.as_str()).unwrap_or("");
        let busy = matches!(phase, "queued" | "issues" | "prs" | "observations");
        let icon = if phase == "queued" {
            Icon::Clock
        } else if busy {
            Icon::Refresh
        } else if status.is_some_and(|s| s.error.is_some()) || g.error.is_some() {
            Icon::TriangleAlert
        } else if g.paused {
            Icon::CirclePause
        } else {
            Icon::Refresh
        };
        let hint = sync_hint(project, status, now());
        let id = project_id.clone();
        canban_ui_kit::chrome::sync_action(icon, hint, busy, busy && phase != "queued", move |_| {
            dispatch(ui, "github.sync.request", json!({"project_id":id}));
        })
        .into_element()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_source_status_never_leaks_into_new_repository_tooltip() {
        let project: Project = serde_json::from_value(json!({"id":"p","name":"P","directory":"/tmp/p","created_at":1,"workflow":Workflow::default(),"github":{"repository":"owner/new","repository_id":2,"generation":"new","intake":"all","synced_at":null,"error":null}})).unwrap();
        let status = GitHubSyncStatus {
            generation: "old".into(),
            phase: "failed".into(),
            error: Some("old repository denied".into()),
            ..Default::default()
        };
        let hint = sync_hint(&project, Some(&status), 100);
        assert!(hint.contains("owner/new"));
        assert!(!hint.contains("denied"));
    }
}
