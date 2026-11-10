//! Global Project -> archived Card conversations. Search is local/debounced;
//! opening history does not launch Agents or restore archived work.
pub mod model;
use crate::{
    drafts::{capture_scroll, scroll_position},
    presenter::*,
};
use canban_types::Card;
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;
use serde_json::json;

pub fn screen(ui: Ui) -> impl IntoElement {
    let query = input::debounced(ui.archive_query, std::time::Duration::from_millis(120));
    let removed = signal(Vec::<Card>::new());
    let error = signal(String::new());
    let epoch = signal(0u64);
    let reload = signal(0u64);
    let lifecycle = computed(move || {
        ui.snapshot.with(|s| {
            s.cards
                .iter()
                .map(|c| (c.id.clone(), c.archived_at, c.removed_at))
                .collect::<Vec<_>>()
        })
    });
    effect(move || {
        let _ = lifecycle.get();
        let _ = reload.get();
        let Some(app) = crate::APP.get().cloned() else {
            removed.set(Vec::new());
            return;
        };
        let ticket = epoch.get_untracked() + 1;
        epoch.set(ticket);
        ui.spawn(async move {
            let result = blocking(move || {
                app.call("card.list", json!({"include_removed":true}))
                    .and_then(|v| Ok(serde_json::from_value::<Vec<Card>>(v)?))
            })
            .await;
            if epoch.try_get() != Some(ticket) {
                return;
            }
            match result {
                Ok(Ok(cards)) => {
                    removed.set(
                        cards
                            .into_iter()
                            .filter(|c| c.removed_at.is_some())
                            .collect(),
                    );
                    error.set(String::new());
                }
                other => error.set(format!("Could not load deleted Cards: {other:?}")),
            }
        });
    });
    let archived = computed(move || {
        model::groups(&ui.snapshot.get(), &[], &query.get(), model::Kind::Archived)
    });
    let deleted = computed(move || {
        model::groups(
            &ui.snapshot.get(),
            &removed.get(),
            &query.get(),
            model::Kind::Deleted,
        )
    });
    let searching = computed(move || !query.get().trim().is_empty());
    let search_folds = signal(std::collections::BTreeSet::<String>::new());
    let search_deleted = signal(true);
    effect(move || {
        let _ = query.get();
        search_folds.set(Default::default());
        search_deleted.set(true);
    });
    let deleted_open = computed(move || {
        if searching.get() {
            search_deleted.get()
        } else {
            ui.archive_deleted.get()
        }
    });
    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .p_5()
        .gap_4()
        .child(heading("Archive"))
        .child(
            view()
                .flex_row()
                .w_full()
                .max_w(px(1000.))
                .gap_2()
                .items_center()
                .min_w(px(0.))
                .child(
                    field(
                        ui.archive_query,
                        "Search projects, Cards, numbers or Agents…",
                    )
                    .label("Search archive")
                    .flex_1()
                    .min_w(px(0.)),
                )
                .child(when(
                    move || !ui.archive_query.get().is_empty(),
                    move || {
                        chrome_button(Icon::Close, "Clear archive search", move |_| {
                            ui.archive_query.set(String::new())
                        })
                    },
                )),
        )
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .content_offset(scroll_position(ui, "archive:list"))
                .on_scroll(move |e: &ScrollEvent| capture_scroll(ui, "archive:list".into(), e))
                .child(
                    view()
                        .w_full()
                        .max_w(px(1000.))
                        .min_w(px(0.))
                        .gap_3()
                        .child(when(
                            move || archived.get().is_empty() && deleted.get().is_empty(),
                            move || {
                                view()
                                    .py_5()
                                    .gap_2()
                                    .child(label(move || {
                                        if searching.get() {
                                            "No matching archived conversations"
                                        } else {
                                            "No archived conversations"
                                        }
                                    }))
                                    .child(muted(move || {
                                        if searching.get() {
                                            "Try a Project name, Card number or Agent."
                                        } else {
                                            "Archived Cards will appear here, grouped by Project."
                                        }
                                    }))
                            },
                        ))
                        .child(groups(ui, archived, searching, search_folds, "archive"))
                        .child(when(
                            move || !deleted.get().is_empty(),
                            move || {
                                tree_group(
                                    || "Deleted Cards".into(),
                                    move || deleted.get().iter().map(|g| g.entries.len()).sum(),
                                    deleted_open,
                                    move || {
                                        if searching.get_untracked() {
                                            search_deleted.update(|v| *v = !*v)
                                        } else {
                                            ui.archive_deleted.update(|v| *v = !*v)
                                        }
                                    },
                                    groups(ui, deleted, searching, search_folds, "deleted"),
                                )
                            },
                        ))
                        .child(when(
                            move || !error.get().is_empty(),
                            move || {
                                view()
                                    .gap_2()
                                    .child(notice(move || error.get(), Tone::Attention))
                                    .child(action("Retry", Variant::Quiet, move |_| {
                                        reload.update(|n| *n += 1)
                                    }))
                            },
                        )),
                ),
        )
}
fn groups(
    ui: Ui,
    groups: Memo<Vec<model::Group>>,
    searching: Memo<bool>,
    search_folds: Signal<std::collections::BTreeSet<String>>,
    scope: &'static str,
) -> impl IntoElement {
    each(
        move || groups.get(),
        |g| g.id.clone(),
        move |group, _| {
            let key = format!("{scope}:{}", group.get_untracked().id);
            let folded_key = key.clone();
            let open = computed(move || {
                !(if searching.get() {
                    search_folds
                } else {
                    ui.archive_folds
                })
                .with(|f| f.contains(&folded_key))
            });
            tree_group(
                move || group.get().name,
                move || group.get().entries.len(),
                open,
                move || {
                    (if searching.get_untracked() {
                        search_folds
                    } else {
                        ui.archive_folds
                    })
                    .update(|folds| {
                        if !folds.remove(&key) {
                            folds.insert(key.clone());
                        }
                    })
                },
                each(
                    move || group.get().entries,
                    |e| e.card.id.clone(),
                    move |entry, _| dynamic(move || row(ui, entry.get())),
                ),
            )
        },
    )
}
fn row(ui: Ui, entry: model::Entry) -> impl IntoElement {
    let card = entry.card;
    let archive_error = card.archive_error.clone();
    let id = card.id.clone();
    let title = format!("#{} {}", card.number, card.title);
    let timestamp = card
        .removed_at
        .or(card.archived_at)
        .unwrap_or(card.created_at);
    let state = if card.removed_at.is_some() {
        "Deleted"
    } else {
        "Archived"
    };
    let agents = entry.agents.join(" · ");
    let meta = format!(
        "{state} {}{}{}",
        crate::components::activity::age(timestamp),
        if agents.is_empty() { "" } else { " · " },
        agents
    );
    let content = view()
        .min_w(px(0.))
        .gap_1()
        .child(
            label(title.clone())
                .font_weight(FontWeight::MEDIUM)
                .truncate()
                .min_w(px(0.)),
        )
        .child(muted(meta).truncate().min_w(px(0.)));
    let main = if card.removed_at.is_some() {
        view()
            .flex_1()
            .min_w(px(0.))
            .py_3()
            .px_3()
            .child(content)
            .into_element()
    } else {
        nsh_base::button()
            .flex_1()
            .min_w(px(0.))
            .items_start()
            .py_3()
            .px_3()
            .bg(CLEAR)
            .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
            .accessibility_label(format!("Open archived Card {title}"))
            .child(content)
            .on_press(move |_: &()| ui.open_card(id.clone()))
            .into_element()
    };
    let mut item = view()
        .min_w(px(0.))
        .rounded(px(RADIUS))
        .bg(SURFACE)
        .hover(|s| s.bg(CARD_HOVER))
        .child(
            view()
                .flex_row()
                .gap_2()
                .items_center()
                .min_w(px(0.))
                .child(main)
                .child(
                    view().pr_2().flex_shrink_0().child(
                        action("Restore", Variant::Quiet, move |_| {
                            crate::card_actions::apply(
                                ui,
                                card.clone(),
                                crate::card_actions::Action::Restore,
                            )
                        })
                        .disabled(move || ui.pending.get()),
                    ),
                ),
        );
    if let Some(error) = archive_error {
        item = item.child(notice(error, Tone::Attention));
    }
    item
}
