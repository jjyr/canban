//! Visible Card's default comparison, sharing the Git workbench's bounded read cache.
use super::*;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Summary {
    additions: u64,
    deletions: u64,
    files: usize,
    truncated: bool,
}
impl Summary {
    fn from_data(data: &Value) -> Option<Self> {
        Some(Self {
            additions: data["total_additions"].as_u64()?,
            deletions: data["total_deletions"].as_u64()?,
            files: data["files"].as_array()?.len(),
            truncated: data["truncated"].as_bool().unwrap_or(false),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SummaryState {
    Loading,
    Ready(Summary),
    Unavailable(String),
    Missing,
}

#[derive(Clone, PartialEq, Eq)]
struct Query {
    project: String,
    host: String,
    params: Value,
    branch: Option<String>,
    offline: bool,
}

pub(super) fn observe(ui: Ui) -> Signal<SummaryState> {
    let state = signal(SummaryState::Missing);
    let ticket = signal(0_u64);
    let query = computed(move || {
        let page = ui.page.get();
        if page != "card" && !(page == "review" && ui.review_state.card_drawer_open.get()) {
            return None;
        }
        let card = ui.current_card()?;
        if card.worktree.is_none() && card.branch.is_none() {
            return None;
        }
        ui.snapshot.with(|s| {
            let project = s.projects.iter().find(|p| p.id == card.project_id);
            let target = crate::review::card_target_branch(&card, project);
            let host = s.card_host(&card.id).to_owned();
            Some(Query {
                project: card.project_id.clone(),
                offline: s.is_host_offline(&host),
                host,
                params: crate::review::card_compare_params(&card, &target),
                branch: card.branch.clone(),
            })
        })
    });
    effect(move || {
        let query = query.get();
        let _ = ui.review_state.summary_revision.get();
        let next = ticket.get_untracked() + 1;
        ticket.set(next);
        let Some(query) = query else {
            state.set(SummaryState::Missing);
            return;
        };
        if query.offline {
            state.set(SummaryState::Unavailable("Host offline".into()));
            return;
        }
        if crate::APP.get().is_none() {
            let data = ui.review_state.compare_data.get();
            state.set(if let Some(error) = data["summary_error"].as_str() {
                SummaryState::Unavailable(error.into())
            } else if data["summary_loading"] == true {
                SummaryState::Loading
            } else {
                Summary::from_data(&data)
                    .map(SummaryState::Ready)
                    .unwrap_or(SummaryState::Loading)
            });
            return;
        }
        let cache_key =
            crate::review::cache::key(&query.host, &query.project, "git.compare", &query.params);
        if let Some(value) = ui
            .review_state
            .read_cache
            .with(|c| c.peek_fresh(&cache_key))
        {
            state.set(
                Summary::from_data(&value.data)
                    .map(SummaryState::Ready)
                    .unwrap_or_else(|| {
                        SummaryState::Unavailable("Git returned no comparison summary".into())
                    }),
            );
            return;
        }
        if !matches!(state.get_untracked(), SummaryState::Ready(_)) {
            state.set(SummaryState::Loading);
        }
        ui.spawn(async move {
            let result =
                crate::review::cached_read(ui, "git.compare", query.params, query.project).await;
            if ticket.try_get() != Some(next) {
                return;
            }
            state.set(match result {
                Ok(value) => Summary::from_data(&value.data)
                    .map(SummaryState::Ready)
                    .unwrap_or_else(|| {
                        SummaryState::Unavailable("Git returned no comparison summary".into())
                    }),
                Err(error) => SummaryState::Unavailable(error),
            });
        });
    });
    state
}

pub(crate) fn open(ui: Ui) {
    if let Some(card) = ui.current_card() {
        if card.worktree.is_some()
            || card.branch.is_some()
            || card.is_github_pr()
            || card.pull_request.is_some()
        {
            crate::review::open_card(ui, &card.id);
        } else {
            ui.notify("Workspace not created");
        }
    } else {
        ui.notify("Open a Card first to view its changes");
    }
}

pub(crate) fn copy_branch(ui: Ui) {
    if let Some(branch) = ui
        .current_card()
        .and_then(|c| c.branch)
        .filter(|b| !b.is_empty())
    {
        copy_text(ui, branch);
    } else {
        ui.notify("Card branch unavailable");
    }
}

pub(crate) fn refresh(ui: Ui) {
    let Some(card) = ui.current_card() else {
        return;
    };
    let host = ui
        .snapshot
        .with_untracked(|s| s.card_host(&card.id).to_owned());
    ui.review_state.read_cache.with(|cache| {
        cache.invalidate_checkout(
            &host,
            &card.project_id,
            card.worktree.as_deref().unwrap_or(""),
            Some(&card.id),
        );
    });
    ui.review_state.summary_revision.update(|n| *n += 1);
    if ui.page.get_untracked() == "review" {
        crate::review::refresh(ui);
    }
}

pub(super) fn button(ui: Ui, branch: String, summary: Signal<SummaryState>) -> View {
    let branch_tip = branch.clone();
    let branch_label = branch.clone();
    let hint = move || {
        let target = ui
            .current_card()
            .map(|c| crate::review::card_target_branch(&c, ui.current_project().as_ref()))
            .unwrap_or_default();
        let details = match summary.get() {
            SummaryState::Ready(s) => format!(
                "{} files · +{} −{}{}",
                s.files,
                s.additions,
                s.deletions,
                if s.truncated {
                    " · Partial diff; counts cover displayed content"
                } else {
                    ""
                }
            ),
            SummaryState::Loading => "Loading changes…".into(),
            SummaryState::Unavailable(error) => {
                format!("Changes unavailable: {error} · Right-click to refresh")
            }
            SummaryState::Missing => "Workspace not created".into(),
        };
        format!(
            "View Card changes in Git\nBranch: {branch_tip}\nBase: {target} · Includes committed and uncommitted changes\n{details}"
        )
    };
    view()
        .min_w(px(0.))
        .max_w(px(360.))
        .tooltip(hint_tooltip(hint))
        .on_context_menu(move |event: &nsh_core::ContextMenuEvent| {
            use nsh_core::element::desktop::{MenuItem, popup_menu_at};
            let at = event.page_position;
            ui.spawn(async move {
                popup_menu_at(
                    vec![
                        MenuItem::new("View Card changes in Git").on_select(move || open(ui)),
                        MenuItem::new("Copy Card branch").on_select(move || copy_branch(ui)),
                        MenuItem::new("Refresh Card diff").on_select(move || refresh(ui)),
                    ],
                    at,
                )
                .await;
            });
        })
        .child(
            canban_ui_kit::press_target()
                .min_w(px(0.))
                .h(px(24.))
                .px_1p5()
                .rounded(px(5.))
                .flex_row()
                .items_center()
                .gap_1()
                .bg(CLEAR)
                .hover(|st| st.bg(INSET))
                .focus(|st| st.border_1().border_color(ACCENT))
                .accessibility_label(move || {
                    Some(format!("View Card changes in Git · {branch_label}").into())
                })
                .child(icon(Icon::Branch).w(px(12.)).h(px(12.)).flex_shrink_0())
                .child(
                    muted(crate::components::git_workspace::compact(&branch, 22))
                        .min_w(px(0.))
                        .truncate(),
                )
                .child(dynamic(move || {
                    let mut row = view().flex_row().gap_1().flex_shrink_0();
                    match summary.get() {
                        SummaryState::Ready(s) => {
                            if s.truncated {
                                row = row.child(muted("≥").text_size(px(11.)));
                            }
                            row.child(
                                label(format!("+{}", s.additions))
                                    .text_color(SUCCESS)
                                    .text_size(px(11.))
                                    .font_family(MONO_FONT),
                            )
                            .child(
                                label(format!("−{}", s.deletions))
                                    .text_color(DANGER)
                                    .text_size(px(11.))
                                    .font_family(MONO_FONT),
                            )
                        }
                        SummaryState::Loading => row.child(muted("…")),
                        SummaryState::Unavailable(_) => {
                            row.child(muted("Diff unavailable").text_size(px(11.)))
                        }
                        SummaryState::Missing => row,
                    }
                }))
                .child(muted("↗").text_size(px(12.)).flex_shrink_0())
                .on_press(move |_: &()| open(ui)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn unavailable_and_truncated_summaries_are_not_reported_as_zero_complete_diffs() {
        assert!(Summary::from_data(&json!({})).is_none());
        assert!(Summary::from_data(&json!({"files":[]})).is_none());
        let summary = Summary::from_data(&json!({"total_additions":12,"total_deletions":4,
            "files":[{"filename":"a"}],"truncated":true}))
        .unwrap();
        assert_eq!(
            (
                summary.additions,
                summary.deletions,
                summary.files,
                summary.truncated
            ),
            (12, 4, 1, true)
        );
        let empty =
            Summary::from_data(&json!({"total_additions":0,"total_deletions":0,"files":[]}))
                .unwrap();
        assert_eq!((empty.additions, empty.deletions, empty.files), (0, 0, 0));
    }
}
