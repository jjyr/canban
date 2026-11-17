//! Antigravity (`agy`) TUI bottom-chrome status refinement.
//!
//! When `agy` finishes its foreground turn and returns to the `>` prompt while a Swarm,
//! subagent, or background command/schedule is still active, its bottom TUI renders an
//! active-items tray delimited by horizontal rule lines (`───...`) above the footer bar:
//!
//! ```text
//! ────────────────────────────────────────────────────────────────────────────────
//! >
//! ────────────────────────────────────────────────────────────────────────────────
//!   ● Swarm(Feature Audit)  reviewer: checking edge cases · 4 posts · alt+s board · alt+x dismiss
//!   ● Agent(worker: Codebase Researcher)  Working · 01m12s
//!   ● [21:54:14] cargo nextest run --workspace running
//! ────────────────────────────────────────────────────────────────────────────────
//! ? for shortcuts              accept-edits · Gemini 4 Carbon · low · 1 subagent(s) · 1 task(s) · /tasks
//! ```
//!
//! Crucially, inspection is anchored strictly to the bottom TUI chrome (or full-screen
//! `/swarm board` / `/tasks` views), so user prompts or conversation history above the
//! prompt box discussing `"Swarm"` or `"xxxxxx running"` never cause false positives.

use super::RefineDecision;

pub const REASON_SWARM: &str = "agy.swarm";
pub const REASON_BACKGROUND_TASK: &str = "agy.background_task";

/// Inspect the visible screen text of an `idle` / `done` `agy` terminal.
pub fn refine_screen(screen_text: &str) -> RefineDecision {
    let mut lines: Vec<&str> = screen_text.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return RefineDecision::Keep;
    }

    if let Some(decision) = check_fullscreen_views(&lines) {
        return decision;
    }

    let chrome = extract_bottom_chrome(&lines);
    for row in &chrome.tray_lines {
        if is_active_swarm_or_subagent_row(row) {
            return RefineDecision::Working {
                reason: REASON_SWARM,
            };
        }
    }
    for footer in &chrome.footer_lines {
        if has_positive_counter(footer, "subagent(s)") {
            return RefineDecision::Working {
                reason: REASON_SWARM,
            };
        }
    }
    for row in &chrome.tray_lines {
        if is_running_background_task_row(row) {
            return RefineDecision::Working {
                reason: REASON_BACKGROUND_TASK,
            };
        }
    }
    for footer in &chrome.footer_lines {
        if has_positive_counter(footer, "task(s)") && footer.contains("/tasks") {
            return RefineDecision::Working {
                reason: REASON_BACKGROUND_TASK,
            };
        }
    }

    RefineDecision::Keep
}

struct BottomChrome<'a> {
    tray_lines: Vec<&'a str>,
    footer_lines: Vec<&'a str>,
}

/// Extract the active-items tray and footer status lines at the bottom of `agy`'s normal TUI,
/// excluding everything in the conversation transcript above the prompt box.
fn extract_bottom_chrome<'a>(lines: &[&'a str]) -> BottomChrome<'a> {
    let window_start = lines.len().saturating_sub(32);
    let tail = &lines[window_start..];
    let rules: Vec<usize> = tail
        .iter()
        .enumerate()
        .filter_map(|(idx, line)| is_horizontal_rule(line).then_some(idx))
        .collect();

    if rules.len() >= 3 {
        // Standard layout when active-items tray is visible:
        //   rules[n-3] -> prompt top rule
        //   prompt (> ...)
        //   rules[n-2] -> prompt bottom / tray top rule
        //   active-items tray rows
        //   rules[n-1] -> tray bottom / footer top rule
        //   footer status line(s)
        let r_prev2 = rules[rules.len() - 3];
        let r_prev1 = rules[rules.len() - 2];
        let r_last = rules[rules.len() - 1];
        let between_prev = &tail[r_prev2 + 1..r_prev1];
        let between_last = &tail[r_prev1 + 1..r_last];
        let after_last = &tail[r_last + 1..];

        if section_looks_like_prompt(between_prev) {
            return BottomChrome {
                tray_lines: between_last
                    .iter()
                    .copied()
                    .filter(|l| !l.trim().is_empty())
                    .collect(),
                footer_lines: after_last
                    .iter()
                    .copied()
                    .filter(|l| !l.trim().is_empty())
                    .collect(),
            };
        }
        // If the prompt itself was between r_prev1..r_last (e.g. a markdown rule in chat above),
        // then there is no tray rule below the prompt; inspect `after_last` for tray + footer.
        if section_looks_like_prompt(between_last) {
            return split_post_prompt_lines(after_last);
        }
    }

    if rules.len() >= 2 {
        let r_prev = rules[rules.len() - 2];
        let r_last = rules[rules.len() - 1];
        let between = &tail[r_prev + 1..r_last];
        let after = &tail[r_last + 1..];

        if section_looks_like_prompt(between) {
            // Prompt is between the last two rules; any tray items or footer sit in `after`.
            return split_post_prompt_lines(after);
        }
        if between.iter().any(|l| strip_tray_prefix(l).is_some()) {
            return BottomChrome {
                tray_lines: between
                    .iter()
                    .copied()
                    .filter(|l| !l.trim().is_empty())
                    .collect(),
                footer_lines: after
                    .iter()
                    .copied()
                    .filter(|l| !l.trim().is_empty())
                    .collect(),
            };
        }
    }

    if let Some(&r_last) = rules.last() {
        return split_post_prompt_lines(&tail[r_last + 1..]);
    }

    BottomChrome {
        tray_lines: Vec::new(),
        footer_lines: Vec::new(),
    }
}

fn split_post_prompt_lines<'a>(lines: &[&'a str]) -> BottomChrome<'a> {
    let mut tray_lines = Vec::new();
    let mut footer_lines = Vec::new();
    for &line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if strip_tray_prefix(line).is_some() {
            tray_lines.push(line);
        } else if looks_like_footer(trimmed) {
            footer_lines.push(line);
        }
    }
    BottomChrome {
        tray_lines,
        footer_lines,
    }
}

fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    let mut count = 0usize;
    for ch in trimmed.chars() {
        if matches!(ch, '─' | '━' | '═') {
            count += 1;
        } else {
            return false;
        }
    }
    count >= 16
}

fn section_looks_like_prompt(lines: &[&str]) -> bool {
    if lines.is_empty() {
        return true;
    }
    lines.iter().any(|line| {
        let t = line.trim_start();
        t == ">" || t.starts_with("> ") || t == "❯" || t.starts_with("❯ ") || t.starts_with("!")
    })
}

fn looks_like_footer(trimmed: &str) -> bool {
    trimmed.starts_with("? for shortcuts")
        || trimmed.contains("subagent(s)")
        || (trimmed.contains("task(s)") && trimmed.contains("/tasks"))
}

/// Strip the bullet or selection cursor prefix used by `ActiveItemsModel` tray items.
fn strip_tray_prefix(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    for prefix in ["● ", "• ", "▸ ", "▹ ", "❯ ", "🠶 ", "* "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some(rest.trim());
        }
    }
    // Focused/selected row in ActiveItemsModel may render `> ● ...` or `❯ ● ...`.
    for cursor in [">", "❯", "▸"] {
        if let Some(after_cursor) = trimmed.strip_prefix(cursor) {
            let after_cursor = after_cursor.trim_start();
            for bullet in ["● ", "• ", "🠶 "] {
                if let Some(rest) = after_cursor.strip_prefix(bullet) {
                    return Some(rest.trim());
                }
            }
        }
    }
    None
}

fn is_active_swarm_or_subagent_row(line: &str) -> bool {
    let Some(body) = strip_tray_prefix(line) else {
        return false;
    };
    if is_swarm_item_label(body) {
        return !is_swarm_row_finished(body);
    }
    if body.starts_with("Agent(") {
        return is_agent_row_active(body);
    }
    false
}

fn is_swarm_item_label(body: &str) -> bool {
    body.starts_with("Swarm(")
        || body == "Swarm"
        || body.starts_with("Swarm ")
        || body.starts_with("Swarm·")
}

/// In `agy` 1.5.5+, a finished Swarm stays in the tray until dismissed (`alt+x`),
/// displaying `Done` or `✓ done` (e.g.
/// `"● Swarm(<title>)  Done · 14 posts · alt+s board · alt+x dismiss"` or `"● Swarm  ✓ done"`).
fn is_swarm_row_finished(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    if lower.contains("✓ done") || lower.contains("✔ done") {
        return true;
    }
    // Strip trailing keyboard shortcut hints (`· alt+s board`, `· alt+x dismiss`, etc.)
    // so we can inspect the status segment cleanly without false-matching a title word.
    let mut core = body.trim();
    for sep in [" · alt+", " · enter ", " · press "] {
        if let Some((before, _)) = core.split_once(sep) {
            core = before.trim();
        }
    }
    let after_title = if let Some(rest) = core.strip_prefix("Swarm(") {
        let Some(idx) = find_matching_paren(rest).or_else(|| rest.find(")  ")) else {
            return false;
        };
        rest[idx + 1..].trim()
    } else {
        core.strip_prefix("Swarm").unwrap_or(core).trim()
    };

    // Inspect only `agy`'s own status segment (the first `·`-delimited segment immediately
    // after `Swarm(...)`), not arbitrary `<member>: <message>` post previews that contain `:`.
    let first_segment = after_title.split('·').next().unwrap_or("").trim();
    let normalized = first_segment
        .trim_end_matches('…')
        .trim_end_matches("...")
        .trim();
    !normalized.contains(':')
        && matches!(
            normalized.to_ascii_lowercase().as_str(),
            "done" | "✓ done" | "✔ done" | "completed" | "stopped" | "killed"
        )
}

fn find_matching_paren(s: &str) -> Option<usize> {
    let mut depth = 1usize;
    for (idx, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_agent_row_active(body: &str) -> bool {
    let Some(rest) = body.strip_prefix("Agent(") else {
        return false;
    };
    let after = find_matching_paren(rest)
        .map(|idx| rest[idx + 1..].trim())
        .unwrap_or(rest);
    let lower = after.to_ascii_lowercase();
    if lower.contains("done")
        || lower.contains("killed")
        || lower.contains("failed")
        || lower.contains("completed")
    {
        return false;
    }
    lower.contains("working") || lower.contains("running") || lower.contains("starting")
}

/// Match background task rows in the active-items tray (`● [21:54:14] <desc> running` or
/// `● <task> running`).
fn is_running_background_task_row(line: &str) -> bool {
    let Some(body) = strip_tray_prefix(line) else {
        return false;
    };
    if is_swarm_item_label(body) || body.starts_with("Agent(") {
        return false;
    }
    // Strip optional trailing key hints if `agy` appends `· ...` after `running`.
    let mut candidate = body.trim();
    if !ends_with_running_token(candidate)
        && let Some((before_hint, _)) = candidate.split_once(" · ")
    {
        candidate = before_hint.trim();
    }
    ends_with_running_token(candidate)
}

fn ends_with_running_token(text: &str) -> bool {
    let Some(prefix) = text.strip_suffix("running") else {
        return false;
    };
    // Require whitespace before `running` and non-empty task description before that.
    prefix.ends_with(char::is_whitespace) && !prefix.trim().is_empty()
}

/// Check full-screen modal views in `agy` (`/swarm board` and `/tasks` overlay) where the
/// normal prompt rules are replaced by an alternate-screen board.
fn check_fullscreen_views(lines: &[&str]) -> Option<RefineDecision> {
    let head_end = lines.len().min(6);
    let tail_start = lines.len().saturating_sub(6);

    // 1. Full-screen `/swarm board`:
    // Header has `Swarm(...)` or `Swarm  ✓ done`, and bottom/pane chrome mentions
    // `Swarm board` or `Roster` + `Posts`.
    let has_board_chrome = lines[tail_start..]
        .iter()
        .any(|l| l.contains("Swarm board") || l.contains("prev/next post"))
        || (lines.iter().any(|l| l.contains("Roster"))
            && lines.iter().any(|l| l.contains("Posts")));
    if has_board_chrome
        && let Some(header) = lines[..head_end]
            .iter()
            .map(|l| l.trim())
            .find(|l| l.contains("Swarm(") || l.starts_with("Swarm "))
    {
        if header.contains("✓ done") || header.contains("✔ done") || is_swarm_row_finished(header)
        {
            return Some(RefineDecision::Keep);
        }
        return Some(RefineDecision::Working {
            reason: REASON_SWARM,
        });
    }

    // 2. `/tasks` modal view (`<N> running, <M> done`):
    for line in lines.iter().take(12).chain(lines[tail_start..].iter()) {
        if let Some(running) = parse_running_done_summary(line) {
            if running > 0 {
                return Some(RefineDecision::Working {
                    reason: REASON_BACKGROUND_TASK,
                });
            }
            return Some(RefineDecision::Keep);
        }
    }

    None
}

fn parse_running_done_summary(line: &str) -> Option<u64> {
    // Matches `%d running, %d done` in `/tasks`.
    let idx = line.find(" running, ")?;
    let after = &line[idx + " running, ".len()..];
    let done_digits_end = after.find(|c: char| !c.is_ascii_digit())?;
    if done_digits_end == 0 || !after[done_digits_end..].starts_with(" done") {
        return None;
    }
    let before = line[..idx].trim_end();
    let num_str = before.rsplit(|c: char| !c.is_ascii_digit()).next()?;
    if num_str.is_empty() {
        return None;
    }
    num_str.parse::<u64>().ok()
}

/// Detect `<N> <unit>` (e.g. `3 subagent(s)` or `2 task(s)`) with `N >= 1`.
fn has_positive_counter(line: &str, unit: &str) -> bool {
    let mut rest = line;
    while let Some(pos) = rest.find(unit) {
        let before = rest[..pos].trim_end();
        let digits: String = before
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let boundary_ok = before.len() == digits.len()
            || before[..before.len() - digits.len()]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_ascii_alphanumeric());
        if boundary_ok && !digits.is_empty() && digits.parse::<u64>().is_ok_and(|n| n > 0) {
            return true;
        }
        rest = &rest[pos + unit.len()..];
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_swarm_tray_row_refines_to_working() {
        let screen = "\
● Done dispatching workers. Waiting on the board.

────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
  ● Swarm(Morpho #3 Hybrid & #4 Exit Risk Assessment)  hybrid-risk: @risk-verifier … · 9 posts · alt+s board · alt+x dismiss
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                                 accept-edits · Gemini 4 Carbon · low";
        assert_eq!(
            refine_screen(screen),
            RefineDecision::Working {
                reason: REASON_SWARM
            }
        );
    }

    #[test]
    fn finished_undismissed_swarm_rows_do_not_refine_to_working() {
        for finished_row in [
            "  ● Swarm(Morpho #3 Hybrid & #4 Exit Risk Assessment)  Done · 14 posts · alt+s board · alt+x dismiss",
            "  ● Swarm(Morpho #3 Hybrid & #4 Exit Risk Assessment)  Done · 14 posts · alt+s…",
            "  ● Swarm(Unbalanced (title)  Done · 8 posts · alt+s board · alt+x dismiss",
            "  ● Swarm(Feature Audit)  ✓ done · 12 posts · alt+s board · alt+x dismiss",
            "  ● Swarm  ✓ done · alt+s board · alt+x dismiss",
            "  ● Swarm(Feature Audit)  Done · alt+s board · alt+x dismiss",
            "  ● Swarm(Feature Audit)  Done",
        ] {
            let screen = format!(
                "\
● All swarm workers finished and reported back.

────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
{finished_row}
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                                 accept-edits · Gemini 4 Carbon · low"
            );
            assert_eq!(
                refine_screen(&screen),
                RefineDecision::Keep,
                "{finished_row}"
            );
        }

        // Conversely, an active Swarm whose worker post preview contains `Done` after `member:`
        // must still be recognized as Working!
        let active_preview_with_done = "\
────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
  ● Swarm(Morpho #3 Hybrid & #4 Exit Risk Assessment)  verifier: Done checking #3 · 6 posts · alt+s board · alt+x dismiss
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                                 accept-edits · Gemini 4 Carbon · low";
        assert_eq!(
            refine_screen(active_preview_with_done),
            RefineDecision::Working {
                reason: REASON_SWARM
            }
        );

        // And if an undismissed `Done` Swarm row shares the tray with a live background task,
        // the terminal still refines to Working (`agy.background_task`) for the live task!
        let finished_swarm_plus_running_task = "\
────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
  ● Swarm(Morpho #3 Hybrid & #4 Exit Risk Assessment)  Done · 14 posts · alt+s board · alt+x dismiss
  ● [22:57:10] mise run acceptance running
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                        accept-edits · Gemini 4 Carbon · 1 task(s) · /tasks";
        assert_eq!(
            refine_screen(finished_swarm_plus_running_task),
            RefineDecision::Working {
                reason: REASON_BACKGROUND_TASK
            }
        );
    }

    #[test]
    fn active_subagent_row_or_footer_count_refines_to_working() {
        let row_screen = "\
────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
  ● Agent(worker: Codebase Researcher)  Working · 2m14s
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                                 accept-edits · Gemini 4 Carbon";
        assert_eq!(
            refine_screen(row_screen),
            RefineDecision::Working {
                reason: REASON_SWARM
            }
        );

        let footer_screen = "\
────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
? for shortcuts             accept-edits · Gemini 4 Carbon · low · 2 subagent(s)";
        assert_eq!(
            refine_screen(footer_screen),
            RefineDecision::Working {
                reason: REASON_SWARM
            }
        );
    }

    #[test]
    fn background_command_and_cron_running_rows_refine_to_working() {
        for row in [
            "  ● [21:54:14] cd /Users/jjy/repo && python3 scripts/acceptance.py running",
            "  ● [17:40:10] cron (every 5 minutes) running",
            "❯ ● [09:12:00] cargo nextest run --workspace running",
        ] {
            let screen = format!(
                "\
● Started verification in the background.

────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
{row}
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                        accept-edits · Gemini 4 Carbon · 1 task(s) · /tasks"
            );
            assert_eq!(
                refine_screen(&screen),
                RefineDecision::Working {
                    reason: REASON_BACKGROUND_TASK
                },
                "{row}"
            );
        }
    }

    #[test]
    fn card_93_prompt_and_chat_history_mentions_never_cause_false_positives() {
        let screen = "\
# Card #93: herdr 返回的状态不精确
特征有两个
1. swarm 模式会在下方 tui 有个 Swarm 的列表
2. 后台任务执行时下方 tui 列表会有 xxxxxx running
  ● Swarm(Example In Chat Transcript)  worker: running · alt+s board · alt+x dismiss
  ● [21:54:14] fake command in markdown block running
这两个状态因为 agent 不活动都会被判断为 done, 但是其实应该是 running

● 已完成分析，请确认方案。

────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                                 accept-edits · Gemini 4 Carbon · low";
        assert_eq!(refine_screen(screen), RefineDecision::Keep);
    }

    #[test]
    fn user_typing_swarm_or_running_inside_prompt_box_is_ignored() {
        let screen = "\
────────────────────────────────────────────────────────────────────────────────
> ● [12:00:00] test running
  ● Swarm(Drafting prompt) running
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                                 accept-edits · Gemini 4 Carbon · low";
        assert_eq!(refine_screen(screen), RefineDecision::Keep);
    }

    #[test]
    fn fullscreen_swarm_board_and_tasks_modal_views_are_handled() {
        let active_board = "\
Swarm(Backend Refactor)  3 working · 8 posts
Roster                           Posts
● author-1  working              #8 @coordinator: status check
● verifier  waiting              #7 @author-1: tests green
tab switch pane · all on Swarm board · esc close";
        assert_eq!(
            refine_screen(active_board),
            RefineDecision::Working {
                reason: REASON_SWARM
            }
        );

        let done_board = "\
Swarm(Backend Refactor)  ✓ done · 12 posts
Roster                           Posts
● author-1  killed               #12 @coordinator: all landed
tab switch pane · all on Swarm board · esc close";
        assert_eq!(refine_screen(done_board), RefineDecision::Keep);

        let active_tasks_modal = "\
Background Tasks  2 running, 1 done
  [21:54:14] mise run acceptance  running
  [21:55:01] one-shot timer (30s) running
  [21:50:00] git status           done
esc to close";
        assert_eq!(
            refine_screen(active_tasks_modal),
            RefineDecision::Working {
                reason: REASON_BACKGROUND_TASK
            }
        );

        let done_tasks_modal = "\
Background Tasks  0 running, 3 done
  [21:50:00] git status           done
esc to close";
        assert_eq!(refine_screen(done_tasks_modal), RefineDecision::Keep);
    }
}
