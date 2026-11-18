//! Product candidates over NSH's native caret completion protocol.
use crate::presenter::*;
use canban_ui_kit::markdown::MarkdownInput;
use nsh_base::{
    EditorApi,
    editor::{
        EditorCompletionAcceptEvent, EditorCompletionItem, EditorCompletionRequest, EditorRange,
    },
};
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const TAG_DETAIL_PREFIX: &str = "Tag";
const CREATE_TAG_DETAIL: &str = "New tag · Create";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceFormat {
    Markdown,
    PlainText,
}

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

fn tag_item(tag: &str, detail: String, range: EditorRange, order: usize) -> EditorCompletionItem {
    let mut entry = item(format!("#{tag}"), detail, format!("#{tag} "), range, order);
    entry.filter_text = Some(tag.to_owned().into());
    entry
}

/// Register a tag when the user accepts an existing or newly created `#tag` completion item.
pub fn accept_tag(ui: Ui, project_id: &str, event: &EditorCompletionAcceptEvent) -> Option<String> {
    let detail = event.item.detail.as_deref().unwrap_or_default();
    if detail != CREATE_TAG_DETAIL && !detail.starts_with(TAG_DETAIL_PREFIX) {
        return None;
    }
    let raw = event
        .item
        .insert_text
        .as_deref()
        .map(str::trim)
        .unwrap_or_else(|| event.item.label.as_ref());
    crate::tags::register(ui, project_id, raw)
}

pub fn source(
    ui: Ui,
    editor: MarkdownInput,
    text: Signal<String>,
    project: Memo<String>,
    name: &str,
    placeholder: &str,
) -> (nsh_base::EditorElement, Signal<String>) {
    attach(
        ui,
        editor,
        editor.source(text, name, placeholder),
        text,
        project,
        ReferenceFormat::Markdown,
        None,
    )
}

pub fn attach(
    ui: Ui,
    editor: MarkdownInput,
    element: nsh_base::EditorElement,
    text: Signal<String>,
    project: Memo<String>,
    format: ReferenceFormat,
    on_mention: Option<std::rc::Rc<dyn Fn(String)>>,
) -> (nsh_base::EditorElement, Signal<String>) {
    let epoch = signal(0u64);
    let remote_error = signal(String::new());
    let cache = stored(std::collections::BTreeMap::<String, Vec<Value>>::new());
    let accept_project = project;
    let source = element
        .completion(true)
        .completion_trigger_characters(vec!["@".into(), "#".into()])
        .on_completion_accept(move |event: &EditorCompletionAcceptEvent| {
            let project_id = accept_project.get_untracked();
            if accept_tag(ui, &project_id, event).is_some() {
                return;
            }
            if let Some(on_mention) = &on_mention {
                let id = event.item.label.to_string();
                if event
                    .item
                    .insert_text
                    .as_ref()
                    .is_some_and(|text| text.as_ref() == format!("@{id} "))
                    && ui.profiles.with_untracked(|p| p.iter().any(|p| p.id == id))
                {
                    on_mention(id);
                }
            }
        })
        .on_completion_request(move |request: &EditorCompletionRequest| {
            epoch.update(|n| *n += 1);
            let generation = epoch.get_untracked();
            let Some((start, trigger, query)) = context(&text.get_untracked(), request.offset)
            else {
                editor.completions(request.id, Vec::new(), false);
                return;
            };
            let Some(api) = editor.editor.api::<EditorApi>() else {
                return;
            };
            let range = EditorRange::new(api.offset_to_position(start), request.position);
            let snapshot = ui.snapshot.get_untracked();
            let local_tags = ui.local_tags.get_untracked();
            let target = project.get_untracked();
            let remote_first = ui
                .current_card()
                .is_some_and(|c| c.github_issue().is_some());
            let mut items = local_candidates_with_format(
                &snapshot,
                &ui.profiles.get_untracked(),
                &local_tags,
                &target,
                trigger,
                &query,
                range,
                format,
            );
            // Recompute against the complete inventory as the query grows and
            // refresh the explicit replacement range through the current caret.
            if trigger == '@' {
                editor.completions(request.id, items, true);
                return;
            }
            let gh = ui.github.gh_status.get_untracked();
            let Some((account, source)) = remote_source(&snapshot, &gh, &target) else {
                remote_error.set(String::new());
                editor.completions(request.id, items, true);
                return;
            };
            let cache_key = format!("{account}:{target}:{}:{query}", source.generation);
            if let Some(rows) = cache.with(|c| c.get(&cache_key).cloned()) {
                append_remote(&mut items, rows, &source.repository, range, format);
                rank_candidates(&mut items, remote_first);
                editor.completions(request.id, items, true);
                return;
            }
            let local = items.clone();
            editor.completions(request.id, items, true);
            let request = request.clone();
            let original = text.get_untracked();
            let source_generation = source.generation.clone();
            let source_project = target.clone();
            set_timeout(std::time::Duration::from_millis(250), move || {
                if epoch.try_get() != Some(generation) {
                    return;
                }
                if crate::APP.get().is_none() {
                    return;
                }
                ui.spawn(async move {
                    let repository = source.repository.clone();
                    let result = blocking(move || {
                        crate::routed_call(
                            "github.reference.search",
                            json!({
                                "project_id": target,
                                "query": query,
                                "source_generation": source.generation
                            }),
                            None,
                        )
                    })
                    .await;
                    if epoch.try_get() != Some(generation)
                        || text.try_get().as_ref() != Some(&original)
                        || crate::github::active_login_untracked(ui).as_deref()
                            != Some(account.as_str())
                        || !ui.snapshot.with_untracked(|s| {
                            s.projects.iter().any(|p| {
                                p.id == source_project
                                    && p.github
                                        .as_ref()
                                        .is_some_and(|g| g.generation == source_generation)
                            })
                        })
                    {
                        return;
                    }
                    match result {
                        Ok(Ok(rows)) => {
                            remote_error.try_set(String::new());
                            let rows = rows.as_array().cloned().unwrap_or_default();
                            cache.with(|c| {
                                if c.len() >= 100 {
                                    c.clear();
                                }
                                c.insert(cache_key, rows.clone());
                            });
                            let mut items = local;
                            append_remote(&mut items, rows, &repository, range, format);
                            rank_candidates(&mut items, remote_first);
                            editor.completions(request.id, items, true);
                        }
                        Ok(Err(error)) => {
                            remote_error
                                .try_set(format!("GitHub references unavailable: {error:#}"));
                        }
                        Err(error) => {
                            remote_error.try_set(error.to_string());
                        }
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

#[cfg(test)]
fn local_candidates(
    snapshot: &canban_types::Snapshot,
    profiles: &[canban_types::Profile],
    target: &str,
    trigger: char,
    query: &str,
    range: EditorRange,
) -> Vec<EditorCompletionItem> {
    local_candidates_with_format(
        snapshot,
        profiles,
        &BTreeSet::new(),
        target,
        trigger,
        query,
        range,
        ReferenceFormat::Markdown,
    )
}

#[allow(clippy::too_many_arguments)]
fn local_candidates_with_format(
    snapshot: &canban_types::Snapshot,
    profiles: &[canban_types::Profile],
    local_tags: &BTreeSet<String>,
    target: &str,
    trigger: char,
    query: &str,
    range: EditorRange,
    format: ReferenceFormat,
) -> Vec<EditorCompletionItem> {
    if trigger == '@' {
        let project = snapshot.projects.iter().find(|p| p.id == target);
        return profiles
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let (prov, overridden) = crate::profiles::effective_provider_name(p, project);
                let detail = if p.is_identity() {
                    if overridden {
                        format!("Role · {prov} (Project override)")
                    } else {
                        format!("Role · {prov}")
                    }
                } else {
                    format!("Agent · {prov}")
                };
                item(p.id.clone(), detail, format!("@{} ", p.id), range, i)
            })
            .collect();
    }

    let query_lower = query.to_lowercase();
    let known = crate::tags::known_map(snapshot, local_tags, Some(target));
    let ordered_tags = crate::tags::ordered_known(snapshot, local_tags, Some(target));
    let counts = crate::tags::tag_card_counts(snapshot, &known);

    let mut out = Vec::new();

    for tag in &ordered_tags {
        if query.is_empty() || tag.to_lowercase().contains(&query_lower) {
            let count = counts.get(&tag.to_lowercase()).copied().unwrap_or(0);
            let detail = if count == 0 {
                TAG_DETAIL_PREFIX.to_owned()
            } else {
                format!(
                    "{TAG_DETAIL_PREFIX} · {count} Card{}",
                    if count == 1 { "" } else { "s" }
                )
            };
            let order = out.len();
            out.push(tag_item(tag, detail, range, order));
        }
    }

    let mut cards = snapshot
        .cards
        .iter()
        .filter(|c| c.removed_at.is_none())
        .collect::<Vec<_>>();
    cards.sort_by_key(|c| (c.project_id != target, c.archived_at.is_some(), c.number));
    for c in cards
        .into_iter()
        .filter(|c| {
            query.is_empty()
                || c.number.to_string().starts_with(query)
                || c.title.to_lowercase().contains(&query_lower)
        })
        .take(50)
    {
        let project = snapshot
            .projects
            .iter()
            .find(|p| p.id == c.project_id)
            .map(|p| p.name.as_str())
            .unwrap_or("Project");
        let order = out.len() + 100;
        let insert = match format {
            ReferenceFormat::Markdown => {
                format!("[{} #{}](card:{}) ", escape(project), c.number, c.id)
            }
            ReferenceFormat::PlainText => {
                if c.project_id == target {
                    format!("#{} ", c.number)
                } else {
                    format!("{project} #{} ", c.number)
                }
            }
        };
        out.push(item(
            format!("{} {}", c.number, c.title),
            format!("Card · {project}"),
            insert,
            range,
            order,
        ));
    }

    if let Some(new_tag) = canban_types::normalize_tag(query)
        && !known.contains_key(&new_tag.to_lowercase())
    {
        let order = out.len() + 200;
        out.push(tag_item(
            &new_tag,
            CREATE_TAG_DETAIL.to_owned(),
            range,
            order,
        ));
    }

    out
}

fn rank_candidates(items: &mut [EditorCompletionItem], remote_first: bool) {
    if !remote_first {
        return;
    }
    for (i, item) in items.iter_mut().enumerate() {
        let is_create_tag = item.detail.as_deref() == Some(CREATE_TAG_DETAIL);
        let remote = item
            .insert_text
            .as_ref()
            .is_some_and(|text| text.contains("](https://github.com/"));
        let bucket = if is_create_tag {
            2000
        } else if remote {
            0
        } else {
            1000
        };
        item.sort_text = Some(format!("{:05}", i + bucket).into());
    }
}

fn append_remote(
    items: &mut Vec<EditorCompletionItem>,
    rows: Vec<Value>,
    repository: &str,
    range: EditorRange,
    format: ReferenceFormat,
) {
    // Keep "New tag · Create" at the tail if present so exact/existing matches lead.
    let create_item = items
        .iter()
        .position(|it| it.detail.as_deref() == Some(CREATE_TAG_DETAIL))
        .map(|idx| items.remove(idx));
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
        let insert = match format {
            ReferenceFormat::Markdown => format!("[{}#{number}]({url}) ", escape(repository)),
            ReferenceFormat::PlainText => format!("{repository}#{number} "),
        };
        items.push(item(
            format!("{number} {}", row["title"].as_str().unwrap_or_default()),
            format!(
                "{kind} · {repository} · {}",
                row["state"].as_str().unwrap_or_default()
            ),
            insert,
            range,
            items.len() + 100,
        ));
    }
    if let Some(mut create) = create_item {
        create.sort_text = Some(format!("{:05}", items.len() + 200).into());
        items.push(create);
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
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].insert_text.as_ref().unwrap().to_string(),
            "[Project #65](card:c65) "
        );
        assert_eq!(rows[0].range, Some(range));
        assert_eq!(rows[1].label.as_ref(), "#needle");
        assert_eq!(rows[1].detail.as_deref(), Some(CREATE_TAG_DETAIL));
        assert_eq!(rows[1].insert_text.as_deref(), Some("#needle "));
        assert_eq!(
            local_candidates(&s, &profiles, "p", '#', "65", range).len(),
            1
        );
    }
    #[test]
    fn plain_text_title_candidates_format_card_and_remote_references_without_markdown_links() {
        let mut s = snapshot();
        s.projects.push(serde_json::from_value(json!({"id":"p2","name":"Other","directory":"/fixture2","created_at":1,"workflow":canban_types::Workflow::default()})).unwrap());
        s.cards.push(serde_json::from_value(json!({"id":"other-card","project_id":"p2","number":9,"title":"Cross project needle","body":"Context","stage":"backlog","revision":1,"created_at":1})).unwrap());
        s.projects[0].tags = vec!["bug".into()];
        let profiles = canban_types::Config::default().profiles;
        let range = EditorRange::new(
            nsh_base::editor::EditorPosition::new(0, 0),
            nsh_base::editor::EditorPosition::new(0, 5),
        );

        let mention_items = local_candidates_with_format(
            &s,
            &profiles,
            &BTreeSet::new(),
            "p",
            '@',
            "",
            range,
            ReferenceFormat::PlainText,
        );
        assert!(
            mention_items
                .iter()
                .any(|item| item.insert_text.as_deref() == Some("@developer "))
        );

        let mut hash_items = local_candidates_with_format(
            &s,
            &profiles,
            &BTreeSet::new(),
            "p",
            '#',
            "needle",
            range,
            ReferenceFormat::PlainText,
        );
        assert_eq!(hash_items[0].insert_text.as_deref(), Some("#65 "));
        assert_eq!(hash_items[1].insert_text.as_deref(), Some("Other #9 "));
        assert_eq!(hash_items[2].insert_text.as_deref(), Some("#needle "));

        append_remote(
            &mut hash_items,
            vec![json!({
                "number": 42,
                "title": "Remote issue",
                "state": "open",
                "html_url": "https://github.com/owner/repo/issues/42",
                "pull_request": null
            })],
            "owner/repo",
            range,
            ReferenceFormat::PlainText,
        );
        assert!(
            hash_items
                .iter()
                .any(|item| item.insert_text.as_deref() == Some("owner/repo#42 "))
        );
    }
    #[test]
    fn hash_completion_suggests_existing_tags_with_counts_and_offers_new_tag_creation() {
        let mut s = snapshot();
        s.projects[0].tags = vec!["bug".into(), "ui-kit".into(), "前端".into()];
        s.cards[0].body = "Track #bug and #ui-kit".into();
        s.cards[1].body = "Another #bug".into();
        let profiles = canban_types::Config::default().profiles;
        let range = EditorRange::new(
            nsh_base::editor::EditorPosition::new(0, 0),
            nsh_base::editor::EditorPosition::new(0, 4),
        );

        // Empty `#` query surfaces existing tags alongside the 50 local Cards.
        let empty = local_candidates(&s, &profiles, "p", '#', "", range);
        assert_eq!(empty.len(), 53);
        assert_eq!(empty[0].label.as_ref(), "#bug");
        assert_eq!(empty[0].detail.as_deref(), Some("Tag · 2 Cards"));
        assert_eq!(empty[1].label.as_ref(), "#ui-kit");
        assert_eq!(empty[1].detail.as_deref(), Some("Tag · 1 Card"));
        assert_eq!(empty[2].label.as_ref(), "#前端");
        assert_eq!(empty[2].detail.as_deref(), Some("Tag"));

        // Exact case-insensitive match on an existing tag does NOT duplicate "New tag · Create".
        let exact = local_candidates(&s, &profiles, "p", '#', "BUG", range);
        assert!(exact.iter().any(|r| r.label.as_ref() == "#bug"));
        assert!(
            !exact
                .iter()
                .any(|r| r.detail.as_deref() == Some(CREATE_TAG_DETAIL))
        );

        // Partial tag prefix suggests existing `#ui-kit` AND offers creating `#ui`.
        let partial = local_candidates(&s, &profiles, "p", '#', "ui", range);
        assert_eq!(partial[0].label.as_ref(), "#ui-kit");
        let created = partial
            .iter()
            .find(|r| r.detail.as_deref() == Some(CREATE_TAG_DETAIL))
            .unwrap();
        assert_eq!(created.label.as_ref(), "#ui");
        assert_eq!(created.filter_text.as_deref(), Some("ui"));
        assert_eq!(created.insert_text.as_deref(), Some("#ui "));
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
        let mixed = "请让 @dev";
        assert_eq!(
            context(mixed, mixed.encode_utf16().count() as u32),
            Some((3, '@', "dev".into()))
        );
    }
}
