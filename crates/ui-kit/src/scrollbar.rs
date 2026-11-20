//! Canban's overlay scrollbars, powered by `nsh-base`'s `ScrollbarTheme` and
//! Tailwind theme tokens (`scrollbar-thumb`, `scrollbar-thumb-hover`).
//!
//! [`install`] registers the global built-in scrollbar theme via
//! [`nsh_base::set_scrollbar_theme`], replacing platform indicators across all
//! `scroll()`, `list()`, and multiline `input()` views. [`vertical_scrollbar`]
//! provides an explicitly placeable vertical rail (for example in Board column
//! outer gutters paired with `scroll().scrollbar(false)`).
use nsh_base::{
    ScrollbarElement, ScrollbarEntrance, ScrollbarLook, ScrollbarMode, ScrollbarMotion,
    ScrollbarTarget, ScrollbarTheme, scrollbar, set_scrollbar_theme,
};
use nsh_core::{geometry::Axis, prelude::*};

const TRACK_SPAN: f32 = 9.0;
const RAIL_PAD_ALONG: f32 = 3.0;
const RAIL_PAD_EDGE: f32 = 1.5;
const MIN_THUMB: f32 = 32.0;

const VERTICAL_TRACK_CLASS: &str =
    "absolute inset-y-0 right-0 w-[9px] py-[3px] pr-[1.5px] items-end cursor-pointer";
const VERTICAL_THUMB_CLASS: &str = "w-[3.5px] rounded-full bg-scrollbar-thumb cursor-pointer transition-all duration-150 hover:w-[5px] hover:bg-scrollbar-thumb-hover active:w-[5px] active:bg-scrollbar-thumb-hover motion-reduce:transition-none";
const HORIZONTAL_TRACK_CLASS: &str =
    "absolute inset-x-0 bottom-0 h-[9px] px-[3px] pb-[1.5px] items-end cursor-pointer";
const HORIZONTAL_THUMB_CLASS: &str = "h-[3.5px] rounded-full bg-scrollbar-thumb cursor-pointer transition-all duration-150 hover:h-[5px] hover:bg-scrollbar-thumb-hover active:h-[5px] active:bg-scrollbar-thumb-hover motion-reduce:transition-none";

fn scrollbar_motion() -> ScrollbarMotion {
    let mut motion = ScrollbarMotion::default();
    motion.idle = 850.0;
    motion.enter = 120.0;
    motion.exit = 220.0;
    motion.entrance = ScrollbarEntrance::Fade;
    motion.thumb_hover_entrance = ScrollbarEntrance::Fade;
    motion
}

pub(crate) fn install() {
    let mut vertical = ScrollbarLook::default();
    vertical.class = Some(VERTICAL_TRACK_CLASS.into());
    vertical.thumb_class = Some(VERTICAL_THUMB_CLASS.into());

    let mut horizontal = ScrollbarLook::default();
    horizontal.class = Some(HORIZONTAL_TRACK_CLASS.into());
    horizontal.thumb_class = Some(HORIZONTAL_THUMB_CLASS.into());

    let mut theme = ScrollbarTheme::default();
    theme.mode = ScrollbarMode::Hover;
    theme.motion = scrollbar_motion();
    theme.min_thumb_length = px(MIN_THUMB);
    theme.vertical = Some(vertical);
    theme.horizontal = Some(horizontal);

    set_scrollbar_theme(Some(theme));
}

/// Explicitly placed vertical scrollbar bound to `scroll` (`scroll().scrollbar(false)`).
/// Callers position the returned element (for example in a column's outer gutter
/// via `.absolute().top_0().bottom_0().right(px(-8.))`).
pub fn vertical_scrollbar(scroll: impl Into<ScrollbarTarget>) -> ScrollbarElement {
    scrollbar()
        .scroll(scroll.into())
        .axis(Axis::Vertical)
        .mode(ScrollbarMode::Hover)
        .motion(scrollbar_motion())
        .min_thumb_length(px(MIN_THUMB))
        .thumb_class(VERTICAL_THUMB_CLASS)
        .w(px(TRACK_SPAN))
        .py(px(RAIL_PAD_ALONG))
        .pr(px(RAIL_PAD_EDGE))
        .items_end()
        .cursor_pointer()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_core::{ApproxTextEngine, Core, MountItem, NodeId, ROOT_ID, Size, geometry::size};
    use std::collections::HashMap;

    #[component]
    fn HostWithScroll() -> impl IntoElement {
        crate::tokens::install_theme();
        let col_scroll = element_ref();
        view()
            .w(px(320.0))
            .h(px(200.0))
            .gap_3()
            .child(
                scroll()
                    .w(px(300.0))
                    .h(px(120.0))
                    .child(view().w_full().h(px(640.0))),
            )
            .child(
                view()
                    .relative()
                    .w(px(260.0))
                    .h(px(120.0))
                    .child(
                        scroll()
                            .element_ref(&col_scroll)
                            .scrollbar(false)
                            .flex_1()
                            .child(view().w_full().h(px(480.0))),
                    )
                    .child(
                        vertical_scrollbar(col_scroll)
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(px(-8.0)),
                    ),
            )
    }

    #[test]
    fn builtin_scrollbar_host_emits_box_none_view_props_on_initial_mount() {
        let mut core = Core::new(ApproxTextEngine, size(px(800.0), px(600.0)));
        core.mount_root(&nsh_core::element::Root::of::<HostWithScroll>());
        let batch = core.commit().expect("initial mount batch");

        let mut scroll_nodes = Vec::new();
        let mut children_of: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut pointer_events: HashMap<NodeId, PointerEvents> = HashMap::new();
        let mut frames: HashMap<NodeId, Bounds<Points>> = HashMap::new();

        for item in &batch.items {
            match item {
                MountItem::ScrollConfig {
                    id,
                    scrollbar_visible,
                    replaced,
                    ..
                } => {
                    scroll_nodes.push((*id, *scrollbar_visible, *replaced));
                }
                MountItem::Insert { parent, child, .. } => {
                    children_of.entry(*parent).or_default().push(*child);
                }
                MountItem::ViewProps { id, props } => {
                    pointer_events.insert(*id, props.pointer_events);
                }
                MountItem::Frame { id, frame } => {
                    frames.insert(*id, *frame);
                }
                _ => {}
            }
        }

        // First scroll() uses the built-in themed scrollbar (platform indicator replaced & hidden).
        let (builtin_scroll_id, _, _) = scroll_nodes
            .iter()
            .copied()
            .find(|(_, visible, replaced)| !*visible && *replaced)
            .expect("built-in scrollbar replaces platform indicator on default scroll()");

        // Its decoration child covers the 300x120 viewport and MUST emit BoxNone on first mount
        // so clicks/drags inside scroll() pass straight through to content.
        let inserted = children_of
            .get(&builtin_scroll_id)
            .expect("scroll children");
        let decoration_host = *inserted
            .last()
            .expect("decoration scrollbar mounted last in scroll()");
        assert_eq!(
            frames.get(&decoration_host).map(|f| f.size),
            Some(Size {
                width: px(300.0),
                height: px(120.0),
            })
        );
        assert_eq!(
            pointer_events.get(&decoration_host),
            Some(&PointerEvents::BoxNone),
            "full-viewport scrollbar host must propagate BoxNone in MountItem::ViewProps on creation"
        );

        // Second scroll() opts out of built-in bar (`scrollbar(false)`) and pairs with an
        // outer-gutter `vertical_scrollbar` whose parent expands its hit-testable overflow.
        assert!(
            scroll_nodes
                .iter()
                .any(|(_, visible, replaced)| !*visible && !*replaced),
            "explicitly paired column scroll disables built-in indicator"
        );
        assert!(
            batch.items.iter().any(|item| matches!(
                item,
                MountItem::Overflow { id, extent: Some(ext) }
                    if *id != ROOT_ID && ext.origin.x.0 + ext.size.width.0 > 260.0
            )),
            "outer-gutter vertical_scrollbar extends parent hit-test overflow cleanly into column gap"
        );
    }

    #[test]
    fn scrollbar_tokens_track_named_theme_changes() {
        #[component]
        fn ProbeRoot() -> impl IntoElement {
            crate::tokens::install_theme();
            let (default_thumb, default_hover) = crate::tokens::scrollbar_thumb_colors();
            assert_ne!(default_thumb.dark, default_hover.dark);
            crate::tokens::set_theme("gruvbox");
            let (gruvbox_thumb, gruvbox_hover) = crate::tokens::scrollbar_thumb_colors();
            assert_ne!(default_thumb.dark, gruvbox_thumb.dark);
            assert_ne!(gruvbox_thumb.dark, gruvbox_hover.dark);
            view()
        }
        let mut core = Core::new(ApproxTextEngine, size(px(800.0), px(600.0)));
        core.mount_root(&nsh_core::element::Root::of::<ProbeRoot>());
        let _ = core.commit();
    }
}
