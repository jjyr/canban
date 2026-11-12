//! Product candidates over NSH's native caret completion protocol.
use crate::presenter::*;
use canban_ui_kit::markdown::MarkdownInput;
use nsh_base::{
    EditorApi,
    editor::{EditorCompletionItem, EditorCompletionRequest, EditorRange},
};
use nsh_core::prelude::*;
use serde_json::{Value, json};

/// UTF-16 caret -> token start/trigger/query. Markdown syntax stays literal.
pub fn context(body: &str, offset: u32) -> Option<(u32, char, String)> {
    let prefix: String = body
        .chars()
        .scan(0u32, |n, c| {
            *n += c.len_utf16() as u32;
            Some((*n, c))
        })
        .take_while(|(n, _)| *n <= offset)
        .map(|(_, c)| c)
        .collect();
    let start = prefix.rfind(['@', '#'])?;
    let trigger = prefix[start..].chars().next()?;
    let query = &prefix[start + 1..];
    if query
        .chars()
        .any(|c| !c.is_alphanumeric() && c != '-' && c != '_')
    {
        return None;
    }
    // Reuse the shared code/email/quote/fence boundary without altering offsets.
    let mut check = prefix.clone();
    check.replace_range(start..start + 1, "@");
    if !crate::profiles::mention_context(&check, offset) {
        return None;
    }
    Some((
        prefix[..start].encode_utf16().count() as u32,
        trigger,
        query.into(),
    ))
}
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace(['\n', '\r'], " ")
}
fn item(
    label: String,
    detail: String,
    insert: String,
    range: EditorRange,
    order: usize,
) -> EditorCompletionItem {
    let mut item = EditorCompletionItem::new(label);
    item.detail = Some(detail.into());
    item.insert_text = Some(insert.into());
    item.range = Some(range);
    item.sort_text = Some(format!("{order:05}").into());
    item
}
pub fn source(
    ui: Ui,
    editor: MarkdownInput,
    text: Signal<String>,
    project: Memo<String>,
    name: &str,
    placeholder: &str,
) -> (nsh_base::EditorElement, Signal<String>) {
    let epoch = signal(0u64);
    let remote_error = signal(String::new());
    let cache = stored(std::collections::BTreeMap::<String, Vec<Value>>::new());
    let source = editor.source(text,name,placeholder).completion(true)
        .completion_trigger_characters(vec!["@".into(), "#".into()])
        .on_completion_request(move |request: &EditorCompletionRequest| {
            epoch.update(|n| *n += 1);
            let generation = epoch.get_untracked();
            let Some((start,trigger,query)) = context(&text.get_untracked(), request.offset) else {
                editor.completions(request.id,Vec::new(),false); return;
            };
            let Some(api) = editor.editor.api::<EditorApi>() else { return; };
            let range = EditorRange::new(api.offset_to_position(start), request.position);
            let snapshot = ui.snapshot.get_untracked();
            let target = project.get_untracked();
            let remote_first = ui.current_card().is_some_and(|c| c.github_issue().is_some());
            let mut items = local_candidates(&snapshot,&ui.profiles.get_untracked(),&target,trigger,&query,range);
            // Recompute against the complete inventory as the query grows and
            // refresh the explicit replacement range through the current caret.
            if trigger == '@' { editor.completions(request.id,items,true); return; }
            let gh = ui.github.gh_status.get_untracked();
            let Some((account, source)) = remote_source(&snapshot, &gh, &target) else {
                remote_error.set(String::new());
                editor.completions(request.id,items,true); return;
            };
            let cache_key = format!("{account}:{target}:{}:{query}",source.generation);
            if let Some(rows) = cache.with(|c| c.get(&cache_key).cloned()) {
                append_remote(&mut items,rows,&source.repository,range);
                rank_candidates(&mut items, remote_first);
                editor.completions(request.id,items,true); return;
            }
            let local = items.clone();
            editor.completions(request.id,items,true);
            let request = request.clone();
            let original = text.get_untracked();
            let source_generation = source.generation.clone();
            let source_project = target.clone();
            set_timeout(std::time::Duration::from_millis(250),move || {
                if epoch.try_get() != Some(generation) { return; }
                let Some(app) = crate::APP.get().cloned() else { return; };
                ui.spawn(async move {
                    let repository = source.repository.clone();
                    let result = blocking(move || app.call("github.reference.search",json!({"project_id":target,"query":query,"source_generation":source.generation}))).await;
                    if epoch.try_get() != Some(generation)
                        || text.try_get().as_ref() != Some(&original)
                        || crate::github::active_login_untracked(ui).as_deref() != Some(account.as_str())
                        || !ui.snapshot.with_untracked(|s| {
                            s.projects.iter().any(|p| {
                                p.id == source_project
                                    && p.github.as_ref().is_some_and(|g| g.generation == source_generation)
                            })
                        })
                    {
                        return;
                    }
                    match result {
                        Ok(Ok(rows)) => {
                            remote_error.try_set(String::new());
                            let rows = rows.as_array().cloned().unwrap_or_default();
                            cache.with(|c| { if c.len() >= 100 { c.clear(); } c.insert(cache_key,rows.clone()); });
                            let mut items = local;
                            append_remote(&mut items,rows,&repository,range);
                            rank_candidates(&mut items, remote_first);
                            editor.completions(request.id,items,true);
                        }
                        Ok(Err(error)) => { remote_error.try_set(format!("GitHub references unavailable: {error:#}")); }
                        Err(error) => { remote_error.try_set(error.to_string()); }
                    }
                });
            });
        });
    (source, remote_error)
}
fn remote_source(
    snapshot: &canban_types::Snapshot,
    gh: &Value,
    project: &str,
) -> Option<(String, canban_types::GitHubSource)> {
    let login = crate::github::active_login_from(snapshot, gh)?;
    let source = snapshot
        .projects
        .iter()
        .find(|p| p.id == project)
        .and_then(|p| p.github.clone())?;
    Some((login, source))
}
fn local_candidates(
    snapshot: &canban_types::Snapshot,
    profiles: &[canban_types::Profile],
    target: &str,
    trigger: char,
    query: &str,
    range: EditorRange,
) -> Vec<EditorCompletionItem> {
    if trigger == '@' {
        return profiles
            .iter()
            .enumerate()
            .map(|(i, p)| {
                item(
                    p.id.clone(),
                    format!("Profile · {}", crate::profiles::provider(p).1),
                    format!("@{} ", p.id),
                    range,
                    i,
                )
            })
            .collect();
    }
    let mut cards = snapshot
        .cards
        .iter()
        .filter(|c| c.removed_at.is_none())
        .collect::<Vec<_>>();
    cards.sort_by_key(|c| (c.project_id != target, c.archived_at.is_some(), c.number));
    let query_lower = query.to_lowercase();
    cards
        .into_iter()
        .filter(|c| {
            query.is_empty()
                || c.number.to_string().starts_with(query)
                || c.title.to_lowercase().contains(&query_lower)
        })
        .take(50)
        .enumerate()
        .map(|(i, c)| {
            let project = snapshot
                .projects
                .iter()
                .find(|p| p.id == c.project_id)
                .map(|p| p.name.as_str())
                .unwrap_or("Project");
            item(
                format!("{} {}", c.number, c.title),
                format!("Card · {project}"),
                format!("[{} #{}](card:{}) ", escape(project), c.number, c.id),
                range,
                i,
            )
        })
        .collect()
}
fn rank_candidates(items: &mut [EditorCompletionItem], remote_first: bool) {
    if !remote_first {
        return;
    }
    for (i, item) in items.iter_mut().enumerate() {
        let remote = item
            .insert_text
            .as_ref()
            .is_some_and(|text| text.contains("](https://github.com/"));
        item.sort_text = Some(format!("{:05}", i + if remote { 0 } else { 1000 }).into());
    }
}
fn append_remote(
    items: &mut Vec<EditorCompletionItem>,
    rows: Vec<Value>,
    repository: &str,
    range: EditorRange,
) {
    for row in rows {
        let number = row["number"].as_u64().unwrap_or(0);
        let kind = if row["pull_request"].is_null() {
            "Issue"
        } else {
            "PR"
        };
        let url = row["html_url"].as_str().unwrap_or_default();
        if !url.starts_with("https://github.com/") {
            continue;
        }
        items.push(item(
            format!("{number} {}", row["title"].as_str().unwrap_or_default()),
            format!(
                "{kind} · {repository} · {}",
                row["state"].as_str().unwrap_or_default()
            ),
            format!("[{}#{number}]({url}) ", escape(repository)),
            range,
            items.len(),
        ));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> canban_types::Snapshot {
        let mut s = canban_types::Snapshot::default();
        s.projects.push(serde_json::from_value(json!({"id":"p","name":"Project","directory":"/fixture","created_at":1,"workflow":canban_types::Workflow::default(),"github":{"repository":"owner/repo","repository_id":1,"generation":"source-1","intake":"all"}})).unwrap());
        for number in 1..=65 {
            s.cards.push(serde_json::from_value(json!({"id":format!("c{number}"),"project_id":"p","number":number,"title":if number==65 { "Unique needle".to_owned() } else { format!("Task {number}") },"body":"Context","stage":"backlog","revision":1,"created_at":1})).unwrap());
        }
        s
    }
    #[test]
    fn query_is_applied_before_the_limit_and_replacement_tracks_the_current_caret() {
        let s = snapshot();
        let profiles = canban_types::Config::default().profiles;
        let range = EditorRange::new(
            nsh_base::editor::EditorPosition::new(0, 0),
            nsh_base::editor::EditorPosition::new(0, 7),
        );
        assert_eq!(
            local_candidates(&s, &profiles, "p", '#', "", range).len(),
            50
        );
        let rows = local_candidates(&s, &profiles, "p", '#', "needle", range);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].insert_text.as_ref().unwrap().to_string(),
            "[Project #65](card:c65) "
        );
        assert_eq!(rows[0].range, Some(range));
        assert_eq!(
            local_candidates(&s, &profiles, "p", '#', "65", range).len(),
            1
        );
    }
    #[test]
    fn disconnected_account_never_supplies_a_remote_completion_source() {
        let mut s = snapshot();
        let disconnected_gh = serde_json::json!({"state": "signed_out"});
        let connected_gh = serde_json::json!({"state": "connected", "login": "jjyr"});
        assert!(remote_source(&s, &disconnected_gh, "p").is_none());
        assert!(remote_source(&s, &connected_gh, "p").is_some());
        s.github.state = "connected".into();
        s.github.login = Some("app-user".into());
        assert!(remote_source(&s, &disconnected_gh, "p").is_some());
        s.github.state = "disconnected".into();
        s.github.login = None;
        assert!(remote_source(&s, &disconnected_gh, "p").is_none());
    }
    #[test]
    fn triggers_respect_markdown_and_utf16() {
        for body in [
            "email@example",
            "https://host/#ref",
            "`#123",
            "```rust\n#123",
            "~~~\n@dev",
            "> #123",
            "# Heading",
            "##",
        ] {
            assert!(
                context(body, body.encode_utf16().count() as u32).is_none(),
                "{body}"
            );
        }
        assert_eq!(context("中文 😀 #123", 10), Some((6, '#', "123".into())));
        assert_eq!(context("See @dev", 8), Some((4, '@', "dev".into())));
    }
}
