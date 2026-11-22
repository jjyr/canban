//! Unified Pull Request + CI status pills for Board Cards, Card headers, and Gallery.
use crate::presenter::*;
use canban_types::{Card, GitHubPrChecks, GitHubReference, Snapshot};
use canban_ui_kit::*;
use nsh_core::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardPrView {
    pub card_id: String,
    pub repository: String,
    pub number: u64,
    pub pr_url: String,
    pub head_sha: String,
    pub merged: bool,
    pub pr_state: String,
    pub ci_state: String,
    pub checks: Option<GitHubPrChecks>,
    pub show_pr_chip: bool,
}

impl CardPrView {
    pub fn normalized_ci_state(&self) -> Option<&'static str> {
        let raw = self
            .checks
            .as_ref()
            .map(|c| c.state.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(self.ci_state.as_str());
        match raw {
            "running" | "pending" | "in_progress" | "queued" => Some("running"),
            "success" => Some("success"),
            "failure" | "error" | "timed_out" | "cancelled" | "action_required" => Some("failure"),
            _ => None,
        }
    }

    pub fn checks_url(&self) -> Option<String> {
        if let Some(checks) = &self.checks
            && checks.failed_checks.len() == 1
            && let Some(url) = checks.failed_checks[0].url.as_deref()
            && !url.is_empty()
        {
            return Some(url.to_owned());
        }
        if let Some(url) = self.checks.as_ref().and_then(|c| c.url.as_deref())
            && !url.is_empty()
        {
            return Some(url.to_owned());
        }
        if !self.pr_url.is_empty() {
            Some(format!("{}/checks", self.pr_url.trim_end_matches('/')))
        } else {
            None
        }
    }

    pub fn checks_overview_url(&self) -> Option<String> {
        if let Some(url) = self.checks.as_ref().and_then(|c| c.url.as_deref())
            && !url.is_empty()
        {
            return Some(url.to_owned());
        }
        (!self.pr_url.is_empty()).then(|| format!("{}/checks", self.pr_url.trim_end_matches('/')))
    }

    pub fn ci_label(&self, include_counts: bool) -> Option<(String, Tone, bool, Option<Icon>)> {
        let state = self.normalized_ci_state()?;
        let checks = self.checks.as_ref();
        Some(match state {
            "running" => {
                let text = if include_counts
                    && let Some(c) = checks
                    && c.total > 0
                {
                    format!("CI running ({}/{})", c.passed, c.total)
                } else {
                    "CI running".into()
                };
                (text, Tone::Info, true, None)
            }
            "success" => {
                let text = if include_counts
                    && let Some(c) = checks
                    && c.total > 0
                {
                    format!("CI passed ({}/{})", c.passed, c.total)
                } else {
                    "CI passed".into()
                };
                (text, Tone::Success, false, Some(Icon::Check))
            }
            _ => {
                let text = if let Some(c) = checks
                    && c.total > 0
                    && c.failed > 0
                {
                    format!("CI failed ({}/{})", c.failed, c.total)
                } else {
                    "CI failed".into()
                };
                (text, Tone::Danger, false, Some(Icon::Close))
            }
        })
    }

    pub fn ci_tooltip(&self) -> String {
        let mut lines = Vec::new();
        match self.normalized_ci_state() {
            Some("running") => {
                if let Some(c) = &self.checks
                    && c.total > 0
                {
                    lines.push(format!(
                        "CI running on PR #{} · {}/{} passed, {} running",
                        self.number, c.passed, c.total, c.running
                    ));
                } else {
                    lines.push(format!("CI running on PR #{}", self.number));
                }
            }
            Some("success") => {
                if let Some(c) = &self.checks
                    && c.total > 0
                {
                    lines.push(format!(
                        "CI passed on PR #{} · {}/{} checks passed",
                        self.number, c.passed, c.total
                    ));
                } else {
                    lines.push(format!("CI passed on PR #{}", self.number));
                }
            }
            Some("failure") => {
                if let Some(c) = &self.checks
                    && c.total > 0
                {
                    lines.push(format!(
                        "CI failed on PR #{} · {}/{} failed",
                        self.number, c.failed, c.total
                    ));
                    if !c.failed_checks.is_empty() {
                        let names: Vec<&str> = c
                            .failed_checks
                            .iter()
                            .take(5)
                            .map(|f| f.name.as_str())
                            .collect();
                        lines.push(format!("Failed: {}", names.join(", ")));
                    }
                } else {
                    lines.push(format!("CI failed on PR #{}", self.number));
                }
            }
            _ => lines.push(format!("PR #{} · {}", self.number, self.pr_state)),
        }
        lines.push("Click to open CI checks on GitHub · Right-click for options".into());
        lines.join("\n")
    }
}

pub fn from_card(
    snapshot: &Snapshot,
    idx: &crate::index::SnapshotIndex,
    card: &Card,
) -> Option<CardPrView> {
    if let Some(pr) = card
        .pull_request
        .as_ref()
        .filter(|p| p.number > 0 && !p.url.is_empty())
    {
        return Some(CardPrView {
            card_id: card.id.clone(),
            repository: pr.repository.clone(),
            number: pr.number,
            pr_url: pr.url.clone(),
            head_sha: pr.head.clone(),
            merged: pr.merged,
            pr_state: if pr.merged {
                "merged".into()
            } else {
                pr.state.clone()
            },
            ci_state: pr.checks.clone(),
            checks: pr.checks_summary.clone(),
            show_pr_chip: !card.is_github_pr(),
        });
    }
    if card.is_github_pr()
        && let Some(src) = card.github_issue()
    {
        let cached = idx.github_issue(&card.project_id, src.repository_id, src.issue_id);
        let (pr_state, merged, head_sha, checks) = if let Some(g) = cached {
            (
                g.state.clone(),
                g.state == "merged" || card.delivery.is_some(),
                g.pr_head_sha.clone().unwrap_or_default(),
                g.pr_checks.clone(),
            )
        } else {
            (
                if card.delivery.is_some() {
                    "merged".into()
                } else {
                    "open".into()
                },
                card.delivery.is_some(),
                String::new(),
                snapshot
                    .github_issues
                    .iter()
                    .find(|g| {
                        g.project_id == card.project_id
                            && g.source.repository_id == src.repository_id
                            && g.source.issue_id == src.issue_id
                    })
                    .and_then(|g| g.pr_checks.clone()),
            )
        };
        return Some(CardPrView {
            card_id: card.id.clone(),
            repository: src.repository.clone(),
            number: src.number,
            pr_url: src.url.clone(),
            head_sha,
            merged,
            pr_state,
            ci_state: checks.as_ref().map(|c| c.state.clone()).unwrap_or_default(),
            checks,
            show_pr_chip: false,
        });
    }
    None
}

fn open_context_menu(ui: Ui, view_model: &CardPrView, at: Point<Points>) {
    use nsh_core::element::desktop::{MenuItem, popup_menu_at};
    let pr_url = view_model.pr_url.clone();
    let checks_url = view_model.checks_overview_url();
    let repo = view_model.repository.clone();
    let number = view_model.number;
    let card_id = view_model.card_id.clone();
    let failed_checks = view_model
        .checks
        .as_ref()
        .map(|c| c.failed_checks.clone())
        .unwrap_or_default();
    let can_open_in_app = !repo.is_empty() && number > 0;
    let mut items = Vec::new();
    if !pr_url.is_empty() {
        let url = pr_url.clone();
        items.push(
            MenuItem::new(format!("Open PR #{number} on GitHub ↗"))
                .on_select(move || crate::github::open_url(ui, url.clone())),
        );
    }
    if let Some(url) = checks_url {
        items.push(
            MenuItem::new("Open CI Checks on GitHub ↗")
                .on_select(move || crate::github::open_url(ui, url.clone())),
        );
    }
    for check in failed_checks.into_iter().take(5) {
        if let Some(url) = check.url {
            items.push(
                MenuItem::new(format!("Open failed check: {} ↗", check.name))
                    .on_select(move || crate::github::open_url(ui, url.clone())),
            );
        }
    }
    if can_open_in_app {
        let gh_ref = GitHubReference {
            repository: repo,
            kind: "pr".into(),
            number: Some(number),
            sha: None,
            url: pr_url,
        };
        items.push(MenuItem::separator());
        items.push(MenuItem::new("View Checks in Canban").on_select(move || {
            crate::github::open_section(ui, gh_ref.clone(), "checks");
        }));
    }
    if !card_id.is_empty() {
        items.push(MenuItem::separator());
        items.push(MenuItem::new("Refresh PR & CI").on_select(move || {
            dispatch(
                ui,
                "github.pr.refresh",
                serde_json::json!({ "card_id": card_id }),
            );
        }));
    }
    if items.is_empty() {
        return;
    }
    ui.spawn(async move {
        popup_menu_at(items, at).await;
    });
}

/// Renders compact or header-sized PR + CI status pills with direct clicks to GitHub PR and CI Checks.
pub fn pill(ui: Ui, info: &CardPrView, compact: bool) -> Option<View> {
    let ci_badge = info.ci_label(!compact);
    if !info.show_pr_chip && ci_badge.is_none() {
        return None;
    }
    let menu_model = info.clone();
    let mut row = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_1()
        .min_w(px(0.))
        .on_context_menu(move |event: &nsh_core::ContextMenuEvent| {
            open_context_menu(ui, &menu_model, event.page_position);
        });

    if info.show_pr_chip {
        let pr_url = info.pr_url.clone();
        let pr_num = info.number;
        let pr_tone = if info.merged {
            Tone::Highlight
        } else {
            Tone::Neutral
        };
        let label_text = if compact {
            format!("PR #{pr_num}")
        } else if info.merged {
            format!("PR #{pr_num} · merged ↗")
        } else {
            format!("PR #{pr_num} ↗")
        };
        let tip = format!(
            "PR #{pr_num} ({})\nClick to open PR on GitHub · Right-click for options",
            info.pr_state
        );
        let a11y = format!("Open PR #{pr_num} on GitHub");
        row = row.child(
            view().flex_shrink_0().tooltip(hint_tooltip(tip)).child(
                status_button(
                    label_text,
                    pr_tone,
                    false,
                    Some(Icon::GitHub),
                    compact,
                    move |_| crate::github::open_url(ui, pr_url.clone()),
                )
                .accessibility_label(move || Some(a11y.clone().into())),
            ),
        );
    }

    if let Some((text, tone, live, leading)) = ci_badge {
        let jump_url = info.checks_url().unwrap_or_else(|| info.pr_url.clone());
        let label_text = if compact { text } else { format!("{text} ↗") };
        let tip = info.ci_tooltip();
        let a11y = format!("{} · Open CI checks on GitHub", label_text);
        row = row.child(
            view().flex_shrink_0().tooltip(hint_tooltip(tip)).child(
                status_button(label_text, tone, live, leading, compact, move |_| {
                    crate::github::open_url(ui, jump_url.clone())
                })
                .accessibility_label(move || Some(a11y.clone().into())),
            ),
        );
    }

    Some(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canban_types::{GitHubPr, GitHubPrCheck, WorkspaceBinding};

    #[test]
    fn pr_view_normalizes_ci_states_and_prefers_single_failed_check_url() {
        let mut s = Snapshot::default();
        let mut card: Card = serde_json::from_value(serde_json::json!({
            "id": "c1",
            "project_id": "p1",
            "number": 143,
            "title": "PR tracking",
            "body": "",
            "stage": "develop",
            "revision": 1,
            "created_at": 1
        }))
        .unwrap();
        card.pull_request = Some(GitHubPr {
            review_summary: String::new(),
            repository: "owner/repo".into(),
            repository_id: 1,
            number: 148,
            url: "https://github.com/owner/repo/pull/148".into(),
            state: "open".into(),
            head: "abc".into(),
            head_branch: "canban-0afd/143".into(),
            base_branch: "main".into(),
            merged: false,
            checks: "failure".into(),
            checks_summary: Some(GitHubPrChecks {
                state: "failure".into(),
                head_sha: "abc".into(),
                url: Some("https://github.com/owner/repo/pull/148/checks".into()),
                total: 4,
                passed: 3,
                failed: 1,
                running: 0,
                failed_checks: vec![GitHubPrCheck {
                    name: "clippy".into(),
                    status: "completed".into(),
                    conclusion: Some("failure".into()),
                    url: Some("https://github.com/owner/repo/actions/runs/1/job/2".into()),
                }],
            }),
            workspace: WorkspaceBinding {
                directory: "/tmp".into(),
                path: "/tmp/wt".into(),
                branch: Some("canban-0afd/143".into()),
                base_commit: None,
                target_branch: Some("main".into()),
            },
        });
        s.cards.push(card.clone());
        let idx = crate::index::SnapshotIndex::build(&s);
        let view = from_card(&s, &idx, &card).unwrap();
        assert_eq!(view.normalized_ci_state(), Some("failure"));
        assert_eq!(
            view.checks_url().as_deref(),
            Some("https://github.com/owner/repo/actions/runs/1/job/2")
        );
        assert_eq!(
            view.checks_overview_url().as_deref(),
            Some("https://github.com/owner/repo/pull/148/checks")
        );
        let (label, tone, live, _) = view.ci_label(true).unwrap();
        assert_eq!(label, "CI failed (1/4)");
        assert!(matches!(tone, Tone::Danger));
        assert!(!live);
    }
}
