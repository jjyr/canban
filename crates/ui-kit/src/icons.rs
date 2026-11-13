//! Shared Lucide vector icons and accessible 40-point navigation actions.
use super::{label, tokens::*};
use nsh_core::element::ImageSrc;
use nsh_core::prelude::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    History,
    Undo,
    Trigger,
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
}
impl Icon {
    fn source(self) -> &'static str {
        match self {
            Self::History => include_str!("../assets/lucide/history.svg"),
            Self::Undo => include_str!("../assets/lucide/undo-2.svg"),
            Self::Trigger => include_str!("../assets/lucide/workflow.svg"),
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
        .tooltip(
            nsh_base::tooltip()
                .placement(Placement::Right)
                .px_3()
                .py_2()
                .rounded(px(RADIUS))
                .bg(SURFACE)
                .border_1()
                .border_color(BORDER)
                .child(label(hint)),
        )
        .child(
            crate::press_target()
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
