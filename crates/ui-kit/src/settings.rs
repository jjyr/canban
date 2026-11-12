//! Shared settings section/field geometry built on borderless `SectionGroup`.
//! The screen owns one outer scroll.
use super::{SectionGroup, label, muted, section_group};
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
