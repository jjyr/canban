# MVP acceptance after profile and delivery updates

Date: 2026-11-07. These are executed checks, separate from broader design proposals. All integration/native runs use disposable Projects, a separate CANBAN_HOME and an isolated Herdr socket/state/config. Normal ~/.canban and the user's live Core were not changed.

## Design implemented

- Shared command-defined profiles/presets with whole-command overrides; generic runtime invocation and opaque exact-history IDs.
- Herdr root status/turn/control reused for lifecycle. No injected Codex notify/Claude Stop hooks. Explicit completion remains the workflow boundary; only a later relevant root done permits configured normal exit.
- Native Review with basic Diff, rework through existing workflow/mentions, explicit commit/merge, durable receipts, and Archive as a separate action.
- Visible Core-unavailable state, deduplicated attention, same-binding reconnect, disconnected input discard, missing-history failure and explicit fresh-context recovery.

## Real Codex tasks

`mise run acceptance-real-interactive` passed with the new profile/lifecycle contract. It covered live @reply without losing the original task callback, distinct Developer/Reviewer histories, ordinary normal exit and exact-ID follow-up Resume. Temporary evidence: `/private/tmp/cb-real-interactive-ktuvnwiv/home`.

`mise run acceptance-real-workflows` passed. Two Projects ran concurrently:

| Task | Executed flow | Independent check |
| --- | --- | --- |
| Unicode slug | Backlog -> Develop -> blocked + notice -> human @developer separator choice -> reply -> implementation -> Review | Seven unittest cases, including Unicode/whitespace |
| Integer total | Backlog -> Develop -> Review -> human negative-input requirement -> Develop -> second Review | Eight unittest cases; separate direct negative-input rejection assertion |

The two Develop invocations for rework used the same provider conversation ID. Reviewer remained an independent profile/history. All six invocations explicitly completed and exited; the question had a durable reply. Source repositories remained unchanged until acceptance. Evidence: `/private/tmp/cb-real-workflows-nx7j1zqd/evidence.json`.

## Delivering those actual changes

`scripts/acceptance-delivery.py /private/tmp/cb-real-workflows-nx7j1zqd` passed using the previously reviewed real output, without another model call. Both Cards were previewed, committed and merged into their source main branches. The exact same acceptance request returned its receipt without a second merge. Tests passed again in the target source checkout. Worktrees remained until Archive; archives retained readable reports, removed clean worktrees and supported Restore.

| Source | Accepted commit |
| --- | --- |
| human-input | 3883a2db84a82180ba80cbc48fa8d37c50f30f70 |
| review-rework | cd06febc9cc42c5c45ddcdcdd349d2983b3c2f90 |

Temporary evidence: `delivery-evidence.json` and saved diffs in that same base directory. This establishes code delivery on disposable local Git repositories; it does not establish arbitrary repository hook/signing configurations or remote publishing.

## Native checks

Production NSH components and the production libghostty-vt/AppKit terminal were operated through the actual packaged app, using deterministic fixture Agents and real Herdr PTYs.

- `/private/tmp/cb-native-oqgquwez`: Unicode paste/input, long output, live detach/reopen, unavailable Core attention, initial attach failure/retry and reconnect to the original terminal. Input submitted while disconnected (`MUST_NOT_REPLAY`) was absent from the fixture audit; fresh `AFTER_RECONNECT` was delivered. Native Review displayed the actual diff, rejected a stale preview, accepted after refresh, showed commit `7a8740f1db4a5083a4b5d49f41d967885720a650`, and returned to Develop with the same Developer conversation. Source files were checked separately.
- `/private/tmp/cb-native-w9x9mwfu`: corrected scroll direction with 12,000 lines, upward scroll from row 11965 to 11889 and return to the live tail; window zoom/redraw and continued input; native-host restart during live work retained the same Attempt, Core, Terminal and provider ID, then accepted further input after reopening the Card Agent. Attach now waits for native layout and reapplies latest size/focus after handshake, avoiding an initial 80-column history snapshot and lost resize messages during reconnect.

These are bounded interaction checks, not a full-duration stress guarantee. The user explicitly requested skipping Chinese IME verification. Unicode paste is verified; marked-text/candidate behavior is not marked accepted.

## Deterministic recovery and regression

`mise run acceptance-interactive` passed using real Herdr and the fixture provider. It exercises live-host restart without duplicate launch, temporary transport outage and deduplicated notices, original-binding reconnection, interrupt/continue, exact-ID Resume, provider-history deletion, visible failed Resume and explicit fresh-context recovery, plus compressed reply read/restore. Latest evidence home: `/private/tmp/cb-interactive-arq7mirt/home`; standard fixture acceptance passed at `/private/tmp/cb-ppw_m1r6/home`.

A failure-injection regression reproduced a question stuck in delivering when Core disconnected during its pre-send status check. The failure is now persisted as delivery_unknown with attention and is not resent automatically.

Rust regression covers whole-command overrides/custom presets/migration backup, later-root-done vs interrupted/blocked/error/old completion, stale diff rejection, dirty target protection, divergent-target integration, conflicts retained in Card, ignored-source-file protection, hook-added-code review protection, merge receipt retry/restart, Archive separation and restoring the retained Card commit after the Project branch advances, plus 12,000-line VT scrollback/resize. Standard fixture acceptance covers the broader Kanban/storage/routing loop.

Desktop-controller limit: after a second native-host restart, further CUA lookup returned `cgWindowNotFound` even though the isolated UI process and AppServer/Core remained live. The earlier observed native flows above passed; additional repeated-restart/window-visibility checks were not completed. This is retained as an unresolved verification boundary rather than inferred product success or a diagnosed application crash.


### Follow-up: repeated native restart diagnosis

A fresh isolated reproduction at `/private/tmp/cb-native-q3_y77d6` completed two native-host restarts using the same bundle/home/Core. Native Board/Card navigation, terminal attachment, screenshot capture and provider receipt of `SECOND_RESTART_WINDOW_OK` passed after the second restart. The one Attempt, Core ID, Terminal address and provider ID were unchanged. Evidence is `restart-diagnostic-evidence.json` in that temporary base. No application code was changed for this diagnosis.

The earlier `cgWindowNotFound` is therefore an unreproduced desktop-controller/window-lookup incident, not an established deterministic second-restart Canban defect. The exact historical cause remains unknown: the old process/Core being live did not prove that its window was onscreen, and neither an obsolete controller reference nor a macOS visibility/Space state was captured at failure time. The repeated-restart visibility acceptance now has a successful follow-up; broader intermittent-window behavior is not declared fixed.

## Remaining limits

Claude actual model/report/normal-exit/Resume acceptance still needs valid local authentication; earlier smoke returned `Invalid API key / Please run /login`. No credentials were changed. Preset/configuration tests do not establish live Claude acceptance.

Provider-owned history is not backed up by Card archives. A permanently replaced Core is never confused with another Core's reused terminal address; restoring the original runtime or explicit inspection is required. Reports cannot be submitted while AppServer is absent; the Agent must retry after it returns. Terminal mouse-reporting, links/graphics, full IME, long-duration pressure/recovery and every third-party permission dialog are outside this bounded acceptance. Shear, GitHub backend and workflow-template GUI remain deferred.

Final validation: 36 workspace tests passed, workspace Clippy with warnings denied passed, formatting/diff checks passed, and both deterministic acceptance tasks passed. The development bundle is `.local/Canban.app`. Temporary artifacts are diagnostic evidence, not durable portable backups.

## Standalone Gallery extraction

Gallery entry and fixtures now live in `examples/gallery`; `canban-ui` exports production components as a library. Main-app startup/navigation no longer contains a Gallery mode. `mise run gallery` builds/packages `.local/CanbanGallery.app` separately and needs no AppServer, Canban home, CLI or Agent/Core connection.

Native extraction acceptance used `/private/tmp/cb-gallery-check-bq6bd3u1`: production sidebar had no Gallery entry; the standalone example exercised button count, mention insertion, native editable value/Markdown preview, theme switching, scrolling, shared Diff and static VT content. The Gallery's sentinel CANBAN_HOME, application socket and Core socket were never created; the Gallery bundle contains only its example executable. Evidence is `gallery-evidence.json` in that temporary directory. An additional fresh preview process also left its sentinel home absent.

Static native content was initially blank in the background capture and displayed after raising the example window, without clicking the Terminal itself. A trial forced-display change did not independently solve this and was removed; this extraction does not claim a renderer fix or new IME acceptance. Put the Gallery window in front when inspecting native components. Component check/development instructions are in `examples/README.md`.
