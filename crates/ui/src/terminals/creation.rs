//! Provisional placement after creation, using only locations already in the tree.
use super::Entry;
use canban_types::{Snapshot, TerminalContext, TerminalEntry};
use serde_json::Value;
use std::path::Path;

pub(crate) fn location_hint(
    snapshot: &Snapshot,
    rows: &[Entry],
    params: &Value,
    terminal: &TerminalEntry,
    host: &str,
) -> Option<TerminalContext> {
    let runtime = terminal.runtime.as_ref()?;
    let inherited = params["inherit_terminal_id"].as_str().and_then(|id| {
        rows.iter()
            .find(|e| e.saved.id == id && e.host_id == host)
            .and_then(|e| e.location.as_ref())
    });
    let card = params["card_id"]
        .as_str()
        .or_else(|| inherited.and_then(|c| c.card_id.as_deref()))
        .and_then(|id| snapshot.cards.iter().find(|c| c.id == id));
    let project = params["project_id"]
        .as_str()
        .or_else(|| card.map(|c| c.project_id.as_str()))
        .or_else(|| inherited.and_then(|c| c.project_id.as_deref()))
        .and_then(|id| snapshot.projects.iter().find(|p| p.id == id));
    if params["cwd"].as_str().is_none_or(str::is_empty)
        && !params["setup_argv"].is_array()
        && card.is_some_and(|c| {
            c.active()
                && !c.resolved()
                && c.worktree
                    .as_deref()
                    .is_none_or(|w| !Path::new(w).is_absolute())
        })
    {
        // AppServer may have just allocated this Card's first worktree; its path
        // is not known here yet. Do not mistake the Project root for that path.
        return None;
    }
    // Mirror known launch inputs conservatively. Unknown homes, new worktrees and
    // symlink aliases are left to the existing background discovery path.
    let location = card
        .and_then(|c| c.workspace.as_ref().map(|w| w.location.clone()))
        .or_else(|| project.map(|p| p.work_location()));
    let cwd = params["cwd"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            card.and_then(|c| c.worktree.as_deref())
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            location
                .as_ref()
                .filter(|l| l.machine == runtime.machine)
                .map(|l| l.directory.as_str())
        })
        .or_else(|| {
            params["machine"]
                .is_null()
                .then(|| inherited.and_then(|c| c.cwd.as_deref()))
                .flatten()
        })?;
    let known = rows
        .iter()
        .filter(|e| e.listed() && e.host_id == host)
        .filter_map(|e| e.location.as_ref())
        .filter(|c| c.machine == runtime.machine && c.cwd.is_some())
        .filter_map(|c| {
            let root = c.directory_root.as_deref().or(c.cwd.as_deref())?;
            let same = Path::new(cwd) == Path::new(root);
            let git = c.branch.is_some() || c.card_id.is_some() || c.worktree_id.is_some();
            (same || (git && Path::new(cwd).starts_with(root))).then_some((root, c))
        })
        .max_by_key(|(root, _)| root.len())?
        .1;
    let mut hint = known.clone();
    hint.terminal_id = terminal.id.clone();
    hint.core_id = runtime.core_id.clone()?;
    hint.terminal = runtime.terminal.clone()?;
    hint.directory_root = known.directory_root.clone().or_else(|| known.cwd.clone());
    hint.cwd = Some(cwd.into());
    Some(hint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (Snapshot, TerminalEntry) {
        let terminal: TerminalEntry = serde_json::from_value(json!({
            "id":"new", "number":2, "created_at":1,
            "runtime":{"machine":"local", "core_id":"new-core", "terminal":"new-pty",
                "state":"running", "connection":"connected"}
        }))
        .unwrap();
        let mut old = terminal.clone();
        old.id = "old".into();
        let s = Snapshot {
            projects: vec![
                serde_json::from_value(json!({
                    "id":"p", "name":"Project", "directory":"/repo", "workflow":canban_types::Workflow::default(), "created_at":1
                }))
                .unwrap(),
            ],
            terminals: vec![old],
            terminal_contexts: vec![TerminalContext {
                terminal_id: "old".into(),
                machine: "local".into(),
                cwd: Some("/repo/src".into()),
                directory_root: Some("/repo".into()),
                project_id: Some("p".into()),
                branch: Some("main".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        (s, terminal)
    }

    #[test]
    fn known_launch_inputs_join_existing_tree_without_discovery() {
        let (s, terminal) = fixture();
        for params in [
            json!({"cwd":"/repo/src"}),
            json!({"project_id":"p"}),
            json!({"inherit_terminal_id":"old"}),
            json!({"project_id":"p", "agent":true}),
        ] {
            let hint =
                location_hint(&s, &super::super::entries(&s), &params, &terminal, "local").unwrap();
            assert_eq!(hint.terminal_id, "new");
            assert_eq!(hint.core_id, "new-core");
            assert_eq!(hint.terminal, "new-pty");
            assert_eq!(hint.directory_root.as_deref(), Some("/repo"));
            assert_eq!(hint.project_id.as_deref(), Some("p"));
        }
    }

    #[test]
    fn misses_and_other_hosts_machines_or_closed_rows_stay_unassigned() {
        let (mut s, mut terminal) = fixture();
        for params in [
            json!({"cwd":"/repo-other"}),
            json!({"cwd":"/unknown"}),
            json!({}),
        ] {
            assert!(
                location_hint(&s, &super::super::entries(&s), &params, &terminal, "local")
                    .is_none()
            );
        }
        let params = json!({"cwd":"/repo"});
        assert!(
            location_hint(&s, &super::super::entries(&s), &params, &terminal, "remote").is_none()
        );
        terminal.runtime.as_mut().unwrap().machine = "ssh".into();
        assert!(
            location_hint(&s, &super::super::entries(&s), &params, &terminal, "local").is_none()
        );
        terminal.runtime.as_mut().unwrap().machine = "local".into();
        s.terminals[0].runtime.as_mut().unwrap().state = "finished".into();
        assert!(
            location_hint(&s, &super::super::entries(&s), &params, &terminal, "local").is_none()
        );
    }

    #[test]
    fn deepest_known_worktree_wins_and_ordinary_subdirectories_are_distinct() {
        let (mut s, terminal) = fixture();
        let mut nested = s.terminals[0].clone();
        nested.id = "nested".into();
        s.terminals.push(nested);
        let mut loc = s.terminal_contexts[0].clone();
        loc.terminal_id = "nested".into();
        loc.cwd = Some("/repo/nested".into());
        loc.directory_root = loc.cwd.clone();
        loc.branch = Some("nested".into());
        s.terminal_contexts.push(loc);
        let params = json!({"cwd":"/repo/nested/src"});
        let hint =
            location_hint(&s, &super::super::entries(&s), &params, &terminal, "local").unwrap();
        assert_eq!(hint.branch.as_deref(), Some("nested"));
        assert_eq!(hint.cwd.as_deref(), Some("/repo/nested/src"));
        s.terminal_contexts.truncate(1);
        s.terminal_contexts[0].branch = None;
        assert!(
            location_hint(&s, &super::super::entries(&s), &params, &terminal, "local").is_none()
        );
    }

    #[test]
    fn card_launch_matches_its_worktree_and_remote_directories_keep_their_owner() {
        let (mut s, mut terminal) = fixture();
        s.cards.push(
            serde_json::from_value(json!({
                "id":"c", "project_id":"p", "number":1, "title":"Card", "body":"",
                "stage":"develop", "revision":1, "created_at":1, "worktree":"/repo/card"
            }))
            .unwrap(),
        );
        s.terminal_contexts[0].cwd = Some("/repo/card/src".into());
        s.terminal_contexts[0].directory_root = Some("/repo/card".into());
        s.terminal_contexts[0].card_id = Some("c".into());
        let mut root = s.terminals[0].clone();
        root.id = "root".into();
        s.terminals.push(root);
        let mut context = s.terminal_contexts[0].clone();
        context.terminal_id = "root".into();
        context.cwd = Some("/repo".into());
        context.directory_root = context.cwd.clone();
        context.card_id = None;
        s.terminal_contexts.push(context);
        let hint = location_hint(
            &s,
            &super::super::entries(&s),
            &json!({"card_id":"c"}),
            &terminal,
            "local",
        )
        .unwrap();
        assert_eq!(hint.card_id.as_deref(), Some("c"));
        assert_eq!(hint.cwd.as_deref(), Some("/repo/card"));
        s.cards[0].worktree = None;
        assert!(
            location_hint(
                &s,
                &super::super::entries(&s),
                &json!({"card_id":"c"}),
                &terminal,
                "local"
            )
            .is_none()
        );
        s.cards[0].worktree = Some("/repo/card".into());

        s.entity_hosts.insert("old".into(), "backend-remote".into());
        s.terminal_contexts[0].machine = "ssh-core".into();
        s.terminal_contexts[0].worktree_id = Some("managed".into());
        terminal.runtime.as_mut().unwrap().machine = "ssh-core".into();
        let hint = location_hint(
            &s,
            &super::super::entries(&s),
            &json!({"cwd":"/repo/card"}),
            &terminal,
            "backend-remote",
        )
        .unwrap();
        assert_eq!(hint.machine, "ssh-core");
        assert_eq!(hint.worktree_id.as_deref(), Some("managed"));
    }
}
