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
    untracked(sync_nsh_tokens);
    super::scrollbar::install();
}
pub fn set_theme(name: &str) {
    let name = canban_themes::resolve(name).id;
    THEME.with(|p| {
        if let Some(s) = *p.borrow()
            && s.get_untracked() != name
        {
            s.set(name.into());
            untracked(sync_nsh_tokens);
        }
    });
}
fn sync_nsh_tokens() {
    let mut t = nsh_base::Theme::default();
    t.set_color("background", BACKGROUND.resolve());
    t.set_color("foreground", TEXT.resolve());
    t.set_color("card", SURFACE.resolve());
    t.set_color("card-foreground", TEXT.resolve());
    t.set_color("popover", SURFACE.resolve());
    t.set_color("popover-foreground", TEXT.resolve());
    t.set_color("primary", ACCENT.resolve());
    t.set_color("primary-foreground", ON_ACCENT.resolve());
    t.set_color("secondary", INSET.resolve());
    t.set_color("secondary-foreground", TEXT.resolve());
    t.set_color("muted", INSET.resolve());
    t.set_color("muted-foreground", MUTED.resolve());
    t.set_color("accent", SELECTED.resolve());
    t.set_color("accent-foreground", TEXT.resolve());
    t.set_color("destructive", DANGER.resolve());
    t.set_color("border", BORDER.resolve());
    t.set_color("input", INSET.resolve());
    t.set_color("ring", ACCENT.resolve());
    t.set_number("radius", RADIUS);
    t.set_font("font-mono", MONO_FONT);
    nsh_core::element::set_tokens(t);
    super::linux_gtk::sync_theme(current_theme().chrome());
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeColor(usize);
impl ThemeColor {
    pub fn resolve(self) -> AdaptiveColor {
        if self.0 == 16 {
            return AdaptiveColor::TRANSPARENT;
        }
        let color = if self.0 == 17 {
            rgb(current_theme().chrome()[1]).to_hsla().lighten(0.05)
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
/// spacing so per-grapheme caret X measurement matches CoreText line rendering,
/// and provides a crisp Linux Pango/Fontconfig monospaced + CJK fallback cascade.
#[cfg(target_os = "macos")]
pub const MONO_FONT: &str = "SF Mono, Menlo, PingFang SC";

#[cfg(not(target_os = "macos"))]
pub const MONO_FONT: &str = "JetBrains Mono, DejaVu Sans Mono, Liberation Mono, Noto Sans Mono, Noto Sans CJK SC, monospace";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_font_uses_named_latin_and_cjk_families_without_system_ui_monospace() {
        let families: Vec<_> = nsh_core::font_families(MONO_FONT).collect();
        #[cfg(target_os = "macos")]
        assert_eq!(
            families,
            vec![
                nsh_core::FamilyName::Named("SF Mono"),
                nsh_core::FamilyName::Named("Menlo"),
                nsh_core::FamilyName::Named("PingFang SC"),
            ]
        );
        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            families,
            vec![
                nsh_core::FamilyName::Named("JetBrains Mono"),
                nsh_core::FamilyName::Named("DejaVu Sans Mono"),
                nsh_core::FamilyName::Named("Liberation Mono"),
                nsh_core::FamilyName::Named("Noto Sans Mono"),
                nsh_core::FamilyName::Named("Noto Sans CJK SC"),
                nsh_core::FamilyName::Monospace,
            ]
        );
    }

    #[test]
    fn theme_install_and_switch_do_not_track_enclosing_dynamic_region() {
        use nsh_core::prelude::*;
        use nsh_core::{
            ApproxTextEngine, Core, EventKind, EventPayload, MountItem, ROOT_ID, element::Root,
            geometry::size,
        };
        use std::cell::{Cell, RefCell};

        thread_local! {
            static MOUNTS: Cell<usize> = const { Cell::new(0) };
            static ACTIVE_THEME: RefCell<String> = const { RefCell::new(String::new()) };
        }
        MOUNTS.with(|m| m.set(0));
        ACTIVE_THEME.with(|t| t.borrow_mut().clear());

        #[component]
        fn ProbeRoot() -> impl IntoElement {
            view()
                .on_press(move |_: &()| {
                    set_theme("monokai");
                    ACTIVE_THEME.with(|t| {
                        *t.borrow_mut() = THEME
                            .with(|p| p.borrow().map(|s| s.get_untracked()))
                            .unwrap_or_default();
                    });
                })
                .child(dynamic(|| {
                    MOUNTS.with(|m| m.set(m.get() + 1));
                    install_theme();
                    view().bg(BACKGROUND)
                }))
        }

        let mut core = Core::new(ApproxTextEngine, size(px(400.0), px(300.0)));
        core.mount_root(&Root::of::<ProbeRoot>());
        let batch = core.commit().expect("initial mount batch");
        assert_eq!(MOUNTS.with(|m| m.get()), 1);

        let host = batch
            .items
            .iter()
            .find_map(|item| match item {
                MountItem::Insert { parent, child, .. } if *parent == ROOT_ID => Some(*child),
                _ => None,
            })
            .expect("root host node");
        assert!(core.component_event(host, EventKind::Press, &EventPayload::None));
        let _ = core.commit();
        assert_eq!(MOUNTS.with(|m| m.get()), 1);
        assert_eq!(ACTIVE_THEME.with(|t| t.borrow().clone()), "monokai");
    }
}
