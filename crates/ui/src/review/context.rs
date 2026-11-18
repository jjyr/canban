//! Hunk context expansion: progressive gap expansion, bounded file line loading, and scroll anchoring.
use super::model::{AnchorEdge, DiffLine, parse_unified_hunks};
use crate::presenter::*;
use canban_ui_kit::tokens::*;
use canban_ui_kit::*;
use nsh_core::prelude::*;
use serde_json::{Value, json};
use std::sync::Arc;

pub const CONTEXT_STEP: u64 = 20;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExpandDirection {
    Up,
    Down,
    All,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExpandKind {
    TopOnly,
    SplitUpDown,
    MergeAll,
    BottomOnly,
}

#[derive(Clone)]
pub struct FileRenderContext {
    pub file: Value,
    pub key: String,
    pub path: String,
    pub section: String,
    pub status: String,
    pub patch: String,
    pub mode: String,
    pub compare_ref: String,
    pub compare_sha: String,
    pub cwd: Option<String>,
    pub pr_target: Option<Value>,
    pub pr_base_url: Option<String>,
    pub remote_threads: Arc<Vec<Value>>,
    pub login: Memo<Option<String>>,
}

impl FileRenderContext {
    pub fn cache_key(&self) -> String {
        format!("{}\0{}\0{}", self.mode, self.compare_sha, self.key)
    }

    pub fn preview_lines(&self) -> Option<Vec<String>> {
        self.file["preview_lines"].as_array().map(|arr| {
            arr.iter()
                .map(|v| v.as_str().unwrap_or("").to_owned())
                .collect()
        })
    }
}

pub fn anchor_ref_for(
    st: super::model::State,
    file_key: &str,
    hunk_idx: usize,
    edge: AnchorEdge,
) -> ElementRef {
    // Always mint a fresh ElementRef in the active render scope so dynamic
    // rebuilds never leave a disposed Kept<Target> cached across renders.
    let r = element_ref();
    st.anchor_targets.with(|m| {
        m.insert((file_key.to_owned(), hunk_idx, edge), r);
    });
    r
}

pub fn expanded_context_line(
    cached: &super::cache::CachedFileLines,
    old_line: u64,
    new_line: u64,
) -> DiffLine {
    let idx = new_line.saturating_sub(1) as usize;
    let text = cached.lines.get(idx).cloned().unwrap_or_default();
    DiffLine {
        kind: ' ',
        old_line: Some(old_line),
        new_line: Some(new_line),
        text,
    }
}

pub fn render_static_hunk_bar(header_text: &str) -> View {
    view().px_3().py_1().bg(INSET).child(
        label(header_text.to_owned())
            .font_family(MONO_FONT)
            .text_size(px(11.5))
            .text_color(MUTED),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_expander_bar(
    ui: Ui,
    ctx: &FileRenderContext,
    gap_idx: usize,
    above_hunk: Option<usize>,
    below_hunk: Option<usize>,
    remaining: Option<u64>,
    kind: ExpandKind,
    label_text: String,
    is_loading: bool,
    has_error: bool,
) -> View {
    let make_btn = |icon_kind: Icon,
                    tooltip_txt: String,
                    a11y: String,
                    width: f32,
                    direction: ExpandDirection| {
        let ctx_btn = ctx.clone();
        view().tooltip(hint_tooltip(tooltip_txt)).child(
            canban_ui_kit::press_target()
                .w(px(width))
                .h(px(22.))
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .bg(CLEAR)
                .hover(|s| s.bg(CARD_HOVER))
                .focus(|s| s.border_1().border_color(ACCENT))
                .disabled(move || is_loading)
                .accessibility_label(a11y)
                .on_press(move |_: &()| {
                    expand_context_gap(
                        ui,
                        ctx_btn.clone(),
                        gap_idx,
                        above_hunk,
                        below_hunk,
                        direction,
                    );
                })
                .child(icon_tinted(icon_kind, MUTED).w(px(13.)).h(px(13.))),
        )
    };

    let gutter = match kind {
        ExpandKind::TopOnly => {
            let tip = match remaining {
                Some(r) if r <= CONTEXT_STEP => {
                    format!(
                        "Expand remaining {r} line{} above",
                        if r == 1 { "" } else { "s" }
                    )
                }
                _ => format!("Expand {CONTEXT_STEP} lines above"),
            };
            view()
                .w(px(74.))
                .flex_shrink_0()
                .flex_row()
                .items_center()
                .justify_center()
                .child(make_btn(
                    Icon::ArrowUp,
                    tip.clone(),
                    tip,
                    60.,
                    ExpandDirection::Up,
                ))
        }
        ExpandKind::BottomOnly => {
            let tip = match remaining {
                Some(r) if r <= CONTEXT_STEP => {
                    format!(
                        "Expand remaining {r} line{} below",
                        if r == 1 { "" } else { "s" }
                    )
                }
                _ => format!("Expand {CONTEXT_STEP} lines below"),
            };
            view()
                .w(px(74.))
                .flex_shrink_0()
                .flex_row()
                .items_center()
                .justify_center()
                .child(make_btn(
                    Icon::ArrowDown,
                    tip.clone(),
                    tip,
                    60.,
                    ExpandDirection::Down,
                ))
        }
        ExpandKind::MergeAll => {
            let r = remaining.unwrap_or(CONTEXT_STEP);
            let tip = format!(
                "Expand all {r} hidden line{}",
                if r == 1 { "" } else { "s" }
            );
            view()
                .w(px(74.))
                .flex_shrink_0()
                .flex_row()
                .items_center()
                .justify_center()
                .child(make_btn(
                    Icon::ChevronsUpDown,
                    tip.clone(),
                    tip,
                    60.,
                    ExpandDirection::All,
                ))
        }
        ExpandKind::SplitUpDown => view()
            .w(px(74.))
            .flex_shrink_0()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_1()
            .child(make_btn(
                Icon::ArrowUp,
                format!("Expand {CONTEXT_STEP} lines up"),
                format!("Expand {CONTEXT_STEP} lines up"),
                34.,
                ExpandDirection::Up,
            ))
            .child(make_btn(
                Icon::ArrowDown,
                format!("Expand {CONTEXT_STEP} lines down"),
                format!("Expand {CONTEXT_STEP} lines down"),
                34.,
                ExpandDirection::Down,
            )),
    };

    view()
        .w_full()
        .min_h(px(26.))
        .px_2()
        .py_0p5()
        .flex_row()
        .items_center()
        .gap_2()
        .bg(INSET)
        .child(gutter)
        .child(
            label(label_text)
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .font_family(MONO_FONT)
                .text_size(px(11.5))
                .text_color(if has_error { DANGER } else { MUTED }),
        )
}

pub fn expand_context_gap(
    ui: Ui,
    ctx: FileRenderContext,
    gap_idx: usize,
    above_hunk: Option<usize>,
    below_hunk: Option<usize>,
    direction: ExpandDirection,
) {
    let st = ui.review_state;
    let cache_key = ctx.cache_key();
    let has_cached = st.file_lines_cache.with(|c| c.get(&cache_key).is_some());

    if !has_cached && crate::APP.get().is_none() {
        if let Some(lines) = ctx.preview_lines() {
            let total = lines.len();
            st.file_lines_cache
                .with(|c| c.insert(cache_key.clone(), lines, total));
        } else {
            return;
        }
    }

    let already_cached = st.file_lines_cache.with(|c| c.get(&cache_key).is_some());

    if already_cached {
        ui.spawn(async move {
            apply_expanded_with_scroll_anchor(ui, ctx, gap_idx, above_hunk, below_hunk, direction)
                .await;
        });
        return;
    }

    if st
        .file_lines_loading
        .with_untracked(|s| s.contains(&ctx.key))
    {
        return;
    }

    let ticket = st.compare_ticket.get_untracked();
    let context = st.read_context.with(|v| v.clone());
    let file_key = ctx.key.clone();
    let project_id = ui.project.get_untracked();
    let card_id = ui.card.get_untracked();

    let (method, params, proj_scope): (&'static str, Value, Option<String>) =
        if ctx.mode == "github_pr" {
            let Some(ref pr) = ctx.pr_target else {
                return;
            };
            let repo = pr["repository"].as_str().unwrap_or("");
            let blob_sha = ctx.file["sha"].as_str().unwrap_or("");
            (
                "review.github_pr_file",
                json!({
                    "repository": repo,
                    "sha": ctx.compare_sha,
                    "blob_sha": blob_sha,
                    "path": ctx.path,
                }),
                None,
            )
        } else {
            let source = if ctx.section == "staged" || ctx.compare_ref == "STAGED" {
                "index"
            } else if ctx.section == "unstaged"
                || matches!(
                    ctx.compare_ref.as_str(),
                    "" | "UNCOMMITTED" | "WORKDIR" | "UNSTAGED"
                )
                || (ctx.mode == "card_local" && ctx.cwd.is_some())
            {
                "workdir"
            } else {
                "rev"
            };
            let rev = if !ctx.compare_sha.is_empty() {
                ctx.compare_sha.clone()
            } else {
                ctx.compare_ref.clone()
            };
            let mut p = json!({
                "project_id": project_id,
                "path": ctx.path,
                "source": source,
                "rev": rev,
            });
            if let Some(ref cwd) = ctx.cwd {
                p["cwd"] = json!(cwd);
            }
            if !card_id.is_empty() {
                p["card_id"] = json!(card_id);
            }
            ("git.file_lines", p, Some(project_id.clone()))
        };

    st.file_lines_loading.update(|s| {
        s.insert(file_key.clone());
    });
    st.file_lines_errors.update(|m| {
        m.remove(&file_key);
    });

    ui.spawn(async move {
        let res = blocking(move || {
            crate::routed_call(method, params, proj_scope.as_deref()).map_err(|e| format!("{e:#}"))
        })
        .await;

        if st.compare_ticket.try_get() != Some(ticket) || st.read_context.with(|v| *v != context) {
            return;
        }
        let payload = match res.map_err(|err| err.to_string()).and_then(|result| result) {
            Ok(val) => val,
            Err(err) => {
                batch(|| {
                    st.file_lines_loading.update(|s| {
                        s.remove(&file_key);
                    });
                    st.file_lines_errors.update(|m| {
                        m.insert(file_key, err);
                    });
                });
                return;
            }
        };

        let lines: Vec<String> = payload["lines"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|v| v.as_str().unwrap_or("").to_owned())
            .collect();
        let total_lines = payload["total_lines"]
            .as_u64()
            .map(|n| n as usize)
            .unwrap_or(lines.len());
        st.file_lines_cache
            .with(|c| c.insert(cache_key, lines, total_lines));

        apply_expanded_with_scroll_anchor(ui, ctx, gap_idx, above_hunk, below_hunk, direction)
            .await;
    });
}

struct ViewportAnchor {
    scroll_y: f32,
    line_y: f32,
    content_overflow: f32,
}

impl ViewportAnchor {
    fn new(
        scroll: nsh_core::Measurement,
        content: nsh_core::Measurement,
        anchor: nsh_core::Measurement,
    ) -> Self {
        Self {
            scroll_y: scroll.page.y.0 - content.page.y.0,
            line_y: anchor.page.y.0 - content.page.y.0,
            content_overflow: content.frame.size.height.0 - scroll.frame.size.height.0,
        }
    }
}

async fn measure_viewport_anchor(
    vp: super::model::DiffViewport,
    anchor: &ElementRef,
) -> Option<ViewportAnchor> {
    // Queue all three measurements before awaiting so Core answers them in one commit.
    let s = vp.scroll.measure();
    let c = vp.content.measure();
    let a = anchor.measure();
    let (sb, cb, ab) = (s.await?, c.await?, a.await?);
    Some(ViewportAnchor::new(sb, cb, ab))
}

async fn apply_expanded_with_scroll_anchor(
    ui: Ui,
    ctx: FileRenderContext,
    gap_idx: usize,
    above_hunk: Option<usize>,
    below_hunk: Option<usize>,
    direction: ExpandDirection,
) {
    let st = ui.review_state;
    let viewport = st.viewport.with(|v| *v);

    // Pick the hunk edge that must remain visually pinned in the viewport,
    // and measure both viewport + anchor in a single commit round-trip.
    let (anchor_key, pre_anchor) = match (viewport, direction) {
        (Some(vp), ExpandDirection::All) => match (above_hunk, below_hunk) {
            (Some(above), Some(below)) => {
                let above_key = (ctx.key.clone(), above, AnchorEdge::HunkEnd);
                let below_key = (ctx.key.clone(), below, AnchorEdge::HunkStart);
                let refs = st
                    .anchor_targets
                    .with(|m| (m.get(&above_key).copied(), m.get(&below_key).copied()));
                match refs {
                    (Some(aref), Some(bref)) => {
                        let s = vp.scroll.measure();
                        let c = vp.content.measure();
                        let a = aref.measure();
                        let b = bref.measure();
                        if let (Some(sb), Some(cb), Some(ab), Some(bb)) =
                            (s.await, c.await, a.await, b.await)
                        {
                            let use_below = ab.page.y.0 < sb.page.y.0;
                            let chosen_key = if use_below { below_key } else { above_key };
                            let chosen = if use_below { bb } else { ab };
                            (Some(chosen_key), Some(ViewportAnchor::new(sb, cb, chosen)))
                        } else {
                            (Some(above_key), None)
                        }
                    }
                    (Some(aref), None) => {
                        let m = measure_viewport_anchor(vp, &aref).await;
                        (Some(above_key), m)
                    }
                    (None, Some(bref)) => {
                        let m = measure_viewport_anchor(vp, &bref).await;
                        (Some(below_key), m)
                    }
                    (None, None) => (None, None),
                }
            }
            (None, Some(below)) => {
                let key = (ctx.key.clone(), below, AnchorEdge::HunkStart);
                let m = match st.anchor_targets.with(|m| m.get(&key).copied()) {
                    Some(r) => measure_viewport_anchor(vp, &r).await,
                    None => None,
                };
                (Some(key), m)
            }
            (Some(above), None) => {
                let key = (ctx.key.clone(), above, AnchorEdge::HunkEnd);
                let m = match st.anchor_targets.with(|m| m.get(&key).copied()) {
                    Some(r) => measure_viewport_anchor(vp, &r).await,
                    None => None,
                };
                (Some(key), m)
            }
            (None, None) => (None, None),
        },
        (_, ExpandDirection::Up) => {
            let key = below_hunk.map(|h| (ctx.key.clone(), h, AnchorEdge::HunkStart));
            let m = match (viewport, key.as_ref()) {
                (Some(vp), Some(k)) => match st.anchor_targets.with(|m| m.get(k).copied()) {
                    Some(r) => measure_viewport_anchor(vp, &r).await,
                    None => None,
                },
                _ => None,
            };
            (key, m)
        }
        (_, ExpandDirection::Down) => {
            let key = above_hunk.map(|h| (ctx.key.clone(), h, AnchorEdge::HunkEnd));
            let m = match (viewport, key.as_ref()) {
                (Some(vp), Some(k)) => match st.anchor_targets.with(|m| m.get(k).copied()) {
                    Some(r) => measure_viewport_anchor(vp, &r).await,
                    None => None,
                },
                _ => None,
            };
            (key, m)
        }
        (None, ExpandDirection::All) => (
            above_hunk
                .map(|h| (ctx.key.clone(), h, AnchorEdge::HunkEnd))
                .or_else(|| below_hunk.map(|h| (ctx.key.clone(), h, AnchorEdge::HunkStart))),
            None,
        ),
    };

    let hunks = parse_unified_hunks(&ctx.patch);
    let total_lines = st
        .file_lines_cache
        .with(|c| c.get(&ctx.cache_key()).map(|e| e.total_lines as u64));

    batch(|| {
        // Clearing the loading state rebuilds the rows, so measure the original
        // anchor first and clear it in the same update that expands the gap.
        st.file_lines_loading.update(|files| {
            files.remove(&ctx.key);
        });
        st.file_lines_revision.update(|n| *n += 1);
        st.expanded_gaps.update(|map| {
            let entry = map.entry((ctx.key.clone(), gap_idx)).or_default();
            if gap_idx == 0 {
                if let Some(first) = hunks.first() {
                    let gap_len = first.entry_new().saturating_sub(1);
                    let rem = gap_len.saturating_sub(entry.from_bottom);
                    let step = if direction == ExpandDirection::All {
                        rem
                    } else {
                        rem.min(CONTEXT_STEP)
                    };
                    entry.from_bottom = (entry.from_bottom + step).min(gap_len);
                }
            } else if gap_idx < hunks.len() {
                let gap_len = hunks[gap_idx]
                    .entry_new()
                    .saturating_sub(hunks[gap_idx - 1].exit_new());
                let rem = gap_len.saturating_sub(entry.from_top + entry.from_bottom);
                match direction {
                    ExpandDirection::Up => {
                        entry.from_bottom = (entry.from_bottom + rem.min(CONTEXT_STEP))
                            .min(gap_len - entry.from_top);
                    }
                    ExpandDirection::Down => {
                        entry.from_top = (entry.from_top + rem.min(CONTEXT_STEP))
                            .min(gap_len - entry.from_bottom);
                    }
                    ExpandDirection::All => {
                        entry.from_bottom = (entry.from_bottom + rem).min(gap_len - entry.from_top);
                    }
                }
            } else if let (Some(last), Some(total)) = (hunks.last(), total_lines) {
                let gap_end = (total + 1).max(last.exit_new());
                let gap_len = gap_end.saturating_sub(last.exit_new());
                let rem = gap_len.saturating_sub(entry.from_top);
                let step = if direction == ExpandDirection::All {
                    rem
                } else {
                    rem.min(CONTEXT_STEP)
                };
                entry.from_top = (entry.from_top + step).min(gap_len);
            }
        });
    });

    if let (Some(vp), Some(akey), Some(before)) = (viewport, anchor_key, pre_anchor) {
        next_frame().await;
        next_frame().await;
        if let Some(aref) = st.anchor_targets.with(|m| m.get(&akey).copied())
            && let Some(after) = measure_viewport_anchor(vp, &aref).await
        {
            let target_scroll_y = (before.scroll_y + after.line_y - before.line_y).max(0.);
            if (target_scroll_y - after.scroll_y).abs() > 0.5 {
                // Short diffs need space after the last file for the native scroll view
                // to reach the offset that keeps the original hunk in place.
                let extra = target_scroll_y - after.content_overflow;
                if extra > 0. {
                    vp.bottom_space.update(|height| *height += extra);
                    next_frame().await;
                }
                vp.scroll
                    .scroll_to(Point::new(px(0.), px(target_scroll_y)), false);
            }
        }
    }
}
