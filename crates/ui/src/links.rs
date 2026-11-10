//! Markdown references share routing and previews across task, comments and reports.
mod document;
use crate::presenter::*;
use canban_ui_kit::{tokens::*, *};
use nsh_base::text_view::{CustomNode, MarkdownPlugin, MarkdownPlugins, Parser};
use nsh_core::prelude::*;
use serde_json::json;
const NODE: &str = "canban-reference";

#[derive(Clone, Default, PartialEq)]
pub struct Preview {
    title: String,
    detail: String,
    excerpt: String,
}
#[derive(Clone)]
pub struct CachedPreview {
    at: u64,
    value: Preview,
}
#[derive(Clone, Debug, PartialEq)]
enum Destination {
    Card(String),
    Profile(String),
    Attachment(String),
    Web(String),
    Other,
}
fn destination(raw: &str) -> Destination {
    if let Some(id) = raw.strip_prefix("card:").filter(|id| !id.is_empty()) {
        return Destination::Card(id.into());
    }
    if let Some(id) = raw.strip_prefix("profile:").filter(|id| !id.is_empty()) {
        return Destination::Profile(id.into());
    }
    if let Some(id) = raw.strip_prefix("attachment:").filter(|id| !id.is_empty()) {
        return Destination::Attachment(id.into());
    }
    if let Ok(url) = url::Url::parse(raw)
        && matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
    {
        return Destination::Web(url.into());
    }
    Destination::Other
}
fn github(raw: &str) -> Option<(String, u64, &'static str)> {
    let url = url::Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || url.host_str()? != "github.com"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let parts = url.path_segments()?.collect::<Vec<_>>();
    if parts.len() < 4 || parts[0].is_empty() || parts[1].is_empty() {
        return None;
    }
    let kind = match parts[2] {
        "issues" => "Issue",
        "pull" => "PR",
        _ => return None,
    };
    let number = parts[3].parse::<u64>().ok().filter(|n| *n > 0)?;
    Some((format!("{}/{}", parts[0], parts[1]), number, kind))
}
fn excerpt(body: &str) -> String {
    let limited = body.chars().take(3000).collect::<String>();
    Parser::new()
        .markdown(&limited)
        .plain_text()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect()
}
fn current_project(ui: Ui) -> String {
    if ui.page.get_untracked() == "new-card" {
        ui.card_target.get_untracked()
    } else {
        ui.current_card()
            .map(|c| c.project_id)
            .unwrap_or_else(|| ui.project.get_untracked())
    }
}
fn local_preview(ui: Ui, url: &str, title: &str) -> Preview {
    let s = ui.snapshot.get_untracked();
    let mut value = Preview {
        title: title.into(),
        detail: url.into(),
        excerpt: String::new(),
    };
    match destination(url) {
        Destination::Card(id) => {
            if let Some(c) = s
                .cards
                .iter()
                .find(|c| c.id == id && c.removed_at.is_none())
            {
                let project = s
                    .projects
                    .iter()
                    .find(|p| p.id == c.project_id)
                    .map(|p| p.name.as_str())
                    .unwrap_or("Project");
                value.title = c.title.clone();
                value.detail = format!(
                    "Card · {project} #{} · {}",
                    c.number,
                    if c.archived_at.is_some() {
                        "Archived"
                    } else {
                        &c.stage
                    }
                );
                value.excerpt = excerpt(&c.body);
            } else {
                value.excerpt = "This local Card is unavailable.".into();
            }
        }
        Destination::Profile(id) => {
            if let Some(p) = ui.profiles.get_untracked().iter().find(|p| p.id == id) {
                value.title = format!("@{}", p.id);
                value.detail = format!("{} · Agent profile", crate::profiles::provider(p).1);
                value.excerpt = if p.role_instruction.is_empty() {
                    "Open profile settings".into()
                } else {
                    excerpt(&p.role_instruction)
                };
            }
        }
        Destination::Attachment(id) => {
            if let Some(a) = s.artifacts.iter().find(|a| a.id == id) {
                value.title = a.name.clone();
                value.detail = "Card attachment · Open locally".into();
            }
        }
        Destination::Web(ref web) => {
            if let Some((repo, number, kind)) = github(web) {
                value.detail = format!("GitHub {kind} · {repo}#{number}");
                value.excerpt = if s.github.state == "connected"
                    && s.projects.iter().any(|p| {
                        p.github
                            .as_ref()
                            .is_some_and(|g| g.repository.eq_ignore_ascii_case(&repo))
                    }) {
                    "Hover to load details".into()
                } else if s.github.state == "connected" {
                    "Open in browser".into()
                } else {
                    "Open in browser · GitHub is not connected".into()
                };
                if let Some(issue) = s.github_issues.iter().find(|i| {
                    i.source.repository.eq_ignore_ascii_case(&repo) && i.source.number == number
                }) {
                    value.title = issue.title.clone();
                    value.detail.push_str(&format!(" · {}", issue.state));
                    value.excerpt = excerpt(&issue.body);
                } else if let Some(pr) = s
                    .cards
                    .iter()
                    .flat_map(|c| c.pull_request.iter().chain(&c.pull_request_history))
                    .find(|p| p.repository.eq_ignore_ascii_case(&repo) && p.number == number)
                {
                    value.detail.push_str(&format!(" · {}", pr.state));
                    value.excerpt = excerpt(&pr.review_summary);
                }
            } else if let Ok(url) = url::Url::parse(web) {
                value.detail = url.host_str().unwrap_or("Website").into();
                value.excerpt = web.clone();
            }
        }
        Destination::Other => {}
    }
    value
}
/// Opening a reference never launches or resumes an Agent.
pub fn open(ui: Ui, url: &str) {
    match destination(url) {
        Destination::Card(id) => {
            if ui
                .snapshot
                .with_untracked(|s| s.cards.iter().any(|c| c.id == id && c.removed_at.is_none()))
            {
                ui.open_card(id);
            } else {
                ui.operation_error
                    .set("This local Card is unavailable".into());
            }
        }
        Destination::Profile(id) => {
            if ui
                .profiles
                .with_untracked(|profiles| profiles.iter().any(|p| p.id == id))
            {
                ui.navigate("settings");
            }
        }
        Destination::Attachment(id) => {
            if let Some(a) = ui
                .snapshot
                .with_untracked(|s| s.artifacts.iter().find(|a| a.id == id).cloned())
            {
                open_resource(ui, a.card_id, a.path);
            }
        }
        Destination::Web(url) => ui.spawn(async move {
            let result = blocking(move || {
                std::process::Command::new("/usr/bin/open")
                    .arg(url)
                    .status()
            })
            .await;
            if !matches!(result,Ok(Ok(status)) if status.success()) {
                ui.operation_error
                    .try_set("Could not open the link in your browser".into());
            }
        }),
        Destination::Other => {}
    }
}
fn load(ui: Ui, url: String, title: String, value: Signal<Preview>, loading: Signal<bool>) {
    value.set(local_preview(ui, &url, &title));
    let Some((repository, number, _)) = github(&url) else {
        return;
    };
    let snapshot = ui.snapshot.get_untracked();
    if snapshot.github.state != "connected" {
        return;
    }
    let Some((project, source)) = snapshot.projects.iter().find_map(|p| {
        p.github
            .as_ref()
            .filter(|s| s.repository.eq_ignore_ascii_case(&repository))
            .map(|s| (p.id.clone(), s.clone()))
    }) else {
        return;
    };
    let login = snapshot.github.login.clone();
    let key = format!("{:?}:{}:{repository}#{number}", login, source.generation);
    if let Some(cached) = ui.link_previews.with(|m| {
        m.get(&key)
            .filter(|v| canban_types::now().saturating_sub(v.at) < 60_000)
            .cloned()
    }) {
        value.set(cached.value);
        return;
    }
    if loading.get_untracked() {
        return;
    }
    let Some(app) = crate::APP.get().cloned() else {
        return;
    };
    loading.set(true);
    let source_generation = source.generation.clone();
    let project_id = project.clone();
    ui.spawn(async move {
        let result = blocking(move ||app.call("github.reference.search",json!({"project_id":project,"source_generation":source.generation,"query":number.to_string()}))).await;
        loading.try_set(false);
        if value.try_get().is_none() || !ui.snapshot.with_untracked(|s|s.github.state == "connected" && s.github.login == login && s.projects.iter().any(|p|p.id == project_id && p.github.as_ref().is_some_and(|g|g.generation == source_generation))) { return; }
        match result {
            Ok(Ok(rows)) => if let Some(row) = rows.as_array().and_then(|rows|rows.iter().find(|row|row["number"].as_u64() == Some(number))) {
                let mut next = local_preview(ui,&url,&title);
                next.title = row["title"].as_str().unwrap_or(&title).into();
                next.detail.push_str(&format!(" · {}",row["state"].as_str().unwrap_or_default()));
                next.excerpt = excerpt(row["body"].as_str().unwrap_or_default());
                ui.link_previews.with(|cache| { if cache.len() >= 64 { cache.clear(); } cache.insert(key,CachedPreview { at:canban_types::now(),value:next.clone() }); });
                value.try_set(next);
            },
            _ => { let mut next = local_preview(ui,&url,&title); next.excerpt = "Preview unavailable · Open in browser".into(); value.try_set(next); }
        }
    });
}
fn render(ui: Ui, node: &CustomNode) -> AnyElement {
    let url = node.attributes.get("href").cloned().unwrap_or_default();
    let title = node.text.clone();
    let preview = signal(local_preview(ui, &url, &title));
    let loading = signal(false);
    let load_url = url.clone();
    let preview_content = view()
        .gap_2()
        .child(label(move || preview.get().title).font_weight(FontWeight::SEMIBOLD))
        .child(muted(move || preview.get().detail))
        .child(
            label(move || {
                if loading.get() {
                    "Loading preview…".into()
                } else {
                    preview.get().excerpt
                }
            })
            .text_size(px(12.)),
        );
    canban_ui_kit::reference_link::reference_link(
        title.clone(),
        preview_content,
        move || load(ui, load_url.clone(), title.clone(), preview, loading),
        move || open(ui, &url),
    )
    .into_element()
}
struct ReferencePlugin(Ui);
impl MarkdownPlugin for ReferencePlugin {
    fn name(&self) -> &str {
        NODE
    }
    fn render(&self, node: &CustomNode) -> Option<AnyElement> {
        (node.name == NODE).then(|| render(self.0, node))
    }
}
/// Install once on the native root so TextView component owners inherit it.
pub fn install(ui: Ui) {
    provide_context(MarkdownPlugins::new().with(ReferencePlugin(ui)));
}
pub fn reader(ui: Ui, body: impl Fn() -> String + 'static) -> nsh_base::TextViewElement {
    let source = computed(body);
    let parsed = computed(move || Parser::new().markdown(&source.get()));
    let document = computed(move || {
        let mut document = parsed.get();
        let snapshot = ui.snapshot.get();
        let profiles = ui.profiles.get();
        let project = if ui.page.get() == "new-card" {
            ui.card_target.get()
        } else {
            current_project(ui)
        };
        document::decorate(&mut document, &snapshot, &profiles, &project);
        document
    });
    // The root-installed plugin returns styled spans directly, preserving
    // the paragraph's baseline, selection and inline hit targets.
    nsh_base::text_view()
        .document(move || Some(document.get()))
        .text_size(px(13.))
        .text_color(TEXT)
        .min_w(px(0.))
        .on_link_press(move |e: &nsh_base::TextViewLinkPressEvent| open(ui, &e.url))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn destinations_keep_local_references_local_and_only_open_web_schemes() {
        assert_eq!(destination("card:abc"), Destination::Card("abc".into()));
        assert_eq!(
            destination("profile:developer"),
            Destination::Profile("developer".into())
        );
        assert_eq!(
            destination("attachment:file"),
            Destination::Attachment("file".into())
        );
        for raw in [
            "javascript:alert(1)",
            "data:text/html,example",
            "file:///private/example",
            "#heading",
        ] {
            assert_eq!(destination(raw), Destination::Other);
        }
        assert!(matches!(
            destination("https://example.com/path"),
            Destination::Web(_)
        ));
    }
    #[test]
    fn github_preview_identity_rejects_lookalikes_and_distinguishes_pr_from_issue() {
        assert_eq!(
            github("https://github.com/owner/repo/issues/12#comment"),
            Some(("owner/repo".into(), 12, "Issue"))
        );
        assert_eq!(
            github("https://github.com/owner/repo/pull/13"),
            Some(("owner/repo".into(), 13, "PR"))
        );
        for raw in [
            "https://github.com.evil.invalid/owner/repo/issues/12",
            "https://user@github.com/owner/repo/issues/12",
            "https://github.com:444/owner/repo/issues/12",
            "https://github.com/owner/repo/issues/0",
        ] {
            assert!(github(raw).is_none());
        }
    }
}
