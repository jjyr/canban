//! Platform-independent terminal view state shared across Apple (macOS/iOS) and Linux hosts.
//!
//! Covers cursor blink timing, optimistic CJK/IME commit cursor hold, scrollbar fade timing,
//! link hover spans, dirty-row invalidation planning, and animation timer interval scheduling.

/// Grace period (seconds) to hold an optimistic cursor position after an IME commit
/// while waiting for the PTY to echo the committed text.
pub const COMMIT_HOLD_GRACE_SECS: f64 = 0.12;

/// Window (seconds) after `unmarkText` during which a follow-up `insertText:`
/// (two-step XPC IME commit) still activates optimistic cursor hold.
pub const RECENT_UNMARK_WINDOW_SECS: f64 = 0.06;

/// Cursor blink half-period in seconds (~530ms, matching standard macOS/terminal cadence).
pub const CURSOR_BLINK_INTERVAL_SECS: f64 = 0.53;

/// Duration (seconds) the scrollbar thumb stays fully visible after scroll offset/total changes.
pub const SCROLLBAR_HOLD_SECS: f64 = 0.9;

/// Additional fade-out window (seconds) after `SCROLLBAR_HOLD_SECS` during which the timer ticks.
pub const SCROLLBAR_FADE_TICK_SECS: f64 = 0.25;

/// Visible fade-out tail (seconds) used when computing scrollbar thumb opacity.
pub const SCROLLBAR_FADE_DRAW_SECS: f64 = 0.20;

/// Returns true if `text` is non-empty and consists only of single-line printable characters
/// (no ASCII control characters, DEL, or AppKit function-key PUA U+F700..=U+F8FF).
pub fn is_single_line_printable(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    text.chars().all(|ch| {
        let cp = ch as u32;
        cp >= 0x20 && cp != 0x7F && !(0xF700..=0xF8FF).contains(&cp)
    })
}

/// Returns true if `text` contains any non-ASCII codepoint (`>= U+0080`).
pub fn contains_non_ascii(text: &str) -> bool {
    !text.is_ascii()
}

/// Optimistic CJK/IME commit cursor hold state.
///
/// When an IME commits CJK text or punctuation, the PTY echo can lag by a few milliseconds.
/// Holding the cursor at the expected post-commit column avoids flashing the cursor back to
/// the pre-commit column and keeps consecutive preedit candidate windows anchored smoothly.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommitCursorHold {
    pub active: bool,
    pub origin_x: u16,
    pub origin_y: u16,
    pub target_x: u16,
    pub target_y: u16,
    pub hold_until: f64,
    pub last_unmark_time: f64,
}

impl CommitCursorHold {
    pub fn record_unmark(&mut self, now: f64) {
        self.last_unmark_time = now;
    }

    pub fn clear(&mut self) -> Option<(u16, u16)> {
        if !self.active {
            return None;
        }
        let rows = (self.origin_y, self.target_y);
        self.active = false;
        self.hold_until = 0.0;
        Some(rows)
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Records an optimistic cursor hold for a committed text run.
    ///
    /// Returns `Some(row)` when a row should be invalidated (either because the hold was
    /// activated/updated or because a non-printable commit cleared an existing hold).
    #[allow(clippy::too_many_arguments)]
    pub fn record_commit(
        &mut self,
        text: &str,
        cell_width: usize,
        had_marked: bool,
        cursor_visible: bool,
        cursor_x: u16,
        cursor_y: u16,
        cols: u16,
        rows: u16,
        now: f64,
    ) -> Option<u16> {
        if !cursor_visible || cols == 0 || rows == 0 {
            return None;
        }
        if !is_single_line_printable(text) {
            return self.clear().map(|(_, target_y)| target_y);
        }
        let recent_unmark = self.last_unmark_time > 0.0
            && (now - self.last_unmark_time) <= RECENT_UNMARK_WINDOW_SECS;
        let should_hold = had_marked || recent_unmark || contains_non_ascii(text);
        if !should_hold || cell_width == 0 {
            return None;
        }

        let cur_x = cursor_x.min(cols - 1);
        let cur_y = cursor_y.min(rows - 1);
        let base_x = if self.active
            && now < self.hold_until
            && cur_x == self.origin_x
            && cur_y == self.origin_y
        {
            self.target_x
        } else {
            self.origin_x = cur_x;
            self.origin_y = cur_y;
            cur_x
        };
        let next_x = ((base_x as usize).saturating_add(cell_width)).min((cols - 1) as usize) as u16;
        self.target_x = next_x;
        self.target_y = cur_y;
        self.hold_until = now + COMMIT_HOLD_GRACE_SECS;
        self.active = true;
        Some(cur_y)
    }

    /// Computes the effective `(x, y, is_optimistic)` cursor position for drawing and IME rects.
    pub fn effective_cursor(
        &self,
        cursor_visible: bool,
        cursor_x: u16,
        cursor_y: u16,
        cols: u16,
        rows: u16,
        now: f64,
    ) -> (u16, u16, bool) {
        if cols == 0 || rows == 0 {
            return (0, 0, false);
        }
        let cur_x = cursor_x.min(cols - 1);
        let cur_y = cursor_y.min(rows - 1);
        if self.active
            && cursor_visible
            && now < self.hold_until
            && cur_x == self.origin_x
            && cur_y == self.origin_y
        {
            return (
                self.target_x.min(cols - 1),
                self.target_y.min(rows - 1),
                true,
            );
        }
        (cur_x, cur_y, false)
    }

    /// Reconciles the hold against a freshly polled VT snapshot.
    ///
    /// Clears the hold if the cursor became hidden, moved away from the pre-commit origin
    /// (PTY echo arrived), or the grace deadline elapsed.
    pub fn reconcile_snapshot(
        &mut self,
        cursor_visible: bool,
        cursor_x: u16,
        cursor_y: u16,
        now: f64,
    ) -> Option<(u16, u16)> {
        if !self.active {
            return None;
        }
        if !cursor_visible
            || cursor_x != self.origin_x
            || cursor_y != self.origin_y
            || now >= self.hold_until
        {
            return self.clear();
        }
        None
    }

    /// Expires the hold if `now >= hold_until`, returning `(origin_y, target_y)` to repaint.
    pub fn expire_if_due(&mut self, now: f64) -> Option<(u16, u16)> {
        if self.active && now >= self.hold_until {
            return self.clear();
        }
        None
    }
}

/// Cursor blink visibility & cadence state.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorBlinkState {
    pub visible: bool,
    pub last_toggle_time: f64,
}

impl Default for CursorBlinkState {
    fn default() -> Self {
        Self {
            visible: true,
            last_toggle_time: 0.0,
        }
    }
}

impl CursorBlinkState {
    pub fn new(now: f64) -> Self {
        Self {
            visible: true,
            last_toggle_time: now,
        }
    }

    pub fn reset(&mut self, now: f64) {
        self.visible = true;
        self.last_toggle_time = now;
    }

    /// Advances blink state at `now`. Returns `true` if visibility changed and the cursor row
    /// should be invalidated.
    pub fn tick(
        &mut self,
        focused: bool,
        cursor_visible: bool,
        cursor_blinking: bool,
        now: f64,
    ) -> bool {
        if focused && cursor_visible && cursor_blinking {
            if now - self.last_toggle_time >= CURSOR_BLINK_INTERVAL_SECS {
                self.last_toggle_time = now;
                self.visible = !self.visible;
                return true;
            }
            false
        } else if !self.visible {
            self.visible = true;
            true
        } else {
            false
        }
    }

    /// Whether the cursor should be rendered on the current frame.
    pub fn should_draw_cursor(
        &self,
        focused: bool,
        cursor_visible: bool,
        cursor_blinking: bool,
    ) -> bool {
        cursor_visible && (!focused || !cursor_blinking || self.visible)
    }
}

/// Overlay scrollbar fade timing state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScrollbarFadeState {
    pub visible_until: f64,
    pub prev_offset: u64,
    pub prev_total: u64,
}

impl ScrollbarFadeState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn on_snapshot(&mut self, total: u64, len: u64, offset: u64, now: f64) {
        if total > len && (offset != self.prev_offset || total != self.prev_total) {
            self.visible_until = now + SCROLLBAR_HOLD_SECS;
        }
        self.prev_offset = offset;
        self.prev_total = total;
    }

    pub fn needs_fade_tick(&self, now: f64) -> bool {
        now < self.visible_until + SCROLLBAR_FADE_TICK_SECS
    }

    /// Computes scrollbar thumb alpha (`0.0..=0.42`) for the current frame.
    pub fn thumb_alpha(&self, total: u64, len: u64, offset: u64, now: f64) -> f64 {
        if total <= len {
            return 0.0;
        }
        let scrolled_back = offset.saturating_add(len) < total;
        if scrolled_back {
            0.42
        } else if now < self.visible_until + SCROLLBAR_FADE_DRAW_SECS {
            ((self.visible_until + SCROLLBAR_FADE_DRAW_SECS - now) * 2.1).clamp(0.0, 0.42)
        } else {
            0.0
        }
    }
}

/// Currently hovered OSC-8 / auto-detected link span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverLinkState {
    pub row: u16,
    pub start_col: u16,
    pub end_col: u16,
    pub uri: String,
}

impl HoverLinkState {
    pub fn contains(&self, row: u16, col: u16) -> bool {
        self.row == row && col >= self.start_col && col <= self.end_col
    }

    pub fn same_span(&self, row: u16, start_col: u16, end_col: u16, uri: &str) -> bool {
        self.row == row && self.start_col == start_col && self.end_col == end_col && self.uri == uri
    }
}

/// Dirty invalidation plan produced after polling a render snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirtyPlan {
    None,
    Full,
    Rows(Vec<u16>),
}

/// Computes which rows (or full viewport) must be repainted for a polled snapshot.
pub fn plan_dirty_invalidation(
    force_full: bool,
    dirty_state: u8,
    dirty_rows: &[u8],
    rows: u16,
    old_cursor_y: Option<u16>,
    new_cursor_y: Option<u16>,
    images_changed: bool,
) -> DirtyPlan {
    if force_full || dirty_state == 2 || images_changed {
        return DirtyPlan::Full;
    }
    if dirty_state != 1 {
        return DirtyPlan::None;
    }
    let mut out = Vec::new();
    let max_rows = (rows as usize).min(dirty_rows.len());
    for (r, &flag) in dirty_rows.iter().take(max_rows).enumerate() {
        if flag != 0 {
            out.push(r as u16);
        }
    }
    for cy in [old_cursor_y, new_cursor_y].into_iter().flatten() {
        if cy < rows && !out.contains(&cy) {
            out.push(cy);
        }
    }
    out.sort_unstable();
    DirtyPlan::Rows(out)
}

/// Inputs to animation timer interval calculation.
#[derive(Clone, Copy, Debug, Default)]
pub struct AnimScheduleInput {
    pub focused: bool,
    pub cursor_visible: bool,
    pub cursor_blinking: bool,
    pub autoscrolling: bool,
    pub next_kitty_animation_ms: u32,
}

/// Combined platform-independent terminal view state.
#[derive(Clone, Debug, Default)]
pub struct TerminalViewState {
    pub commit_hold: CommitCursorHold,
    pub cursor_blink: CursorBlinkState,
    pub scrollbar: ScrollbarFadeState,
    pub hover_link: Option<HoverLinkState>,
    pub pending_layout_redraws: usize,
    pub prev_image_placements_gen: u64,
    pub prev_image_placements_len: usize,
}

impl TerminalViewState {
    pub fn new(now: f64) -> Self {
        Self {
            cursor_blink: CursorBlinkState::new(now),
            ..Self::default()
        }
    }

    pub fn reset_for_rebind(&mut self, now: f64) {
        self.commit_hold.reset();
        self.cursor_blink.reset(now);
        self.scrollbar.reset();
        self.hover_link = None;
        self.prev_image_placements_gen = 0;
        self.prev_image_placements_len = 0;
    }

    /// Updates tracked Kitty image placement metadata and returns whether placements changed.
    pub fn update_image_placements(&mut self, generation: u64, len: usize) -> bool {
        let changed =
            generation != self.prev_image_placements_gen || len != self.prev_image_placements_len;
        self.prev_image_placements_gen = generation;
        self.prev_image_placements_len = len;
        changed
    }

    /// Updates hovered link state; returns rows that should be invalidated if the hover changed.
    pub fn set_hover_link(&mut self, next: Option<HoverLinkState>) -> Vec<u16> {
        let same = match (&self.hover_link, &next) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_span(b.row, b.start_col, b.end_col, &b.uri),
            _ => false,
        };
        if same {
            return Vec::new();
        }
        let mut dirty = Vec::new();
        if let Some(prev) = &self.hover_link {
            dirty.push(prev.row);
        }
        if let Some(curr) = &next
            && !dirty.contains(&curr.row)
        {
            dirty.push(curr.row);
        }
        self.hover_link = next;
        dirty
    }

    /// Computes the desired animation timer interval in seconds, or `None` when idle.
    pub fn desired_timer_interval_secs(&self, input: AnimScheduleInput, now: f64) -> Option<f64> {
        let need_cursor_blink = input.focused && input.cursor_visible && input.cursor_blinking;
        let need_autoscroll = input.autoscrolling;
        let need_scrollbar_fade = self.scrollbar.needs_fade_tick(now);
        let need_layout_settle = self.pending_layout_redraws > 0;
        let need_commit_hold = self.commit_hold.active;
        let need_kitty_anim = input.next_kitty_animation_ms > 0;

        if !(need_cursor_blink
            || need_autoscroll
            || need_scrollbar_fade
            || need_layout_settle
            || need_commit_hold
            || need_kitty_anim)
        {
            return None;
        }

        let mut interval: f64 = if need_autoscroll || need_commit_hold {
            0.033
        } else {
            0.08
        };
        if need_kitty_anim {
            let kitty_interval = (input.next_kitty_animation_ms as f64 / 1000.0).clamp(0.016, 0.5);
            interval = interval.min(kitty_interval);
        }
        Some(interval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_cursor_hold_accumulates_cjk_commits_and_reconciles_on_echo_or_timeout() {
        let mut hold = CommitCursorHold::default();
        let t0 = 10.0;

        // 1. Commit "你好" (4 cells) from col 2 with prior marked text.
        let dirty = hold.record_commit("你好", 4, true, true, 2, 0, 80, 24, t0);
        assert_eq!(dirty, Some(0));
        assert!(hold.active);
        assert_eq!(
            hold.effective_cursor(true, 2, 0, 80, 24, t0 + 0.02),
            (6, 0, true)
        );

        // 2. Consecutive CJK punctuation "，" (2 cells, no prior marked text) accumulates to col 8.
        let dirty2 = hold.record_commit("，", 2, false, true, 2, 0, 80, 24, t0 + 0.03);
        assert_eq!(dirty2, Some(0));
        assert_eq!(
            hold.effective_cursor(true, 2, 0, 80, 24, t0 + 0.04),
            (8, 0, true)
        );

        // 3. When PTY echo moves cursor to col 8, reconcile clears hold cleanly.
        assert_eq!(hold.reconcile_snapshot(true, 8, 0, t0 + 0.05), Some((0, 0)));
        assert!(!hold.active);
        assert_eq!(
            hold.effective_cursor(true, 8, 0, 80, 24, t0 + 0.05),
            (8, 0, false)
        );

        // 4. Two-step XPC IME commit: unmarkText followed by insertText within 60ms.
        hold.record_unmark(t0 + 1.0);
        assert_eq!(
            hold.record_commit("a", 1, false, true, 8, 0, 80, 24, t0 + 1.02),
            Some(0)
        );
        assert_eq!(
            hold.effective_cursor(true, 8, 0, 80, 24, t0 + 1.05),
            (9, 0, true)
        );

        // 5. Non-echo timeout expires hold back to VT cursor.
        assert_eq!(hold.expire_if_due(t0 + 1.20), Some((0, 0)));
        assert!(!hold.active);
        assert_eq!(
            hold.effective_cursor(true, 8, 0, 80, 24, t0 + 1.20),
            (8, 0, false)
        );
    }

    #[test]
    fn commit_cursor_hold_clears_on_control_chars_and_clamps_to_cols() {
        let mut hold = CommitCursorHold::default();
        assert_eq!(
            hold.record_commit("你好", 4, true, true, 78, 3, 80, 24, 5.0),
            Some(3)
        );
        assert_eq!(
            hold.effective_cursor(true, 78, 3, 80, 24, 5.01),
            (79, 3, true)
        );

        // Non-printable text (newline) clears active hold.
        assert_eq!(
            hold.record_commit("\n", 1, false, true, 78, 3, 80, 24, 5.02),
            Some(3)
        );
        assert!(!hold.active);
    }

    #[test]
    fn cursor_blink_toggles_at_interval_and_stays_visible_when_unfocused() {
        let mut blink = CursorBlinkState::new(1.0);
        assert!(blink.should_draw_cursor(true, true, true));

        // Before 0.53s: no toggle.
        assert!(!blink.tick(true, true, true, 1.40));
        assert!(blink.visible);

        // At 0.53s: toggles off.
        assert!(blink.tick(true, true, true, 1.54));
        assert!(!blink.visible);
        assert!(!blink.should_draw_cursor(true, true, true));

        // Unfocused: stays drawn and resets visible on tick.
        assert!(blink.should_draw_cursor(false, true, true));
        assert!(blink.tick(false, true, true, 1.60));
        assert!(blink.visible);
    }

    #[test]
    fn scrollbar_fade_tracks_scroll_activity_and_fades_out() {
        let mut sb = ScrollbarFadeState::default();
        sb.on_snapshot(100, 24, 76, 2.0);
        assert!(sb.needs_fade_tick(2.5));
        assert!((sb.thumb_alpha(100, 24, 76, 2.5) - 0.42).abs() < 1e-6);

        // Scrolled back keeps constant 0.42 alpha even after fade window.
        assert!((sb.thumb_alpha(100, 24, 40, 10.0) - 0.42).abs() < 1e-6);

        // At bottom after fade window: hidden and no timer needed.
        assert!(!sb.needs_fade_tick(3.2));
        assert_eq!(sb.thumb_alpha(100, 24, 76, 3.2), 0.0);
    }

    #[test]
    fn dirty_plan_handles_full_incremental_and_cursor_rows() {
        assert_eq!(
            plan_dirty_invalidation(true, 0, &[0, 0, 0], 3, None, None, false),
            DirtyPlan::Full
        );
        assert_eq!(
            plan_dirty_invalidation(false, 2, &[0, 0, 0], 3, None, None, false),
            DirtyPlan::Full
        );
        assert_eq!(
            plan_dirty_invalidation(false, 1, &[0, 1, 0, 0], 4, Some(0), Some(3), false),
            DirtyPlan::Rows(vec![0, 1, 3])
        );
        assert_eq!(
            plan_dirty_invalidation(false, 0, &[0, 0], 2, None, None, true),
            DirtyPlan::Full
        );
        assert_eq!(
            plan_dirty_invalidation(false, 0, &[0, 0], 2, None, None, false),
            DirtyPlan::None
        );
    }

    #[test]
    fn view_state_hover_and_anim_timer_scheduling() {
        let mut state = TerminalViewState::new(10.0);
        // Idle -> no timer needed.
        assert_eq!(
            state.desired_timer_interval_secs(AnimScheduleInput::default(), 10.0),
            None
        );
        // Blink -> 80ms.
        assert_eq!(
            state.desired_timer_interval_secs(
                AnimScheduleInput {
                    focused: true,
                    cursor_visible: true,
                    cursor_blinking: true,
                    ..Default::default()
                },
                10.0
            ),
            Some(0.08)
        );
        // Kitty animation faster than 80ms -> clamped to 20ms.
        assert_eq!(
            state.desired_timer_interval_secs(
                AnimScheduleInput {
                    next_kitty_animation_ms: 20,
                    ..Default::default()
                },
                10.0
            ),
            Some(0.02)
        );
        // Hover link diff returns affected rows.
        let link1 = HoverLinkState {
            row: 2,
            start_col: 4,
            end_col: 12,
            uri: "https://canban.dev".into(),
        };
        assert_eq!(state.set_hover_link(Some(link1.clone())), vec![2]);
        assert!(state.set_hover_link(Some(link1)).is_empty());
        let link2 = HoverLinkState {
            row: 5,
            start_col: 0,
            end_col: 8,
            uri: "https://example.com".into(),
        };
        assert_eq!(state.set_hover_link(Some(link2)), vec![2, 5]);
        assert_eq!(state.set_hover_link(None), vec![5]);
    }
}
