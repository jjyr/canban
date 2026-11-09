//! Shared overlay indicators. NSH owns scroll metrics, thumb dragging and hit testing.
use nsh_base::{ScrollbarLook, ScrollbarMode, ScrollbarTheme, set_scrollbar_theme};

pub(crate) fn install(theme: &canban_themes::Theme) {
    let mut vertical = ScrollbarLook::default();
    vertical.class = Some("absolute inset-y-0 right-0 w-[12px] p-[3px]".into());
    vertical.thumb_class = Some(
        format!(
            "rounded-full bg-[#{:06x}] opacity-60 group-hover:opacity-90",
            theme.chrome()[4]
        )
        .into(),
    );
    vertical.accessibility_label = Some("Scroll vertically".into());
    let mut horizontal = vertical.clone();
    horizontal.class = Some("absolute inset-x-0 bottom-0 h-[12px] p-[3px]".into());
    horizontal.accessibility_label = Some("Scroll horizontally".into());
    let mut bars = ScrollbarTheme::default();
    bars.mode = ScrollbarMode::Hover;
    bars.motion.idle = 900.;
    bars.motion.enter = 100.;
    bars.motion.exit = 180.;
    bars.vertical = Some(vertical);
    bars.horizontal = Some(horizontal);
    set_scrollbar_theme(Some(bars));
}
