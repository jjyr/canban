use nsh_core::{
    element::{AdaptiveColor, IntoReactive, Reactive, rgb},
    prelude::*,
};
use std::{cell::RefCell, rc::Rc};
thread_local! { static THEME: RefCell<Option<Signal<String>>> = const { RefCell::new(None) }; }
/// Canban always uses dark native controls, independent of system appearance.
pub fn install_theme() {
    set_color_scheme(Appearance::Dark);
    THEME.with(|p| *p.borrow_mut() = Some(signal(canban_themes::DEFAULT.into())));
    sync_nsh_tokens();
    super::scrollbar::install(canban_themes::resolve(canban_themes::DEFAULT));
}
pub fn set_theme(name: &str) {
    let name = canban_themes::resolve(name).id;
    THEME.with(|p| {
        if let Some(s) = *p.borrow()
            && s.get_untracked() != name
        {
            s.set(name.into());
            sync_nsh_tokens();
            super::scrollbar::install(canban_themes::resolve(name));
        }
    });
}
fn sync_nsh_tokens() {
    let mut t = nsh_base::Theme::default();
    t.background = BACKGROUND.resolve();
    t.foreground = TEXT.resolve();
    t.card = SURFACE.resolve();
    t.card_foreground = TEXT.resolve();
    t.popover = SURFACE.resolve();
    t.popover_foreground = TEXT.resolve();
    t.primary = ACCENT.resolve();
    t.primary_foreground = ON_ACCENT.resolve();
    t.secondary = INSET.resolve();
    t.secondary_foreground = TEXT.resolve();
    t.muted = INSET.resolve();
    t.muted_foreground = MUTED.resolve();
    t.accent = SELECTED.resolve();
    t.accent_foreground = TEXT.resolve();
    t.destructive = DANGER.resolve();
    t.border = BORDER.resolve();
    t.input = INSET.resolve();
    t.ring = ACCENT.resolve();
    t.radius = px(RADIUS);
    t.font_mono = Some(MONO_FONT.into());
    nsh_core::element::set_tokens(t);
}
pub fn current_theme() -> &'static canban_themes::Theme {
    let name = THEME.with(|p| {
        p.borrow()
            .as_ref()
            .and_then(|s| s.try_get())
            .unwrap_or_default()
    });
    canban_themes::resolve(&name)
}
#[derive(Clone, Copy)]
pub struct ThemeColor(usize);
impl ThemeColor {
    pub fn resolve(self) -> AdaptiveColor {
        if self.0 == 16 {
            return AdaptiveColor::TRANSPARENT;
        }
        let color = if self.0 == 17 {
            rgb(current_theme().chrome()[1]).to_hsla().lighten(0.025)
        } else {
            rgb(current_theme().chrome()[self.0]).to_hsla()
        };
        AdaptiveColor {
            light: color,
            dark: color,
        }
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
pub const BACKGROUND: ThemeColor = ThemeColor(0);
pub const SURFACE: ThemeColor = ThemeColor(1);
pub const INSET: ThemeColor = ThemeColor(2);
pub const TEXT: ThemeColor = ThemeColor(3);
pub const MUTED: ThemeColor = ThemeColor(4);
pub const BORDER: ThemeColor = ThemeColor(5);
pub const ACCENT: ThemeColor = ThemeColor(6);
pub const ACCENT_FILL: ThemeColor = ThemeColor(7);
pub const ON_ACCENT: ThemeColor = ThemeColor(8);
pub const DANGER: ThemeColor = ThemeColor(9);
pub const SUCCESS: ThemeColor = ThemeColor(10);
pub const SELECTED: ThemeColor = ThemeColor(11);
pub const ATTENTION: ThemeColor = ThemeColor(12);
pub const ATTENTION_SURFACE: ThemeColor = ThemeColor(13);
pub const SUCCESS_SURFACE: ThemeColor = ThemeColor(14);
pub const DANGER_SURFACE: ThemeColor = ThemeColor(15);
pub const CLEAR: ThemeColor = ThemeColor(16);
/// Quiet hover for card-like surfaces across every named theme.
pub const CARD_HOVER: ThemeColor = ThemeColor(17);
pub const RADIUS: f32 = 8.0;
pub const CONTROL_HEIGHT: f32 = 34.0;
pub const BODY_SIZE: f32 = 13.0;
pub const CAPTION_SIZE: f32 = 12.0;
/// Explicit Latin + CJK fixed-advance cascade for NSH rope editors.
/// Avoids macOS `.AppleSystemUIFontMonospaced` synthetic CJK-Latin inter-script
/// spacing so per-grapheme caret X measurement matches CoreText line rendering.
pub const MONO_FONT: &str = "Menlo, PingFang SC";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_font_uses_named_latin_and_cjk_families_without_system_ui_monospace() {
        let families: Vec<_> = nsh_core::font_families(MONO_FONT).collect();
        assert_eq!(
            families,
            vec![
                nsh_core::FamilyName::Named("Menlo"),
                nsh_core::FamilyName::Named("PingFang SC"),
            ]
        );
    }
}
