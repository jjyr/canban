//! Mobile terminal accessory key bar, sticky `Ctrl` encoding, and pinch-zoom scaling helpers.
use nsh_base::TerminalModes;
use nsh_core::{Haptic, platform::Os, prelude::*};
use std::rc::Rc;

/// Minimum and maximum font size (in points) allowed by pinch-to-zoom on mobile terminals.
pub const MIN_TERMINAL_FONT_PT: f32 = 9.0;
pub const MAX_TERMINAL_FONT_PT: f32 = 28.0;

/// Hardware/navigation key buttons exposed in the mobile terminal accessory bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessoryKey {
    Esc,
    Tab,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    CtrlC,
}

/// Encodes an [`AccessoryKey`] press into VT/ASCII bytes respecting current [`TerminalModes`].
pub fn encode_accessory_key(key: AccessoryKey, modes: TerminalModes) -> &'static [u8] {
    match key {
        AccessoryKey::Esc => b"\x1b",
        AccessoryKey::Tab => b"\t",
        AccessoryKey::CtrlC => b"\x03",
        AccessoryKey::ArrowUp => {
            if modes.application_cursor {
                b"\x1bOA"
            } else {
                b"\x1b[A"
            }
        }
        AccessoryKey::ArrowDown => {
            if modes.application_cursor {
                b"\x1bOB"
            } else {
                b"\x1b[B"
            }
        }
        AccessoryKey::ArrowRight => {
            if modes.application_cursor {
                b"\x1bOC"
            } else {
                b"\x1b[C"
            }
        }
        AccessoryKey::ArrowLeft => {
            if modes.application_cursor {
                b"\x1bOD"
            } else {
                b"\x1b[D"
            }
        }
    }
}

/// Applies sticky `Ctrl` modifier to the first byte/char of `input`, preserving any trailing text.
/// Returns `None` when `input` does not begin with a mappable ASCII control character.
pub fn apply_sticky_ctrl(input: &str) -> Option<Vec<u8>> {
    let mut chars = input.chars();
    let ch = chars.next()?;
    let ctrl_byte = match ch {
        'a'..='z' => (ch as u8) - b'a' + 1,
        'A'..='Z' => (ch as u8) - b'A' + 1,
        '@' | ' ' => 0x00,
        '[' => 0x1b,
        '\\' => 0x1c,
        ']' => 0x1d,
        '^' => 0x1e,
        '_' => 0x1f,
        '?' => 0x7f,
        _ => return None,
    };
    let rest = chars.as_str().as_bytes();
    let mut out = Vec::with_capacity(1 + rest.len());
    out.push(ctrl_byte);
    out.extend_from_slice(rest);
    Some(out)
}

/// Clamps mobile terminal font size (points) to `[MIN_TERMINAL_FONT_PT, MAX_TERMINAL_FONT_PT]`.
pub fn clamp_terminal_font_size(size: f32) -> f32 {
    if !size.is_finite() {
        return 13.0;
    }
    size.clamp(MIN_TERMINAL_FONT_PT, MAX_TERMINAL_FONT_PT)
}

/// Computes the scaled terminal font size from a pinch delta fraction (e.g. `0.1` -> `+10%`).
pub fn scale_font_size(current: f32, pinch_delta: f32) -> f32 {
    let factor = (1.0 + pinch_delta).clamp(0.5, 2.0);
    clamp_terminal_font_size(current * factor)
}

fn bar_color(hex: u32, alpha: f32) -> AdaptiveColor {
    let a = ((alpha.clamp(0.0, 1.0) * 255.0).round() as u32) & 0xff;
    let packed = (hex << 8) | a;
    let hsla: Hsla = rgba(packed).into();
    AdaptiveColor {
        light: hsla,
        dark: hsla,
    }
}

fn maybe_haptic() {
    if try_os() == Some(Os::Ios) {
        haptic(Haptic::Selection);
    }
}

/// Renders a docked 44pt accessory strip for mobile terminals (`Esc`, `Tab`, sticky `Ctrl`,
/// arrows, `^C`, `Paste`, and optional `⤢` fullscreen/landscape toggle).
#[allow(clippy::too_many_arguments)]
pub fn accessory_bar(
    ctrl_sticky: Signal<bool>,
    read_only: impl Fn() -> bool + Copy + 'static,
    on_key: impl Fn(AccessoryKey) + Clone + 'static,
    on_paste: impl Fn() + 'static,
    on_fullscreen: Option<Rc<dyn Fn()>>,
) -> View {
    let key_btn = move |label_text: &'static str, a11y: &'static str, key: AccessoryKey| {
        let on_key = on_key.clone();
        view()
            .focusable(false)
            .cursor_pointer()
            .h(px(32.))
            .min_w(px(36.))
            .px(px(10.))
            .rounded(px(6.))
            .items_center()
            .justify_center()
            .border_1()
            .border_color(bar_color(0xffffff, 0.14))
            .bg(bar_color(0x2a2d36, 0.92))
            .opacity(move || if read_only() { 0.4_f32 } else { 1.0_f32 })
            .label(a11y)
            .child(
                text(label_text)
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(bar_color(0xf0f2f7, 1.0)),
            )
            .on_press(move |_: &PressEvent| {
                if read_only() {
                    return;
                }
                maybe_haptic();
                on_key(key);
            })
    };

    let ctrl_btn = view()
        .focusable(false)
        .cursor_pointer()
        .h(px(32.))
        .min_w(px(44.))
        .px(px(10.))
        .rounded(px(6.))
        .items_center()
        .justify_center()
        .border_1()
        .border_color(move || {
            if ctrl_sticky.get() {
                bar_color(0x4c9eff, 0.95)
            } else {
                bar_color(0xffffff, 0.14)
            }
        })
        .bg(move || {
            if ctrl_sticky.get() {
                bar_color(0x2d68c4, 0.95)
            } else {
                bar_color(0x2a2d36, 0.92)
            }
        })
        .opacity(move || if read_only() { 0.4_f32 } else { 1.0_f32 })
        .label("Control modifier")
        .child(
            text("Ctrl")
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(bar_color(0xf5f7fa, 1.0)),
        )
        .on_press(move |_: &PressEvent| {
            if read_only() {
                return;
            }
            maybe_haptic();
            ctrl_sticky.update(|v| *v = !*v);
        });

    let paste_btn = view()
        .focusable(false)
        .cursor_pointer()
        .h(px(32.))
        .px(px(10.))
        .rounded(px(6.))
        .items_center()
        .justify_center()
        .border_1()
        .border_color(bar_color(0xffffff, 0.14))
        .bg(bar_color(0x2a2d36, 0.92))
        .opacity(move || if read_only() { 0.4_f32 } else { 1.0_f32 })
        .label("Paste from clipboard")
        .child(
            text("Paste")
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(bar_color(0xf0f2f7, 1.0)),
        )
        .on_press(move |_: &PressEvent| {
            if read_only() {
                return;
            }
            maybe_haptic();
            on_paste();
        });

    let mut row = view()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(8.))
        .h_full()
        .child(key_btn("Esc", "Escape", AccessoryKey::Esc))
        .child(key_btn("Tab", "Tab", AccessoryKey::Tab))
        .child(ctrl_btn)
        .child(key_btn("↑", "Up Arrow", AccessoryKey::ArrowUp))
        .child(key_btn("↓", "Down Arrow", AccessoryKey::ArrowDown))
        .child(key_btn("←", "Left Arrow", AccessoryKey::ArrowLeft))
        .child(key_btn("→", "Right Arrow", AccessoryKey::ArrowRight))
        .child(key_btn("^C", "Interrupt Ctrl+C", AccessoryKey::CtrlC))
        .child(paste_btn);

    if let Some(toggle) = on_fullscreen {
        row = row.child(
            view()
                .focusable(false)
                .cursor_pointer()
                .h(px(32.))
                .min_w(px(34.))
                .px(px(8.))
                .rounded(px(6.))
                .items_center()
                .justify_center()
                .border_1()
                .border_color(bar_color(0xffffff, 0.14))
                .bg(bar_color(0x2a2d36, 0.92))
                .label("Toggle fullscreen")
                .child(
                    text("⤢")
                        .text_size(px(14.))
                        .text_color(bar_color(0xf0f2f7, 1.0)),
                )
                .on_press(move |_: &PressEvent| {
                    maybe_haptic();
                    toggle();
                }),
        );
    }

    view()
        .w_full()
        .h(px(44.))
        .flex_shrink_0()
        .border_t_1()
        .border_color(bar_color(0xffffff, 0.10))
        .bg(bar_color(0x181a20, 0.98))
        .child(
            scroll()
                .horizontal()
                .scrollbar(false)
                .size_full()
                .child(row),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessory_keys_respect_normal_and_application_cursor_modes() {
        let normal = TerminalModes::default();
        assert_eq!(encode_accessory_key(AccessoryKey::Esc, normal), b"\x1b");
        assert_eq!(encode_accessory_key(AccessoryKey::Tab, normal), b"\t");
        assert_eq!(encode_accessory_key(AccessoryKey::CtrlC, normal), b"\x03");
        assert_eq!(
            encode_accessory_key(AccessoryKey::ArrowUp, normal),
            b"\x1b[A"
        );
        assert_eq!(
            encode_accessory_key(AccessoryKey::ArrowDown, normal),
            b"\x1b[B"
        );
        assert_eq!(
            encode_accessory_key(AccessoryKey::ArrowRight, normal),
            b"\x1b[C"
        );
        assert_eq!(
            encode_accessory_key(AccessoryKey::ArrowLeft, normal),
            b"\x1b[D"
        );

        let mut decckm = TerminalModes::default();
        decckm.application_cursor = true;
        assert_eq!(
            encode_accessory_key(AccessoryKey::ArrowUp, decckm),
            b"\x1bOA"
        );
        assert_eq!(
            encode_accessory_key(AccessoryKey::ArrowDown, decckm),
            b"\x1bOB"
        );
        assert_eq!(
            encode_accessory_key(AccessoryKey::ArrowRight, decckm),
            b"\x1bOC"
        );
        assert_eq!(
            encode_accessory_key(AccessoryKey::ArrowLeft, decckm),
            b"\x1bOD"
        );
    }

    #[test]
    fn sticky_ctrl_transforms_first_character_and_preserves_tail() {
        assert_eq!(apply_sticky_ctrl("c"), Some(vec![0x03]));
        assert_eq!(apply_sticky_ctrl("C"), Some(vec![0x03]));
        assert_eq!(apply_sticky_ctrl("d"), Some(vec![0x04]));
        assert_eq!(apply_sticky_ctrl("z"), Some(vec![0x1a]));
        assert_eq!(apply_sticky_ctrl("[hello"), Some(b"\x1bhello".to_vec()));
        assert_eq!(apply_sticky_ctrl("\\"), Some(vec![0x1c]));
        assert_eq!(apply_sticky_ctrl("]"), Some(vec![0x1d]));
        assert_eq!(apply_sticky_ctrl("@"), Some(vec![0x00]));
        assert_eq!(apply_sticky_ctrl(" "), Some(vec![0x00]));
        assert_eq!(apply_sticky_ctrl("_"), Some(vec![0x1f]));
        assert_eq!(apply_sticky_ctrl("?"), Some(vec![0x7f]));
        assert_eq!(apply_sticky_ctrl(""), None);
        assert_eq!(apply_sticky_ctrl("你"), None);
    }

    #[test]
    fn pinch_font_scaling_clamps_within_mobile_bounds() {
        assert_eq!(scale_font_size(13.0, 0.2), 15.6);
        assert_eq!(scale_font_size(26.0, 0.5), MAX_TERMINAL_FONT_PT);
        assert_eq!(scale_font_size(10.0, -0.5), MIN_TERMINAL_FONT_PT);
        assert_eq!(clamp_terminal_font_size(f32::NAN), 13.0);
    }
}
