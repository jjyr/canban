//! Search dialog adapter: NSH Command owns the candidate list, pointer selection,
//! accessibility and scrolling. A separate controlled input makes Escape close
//! immediately (Command's built-in search clears a nonempty query first).
use super::{
    field,
    icons::{Icon, icon},
    label, muted,
    tokens::*,
};
use nsh_core::{keymap::When, prelude::*};
use std::rc::Rc;
#[derive(Clone, Debug, PartialEq)]
pub struct SearchChoice {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub icon: Icon,
    pub selected: bool,
    pub attention: usize,
}
pub fn search_dialog(
    title: impl Fn() -> String + Copy + 'static,
    query: Signal<String>,
    choices: Memo<Vec<SearchChoice>>,
    select: impl Fn(String) + 'static,
    close: impl Fn() + Copy + 'static,
    placeholder: impl Fn() -> String + 'static,
    footer: impl IntoElement,
) -> impl IntoElement {
    // Escape belongs to the mounted modal, not just its input. Mode changes,
    // list selection and the footer can all transfer focus within the dialog.
    on_key_with(
        "escape",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| close(),
    );
    let select = Rc::new(select);
    let highlighted = signal(None::<String>);
    let input_focused = signal(false);
    let command = component_ref::<nsh_base::Command>();
    effect(move || {
        let rows = choices.get();
        let _ = query.get();
        if let Some(first) = rows.first()
            && let Some(api) = command.api::<nsh_base::CommandApi>()
        {
            api.highlight(first.id.clone());
        }
    });
    let submit = {
        let select = select.clone();
        move || {
            let id = highlighted
                .get_untracked()
                .filter(|id| choices.with_untracked(|cs| cs.iter().any(|c| &c.id == id)))
                .or_else(|| choices.with_untracked(|cs| cs.first().map(|c| c.id.clone())));
            if let Some(id) = id {
                select(id);
            }
        }
    };
    let keyboard_submit = submit.clone();
    effect(move || {
        if !input_focused.get() {
            return;
        }
        let submit = keyboard_submit.clone();
        on_key_with(
            "enter",
            KeyOptions {
                when: When::Always,
                ..Default::default()
            },
            move |_| submit(),
        );
        for (key, forward) in [("down", true), ("up", false)] {
            on_key_with(
                key,
                KeyOptions {
                    when: When::Always,
                    repeat: true,
                    ..Default::default()
                },
                move |_| {
                    let rows = choices.get_untracked();
                    if rows.is_empty() {
                        return;
                    }
                    let current = highlighted
                        .get_untracked()
                        .and_then(|id| rows.iter().position(|r| r.id == id))
                        .unwrap_or(0);
                    let next = if forward {
                        (current + 1) % rows.len()
                    } else {
                        (current + rows.len() - 1) % rows.len()
                    };
                    if let Some(api) = command.api::<nsh_base::CommandApi>() {
                        api.highlight(rows[next].id.clone());
                    }
                },
            );
        }
    });
    nsh_base::dialog()
        .animation(ModalAnimation::None)
        .close_on_escape(true)
        .close_on_backdrop_press(true)
        .on_cancel(close)
        .child(
            view()
                .flex_1()
                .items_center()
                .justify_start()
                .pt(px(72.))
                .px_4()
                .bg(rgba(0x00000055))
                .child(
                    view()
                        .absolute()
                        .inset_0()
                        .on_press(close)
                        .label("Dismiss search"),
                )
                .child(
                    view()
                        .w_full()
                        .max_w(px(640.))
                        .max_h(Percentage(0.78))
                        .bg(SURFACE)
                        .rounded(px(12.))
                        .border_1()
                        .border_color(BORDER)
                        .overflow_hidden()
                        .child(
                            view()
                                .px_4()
                                .py_3()
                                .flex_row()
                                .justify_between()
                                .child(label(title))
                                .child(muted("Esc to close")),
                        )
                        .child(
                            view().px_4().pb_3().child(
                                field(query, "Search projects, cards, notes, agents…")
                                    .placeholder(placeholder)
                                    .label("Quick Open search")
                                    .auto_focus(true)
                                    .on_focus(move || input_focused.set(true))
                                    .on_blur(move || input_focused.set(false))
                                    .on_submit(move |_: &str| submit()),
                            ),
                        )
                        .child(
                            nsh_base::command()
                                .component_ref(&command)
                                .items(move || {
                                    choices
                                        .get()
                                        .iter()
                                        .map(|c| {
                                            nsh_base::CommandItem::new(
                                                c.id.clone(),
                                                format!(
                                                    "{}{}{}{}",
                                                    c.title,
                                                    if c.detail.is_empty() {
                                                        String::new()
                                                    } else {
                                                        format!(" · {}", c.detail)
                                                    },
                                                    if c.attention == 0 {
                                                        String::new()
                                                    } else {
                                                        format!(" · {} need attention", c.attention)
                                                    },
                                                    if c.selected { " · Current" } else { "" }
                                                ),
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                })
                                .searchable(false)
                                .filterable(false)
                                .accessibility_label(move || Some(title().into()))
                                .item_class("group")
                                .list_class("px-2 py-2")
                                .render_item(move |e: &nsh_base::CommandEntry| {
                                    let c = choices.with_untracked(|cs| {
                                        cs.iter().find(|c| c.id == e.id.as_str()).cloned()
                                    });
                                    c.map(|c| {
                                        let id = c.id.clone();
                                        let mut body = view()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .gap_1()
                                            .child(label(c.title));
                                        if !c.detail.is_empty() {
                                            body = body.child(muted(c.detail));
                                        }
                                        view()
                                            .rounded(px(RADIUS))
                                            .px_3()
                                            .py_2()
                                            .bg(move || {
                                                if highlighted.get().as_ref() == Some(&id) {
                                                    SELECTED
                                                } else {
                                                    CLEAR
                                                }
                                            })
                                            .flex_row()
                                            .items_center()
                                            .gap_3()
                                            .child(icon(c.icon))
                                            .child(body)
                                            .child(super::attention_count(move || c.attention))
                                            .child(when(move || c.selected, || icon(Icon::Check)))
                                            .into_element()
                                    })
                                    .unwrap_or_else(|| view().into_element())
                                })
                                .on_highlight_change(move |id: &Option<SharedString>| {
                                    highlighted.set(id.as_ref().map(ToString::to_string))
                                })
                                .empty(view().p_4().child(muted("No matching destinations")))
                                .on_select(move |id: &SharedString| select(id.to_string()))
                                .on_cancel(close)
                                .h(px(300.)),
                        )
                        .child(
                            view()
                                .border_t_1()
                                .border_color(BORDER)
                                .px_4()
                                .py_2()
                                .child(footer),
                        ),
                ),
        )
}
