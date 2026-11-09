use nsh_core::{
    element::{AdaptiveColor, IntoReactive, Reactive, rgb},
    prelude::*,
};
use std::{cell::RefCell, rc::Rc};
thread_local! { static PALETTE: RefCell<Option<Signal<String>>> = const { RefCell::new(None) }; }
pub fn install_theme() {
    PALETTE.with(|p| *p.borrow_mut() = Some(signal("canban".into())));
}
pub fn set_palette(name: &str) {
    PALETTE.with(|p| {
        if let Some(s) = *p.borrow()
            && s.get_untracked() != name
        {
            s.set(name.into());
        }
    });
}
#[derive(Clone, Copy)]
pub struct ThemeColor(usize, AdaptiveColor);
impl ThemeColor {
    pub fn resolve(self) -> AdaptiveColor {
        let name = PALETTE.with(|p| {
            p.borrow()
                .as_ref()
                .and_then(|s| s.try_get())
                .unwrap_or_default()
        });
        let colors = match (name.as_str(), self.0) {
            ("forest", 0) => Some((0xf2f7f4, 0x101914)),
            ("forest", 1) => Some((0xffffff, 0x19271f)),
            ("forest", 2) => Some((0xe6eee8, 0x132018)),
            ("forest", 6) => Some((0x247253, 0x85cdaa)),
            ("forest", 7) => Some((0x247253, 0x2b7354)),
            ("forest", 11) => Some((0xe0efe6, 0x254735)),
            ("plum", 0) => Some((0xf8f4fa, 0x19121f)),
            ("plum", 1) => Some((0xffffff, 0x281e30)),
            ("plum", 2) => Some((0xeee7f2, 0x211729)),
            ("plum", 6) => Some((0x7947a6, 0xc6a0ed)),
            ("plum", 7) => Some((0x7947a6, 0x784aa4)),
            ("plum", 11) => Some((0xeee3f6, 0x443052)),
            _ => None,
        };
        colors.map(|(l, d)| tone(l, d)).unwrap_or(self.1)
    }
}
impl From<ThemeColor> for AdaptiveColor {
    fn from(c: ThemeColor) -> Self {
        c.resolve()
    }
}
impl IntoReactive<AdaptiveColor> for ThemeColor {
    fn into_reactive(self) -> Reactive<AdaptiveColor> {
        Reactive::Bound(Rc::new(move || self.resolve()))
    }
}
const fn tone(light: u32, dark: u32) -> AdaptiveColor {
    AdaptiveColor {
        light: rgb(light).to_hsla(),
        dark: rgb(dark).to_hsla(),
    }
}
pub const BACKGROUND: ThemeColor = ThemeColor(0, tone(0xf5f6f8, 0x101318));
pub const SURFACE: ThemeColor = ThemeColor(1, tone(0xffffff, 0x191e26));
pub const INSET: ThemeColor = ThemeColor(2, tone(0xedeff3, 0x13171d));
pub const TEXT: ThemeColor = ThemeColor(3, tone(0x1b2533, 0xe5eaf2));
pub const MUTED: ThemeColor = ThemeColor(4, tone(0x657185, 0x9aa8bb));
pub const BORDER: ThemeColor = ThemeColor(5, tone(0xdce1e9, 0x303946));
pub const ACCENT: ThemeColor = ThemeColor(6, tone(0x285bca, 0x80aaff));
pub const ACCENT_FILL: ThemeColor = ThemeColor(7, tone(0x285bca, 0x315faa));
pub const ON_ACCENT: ThemeColor = ThemeColor(8, tone(0xffffff, 0xffffff));
pub const DANGER: ThemeColor = ThemeColor(9, tone(0xad3c36, 0xffa39c));
pub const SUCCESS: ThemeColor = ThemeColor(10, tone(0x27745c, 0x7dd6b5));
pub const RADIUS: f32 = 8.0;
pub const CONTROL_HEIGHT: f32 = 34.0;

pub const SELECTED: ThemeColor = ThemeColor(11, tone(0xe8effb, 0x26364e));
pub const ATTENTION: ThemeColor = ThemeColor(12, tone(0x875a13, 0xf0c679));
pub const ATTENTION_SURFACE: ThemeColor = ThemeColor(13, tone(0xfff3da, 0x3a3020));
pub const SUCCESS_SURFACE: ThemeColor = ThemeColor(14, tone(0xe7f3ed, 0x20392f));
pub const DANGER_SURFACE: ThemeColor = ThemeColor(15, tone(0xfbece9, 0x402a2a));
pub const BODY_SIZE: f32 = 13.0;
pub const CAPTION_SIZE: f32 = 12.0;

pub const CLEAR: ThemeColor = ThemeColor(16, AdaptiveColor::TRANSPARENT);
