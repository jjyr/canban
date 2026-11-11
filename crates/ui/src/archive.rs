//! Project-scoped archived Card conversations. Search is local/debounced;
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
    let archived =
        computed(move || model::project_entries(&ui.snapshot.get(), &ui.project.get(), &query.get()));
    let searching = computed(move || !query.get().trim().is_empty());
    let scroll_key = format!("archive:list:{}", ui.project.get_untracked());
    let scroll_capture_key = scroll_key.clone();
    view()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .p_5()
        .gap_4()
        .child(subpage_header(
            "Back to Board",
            move || {
                ui.current_project()
                    .map(|p| format!("{} / Archive", p.name))
                    .unwrap_or_else(|| "Archive".into())
            },
            move |_| ui.navigate("board"),
        ))
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
                        "Search archived Cards, numbers or Agents…",
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
                .content_offset(scroll_position(ui, &scroll_key))
                .on_scroll(move |e: &ScrollEvent| {
                    capture_scroll(ui, scroll_capture_key.clone(), e)
                })
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
                                            "Try a Card title, number or Agent."
                                        } else {
                                            "Archived Cards for this Project will appear here."
                                        }
                                    }))
                            },
                        ))
                        .child(each(
                            move || archived.get(),
                            |e| e.card.id.clone(),
                            move |entry, _| dynamic(move || row(ui, entry.get())),
                        )),
                ),
        )
}
fn row(ui: Ui, entry: model::Entry) -> impl IntoElement {
    let card = entry.card;
    let archive_error = card.archive_error.clone();
    let merged = card.delivery.is_some() || card.pull_request.as_ref().is_some_and(|p| p.merged);
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
    let mut header = view()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .child(
            label(title.clone())
                .font_weight(FontWeight::MEDIUM)
                .truncate()
                .min_w(px(0.)),
        );
    if merged {
        header = header.child(status("Merged", Tone::Success));
    }
    let content = view()
        .min_w(px(0.))
        .gap_1()
        .child(header)
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
