//! Shared Lucide vector icons and accessible 40-point navigation actions.
use super::tokens::*;
use nsh_core::element::{ImageSrc, rgb};
use nsh_core::prelude::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Clock,
    History,
    Undo,
    Trigger,
    Zap,
    Update,
    Info,
    Folder,
    Coffee,
    Theme,
    Keyboard,
    Bug,
    Branch,
    Compare,
    More,
    Grip,
    Close,
    Forward,
    Back,
    Sidebar,
    Terminal,
    Search,
    Projects,
    Board,
    Note,
    Agent,
    Codex,
    Claude,
    Agy,
    Gemini,
    OpenCode,
    Cursor,
    Pi,
    Omp,
    Copilot,
    Droid,
    Amp,
    Kimi,
    Kiro,
    Qwen,
    Devin,
    Cline,
    Grok,
    Hermes,
    Kilo,
    Qoder,
    Letta,
    Mastra,
    Maki,
    Muse,
    GitHub,
    Overview,
    Archive,
    Settings,
    Plus,
    ChevronDown,
    Check,
    Edit,
    Refresh,
    Copy,
    Expand,
    Collapse,
    Paperclip,
    ShieldAlert,
    CircleHelp,
    CirclePause,
    Eye,
    GitMerge,
    TriangleAlert,
    MessageSquare,
    Server,
    Diagnostics,
    Insights,
    ArrowUp,
    ArrowDown,
    ChevronsUpDown,
}
impl Icon {
    fn source(self) -> &'static str {
        match self {
            Self::Clock => include_str!("../assets/lucide/clock.svg"),
            Self::History => include_str!("../assets/lucide/history.svg"),
            Self::Undo => include_str!("../assets/lucide/undo-2.svg"),
            Self::Trigger => include_str!("../assets/lucide/workflow.svg"),
            Self::Zap => include_str!("../assets/lucide/zap.svg"),
            Self::Update => include_str!("../assets/utility/update.svg"),
            Self::Folder => include_str!("../assets/lucide/folder.svg"),
            Self::Coffee => include_str!("../assets/utility/coffee.svg"),
            Self::Theme => include_str!("../assets/utility/palette.svg"),
            Self::Keyboard => include_str!("../assets/utility/keyboard.svg"),
            Self::Bug => include_str!("../assets/utility/bug.svg"),
            Self::Info => include_str!("../assets/lucide/info.svg"),
            Self::Branch => include_str!("../assets/lucide/git-branch.svg"),
            Self::Compare => include_str!("../assets/lucide/git-compare-arrows.svg"),
            Self::Grip => include_str!("../assets/lucide/grip-vertical.svg"),
            Self::More => include_str!("../assets/lucide/ellipsis.svg"),
            Self::Close => include_str!("../assets/lucide/x.svg"),
            Self::Forward => include_str!("../assets/lucide/chevron-right.svg"),
            Self::Back => include_str!("../assets/lucide/chevron-left.svg"),
            Self::Sidebar => include_str!("../assets/lucide/panel-left.svg"),
            Self::Terminal => include_str!("../assets/lucide/terminal.svg"),
            Self::Search => include_str!("../assets/lucide/search.svg"),
            Self::Projects => include_str!("../assets/lucide/folder-open.svg"),
            Self::Board => include_str!("../assets/lucide/square-kanban.svg"),
            Self::Note => include_str!("../assets/lucide/file-text.svg"),
            Self::Agent => include_str!("../assets/lucide/bot.svg"),
            Self::Codex => include_str!("../assets/providers/codex.svg"),
            Self::GitHub => include_str!("../assets/providers/github.svg"),
            Self::Claude => include_str!("../assets/providers/claude.svg"),
            Self::Agy => include_str!("../assets/providers/agy.svg"),
            Self::Gemini => include_str!("../assets/providers/gemini.svg"),
            Self::OpenCode => include_str!("../assets/providers/opencode.svg"),
            Self::Cursor => include_str!("../assets/providers/cursor.svg"),
            Self::Pi => include_str!("../assets/providers/pi.svg"),
            Self::Omp => include_str!("../assets/providers/omp.svg"),
            Self::Copilot => include_str!("../assets/providers/copilot.svg"),
            Self::Droid => include_str!("../assets/providers/droid.svg"),
            Self::Amp => include_str!("../assets/providers/amp.svg"),
            Self::Kimi => include_str!("../assets/providers/kimi.svg"),
            Self::Kiro => include_str!("../assets/providers/kiro.svg"),
            Self::Qwen => include_str!("../assets/providers/qwen.svg"),
            Self::Devin => include_str!("../assets/providers/devin.svg"),
            Self::Cline => include_str!("../assets/providers/cline.svg"),
            Self::Grok => include_str!("../assets/providers/grok.svg"),
            Self::Hermes => include_str!("../assets/providers/hermes.svg"),
            Self::Kilo => include_str!("../assets/providers/kilo.svg"),
            Self::Qoder => include_str!("../assets/providers/qodercli.svg"),
            Self::Letta => include_str!("../assets/providers/letta.svg"),
            Self::Mastra => include_str!("../assets/providers/mastracode.svg"),
            Self::Maki => include_str!("../assets/providers/maki.svg"),
            Self::Muse => include_str!("../assets/providers/muse.svg"),
            Self::Overview => include_str!("../assets/lucide/layout-dashboard.svg"),
            Self::Archive => include_str!("../assets/lucide/archive.svg"),
            Self::Settings => include_str!("../assets/lucide/settings.svg"),
            Self::Plus => include_str!("../assets/lucide/plus.svg"),
            Self::ChevronDown => include_str!("../assets/lucide/chevron-down.svg"),
            Self::Check => include_str!("../assets/lucide/check.svg"),
            Self::Edit => include_str!("../assets/lucide/pencil.svg"),
            Self::Refresh => include_str!("../assets/lucide/refresh-cw.svg"),
            Self::Copy => include_str!("../assets/lucide/copy.svg"),
            Self::Expand => include_str!("../assets/lucide/maximize-2.svg"),
            Self::Collapse => include_str!("../assets/lucide/minimize-2.svg"),
            Self::Paperclip => include_str!("../assets/lucide/paperclip.svg"),
            Self::ShieldAlert => include_str!("../assets/lucide/shield-alert.svg"),
            Self::CircleHelp => include_str!("../assets/lucide/circle-help.svg"),
            Self::CirclePause => include_str!("../assets/lucide/circle-pause.svg"),
            Self::Eye => include_str!("../assets/lucide/eye.svg"),
            Self::GitMerge => include_str!("../assets/lucide/git-merge.svg"),
            Self::TriangleAlert => include_str!("../assets/lucide/triangle-alert.svg"),
            Self::MessageSquare => include_str!("../assets/lucide/message-square.svg"),
            Self::Server => include_str!("../assets/lucide/server.svg"),
            Self::Diagnostics => include_str!("../assets/lucide/activity.svg"),
            Self::Insights => include_str!("../assets/lucide/chart-column.svg"),
            Self::ArrowUp => include_str!("../assets/lucide/arrow-up.svg"),
            Self::ArrowDown => include_str!("../assets/lucide/arrow-down.svg"),
            Self::ChevronsUpDown => include_str!("../assets/lucide/chevrons-up-down.svg"),
        }
    }

    fn colored_source(self) -> Option<&'static str> {
        match self {
            Self::Agy => Some(include_str!("../assets/providers/color/agy.svg")),
            Self::Gemini => Some(include_str!("../assets/providers/color/gemini.svg")),
            Self::Copilot => Some(include_str!("../assets/providers/color/copilot.svg")),
            Self::Devin => Some(include_str!("../assets/providers/color/devin.svg")),
            Self::Kimi => Some(include_str!("../assets/providers/color/kimi.svg")),
            Self::Qwen => Some(include_str!("../assets/providers/color/qwen.svg")),
            Self::Qoder => Some(include_str!("../assets/providers/color/qodercli.svg")),
            Self::Muse => Some(include_str!("../assets/providers/color/muse.svg")),
            _ => None,
        }
    }

    /// Vibrant brand palette for single-hue provider marks on dark surfaces.
    fn brand_rgb(self) -> Option<u32> {
        match self {
            Self::Codex => Some(0x10a37f),
            Self::Claude => Some(0xd97757),
            Self::OpenCode => Some(0x60a5fa),
            Self::Cursor => Some(0x38bdf8),
            Self::Pi => Some(0x2dd4bf),
            Self::Omp => Some(0xfb923c),
            Self::Droid => Some(0xa3e635),
            Self::Amp => Some(0xf34e3f),
            Self::Kiro => Some(0xa855f7),
            Self::Cline => Some(0x22d3ee),
            Self::Grok => Some(0xf97316),
            Self::Hermes => Some(0xfbbf24),
            Self::Kilo => Some(0xf8f676),
            Self::Letta => Some(0xc084fc),
            Self::Mastra => Some(0x34d399),
            Self::Maki => Some(0xf472b6),
            _ => None,
        }
    }
}
/// Lucide 0.468.0 SVGs, vendored with their license. NSH renders native vectors
/// and resolves currentColor from the same adaptive semantic tokens as text.
pub fn icon(kind: Icon) -> Svg {
    icon_tinted(kind, MUTED)
}
pub fn icon_tinted(kind: Icon, tint: impl IntoReactive<AdaptiveColor>) -> Svg {
    svg(ImageSrc::svg_markup(kind.source()))
        .w(px(20.))
        .h(px(20.))
        .flex_shrink_0()
        .text_color(tint)
        .pointer_events(PointerEvents::None)
}
/// Full-color provider mark for rich identity surfaces such as Card comment
/// avatars. Multi-color SVGs render synchronously as native vector drawings
/// (preserving their authored solid fills and linear/radial gradients),
/// single-hue brand marks use their chromatic brand color, and generic icons
/// fall back to theme `ACCENT`.
pub fn icon_colored(kind: Icon, size: f32) -> Svg {
    if let Some(markup) = kind.colored_source() {
        return svg(ImageSrc::svg_markup(markup))
            .w(px(size))
            .h(px(size))
            .flex_shrink_0()
            .pointer_events(PointerEvents::None);
    }
    if let Some(hex) = kind.brand_rgb() {
        let color = rgb(hex).to_hsla();
        return icon_tinted(
            kind,
            AdaptiveColor {
                light: color,
                dark: color,
            },
        )
        .w(px(size))
        .h(px(size));
    }
    icon_tinted(kind, ACCENT).w(px(size)).h(px(size))
}
pub fn icon_action(
    kind: Icon,
    title: String,
    selected: impl Fn() -> bool + 'static,
    count: impl Fn() -> usize + 'static,
    press: impl Fn(&()) + 'static,
) -> impl IntoElement {
    let hint = title.clone();
    let selected = computed(selected);
    let count = computed(count);
    view()
        .w(px(40.))
        .h(px(40.))
        .tooltip(crate::hint_tooltip(hint).placement(Placement::Right))
        .child(
            crate::press_target()
                .app_class(super::motion::CONTROL_TRANSITION)
                .w(px(40.))
                .h(px(40.))
                .items_center()
                .justify_center()
                .relative()
                .group()
                .rounded(px(RADIUS))
                .bg(move || if selected.get() { SELECTED } else { CLEAR })
                .border_1()
                .border_color(CLEAR)
                .hover(|s| s.bg(INSET))
                .active(|s| s.opacity(0.7))
                .focus(|s| s.border_color(ACCENT))
                .selected(move || selected.get())
                .accessibility_label(move || {
                    let n = count.get();
                    Some(if n > 0 {
                        format!("{title} · {n} updates").into()
                    } else {
                        title.clone().into()
                    })
                })
                .child(
                    icon_tinted(kind, move || if selected.get() { ACCENT } else { MUTED })
                        .group_hover(|s| s.text_color(ACCENT)),
                )
                .child(dynamic(move || {
                    let n = count.get();
                    if n == 0 {
                        view().into_element()
                    } else {
                        view()
                            .absolute()
                            .top_0()
                            .right_0()
                            .min_w(px(16.))
                            .h(px(16.))
                            .px_1()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.))
                            .bg(ATTENTION_SURFACE)
                            .pointer_events(PointerEvents::None)
                            .child(
                                text(if n > 99 { "99+".into() } else { n.to_string() })
                                    .text_size(px(10.))
                                    .text_color(ATTENTION),
                            )
                            .into_element()
                    }
                }))
                .on_press(press),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROVIDER_ICONS: &[Icon] = &[
        Icon::Agent,
        Icon::Codex,
        Icon::Claude,
        Icon::Agy,
        Icon::Gemini,
        Icon::OpenCode,
        Icon::Cursor,
        Icon::Pi,
        Icon::Omp,
        Icon::Copilot,
        Icon::Droid,
        Icon::Amp,
        Icon::Kimi,
        Icon::Kiro,
        Icon::Qwen,
        Icon::Devin,
        Icon::Cline,
        Icon::Grok,
        Icon::Hermes,
        Icon::Kilo,
        Icon::Qoder,
        Icon::Letta,
        Icon::Mastra,
        Icon::Maki,
        Icon::Muse,
    ];

    #[test]
    fn every_agent_icon_has_chromatic_color_and_rasterizes_cleanly() {
        for &kind in PROVIDER_ICONS {
            if let Some(markup) = kind.colored_source() {
                assert!(
                    !markup.contains("<mask") && !markup.contains("<filter"),
                    "{kind:?} colored SVG must stay mask/filter-free for synchronous vector drawing"
                );
                let drawing = nsh_core::svg::drawing(markup)
                    .unwrap_or_else(|e| panic!("{kind:?} colored SVG failed drawing(): {e:?}"));
                assert!(
                    !drawing.ops.is_empty(),
                    "{kind:?} colored SVG produced an empty vector drawing"
                );
                let raster = nsh_core::svg::rasterize(markup.as_bytes(), 32)
                    .unwrap_or_else(|e| panic!("{kind:?} colored SVG failed to rasterize: {e:?}"));
                assert_eq!(raster.width, 32, "{kind:?}");
                assert_eq!(raster.height, 32, "{kind:?}");
                let has_chromatic_pixel =
                    raster
                        .pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .any(|&[r, g, b, a]| {
                            a >= 32
                                && (r.abs_diff(g) >= 16
                                    || g.abs_diff(b) >= 16
                                    || r.abs_diff(b) >= 16)
                        });
                assert!(
                    has_chromatic_pixel,
                    "{kind:?} colored SVG must contain chromatic (non-greyscale) pixels"
                );
            } else if kind == Icon::Agent {
                let raster = nsh_core::svg::rasterize(kind.source().as_bytes(), 32)
                    .unwrap_or_else(|e| panic!("{kind:?} SVG failed to rasterize: {e:?}"));
                assert!(raster.pixels.as_chunks::<4>().0.iter().any(|px| px[3] > 0));
            } else {
                let hex = kind
                    .brand_rgb()
                    .unwrap_or_else(|| panic!("{kind:?} must define colored_source or brand_rgb"));
                let [r, g, b] = [
                    ((hex >> 16) & 0xff) as u8,
                    ((hex >> 8) & 0xff) as u8,
                    (hex & 0xff) as u8,
                ];
                assert!(
                    r.abs_diff(g) >= 16 || g.abs_diff(b) >= 16 || r.abs_diff(b) >= 16,
                    "{kind:?} brand_rgb #{hex:06x} must not be greyscale"
                );
                let raster = nsh_core::svg::rasterize(kind.source().as_bytes(), 32)
                    .unwrap_or_else(|e| panic!("{kind:?} SVG failed to rasterize: {e:?}"));
                assert!(
                    raster.pixels.as_chunks::<4>().0.iter().any(|px| px[3] > 0),
                    "{kind:?}"
                );
            }
        }
    }

    #[test]
    fn insights_icon_vector_rasterizes_cleanly() {
        for kind in [
            Icon::Insights,
            Icon::ArrowUp,
            Icon::ArrowDown,
            Icon::ChevronsUpDown,
        ] {
            let markup = kind.source();
            let drawing = nsh_core::svg::drawing(markup).expect("SVG drawing()");
            assert!(!drawing.ops.is_empty());
            let raster = nsh_core::svg::rasterize(markup.as_bytes(), 32).expect("rasterize()");
            assert!(raster.pixels.as_chunks::<4>().0.iter().any(|px| px[3] > 0));
        }
    }
}
