//! Board composition; stage scroll positions survive focused work and refreshes.
use crate::{components::*, drafts::*, presenter::*};
use canban_types::*;
use canban_ui_kit::*;
use nsh_core::prelude::*;

pub fn activity(s: &Snapshot, c: &Card) -> (String, Tone) {
    if c.human_gate.is_some() {
        return ("Waiting for human".into(), Tone::Attention);
    }
    if c.attention.is_some() {
        return ("Needs attention".into(), Tone::Attention);
    }
    if let Some(r) = s.requests.iter().rev().find(|r| {
        r.card_id == c.id
            && matches!(
                r.state.as_str(),
                "queued" | "starting" | "running" | "working" | "blocked"
            )
    }) {
        return (
            format!(
                "@{} · {}",
                r.profile.id,
                if r.state == "queued" {
                    "queued · waiting for a slot"
                } else {
                    &r.state
                }
            ),
            if r.state == "blocked" {
                Tone::Attention
            } else {
                Tone::Info
            },
        );
    }
    if let Some(r) = s.requests.iter().rev().find(|r| r.card_id == c.id) {
        return (
            format!("@{} · {}", r.profile.id, r.state),
            if r.state == "failed" {
                Tone::Danger
            } else {
                Tone::Neutral
            },
        );
    }
    ("Not scheduled".into(), Tone::Neutral)
}
pub fn matches_filter(s: &Snapshot, c: &Card, filter: &str) -> bool {
    match filter {
        "attention" => c.attention.is_some(),
        "running" => s.requests.iter().any(|r| {
            r.card_id == c.id
                && matches!(
                    r.state.as_str(),
                    "queued" | "starting" | "running" | "working"
                )
        }),
        "review" => c.stage == "review",
        _ => true,
    }
}
pub fn screen(ui: Ui) -> impl IntoElement {
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .p_4()
        .gap_4()
        .child(
            view()
                .flex_row()
                .justify_between()
                .items_center()
                .gap_2()
                .child(heading(move || {
                    ui.current_project()
                        .map(|p| p.name)
                        .unwrap_or_else(|| "Welcome to Canban".into())
                }))
                .child(action("+ New Card", Variant::Primary, move |_| {
                    ui.source_note.set(String::new());
                    ui.page.set(if ui.project.get_untracked().is_empty() {
                        "new-project".into()
                    } else {
                        "new-card".into()
                    });
                })),
        )
        .child(crate::components::scheduling::summary(ui))
        .child(
            view().flex_row().flex_wrap().gap_2().children(
                [
                    ("All", "all"),
                    ("Needs attention", "attention"),
                    ("Running", "running"),
                    ("Review", "review"),
                ]
                .into_iter()
                .map(|(name, key)| {
                    navigation(
                        name.into(),
                        move || ui.board_filter.get() == key,
                        move |_| ui.board_filter.set(key.into()),
                    )
                    .into_element()
                })
                .collect::<Vec<_>>(),
            ),
        )
        .child(dynamic(move || {
            let s = ui.snapshot.get();
            let filter = ui.board_filter.get();
            let project = ui.project.get();
            let Some(p) = s.projects.iter().find(|p| p.id == project) else {
                return empty(
                    "Create a Project",
                    "Each Project has a Board, Notes and a source directory.",
                )
                .into_element();
            };
            let stages = p
                .workflow
                .stages
                .iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>();
            let width = signal(240.0);
            let count = stages.len();
            scroll()
                .horizontal()
                .flex_1()
                .min_h(px(0.0))
                .child(
                    view()
                        .flex_row()
                        .gap_3()
                        .min_h(px(0.0))
                        .flex_1()
                        .on_layout(move |b: &Bounds<Points>| {
                            width.set(
                                (b.size.width.0 - 12.0 * (count.saturating_sub(1)) as f32)
                                    / (count.max(1) as f32),
                            )
                        })
                        .children(
                            p.workflow
                                .stages
                                .iter()
                                .enumerate()
                                .map(|(i, stage)| {
                                    let cards = s
                                        .cards
                                        .iter()
                                        .filter(|c| {
                                            c.project_id == project
                                                && c.stage == stage.id
                                                && c.archived_at.is_none()
                                                && matches_filter(&s, c, &filter)
                                        })
                                        .cloned()
                                        .collect::<Vec<_>>();
                                    let key = format!("board:{project}:{}", stage.id);
                                    let position = scroll_position(ui, &key);
                                    let mut column = view()
                                        .w(px(240.0))
                                        .flex_1()
                                        .min_w(px(200.0))
                                        .min_h(px(0.0))
                                        .gap_3()
                                        .child(
                                            view()
                                                .flex_row()
                                                .justify_between()
                                                .items_center()
                                                .child(
                                                    label(stage.name.clone())
                                                        .font_weight(FontWeight::SEMIBOLD),
                                                )
                                                .child(badge(cards.len().to_string())),
                                        )
                                        .child(muted(
                                            stage
                                                .profile
                                                .as_ref()
                                                .map(|p| format!("@{p}"))
                                                .unwrap_or_else(|| {
                                                    if stage.execute {
                                                        "Tags / default Agent".into()
                                                    } else {
                                                        "Ideas and planned work".into()
                                                    }
                                                }),
                                        ));
                                    let mut items = view().gap_3();
                                    if cards.is_empty() {
                                        items = items.child(muted("No cards"));
                                    }
                                    items = items.children(
                                        cards
                                            .into_iter()
                                            .map(|c| {
                                                card_tile(ui, c, i, width, stages.clone())
                                                    .into_element()
                                            })
                                            .collect::<Vec<_>>(),
                                    );
                                    column = column.child(
                                        scroll()
                                            .key(key.clone())
                                            .content_offset(position)
                                            .on_scroll(move |e: &ScrollEvent| {
                                                capture_scroll(ui, key.clone(), e)
                                            })
                                            .flex_1()
                                            .child(items),
                                    );
                                    column.into_element()
                                })
                                .collect::<Vec<_>>(),
                        ),
                )
                .into_element()
        }))
}
