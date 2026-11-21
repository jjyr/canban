//! Antigravity (`agy`) TUI bottom-chrome status refinement.
//!
//! When `agy` finishes its foreground turn and returns to the `>` prompt while a
//! background command/schedule is still active, its bottom TUI renders an
//! active-items tray delimited by horizontal rule lines (`───...`) above the footer bar:
//!
//! ```text
//! ────────────────────────────────────────────────────────────────────────────────
//! >
//! ────────────────────────────────────────────────────────────────────────────────
//!   ● [21:54:14] cargo nextest run --workspace running
//! ────────────────────────────────────────────────────────────────────────────────
//! ? for shortcuts              accept-edits · Gemini 4 Carbon · low · 1 task(s) · /tasks
//! ```
//!
//! Crucially, inspection is anchored strictly to the bottom TUI chrome (or full-screen
//! `/tasks` view), so user prompts or conversation history above the prompt box
//! discussing `"xxxxxx running"` never cause false positives.

use super::RefineDecision;

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

fn is_swarm_item_label(body: &str) -> bool {
    body.starts_with("Swarm(")
        || body == "Swarm"
        || body.starts_with("Swarm ")
        || body.starts_with("Swarm·")
}

/// Match background task rows in the active-items tray (`● [21:54:14] <desc> running` or
/// `● <task> running`), excluding Swarm and Agent rows.
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

/// Check full-screen `/tasks` modal view in `agy` where the normal prompt rules are replaced
/// by an alternate-screen task list (`<N> running, <M> done`).
fn check_fullscreen_views(lines: &[&str]) -> Option<RefineDecision> {
    let tail_start = lines.len().saturating_sub(6);

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
    fn swarm_and_subagent_rows_do_not_refine_to_working() {
        for screen in [
            "\
● Done dispatching workers. Waiting on the board.

────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
  ● Swarm(Morpho #3 Hybrid & #4 Exit Risk Assessment)  hybrid-risk: running · 9 posts · alt+s board · alt+x dismiss
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                                 accept-edits · Gemini 4 Carbon · low",
            "\
────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
  ● Agent(worker: Codebase Researcher)  Working · 2m14s
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                                 accept-edits · Gemini 4 Carbon",
            "\
────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
? for shortcuts             accept-edits · Gemini 4 Carbon · low · 2 subagent(s)",
            "\
Swarm(Backend Refactor)  3 working · 8 posts
Roster                           Posts
● author-1  working              #8 @coordinator: status check
● verifier  waiting              #7 @author-1: tests green
tab switch pane · all on Swarm board · esc close",
        ] {
            assert_eq!(refine_screen(screen), RefineDecision::Keep, "{screen}");
        }

        // If a Swarm row shares the tray with a live background task, the background task
        // still refines to Working (`agy.background_task`).
        let swarm_plus_running_task = "\
────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
  ● Swarm(Morpho #3 Hybrid & #4 Exit Risk Assessment)  Done · 14 posts · alt+s board · alt+x dismiss
  ● [22:57:10] mise run acceptance running
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                        accept-edits · Gemini 4 Carbon · 1 task(s) · /tasks";
        assert_eq!(
            refine_screen(swarm_plus_running_task),
            RefineDecision::Working {
                reason: REASON_BACKGROUND_TASK
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
    fn prompt_and_chat_history_mentions_never_cause_false_positives() {
        let screen = "\
# Card #93: herdr 返回的状态不精确
  ● [21:54:14] fake command in markdown block running

● 已完成分析，请确认方案。

────────────────────────────────────────────────────────────────────────────────
>
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                                 accept-edits · Gemini 4 Carbon · low";
        assert_eq!(refine_screen(screen), RefineDecision::Keep);
    }

    #[test]
    fn user_typing_running_inside_prompt_box_is_ignored() {
        let screen = "\
────────────────────────────────────────────────────────────────────────────────
> ● [12:00:00] test running
────────────────────────────────────────────────────────────────────────────────
? for shortcuts                                 accept-edits · Gemini 4 Carbon · low";
        assert_eq!(refine_screen(screen), RefineDecision::Keep);
    }

    #[test]
    fn fullscreen_tasks_modal_views_are_handled() {
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
