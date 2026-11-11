# Overview and Terminal navigation acceptance

Date: 2026-11-11. Implemented in the `worktree-optimize-overview` checkout. No installed application or live user Core was upgraded or modified. The [selected design](overview-and-terminal-navigation-proposal.md), [Overview contract](product/overview.md) and [Terminal navigation contract](terminal-workspace.md#unified-terminal-navigation) describe the resulting behavior.

## Changes

Overview removes its context inspector and Terminal preview. It presents compact direct-action rows, combines human actions in Needs you, deduplicates unread Cards against that section, includes active Project configuration Terminals and collapses Idle/Closed. Profile identity remains visible before Project text at narrow widths; full context is available in a tooltip. Back restores row focus, selected state, scroll and expansion.

Terminal resolution is shared across Board, Card, Overview, Review, Timeline and Quick Open. Card-level entry prefers the last explicitly selected live Agent or the sole live Agent; ambiguity opens a native menu with Profile/provider/state/machine. Saved request navigation never substitutes a newer invocation. Start/Resume are explicit, pending-guarded and fenced to the source location; a queued explicit launch attaches when its exact request obtains a PTY, without adding a redundant queue-context Back destination.

## Executed checks

- `mise run build` and `mise run test` passed. After the final UI changes, `mise exec -- cargo test -p canban-ui` passed all 47 tests, including multiple-Agent choice, disconnected-versus-ended preference, historical request identity and queued-launch Back history.
- `mise exec -- cargo clippy -p canban-ui --all-targets -- -D warnings` passed during final refinement; final formatting and whitespace checks are recorded with the delivered changes.
- `mise run acceptance` passed against isolated real Herdr with deterministic providers: Develop/Review, blocked mentions, archive/restore, restart and global budget concurrency. Scratch home: `/private/tmp/cb-3bxuwal6/home`.
- `mise run acceptance-overview` passed: global status budget, exited failure/saved output, output without the original Core, one-minute idle reclamation and exact resume. Scratch base: `/private/tmp/cb-overview-74rrlt_q`.
- Production Gallery was run through `mise run gallery`, using `CANBAN_GALLERY_SCENE=overview`, at 1440 and 720 points. Native interaction verified direct Card handling, multi-Agent selection, remembered reviewer selection, existing-tab reuse, Back retaining Card reading position, long-Markdown unread reply reveal, and Back restoring keyboard focus to the selected Overview row. Narrow layout retained destination controls and truncated long content. Idle/Closed remain independent disclosures.
- `mise run acceptance-native` launched a copy of `.local/Canban.app` with an isolated home/Core at `/private/tmp/cb-native-0l9qdi90`. Native clicks verified Board's nested Terminal action, Overview and Card Profile actions reaching the original real PTY, and `navigation-check` appearing in that PTY. Closed history displayed the originating assignment, original attempt/output and explicit Resume without starting work. Following an old Timeline request after resume showed the old failed invocation and a separate Open current conversation action. Explicit Resume attached automatically after its PTY appeared.
- Runtime audit saved `navigation-final-snapshot.json` and `navigation-verification.json` in that native scratch base. Three attempts across the explicit close/resume checks retained the same provider session `c134351c-bbf3-4abd-8b10-085a53d9001d`.

## Regressions found and repaired

The first unread jump ran before long Markdown completed layout. The final path waits for initial layout, reveals the Timeline and refines the destination from the actual event bounds. Card identity and disposed-owner checks fence delayed work.

The first Resume path could receive a queued receipt before a Terminal binding existed, leaving the user in request context. Explicit launch tracking now waits for that request's PTY and cancels automatic navigation on departure. Ordinary navigation still never launches or resumes.

Native accessibility automation initially could not disambiguate repeated Board button labels after snapshot refresh. Board actions now include their Card number. Native menu AX invocation timed out; selecting the visible menu item with a pointer verified actual menu behavior. Two simultaneous acceptance tasks initially collided while packaging the same bundle; sequential packaging and Overview acceptance passed. These intermediate failures are not counted as successful checks.

## Limits

Gallery uses fixture Terminals; real PTY acceptance uses deterministic providers, not paid Codex/Claude model tasks. The existing remote-machine transport was not re-exercised against a remote host. Project configuration visibility and the migrated Review/Quick Open paths were inspected and compiled; they were not each driven through a fresh full live workflow. No new shortcut was introduced. The installed app remains untouched; the final bundle is checkout-local.
