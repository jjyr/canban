//! Shared styled tooltip bubble. NSH owns trigger hover/focus, delay, placement and Escape.
use super::{label, tokens::*};
use nsh_core::{element::IntoReactive, prelude::*};

/// Styled tooltip container (`SURFACE` fill, `BORDER` stroke, rounded corners, shadow).
/// Callers may adjust `.placement(..)` or `.delay(..)` and attach arbitrary children.
pub fn tooltip_box() -> nsh_base::TooltipElement {
    nsh_base::tooltip()
        .app_class(super::motion::POPUP_CLASS)
        .starting_class(super::motion::TOOLTIP_STARTING_CLASS)
        .ending_class(super::motion::TOOLTIP_ENDING_CLASS)
        .px_2p5()
        .py_1p5()
        .rounded(px(6.))
        .bg(SURFACE)
        .border_1()
        .border_color(BORDER)
        .shadow_md()
}

/// Standard single- or multi-line text tooltip using the shared bubble surface.
pub fn hint_tooltip(message: impl IntoReactive<SharedString>) -> nsh_base::TooltipElement {
    tooltip_box().child(label(message).text_size(px(CAPTION_SIZE)))
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

    /// Bare `nsh_base::tooltip()` has no background or border, rendering floating
    /// text that blends into content underneath. All tooltips must go through
    /// `tooltip_box()`, `hint_tooltip()`, or `reference_link()`.
    #[test]
    fn tooltips_use_shared_styled_surface() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        for crate_dir in ["ui-kit/src", "ui/src"] {
            rust_sources(&root.join(crate_dir), &mut files);
        }
        let allowed = [
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tooltip.rs"),
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/reference_link.rs"),
        ]
        .map(|p| p.canonicalize().unwrap());
        let needle = concat!("nsh_base::", "tooltip()");
        let offenders: Vec<_> = files
            .iter()
            .filter(|f| {
                let c = f.canonicalize().unwrap();
                !allowed.contains(&c)
            })
            .filter(|f| std::fs::read_to_string(f).unwrap().contains(needle))
            .collect();
        assert!(
            offenders.is_empty(),
            "use hint_tooltip() or tooltip_box() instead of bare nsh_base::tooltip(): {offenders:?}"
        );
    }
}
