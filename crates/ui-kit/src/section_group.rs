//! Borderless section group: title header + bottom divider + content body.
//! Reused across Overview, GitHub Account/Repository pages, and Settings screens.
use super::{Icon, Tone, badge, icon, label, muted, status, tokens::*};
use nsh_core::prelude::*;

pub struct SectionGroup {
    title: SharedString,
    description: Option<SharedString>,
    count: Option<usize>,
    tone: Tone,
    body_gap: f32,
    max_w: Option<Length>,
    trailing: Option<AnyElement>,
    children: Vec<AnyElement>,
}

impl SectionGroup {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            description: None,
            count: None,
            tone: Tone::Neutral,
            body_gap: 4.0,
            max_w: None,
            trailing: None,
            children: Vec::new(),
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        let desc = description.into();
        if !desc.trim().is_empty() {
            self.description = Some(desc);
        }
        self
    }

    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    pub fn body_gap(mut self, gap: f32) -> Self {
        self.body_gap = gap;
        self
    }

    pub fn max_w(mut self, max_w: impl Into<Length>) -> Self {
        self.max_w = Some(max_w.into());
        self
    }

    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_element());
        self
    }

    pub fn child(mut self, element: impl IntoElement) -> Self {
        self.children.push(element.into_element());
        self
    }

    pub fn children<I, E>(mut self, elements: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: IntoElement,
    {
        self.children
            .extend(elements.into_iter().map(IntoElement::into_element));
        self
    }
}

impl IntoElement for SectionGroup {
    fn into_element(self) -> AnyElement {
        let title_color = match self.tone {
            Tone::Attention => ATTENTION,
            Tone::Danger => DANGER,
            Tone::Success => SUCCESS,
            Tone::Info => ACCENT,
            Tone::Neutral => TEXT,
        };
        let mut left = view()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .flex_1()
            .child(
                label(self.title)
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(title_color)
                    .truncate(),
            );
        if let Some(count) = self.count {
            left = left.child(match self.tone {
                Tone::Neutral => badge(count.to_string()).into_element(),
                other => status(count.to_string(), other).into_element(),
            });
        }

        let mut header_row = view()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .min_w(px(0.))
            .child(left);
        if let Some(trailing) = self.trailing {
            header_row = header_row.child(trailing);
        }

        let mut header = view()
            .gap_1()
            .pb_2()
            .border_b_1()
            .border_color(BORDER)
            .min_w(px(0.))
            .child(header_row);
        if let Some(desc) = self.description {
            header = header.child(muted(desc));
        }

        let mut root = view().gap_3().min_w(px(0.)).child(header).child(
            view()
                .gap(px(self.body_gap))
                .min_w(px(0.))
                .children(self.children),
        );
        if let Some(mw) = self.max_w {
            root = root.max_w(mw);
        }
        root.into_element()
    }
}

/// Borderless section with a clean header, subtle bottom divider, and stacked body.
pub fn section_group(title: impl Into<SharedString>) -> SectionGroup {
    SectionGroup::new(title)
}

/// Collapsible borderless section group using the same header + bottom divider visual language.
pub fn collapsible_section_group(
    title: impl Into<SharedString>,
    count: Option<usize>,
    open: bool,
    toggle: impl Fn(&()) + 'static,
    children: impl IntoElement,
) -> impl IntoElement {
    let title = title.into();
    let a11y_title = if let Some(c) = count {
        format!("{} · {} item{}", title, c, if c == 1 { "" } else { "s" })
    } else {
        title.to_string()
    };
    let mut left = view()
        .flex_row()
        .items_center()
        .gap_2()
        .min_w(px(0.))
        .child(
            icon(if open {
                Icon::ChevronDown
            } else {
                Icon::Forward
            })
            .w(px(13.))
            .h(px(13.)),
        )
        .child(
            label(title)
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if open { TEXT } else { MUTED }),
        );
    if let Some(c) = count {
        left = left.child(badge(c.to_string()));
    }

    let header_btn = crate::press_target()
        .w_full()
        .min_w(px(0.))
        .py_1p5()
        .px_1()
        .rounded(px(RADIUS))
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .bg(CLEAR)
        .hover(|s| s.bg(INSET))
        .focus(|s| s.ring(px(1.)).ring_color(ACCENT))
        .accessibility_label(a11y_title)
        .on_press(toggle)
        .child(left)
        .child(muted(if open { "Collapse" } else { "Expand" }).text_size(px(11.)));

    let mut container = view().gap_2().min_w(px(0.)).child(
        view()
            .pb_1p5()
            .border_b_1()
            .border_color(BORDER)
            .min_w(px(0.))
            .child(header_btn),
    );
    if open {
        container = container.child(view().gap_1().min_w(px(0.)).child(children));
    }
    container
}
