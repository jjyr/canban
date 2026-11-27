//! Shared settings section/field geometry, single-layer grouped containers,
//! and section jump-rail controls. The screen owns one outer scroll.
use super::{SectionGroup, label, muted, press_target, section_group, tokens::*};
use nsh_core::prelude::*;

pub fn settings_section(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
) -> SectionGroup {
    section_group(title).description(description).body_gap(14.0)
}

pub fn settings_field(title: &str, description: &str, control: impl IntoElement) -> View {
    let mut field = view()
        .min_w(px(0.))
        .gap_1p5()
        .child(label(title.to_owned()).font_weight(FontWeight::MEDIUM))
        .child(control);
    if !description.trim().is_empty() {
        field = field.child(muted(description.to_owned()));
    }
    field
}

/// Single-boundary rounded settings group: optional borderless header above ONE
/// `SURFACE` container with `1px` `BORDER` dividers between rows.
pub struct SettingsGroup {
    title: Option<SharedString>,
    subtitle: Option<SharedString>,
    trailing: Option<AnyElement>,
    rows: Vec<AnyElement>,
}

impl SettingsGroup {
    pub fn new() -> Self {
        Self {
            title: None,
            subtitle: None,
            trailing: None,
            rows: Vec::new(),
        }
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        let t = title.into();
        if !t.trim().is_empty() {
            self.title = Some(t);
        }
        self
    }

    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        let s = subtitle.into();
        if !s.trim().is_empty() {
            self.subtitle = Some(s);
        }
        self
    }

    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_element());
        self
    }

    pub fn row(mut self, element: impl IntoElement) -> Self {
        self.rows.push(element.into_element());
        self
    }

    pub fn rows<I, E>(mut self, elements: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: IntoElement,
    {
        self.rows
            .extend(elements.into_iter().map(IntoElement::into_element));
        self
    }
}

impl Default for SettingsGroup {
    fn default() -> Self {
        Self::new()
    }
}

impl IntoElement for SettingsGroup {
    fn into_element(self) -> AnyElement {
        let mut container = view()
            .rounded(px(RADIUS))
            .bg(SURFACE)
            .border_1()
            .border_color(BORDER)
            .overflow_hidden()
            .min_w(px(0.))
            .gap_0();
        let total = self.rows.len();
        for (idx, row_el) in self.rows.into_iter().enumerate() {
            let mut slot = view().min_w(px(0.)).child(row_el);
            if idx + 1 < total {
                slot = slot.border_b_1().border_color(BORDER);
            }
            container = container.child(slot);
        }

        let has_header = self.title.is_some() || self.subtitle.is_some() || self.trailing.is_some();
        if !has_header {
            return container.into_element();
        }

        let mut header_left = view().flex_1().min_w(px(0.)).gap_0p5();
        if let Some(title) = self.title {
            header_left = header_left.child(
                label(title)
                    .text_size(px(BODY_SIZE))
                    .font_weight(FontWeight::SEMIBOLD),
            );
        }
        if let Some(subtitle) = self.subtitle {
            header_left = header_left.child(muted(subtitle).text_size(px(CAPTION_SIZE)));
        }

        let mut header_row = view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .min_w(px(0.))
            .child(header_left);
        if let Some(trailing) = self.trailing {
            header_row = header_row.child(trailing);
        }

        view()
            .gap_2()
            .min_w(px(0.))
            .child(header_row)
            .child(container)
            .into_element()
    }
}

/// Create a single-layer grouped settings container with `1px` internal row dividers.
pub fn settings_group() -> SettingsGroup {
    SettingsGroup::new()
}

/// Standard two-column settings row: left title + optional description, right control(s).
pub fn settings_row(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl IntoElement,
) -> View {
    let desc = description.into();
    let mut left = view()
        .flex_1()
        .min_w(px(200.))
        .gap_0p5()
        .child(label(title.into()).font_weight(FontWeight::MEDIUM));
    if !desc.trim().is_empty() {
        left = left.child(muted(desc).text_size(px(CAPTION_SIZE)));
    }
    view()
        .px_3p5()
        .py_2p5()
        .flex_row()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_3()
        .min_w(px(0.))
        .child(left)
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .flex_shrink_0()
                .child(control),
        )
}

/// Compact key/value inspection row for runtime & diagnostics groups.
pub fn settings_kv_row(key: impl Into<SharedString>, value: impl IntoElement) -> View {
    view()
        .px_3p5()
        .py_2()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3()
        .min_w(px(0.))
        .child(
            muted(key.into())
                .text_size(px(CAPTION_SIZE))
                .flex_shrink_0(),
        )
        .child(value)
}

/// Group caption inside the Settings jump column (e.g. `INTERFACE`, `EXECUTION`).
pub fn settings_nav_heading(title: impl Into<SharedString>) -> View {
    view().px_2p5().pt_2().pb_0p5().child(
        text(title.into())
            .text_size(px(10.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(MUTED),
    )
}

/// Accessible row inside the Settings jump column. Keeps selected, hover, and
/// keyboard focus distinct while reserving a leading accent bar for active state.
pub fn settings_nav_item(
    title: impl Into<SharedString>,
    selected: impl Fn() -> bool + 'static,
    press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    let title: SharedString = title.into();
    let a11y_title = title.clone();
    let selected = computed(selected);
    press_target()
        .w_full()
        .min_h(px(28.))
        .px_2()
        .py_1()
        .rounded(px(6.))
        .flex_row()
        .items_center()
        .gap_2()
        .selected(move || selected.get())
        .bg(move || if selected.get() { SELECTED } else { CLEAR })
        .border_1()
        .border_color(CLEAR)
        .hover(move |s| {
            if selected.get() {
                s.bg(SELECTED)
            } else {
                s.bg(INSET)
            }
        })
        .focus(|s| s.border_color(ACCENT))
        .active(|s| s.opacity(0.75))
        .accessibility_label(move || Some(a11y_title.clone()))
        .on_press(press)
        .child(
            view()
                .w(px(2.5))
                .h(px(14.))
                .rounded_full()
                .flex_shrink_0()
                .bg(move || if selected.get() { ACCENT } else { CLEAR }),
        )
        .child(
            text(title)
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || if selected.get() { TEXT } else { MUTED })
                .truncate()
                .min_w(px(0.))
                .flex_1(),
        )
}
