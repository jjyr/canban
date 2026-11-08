//! Shared settings section/field geometry. The screen owns one outer scroll.
use super::{label, muted, panel, tokens::*};
use nsh_core::prelude::*;
pub fn settings_section(title: &str, description: &str) -> View {
    panel()
        .min_w(px(0.))
        .gap_4()
        .border_1()
        .border_color(BORDER)
        .child(
            view()
                .gap_1()
                .child(
                    label(title.to_owned())
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(muted(description.to_owned())),
        )
}
pub fn settings_field(title: &str, description: &str, control: impl IntoElement) -> View {
    view()
        .min_w(px(0.))
        .gap_2()
        .child(label(title.to_owned()))
        .child(control)
        .child(muted(description.to_owned()))
}
