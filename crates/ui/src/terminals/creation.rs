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
    let explicit_card = params["card_id"]
        .as_str()
        .is_some_and(|s| !s.trim().is_empty());
    let explicit_project = params["project_id"]
        .as_str()
        .is_some_and(|s| !s.trim().is_empty());
    let inherited_cwd = inherited
        .and_then(|c| c.cwd.as_deref())
        .filter(|s| !s.is_empty());
    let card = if explicit_card {
        params["card_id"]
            .as_str()
            .and_then(|id| snapshot.cards.iter().find(|c| c.id == id))
    } else {
        inherited.and_then(|c| c.card_id.as_deref()).and_then(|id| {
            snapshot
                .cards
                .iter()
                .find(|c| c.id == id && c.archived_at.is_none())
        })
    };
    let project = params["project_id"]
        .as_str()
        .or_else(|| card.map(|c| c.project_id.as_str()))
        .or_else(|| inherited.and_then(|c| c.project_id.as_deref()))
        .and_then(|id| snapshot.projects.iter().find(|p| p.id == id));
    if params["cwd"].as_str().is_none_or(str::is_empty)
        && !params["setup_argv"].is_array()
        && (explicit_card || inherited_cwd.is_none())
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
    let directory = card
        .and_then(|c| c.workspace.as_ref().map(|w| w.directory.as_str()))
        .or_else(|| project.map(|p| p.directory.as_str()));
    let cwd = params["cwd"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            (!explicit_card && !explicit_project)
                .then_some(inherited_cwd)
                .flatten()
        })
        .or_else(|| {
            card.and_then(|c| c.worktree.as_deref())
                .filter(|s| !s.is_empty())
        })
        .or(directory)
        .or(inherited_cwd)?;
    let known = rows
        .iter()
        .filter(|e| e.listed() && e.host_id == host)
        .filter_map(|e| e.location.as_ref())
        .filter(|c| c.cwd.is_some())
        .filter_map(|c| {
            let root = c.directory_root.as_deref().or(c.cwd.as_deref())?;
            let same = Path::new(cwd) == Path::new(root)
                || c.cwd
                    .as_deref()
                    .is_some_and(|d| Path::new(cwd) == Path::new(d));
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
            "runtime":{"core_id":"new-core", "terminal":"new-pty",
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
        let (mut s, terminal) = fixture();
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
        let (mut s, terminal) = fixture();
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
        s.terminal_contexts[0].worktree_id = Some("managed".into());
        let hint = location_hint(
            &s,
            &super::super::entries(&s),
            &json!({"cwd":"/repo/card"}),
            &terminal,
            "backend-remote",
        )
        .unwrap();
        assert_eq!(hint.worktree_id.as_deref(), Some("managed"));
    }

    #[test]
    fn inherit_terminal_prefers_observed_cwd_over_project_root_and_card_root() {
        let (mut s, terminal) = fixture();
        s.worktrees.push(canban_types::ManagedWorktree {
            id: "wt1".into(),
            project_id: "p".into(),
            branch: "feat/managed".into(),
            base_branch: "main".into(),
            base_commit: None,
            path: "/wt/feat-managed".into(),
            directory: "/repo".into(),
            created_at: 1,
        });
        s.cards.push(
            serde_json::from_value(json!({
                "id":"c", "project_id":"p", "number":1, "title":"Card", "body":"",
                "stage":"develop", "revision":1, "created_at":1, "worktree":"/repo/card"
            }))
            .unwrap(),
        );

        let add_terminal = |s: &mut Snapshot, id: &str, ctx: TerminalContext| {
            let mut row = terminal.clone();
            row.id = id.into();
            s.terminals.push(row);
            s.terminal_contexts.push(ctx);
        };

        add_terminal(
            &mut s,
            "managed",
            TerminalContext {
                terminal_id: "managed".into(),
                cwd: Some("/wt/feat-managed/sub".into()),
                directory_root: Some("/wt/feat-managed".into()),
                project_id: Some("p".into()),
                worktree_id: Some("wt1".into()),
                branch: Some("feat/managed".into()),
                ..Default::default()
            },
        );
        add_terminal(
            &mut s,
            "ext",
            TerminalContext {
                terminal_id: "ext".into(),
                cwd: Some("/ext/wt/nested".into()),
                directory_root: Some("/ext/wt".into()),
                project_id: Some("p".into()),
                branch: Some("feat/ext".into()),
                ..Default::default()
            },
        );
        add_terminal(
            &mut s,
            "detached",
            TerminalContext {
                terminal_id: "detached".into(),
                cwd: Some("/ext/detached/sub".into()),
                directory_root: Some("/ext/detached".into()),
                project_id: Some("p".into()),
                branch: None,
                ..Default::default()
            },
        );
        add_terminal(
            &mut s,
            "card_term",
            TerminalContext {
                terminal_id: "card_term".into(),
                cwd: Some("/repo/card/pkg".into()),
                directory_root: Some("/repo/card".into()),
                project_id: Some("p".into()),
                card_id: Some("c".into()),
                branch: Some("canban/1".into()),
                ..Default::default()
            },
        );

        let rows = super::super::entries(&s);

        let managed_hint = location_hint(
            &s,
            &rows,
            &json!({"inherit_terminal_id": "managed"}),
            &terminal,
            "local",
        )
        .unwrap();
        assert_eq!(managed_hint.worktree_id.as_deref(), Some("wt1"));
        assert_eq!(
            managed_hint.directory_root.as_deref(),
            Some("/wt/feat-managed")
        );
        assert_eq!(managed_hint.cwd.as_deref(), Some("/wt/feat-managed/sub"));

        let ext_hint = location_hint(
            &s,
            &rows,
            &json!({"inherit_terminal_id": "ext"}),
            &terminal,
            "local",
        )
        .unwrap();
        assert_eq!(ext_hint.directory_root.as_deref(), Some("/ext/wt"));
        assert_eq!(ext_hint.cwd.as_deref(), Some("/ext/wt/nested"));
        assert_eq!(ext_hint.branch.as_deref(), Some("feat/ext"));

        let detached_hint = location_hint(
            &s,
            &rows,
            &json!({"inherit_terminal_id": "detached"}),
            &terminal,
            "local",
        )
        .unwrap();
        assert_eq!(
            detached_hint.directory_root.as_deref(),
            Some("/ext/detached")
        );
        assert_eq!(detached_hint.cwd.as_deref(), Some("/ext/detached/sub"));

        let card_hint = location_hint(
            &s,
            &rows,
            &json!({"inherit_terminal_id": "card_term"}),
            &terminal,
            "local",
        )
        .unwrap();
        assert_eq!(card_hint.card_id.as_deref(), Some("c"));
        assert_eq!(card_hint.directory_root.as_deref(), Some("/repo/card"));
        assert_eq!(card_hint.cwd.as_deref(), Some("/repo/card/pkg"));

        let root_sub_hint = location_hint(
            &s,
            &rows,
            &json!({"inherit_terminal_id": "old"}),
            &terminal,
            "local",
        )
        .unwrap();
        assert_eq!(root_sub_hint.directory_root.as_deref(), Some("/repo"));
        assert_eq!(root_sub_hint.cwd.as_deref(), Some("/repo/src"));

        let explicit_proj =
            location_hint(&s, &rows, &json!({"project_id": "p"}), &terminal, "local").unwrap();
        assert_eq!(explicit_proj.cwd.as_deref(), Some("/repo"));

        let explicit_card =
            location_hint(&s, &rows, &json!({"card_id": "c"}), &terminal, "local").unwrap();
        assert_eq!(explicit_card.cwd.as_deref(), Some("/repo/card"));
    }
}
