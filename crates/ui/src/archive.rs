//! Global Project -> archived Card conversations. Search is local/debounced;
//! opening history does not launch Agents or restore archived work.
pub mod model;
use crate::{
    drafts::{capture_scroll, scroll_position},
    presenter::*,
};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;

pub fn screen(ui: Ui) -> impl IntoElement {
    let query = input::debounced(ui.archive_query, std::time::Duration::from_millis(120));
    let archived = computed(move || model::groups(&ui.snapshot.get(), &query.get()));
    let searching = computed(move || !query.get().trim().is_empty());
    let search_folds = signal(std::collections::BTreeSet::<String>::new());
    effect(move || {
        let _ = query.get();
        search_folds.set(Default::default());
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
                            move || archived.get().is_empty(),
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
                        .child(groups(ui, archived, searching, search_folds, "archive")),
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
    let timestamp = card.archived_at.unwrap_or(card.created_at);
    let agents = entry.agents.join(" · ");
    let meta = format!(
        "Archived {}{}{}",
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
    let main = nsh_base::button()
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
        .into_element();
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
