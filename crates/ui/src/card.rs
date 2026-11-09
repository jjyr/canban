//! Compact Card reader: two-row header, scrolling context/conversation, fixed composer.
mod composer;
mod details;
mod header;
mod task;
mod timeline;
use crate::{drafts::*, presenter::*};
use canban_ui_kit::{tokens::*, *};
use nsh_core::prelude::*;

#[derive(Clone, Copy)]
pub(super) struct Reading {
    scroll: ElementRef,
    follow: Signal<bool>,
    unread: Signal<usize>,
}
impl Reading {
    fn latest(self) {
        if self.follow.try_get().is_none() {
            return;
        }
        self.follow.set(true);
        self.unread.set(0);
        self.scroll.scroll_to_end(false);
    }
    fn top(self) {
        self.follow.set(false);
        self.scroll.scroll_to(Point::default(), false);
    }
}
pub fn screen(ui: Ui) -> impl IntoElement {
    let Some(card) = ui.current_card() else {
        return view().child(muted("Select a Card")).into_element();
    };
    let id = card.id.clone();
    let key = format!("card:{id}");
    let scroll_key = format!("card:{id}:content");
    let saved = scroll_position(ui, &scroll_key);
    let reading = Reading {
        scroll: element_ref(),
        follow: signal(!ui.scrolls.with(|m| m.contains_key(&scroll_key))),
        unread: signal(0),
    };
    let draft = document(ui, key, Draft::card(&card));
    effect(move || {
        if let Some(card) = ui.current_card() {
            let mut value = draft.get_untracked();
            value.reconcile(Draft::card(&card));
            if value != draft.get_untracked() {
                draft.set(value);
            }
        }
    });
    let last_event = stored(ui.snapshot.with_untracked(|s| {
        s.events
            .iter()
            .filter(|e| e.card_id == id)
            .map(|e| e.sequence)
            .max()
            .unwrap_or(0)
    }));
    let event_card = card.id.clone();
    effect(move || {
        let last = ui.snapshot.with(|s| {
            s.events
                .iter()
                .filter(|e| e.card_id == event_card)
                .map(|e| e.sequence)
                .max()
                .unwrap_or(0)
        });
        let before = last_event.replace(last);
        if last > before && !reading.follow.get_untracked() {
            reading.unread.update(|n| *n += 1);
        }
    });
    view()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .bg(SURFACE)
        .child(header::render(ui, draft, reading))
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.))
                .element_ref(&reading.scroll)
                .content_offset(saved)
                .on_scroll(move |e: &ScrollEvent| {
                    capture_scroll(ui, scroll_key.clone(), e);
                    let bottom = e.content_size.height.0 - e.offset.y.0 - e.size.height.0 <= 48.;
                    reading.follow.set(bottom);
                    if bottom {
                        reading.unread.set(0);
                    }
                })
                .child(
                    view()
                        .p_3()
                        .gap_3()
                        .on_layout(move |_: &Bounds<Points>| {
                            if reading.follow.get_untracked() {
                                reading.scroll.scroll_to_end(false);
                            }
                        })
                        .child(task::render(ui, draft, reading))
                        .child(details::render(ui, draft))
                        .child(when(
                            move || draft.with(|d| d.workspace_details),
                            move || {
                                view()
                                    .gap_2()
                                    .child(action(
                                        "Hide workspace details",
                                        Variant::Quiet,
                                        move |_| draft.update(|d| d.workspace_details = false),
                                    ))
                                    .child(crate::components::git_workspace::panel(ui))
                            },
                        ))
                        .child(timeline::render(ui, reading)),
                ),
        )
        .child(when(
            move || reading.unread.get() > 0,
            move || {
                action(
                    move || format!("New activity ({}) ↓", reading.unread.get()),
                    Variant::Quiet,
                    move |_| reading.latest(),
                )
                .self_center()
            },
        ))
        .child(composer::render(ui, draft, reading))
        .into_element()
}

pub(super) fn proxy(draft: Signal<Draft>, comment: bool) -> Signal<String> {
    let text = signal(if comment {
        draft.get_untracked().comment
    } else {
        draft.get_untracked().body
    });
    effect(move || {
        let value = text.get();
        if comment {
            if value != draft.get_untracked().comment {
                draft.update(|d| d.comment = value);
            }
        } else if value != draft.get_untracked().body {
            draft.update(|d| d.body = value);
        }
    });
    effect(move || {
        let value = if comment {
            draft.get().comment
        } else {
            draft.get().body
        };
        if text.get_untracked() != value {
            text.set(value);
        }
    });
    text
}
