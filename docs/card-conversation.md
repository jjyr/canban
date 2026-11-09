# Card conversation workspace

Status: implemented and natively checked, 2026-11-09. This replaces the earlier Card layout with permanently visible workspace details and Agent management.

## Layout and ownership

```text
+---------------------------------------------------------------+
| #42 Card title (truncated; full title on hover) Term Archive ... X | fixed
| Develop   branch   Agent/status             Review / Continue | fixed
+---------------------------------------------------------------+
| > Task                                  Edit                  | scroll
|   One-line task summary; expand to read or edit                |
|   Context/attachments and workspace details open on demand     |
|                                                               |
| > Activity (creation, scheduling and runtime history)          |
| user                                               timestamp |
|   Comment                                                     |
| @developer                                         timestamp |
|   Reply / full Markdown report                                |
|                                                               |
+-------------------- New activity (N) -------------------------+
| Comment or @Agent...                                          | fixed
| @  [explicit recipients]  +                         Send       |
+---------------------------------------------------------------+
```

The Board keeps its ordinary columns and Cards, without execution-slot summary or filter buttons. Wide layouts retain the Board beside the Card; narrow layouts use the Card pane with a Back action. Branch text is compact, copyable and has a full-value tooltip. Host, path, base commit and manual workspace refresh are available under **... → Workspace details**, rather than consuming permanent conversation space. **Archive/Restore** has a dedicated header icon. **...** also owns stage moves, Archive/Restore, Delete/Remove and context/attachment controls. Board Cards expose the same lifecycle actions in their right-click menu. The X action is labeled **Close Card details** and only dismisses the pane. Review and human Continue remain directly reachable when applicable.

`card.rs` composes independently owned `card/header.rs`, `task.rs`, `details.rs`, `timeline.rs` and `composer.rs`. These use shared UI Kit controls, NSH scroll/input/Markdown/menu behavior and existing product components. The standalone Gallery imports the same screen through the production frame.

## Comments, recipients and shortcuts

- Enter inserts a newline. Command+Enter sends from the focused comment editor.
- Send, the shortcut and the Shortcuts catalog action use one submission intent and the same empty/pending/archived/popup checks. New Card uses the same Command+Enter convention for **Create**.
- One **@** menu adds/removes configured profile names. Selected recipients also appear as removable compact buttons.
- Consecutive recognized leading `@profile` tokens address Agents. Profile names mentioned in ordinary prose or quoted text are not recipients. Removing one recipient retains the remaining question text.
- The desktop passes an explicit `mentions` array to `comment.add`, including `[]` for an ordinary comment. Legacy clients omitting the field retain the existing body-scanning behavior. No CLI interface change is required.
- Drafts survive Card/Terminal navigation and snapshot refresh. An asynchronous send only clears the submitted text if the draft still matches it; newer text remains.

## Timeline and reading behavior

Comments, Agent replies and reports render as Markdown in the conversation. File-backed reply/report contents load through `resource.read`, including archived Card resources. Long bodies have an inline expansion control. A load error is shown inline while retaining the saved summary.

Consecutive routine events collapse into an expandable activity row. This is a presentation projection: every durable event remains recorded and accessible. A report echoed as a reply stays in activity history when the request and non-null resource path match; reports without paths remain visible. The redundant Latest result panel is removed.

The Card content has one scroll owner and saves its offset per Card. First open follows the latest activity. Scrolling away stops following; new activity adds an explicit jump-to-latest affordance. At the bottom, new replies and loaded report growth stay visible. Task/context editing remains inside this scrolling region. Draft/offset restoration is in-memory for the lifetime of the window, not persistent across application restarts.

## Terminal boundary

Clicking an existing live Agent status or Timeline **Open terminal** navigates to its conversation. An exited Agent status is labeled Resume and explicitly restores its exact session; it does not open an ended-session page. Live navigation does not resume or start an Agent. **Open Card Terminals** selects an existing Card conversation if available; otherwise it opens a Card-context empty Terminal view without selecting an unrelated tab or starting a process.

The Terminal chrome offers **Choose Card Agent** for an active associated Card. The empty Card view offers **Choose Agent**. This explicit action invokes the existing `agent.open` Start/Resume boundary. Ended sessions retain Resume/Start fresh; interrupt, close and history remain Terminal controls. **Back to source** returns to the Card and its draft. Closing a Terminal continues to use the existing unified lifecycle close operation.

## Verification

The following were checked against this implementation:

- Workspace Rust tests and strict all-target Clippy; AppServer regressions for explicit empty/single recipients, parser tests for quoted/inline names, duplicate prefixes and whitespace, and Timeline report-echo projection tests.
- `mise run acceptance`: isolated real Herdr connection, Develop → Review, mentions/attention, archive/report restoration, service restart and cross-Board global scheduling.
- Native `.local/Canban.app` build, copied into an isolated acceptance bundle/home/Core with deterministic provider commands: Enter retained a multiline comment; Command+Enter saved it exactly once; a prose `@claude` created no Claude request. Explicit @developer delivered one Question and an inline reply.
- Native existing-Agent navigation retained the draft on return. A Card with no history opened an empty Card-context Terminal view; explicit Choose Agent then started its fixture conversation. Task edit/save and on-demand workspace details worked.
- Gallery: full/narrow conversation layouts, long task/history, bottom-following, reading older messages while a new reply arrives, and the new-activity affordance. Use **Card conversation** and **Append reply** to reproduce.

This is deterministic integration/UI acceptance; it does not establish provider authentication readiness or a new real-model acceptance run. Chinese IME was excluded at the user's request.

## Direct lifecycle actions and recovery (2026-11-09)

Local Cards offer **Delete Card**; GitHub Issue Cards offer **Remove from Board**. Both use the reversible `card.remove` boundary, retaining the Card number, content and Timeline. The operation cancels queued work and excludes the Card from normal snapshots and dispatch. Live or uncertain Agent invocations must be ended/resolved first; a failed removal leaves the Card active. No GitHub Issue or PR is closed or deleted.

**Archived Cards** provides inline Restore for archived records and a **Deleted Cards** recovery section loaded through `card.list(include_removed: true)` for the selected Project. `card.restore` clears local removal/archive state without scheduling work. Archive remains the completed-work/compression lifecycle; Delete is local removal and preserves recoverable data. There is no additional Close workflow stage.

Native isolated checks covered header Archive → archived list → Restore, Board right-click Delete → Deleted Cards → Restore with the same Card number, and X dismissing only the detail pane. Backend tests cover queue cancellation/history preservation and rejection of removal with a live Agent. Gallery includes an **Archived / deleted** scene using production controls.

[Terminal interaction and navigation](terminal-interaction.md) records the current delayed-exit/inactive contract and unified Back/Forward behavior.
