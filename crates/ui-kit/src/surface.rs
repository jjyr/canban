use super::tokens::*;
use nsh_core::prelude::*;
pub fn panel() -> View {
    view().p_4().gap_3().bg(SURFACE).rounded(px(RADIUS))
}
pub fn label(value: impl IntoReactive<SharedString>) -> Text {
    text(value).text_size(px(BODY_SIZE)).text_color(TEXT)
}
pub fn muted(value: impl IntoReactive<SharedString>) -> Text {
    text(value).text_size(px(CAPTION_SIZE)).text_color(MUTED)
}
pub fn heading(value: impl IntoReactive<SharedString>) -> Text {
    text(value)
        .text_size(px(20.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(TEXT)
}
pub fn badge(value: impl IntoReactive<SharedString>) -> impl IntoElement {
    view()
        .px_2()
        .py_1()
        .rounded(px(5.0))
        .bg(INSET)
        .child(muted(value))
}
