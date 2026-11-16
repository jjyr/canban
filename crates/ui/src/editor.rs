//! Shared Card body: source, preview, references and retained draft feedback.
use crate::{drafts::*, presenter::*};
use canban_ui_kit::{markdown::MarkdownInput, *};
use nsh_core::prelude::*;

pub fn body(
    ui: Ui,
    text: Signal<String>,
    project: Memo<String>,
    active: Memo<bool>,
    height: Signal<f32>,
    submit: impl Fn() + 'static,
) -> (MarkdownInput, impl IntoElement) {
    let editor = MarkdownInput::new();
    editor.restore_after(
        computed(move || ui.quick_open.get() || !ui.utility_popup.get().is_empty()),
        active,
    );
    editor.on_submit(
        move || {
            active.get()
                && !ui.quick_open.get()
                && ui.utility_popup.get().is_empty()
                && !ui.native_modal.get()
        },
        submit,
    );
    let focus_ticket = stored(ui.editor_focus.get_untracked());
    effect(move || {
        let ticket = ui.editor_focus.get();
        if focus_ticket.replace(ticket) != ticket && active.get_untracked() {
            spawn(async move {
                next_frame().await;
                if active.try_get() == Some(true) {
                    editor.restore_focus();
                }
            });
        }
    });
    let (source, reference_error) = crate::references::source(
        ui,
        editor,
        text,
        project,
        "Task description",
        "Describe the task in Markdown… @Agent, #Card or #tag",
    );
    let body = editor.render_with_preview(text, source, false, height, move || {
        reader(ui, move || text.get()).into_element()
    });
    (
        editor,
        view().gap_2().child(body).child(when(
            move || !reference_error.get().is_empty(),
            move || notice(move || reference_error.get(), Tone::Attention),
        )),
    )
}
pub fn reader(ui: Ui, body: impl Fn() -> String + 'static) -> nsh_base::TextViewElement {
    crate::links::reader(ui, body)
}

pub fn draft_status(ui: Ui, key: String, _draft: Signal<Draft>) -> impl IntoElement {
    let visible_key = key.clone();
    let status_key = key.clone();
    let error_key = key.clone();
    when(
        move || {
            let _ = ui.draft_status.get();
            !persistence::status(&visible_key).is_empty()
        },
        move || {
            let status_key = status_key.clone();
            let error_key = error_key.clone();
            let retry_key = key.clone();
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(muted(move || {
                    let _ = ui.draft_status.get();
                    persistence::status(&status_key)
                }))
                .child(when(
                    move || {
                        let _ = ui.draft_status.get();
                        persistence::status(&error_key).starts_with("Draft not saved:")
                    },
                    move || {
                        let key = retry_key.clone();
                        compact_action("Retry", Variant::Quiet, move |_| persistence::retry(&key))
                    },
                ))
        },
    )
}
/// The workspace changes width without navigating or replacing the source.
pub fn expand(ui: Ui) -> impl IntoElement {
    dynamic(move || {
        let standalone = ui.card_standalone.get();
        chrome_button_disabled(
            if standalone {
                Icon::Collapse
            } else {
                Icon::Expand
            },
            if standalone {
                "Collapse editor"
            } else {
                "Expand editor"
            },
            move || ui.narrow.get(),
            move |_| {
                ui.card_standalone.update(|v| *v = !*v);
                ui.editor_focus.update(|n| *n += 1);
            },
        )
        .into_element()
    })
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use canban_ui_kit::tokens::MONO_FONT;
    use nsh_core::{
        AttributedText, FontFamily, Points, ResolvedTextStyle, TextEngine,
        canvas::{CanvasFont, measure_line},
    };
    use nsh_platform_macos::CoreTextEngine;

    /// Regression guard: NSH's `Editor` draws lines with `FontFamily::new(MONO_FONT)`
    /// (`listed_fonts` cascade) while measuring per-grapheme caret X via
    /// `CanvasFont::parse(&format!("13px \"{}\"", MONO_FONT))`, which splits on `,`
    /// and measures using only the first family. If the first family in `MONO_FONT`
    /// is not user-installed on macOS (e.g. `SF Mono`), `make_font` falls back to
    /// proportional `.AppleSystemUIFont` during `measure_text`, causing massive
    /// caret X drift relative to the Menlo/PingFang SC rendered line.
    #[test]
    fn macos_editor_caret_measurement_matches_coretext_rendered_line_width() {
        let engine = CoreTextEngine::default();
        let css = format!("13px \"{}\"", MONO_FONT.replace('"', ""));
        let measure_font = CanvasFont::parse(&css).expect("valid editor CSS font");

        let draw_style = ResolvedTextStyle {
            font_size: Points(13.0),
            font_weight: 400,
            font_family: Some(FontFamily::new(MONO_FONT)),
            ..Default::default()
        };

        for sample in [
            "hello world test cursor position 1234567890",
            "我发现编辑器 cursor 位置又坏了，我们之前是好的",
            "Mixed 中英文 test 测试 editor 光标 drift 问题 @developer #12",
        ] {
            let mut drawn = AttributedText {
                no_wrap: true,
                max_lines: 1,
                ..Default::default()
            };
            drawn.push(sample, draw_style);
            let prepared = engine.prepare(&drawn);
            let rendered_width = engine.measure(&prepared, None).width.0;

            let caret_x: f32 = sample
                .chars()
                .map(|ch| {
                    let repeated = ch.to_string().repeat(64);
                    measure_line(&engine, &measure_font, &repeated).0 / 64.0
                })
                .sum();

            let drift = (caret_x - rendered_width).abs();
            assert!(
                drift < 0.5,
                "caret X drifted from rendered CoreText line for {sample:?}: rendered={rendered_width:.2}, caret_x={caret_x:.2}, drift={drift:.2}"
            );
        }
    }
}
