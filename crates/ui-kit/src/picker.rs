//! Anchored searchable popover picker built on `nsh_base::popover()` + `nsh_base::command()`.
//! Supports grouped options, fuzzy multi-term filtering via `canban_ui_kit::search::score`,
//! keyboard navigation (`↑`/`↓`, `Enter`, `Escape`), and optional custom typed values.
use super::{
    field,
    icons::{Icon, icon},
    label, muted,
    tokens::*,
};
use nsh_base::PopoverEvent;
use nsh_core::{keymap::When, prelude::*};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub struct PickerOption {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub group: Option<String>,
    pub icon: Option<Icon>,
    pub mono: bool,
    pub selected: bool,
    pub keywords: Vec<String>,
}

impl PickerOption {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: String::new(),
            group: None,
            icon: None,
            mono: false,
            selected: false,
            keywords: Vec::new(),
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn group(mut self, group: impl Into<String>) -> Self {
        let g = group.into();
        self.group = (!g.is_empty()).then_some(g);
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn keywords(mut self, keywords: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.keywords.extend(keywords.into_iter().map(Into::into));
        self
    }
}

pub type CustomPickerFn = dyn Fn(&str) -> Option<PickerOption>;

/// Filters `options` against `query`, preserving group clustering on empty query
/// and ranking by fuzzy relevance when `query` is non-empty. If `custom` returns
/// an option for a non-empty `query` that is not already present by `id`, appends it.
pub fn filter_picker_options(
    options: &[PickerOption],
    query: &str,
    custom: Option<&CustomPickerFn>,
) -> Vec<PickerOption> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return options.to_vec();
    }

    let mut scored: Vec<(usize, usize, PickerOption)> = options
        .iter()
        .enumerate()
        .filter_map(|(idx, opt)| {
            let mut fields = Vec::with_capacity(3 + opt.keywords.len());
            fields.push(opt.label.clone());
            if !opt.detail.is_empty() {
                fields.push(opt.detail.clone());
            }
            if let Some(g) = &opt.group {
                fields.push(g.clone());
            }
            fields.extend(opt.keywords.iter().cloned());
            super::search::score(&fields, trimmed).map(|s| (s, idx, opt.clone()))
        })
        .collect();

    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    // Remove group headers when sorting across groups by relevance so adjacent
    // items don't produce fragmented duplicate group headers, UNLESS there's only
    // a custom fallback appended at the bottom.
    let mut out: Vec<PickerOption> = scored
        .into_iter()
        .map(|(_, _, mut opt)| {
            opt.group = None;
            opt
        })
        .collect();

    if let Some(make_custom) = custom
        && let Some(mut custom_opt) = make_custom(trimmed)
        && !out.iter().any(|o| o.id == custom_opt.id)
    {
        custom_opt.group = None;
        out.push(custom_opt);
    }

    out
}

pub struct SearchPopover {
    trigger: AnyElement,
    options: Rc<dyn Fn() -> Vec<PickerOption>>,
    on_select: Rc<dyn Fn(String)>,
    placeholder: String,
    empty_text: String,
    custom_option: Option<Rc<CustomPickerFn>>,
    width: f32,
    list_height: f32,
    align: Align,
}

/// Creates an anchored searchable popover picker.
///
/// Pass the `press_target()` directly as `trigger` (do not wrap it in an outer
/// `view()`), because `nsh_base::popover()` attaches its open/close `on_press`
/// handler directly to the `trigger` root element.
pub fn search_popover(
    trigger: impl IntoElement,
    options: impl Fn() -> Vec<PickerOption> + 'static,
    on_select: impl Fn(String) + 'static,
) -> SearchPopover {
    SearchPopover {
        trigger: trigger.into_element(),
        options: Rc::new(options),
        on_select: Rc::new(on_select),
        placeholder: "Search…".into(),
        empty_text: "No matching items".into(),
        custom_option: None,
        width: 300.0,
        list_height: 280.0,
        align: Align::Start,
    }
}

impl SearchPopover {
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn empty_text(mut self, empty_text: impl Into<String>) -> Self {
        self.empty_text = empty_text.into();
        self
    }

    pub fn custom_option(mut self, f: impl Fn(&str) -> Option<PickerOption> + 'static) -> Self {
        self.custom_option = Some(Rc::new(f));
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn list_height(mut self, height: f32) -> Self {
        self.list_height = height;
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }
}

impl IntoElement for SearchPopover {
    fn into_element(self) -> AnyElement {
        let open = signal(false);
        let query = signal(String::new());
        let search_input = element_ref();
        let options_fn = self.options;
        let custom_fn = self.custom_option;
        let on_select = self.on_select;
        let placeholder = self.placeholder;
        let empty_text = self.empty_text;
        let width = self.width;
        let list_height = self.list_height;

        nsh_base::popover()
            .open(move || Some(open.get()))
            .placement(Placement::Bottom)
            .align(self.align)
            .offset(px(4.))
            .margin(px(8.))
            .on_open_change(move |e: &PopoverEvent| {
                if let PopoverEvent::OpenChange(next, _) = e {
                    if *next {
                        query.set(String::new());
                    }
                    open.set(*next);
                }
            })
            .on_open(move || {
                search_input.focus();
            })
            .trigger(self.trigger)
            .child({
                let options_fn = options_fn.clone();
                let custom_fn = custom_fn.clone();
                let on_select = on_select.clone();
                let placeholder = placeholder.clone();
                let empty_text = empty_text.clone();
                let highlighted = signal(None::<String>);
                let input_focused = signal(false);
                let command = component_ref::<nsh_base::Command>();

                let filtered = computed(move || {
                    let all = options_fn();
                    let q = query.get();
                    filter_picker_options(&all, &q, custom_fn.as_deref())
                });

                effect(move || {
                    let rows = filtered.get();
                    let q = query.get();
                    if let Some(api) = command.api::<nsh_base::CommandApi>() {
                        let target = if q.trim().is_empty() {
                            rows.iter().find(|r| r.selected).or_else(|| rows.first())
                        } else {
                            rows.first()
                        };
                        if let Some(item) = target {
                            api.highlight(item.id.clone());
                        }
                    }
                });

                let choose = {
                    let on_select = on_select.clone();
                    move |id: String| {
                        open.set(false);
                        on_select(id);
                    }
                };

                let submit = {
                    let choose = choose.clone();
                    move || {
                        let id = highlighted
                            .get_untracked()
                            .filter(|id| {
                                filtered.with_untracked(|cs| cs.iter().any(|c| &c.id == id))
                            })
                            .or_else(|| {
                                filtered.with_untracked(|cs| cs.first().map(|c| c.id.clone()))
                            });
                        if let Some(id) = id {
                            choose(id);
                        }
                    }
                };

                let kb_submit = submit.clone();
                effect(move || {
                    if !input_focused.get() {
                        return;
                    }
                    let submit = kb_submit.clone();
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
                                let rows = filtered.get_untracked();
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

                view()
                    .w(px(width))
                    .bg(SURFACE)
                    .rounded(px(10.))
                    .border_1()
                    .border_color(BORDER)
                    .shadow_lg()
                    .overflow_hidden()
                    .child(
                        view().p_2().border_b_1().border_color(BORDER).child(
                            field(query, &placeholder)
                                .element_ref(&search_input)
                                .min_h(px(28.))
                                .px_2p5()
                                .py_1()
                                .text_size(px(12.))
                                .label(placeholder.clone())
                                .on_focus(move || input_focused.set(true))
                                .on_blur(move || input_focused.set(false))
                                .on_submit(move |_: &str| submit()),
                        ),
                    )
                    .child(
                        nsh_base::command()
                            .component_ref(&command)
                            .items(move || {
                                filtered
                                    .get()
                                    .iter()
                                    .map(|opt| {
                                        let mut item = nsh_base::CommandItem::new(
                                            opt.id.clone(),
                                            if opt.detail.is_empty() {
                                                opt.label.clone()
                                            } else {
                                                format!("{} · {}", opt.label, opt.detail)
                                            },
                                        )
                                        .checked(opt.selected);
                                        if let Some(g) = &opt.group {
                                            item = item.group(g.clone());
                                        }
                                        item
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .searchable(false)
                            .filterable(false)
                            .accessibility_label(Some(placeholder.into()))
                            .item_class("group")
                            .list_class("px-1.5 py-1.5")
                            .render_group(|g: &nsh_base::CommandGroup| {
                                view()
                                    .px_2()
                                    .pt_1p5()
                                    .pb_0p5()
                                    .child(
                                        muted(g.heading.to_string())
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD),
                                    )
                                    .into_element()
                            })
                            .render_item(move |e: &nsh_base::CommandEntry| {
                                let opt = filtered.with_untracked(|cs| {
                                    cs.iter().find(|c| c.id == e.id.as_str()).cloned()
                                });
                                opt.map(|opt| {
                                    let id = opt.id.clone();
                                    let title_el = if opt.mono {
                                        label(opt.label)
                                            .font_family(MONO_FONT)
                                            .text_size(px(11.5))
                                            .truncate()
                                            .min_w(px(0.))
                                    } else {
                                        label(opt.label).text_size(px(12.)).truncate().min_w(px(0.))
                                    };
                                    let mut row = view()
                                        .cursor_pointer()
                                        .rounded(px(6.))
                                        .px_2()
                                        .py_1p5()
                                        .bg(move || {
                                            if highlighted.get().as_ref() == Some(&id) {
                                                SELECTED
                                            } else {
                                                CLEAR
                                            }
                                        })
                                        .flex_row()
                                        .items_center()
                                        .gap_2();
                                    if let Some(ic) = opt.icon {
                                        row = row.child(icon(ic).w(px(12.)).h(px(12.)));
                                    }
                                    row = row.child(view().flex_1().min_w(px(0.)).child(title_el));
                                    if !opt.detail.is_empty() {
                                        row = row.child(
                                            muted(opt.detail).text_size(px(11.)).flex_shrink_0(),
                                        );
                                    }
                                    if opt.selected {
                                        row = row.child(icon(Icon::Check).w(px(12.)).h(px(12.)));
                                    }
                                    row.into_element()
                                })
                                .unwrap_or_else(|| view().into_element())
                            })
                            .on_highlight_change(move |id: &Option<SharedString>| {
                                highlighted.set(id.as_ref().map(ToString::to_string))
                            })
                            .empty(
                                view()
                                    .p_3()
                                    .items_center()
                                    .child(muted(empty_text).text_size(px(11.5))),
                            )
                            .on_select(move |id: &SharedString| choose(id.to_string()))
                            .on_cancel(move || open.set(false))
                            .h(move || {
                                let rows = filtered.get();
                                if rows.is_empty() {
                                    return px(44.);
                                }
                                let mut groups = 0usize;
                                let mut prev_group: Option<&str> = None;
                                for r in &rows {
                                    if let Some(g) = r.group.as_deref()
                                        && prev_group != Some(g)
                                    {
                                        groups += 1;
                                        prev_group = Some(g);
                                    }
                                }
                                let content_h =
                                    rows.len() as f32 * 31.0 + groups as f32 * 25.0 + 16.0;
                                px(content_h.min(list_height))
                            }),
                    )
            })
            .into_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_options() -> Vec<PickerOption> {
        vec![
            PickerOption::new("main", "main")
                .group("Branches")
                .detail("default")
                .mono(true)
                .selected(true),
            PickerOption::new("feat/git-client", "feat/git-client")
                .group("Branches")
                .detail("↑2")
                .keywords(["#77", "diff", "review"])
                .mono(true),
            PickerOption::new("origin/release", "origin/release")
                .group("Remote")
                .mono(true),
        ]
    }

    #[test]
    fn empty_query_preserves_groups_and_order() {
        let items = sample_options();
        let res = filter_picker_options(&items, "   ", None);
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].group.as_deref(), Some("Branches"));
        assert_eq!(res[2].group.as_deref(), Some("Remote"));
    }

    #[test]
    fn fuzzy_query_matches_keywords_and_ranks_best_match_first() {
        let items = sample_options();
        let res = filter_picker_options(&items, "#77 git", None);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].id, "feat/git-client");
        assert!(res[0].group.is_none());
    }

    #[test]
    fn custom_option_appended_when_not_already_matched_by_id() {
        let items = sample_options();
        let custom = |q: &str| {
            Some(
                PickerOption::new(format!("ref:{q}"), format!("Use ref \"{q}\""))
                    .detail("custom")
                    .mono(true),
            )
        };
        let res = filter_picker_options(&items, "HEAD~3", Some(&custom));
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].id, "ref:HEAD~3");

        let exact = filter_picker_options(
            &items,
            "main",
            Some(&|q: &str| Some(PickerOption::new(q, format!("Use ref \"{q}\"")))),
        );
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].id, "main");
        assert_eq!(exact[0].detail, "default");
    }
}
