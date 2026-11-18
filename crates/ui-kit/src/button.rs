//! Button contract: typed variant; NSH owns press/keyboard/disabled semantics.
use super::{Icon, icon_tinted, tokens::*};
use nsh_core::prelude::*;
#[derive(Clone, Copy)]
pub enum Variant {
    Primary,
    Secondary,
    Danger,
    Quiet,
}
pub fn action(
    label: impl IntoReactive<SharedString>,
    variant: Variant,
    on_press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    base(variant)
        .child(action_label(label, variant))
        .on_press(on_press)
}
fn compact_base(variant: Variant) -> nsh_base::ButtonElement {
    base(variant).min_h(px(26.0)).px_2().rounded(px(5.0))
}
/// Compact action for toolbars, inline status rows and secondary headers.
pub fn compact_action(
    label: impl IntoReactive<SharedString>,
    variant: Variant,
    on_press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    compact_base(variant)
        .child(action_label(label, variant))
        .on_press(on_press)
}
/// Compact action with a leading semantic icon for headers and toolbars.
pub fn compact_action_with_icon(
    kind: Icon,
    label: impl IntoReactive<SharedString>,
    variant: Variant,
    on_press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    compact_base(variant)
        .flex_row()
        .items_center()
        .gap_1p5()
        .child(icon_tinted(kind, foreground(variant)).w(px(14.)).h(px(14.)))
        .child(action_label(label, variant))
        .on_press(on_press)
}
/// The same action contract with a leading semantic icon.
pub fn action_with_icon(
    kind: Icon,
    label: impl IntoReactive<SharedString>,
    variant: Variant,
    on_press: impl Fn(&()) + 'static,
) -> nsh_base::ButtonElement {
    base(variant)
        .flex_row()
        .items_center()
        .gap_2()
        .child(icon_tinted(kind, foreground(variant)).w(px(16.)).h(px(16.)))
        .child(
            action_label(label, variant)
                .truncate()
                .min_w(px(0.))
                .flex_1(),
        )
        .on_press(on_press)
}
/// Unstyled press target for every clickable surface: NSH button behavior plus
/// the shared pointer convention (pointing hand while enabled, arrow while
/// disabled). Product and UI Kit code use this instead of `nsh_base::button()`.
pub fn press_target() -> nsh_base::ButtonElement {
    nsh_base::button()
        .cursor_pointer()
        .disabled_style(|s| s.cursor(Cursor::Default))
}
fn foreground(variant: Variant) -> ThemeColor {
    match variant {
        Variant::Primary => ON_ACCENT,
        Variant::Danger => DANGER,
        Variant::Quiet => MUTED,
        Variant::Secondary => TEXT,
    }
}
fn action_label(label: impl IntoReactive<SharedString>, variant: Variant) -> Text {
    text(label)
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(foreground(variant))
}
fn base(variant: Variant) -> nsh_base::ButtonElement {
    let background = match variant {
        Variant::Primary => ACCENT_FILL,
        Variant::Danger | Variant::Secondary => SURFACE,
        Variant::Quiet => CLEAR,
    };
    press_target()
        .min_h(px(CONTROL_HEIGHT))
        .px_3()
        .justify_center()
        .items_center()
        .rounded(px(RADIUS))
        .bg(background)
        .border_1()
        .border_color(if matches!(variant, Variant::Quiet) {
            CLEAR
        } else {
            BORDER
        })
        .hover(move |s| {
            if matches!(variant, Variant::Primary) {
                s.opacity(0.9)
            } else {
                s.bg(INSET)
            }
        })
        .active(|s| s.opacity(0.6))
        .focus(|s| s.border_color(ACCENT))
        .disabled_style(|s| s.opacity(0.4))
}

/// Controlled toggle: the caller owns its value; pointer and keyboard share one action.
pub fn toggle(
    label: &'static str,
    value: impl Fn() -> bool + Copy + 'static,
    changed: impl Fn(bool) + 'static,
) -> nsh_base::ButtonElement {
    press_target()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3()
        .min_h(px(30.))
        .accessibility_label(move || {
            Some(format!("{label}: {}", if value() { "on" } else { "off" }).into())
        })
        .focus(|s| s.border_1().border_color(ACCENT))
        .child(text(label).text_size(px(12.)).text_color(TEXT))
        .child(
            view()
                .w(px(34.))
                .h(px(20.))
                .rounded_full()
                .p_0p5()
                .bg(move || if value() { ACCENT_FILL } else { INSET })
                .child(
                    view()
                        .w(px(16.))
                        .h(px(16.))
                        .rounded_full()
                        .bg(TEXT)
                        .ml(move || px(if value() { 14. } else { 0. })),
                ),
        )
        .on_press(move |_: &()| changed(!value()))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Clickable surfaces get the pointing hand from `press_target()`; a bare
    /// NSH button would silently fall back to the arrow cursor.
    #[test]
    fn press_targets_use_the_shared_pointer_cursor_constructor() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        for crate_dir in ["ui-kit/src", "ui/src"] {
            rust_sources(&root.join(crate_dir), &mut files);
        }
        let this = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/button.rs");
        let needle = concat!("nsh_base::", "button()");
        let offenders: Vec<_> = files
            .iter()
            .filter(|f| f.canonicalize().unwrap() != this.canonicalize().unwrap())
            .filter(|f| std::fs::read_to_string(f).unwrap().contains(needle))
            .collect();
        assert!(offenders.is_empty(), "use press_target(): {offenders:?}");
    }
}
