//! Shared overlay indicators. NSH macOS owns native dark-mode overlay scrollers,
//! scroll metrics, thumb dragging and hit testing without blocking scroll view content.
use nsh_base::set_scrollbar_theme;

pub(crate) fn install(_theme: &canban_themes::Theme) {
    // Keep platform overlay scrollers active: NSH 0.5's custom built-in scrollbar
    // decoration host mounts over the full scroll viewport without propagating
    // BoxNone pointer passthrough on macOS, intercepting clicks and drags inside scroll().
    set_scrollbar_theme(None);
}
