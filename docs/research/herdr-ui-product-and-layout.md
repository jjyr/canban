# Herdr UI product and layout references for Canban

Research date: 2026-11-07. Status: the compact icon sidebar and fuzzy Quick Open have been implemented; see [implementation and acceptance](../quick-open-and-navigation.md). Other recommendations remain under discussion. This document preserves the research and selection rationale.

The existing Canban implementation was committed as `e289396` before this research. The read-only reference checkout is `/Users/jjy/Workspace/herdr-gpui`, revision `a6db99b4`; it had no tracked or untracked changes during inspection. This assessment uses source, documentation and repository screenshots. It does not claim a fresh interactive acceptance run of Herdr UI.

## Recommendation

Canban already has Board/Card splits, focused Terminal/Review, Notes, a global Overview and exact Card/profile conversations. Herdr UI's strongest remaining contribution is how these surfaces behave as one continuous workspace: compact chrome, collapsible navigation, remembered position, fast destination search and feedback attached to the thing the user is discussing.

Preserve the Kanban-centered information hierarchy. Use the existing NSH/UI Kit boundary for shared controls and adapt the interaction contracts to Canban's durable Card history.

| Opportunity | Herdr UI evidence | Current Canban | Suggested priority |
| --- | --- | --- | --- |
| Compact workspace chrome | Titlebar tabs, shared navigation, sidebar rail | Sidebar and focused split exist; repeated toolbar/header layers consume space | First layout pass |
| Real navigation history | Back/Forward focus trail with invalid-target handling | Explicit Board/Overview origin return, without general history | First layout pass |
| A Card's working tools in one place | Editor groups keep terminal/page/review together | Terminal and Review are separate focused routes | First layout pass |
| Fuzzy Quick Open | Search, keyboard candidate selection and contextual identity | User selected a candidate list for Projects, Cards, Notes and Agents, without a preview pane | Selected first implementation scope |
| Contextual feedback | Selected terminal text, Diff lines and page regions become Agent notes | Card comments/@questions and attachments exist | Next product slice |
| Review reading workflow | Changed-file list, unified/split Diff, inline feedback, viewed marks | Basic Diff/report and explicit delivery exist | After layout; coordinate with planned Shear integration |
| Durable comparison points | Checkpoint list/create/diff/restore commands | Base commit/worktree/delivery receipt exist | Later, when rework comparison needs justify it |
| Consistent transient behavior | Shared motion, semantic toasts, prepared preview state | Shared visual tokens exist; feedback often uses a global status line | Improve with each owning component |

## 1. Reduce chrome and use compact icon navigation

The reference screenshots show most of the window assigned to actual work. Its sidebar can collapse into a rail while keeping destinations and Agent status marks accessible. The titlebar/navigation areas are small and consistent across the working tools.

For Canban:

- User-selected direction: use a compact icon sidebar by default, replacing the current large navigation buttons. This is a primary navigation design, rather than only a focused-work collapse state.
- Keep selected/focus states, accessible labels, hover tooltips and Overview's actionable badge. Board/Notes and other destinations remain discoverable.
- Project-picker and optional expanded-label behavior remain details to settle; they are not requirements for a workspace/Agent tree. Do not change navigation width on background Agent updates.
- Use a compact shared header for Project/Card identity, navigation, stage/attention and expansion. Put uncommon actions in a labeled overflow menu.
- Keep paths, internal request IDs and verbose Git details in inspectable context. Existing exact-copy actions remain available.

Suggested focused arrangement:

```text
+-------+--------------------------------------------------------------------+
| rail  | < >   Project / #42 Add search      Review   Needs you   Expand  ... |
|       +---------------------------+----------------------------------------+
| O !2  | Card context              | @developer v    Connected / Working    |
| Board | Task and scope            | Terminal | Changes | Report             |
| Notes |                           +----------------------------------------+
|       | Timeline                  |                                        |
|       | Comments / reports        | Active working surface                 |
|       |                           |                                        |
|       | Context / resources       |                                        |
|       +---------------------------+----------------------------------------+
|       | Comment... @profile   Send| Status and contextual actions          |
+-------+---------------------------+----------------------------------------+
```

These tool tabs describe a proposal, not a new global tab system. Start with one selected Card and one working surface beside its context. Existing expand/restore and narrow-screen switching can remain the layout foundation. Arbitrary nested splits and cross-Project docking would add a separate product problem.

The Agent selector chooses the exact Card/profile conversation. A tool switch must not launch/resume an Agent, change workflow stage, approve a gate or discard a draft. An absent/exited Agent needs an explicit Start/Resume action. Hiding a terminal viewer retains the existing lifecycle semantics, including viewer-lease protection only while it is actually attached/viewed.

Reference: `README.md:30`, `crates/herdr-gpui/src/sidebar/rail.rs`, `crates/herdr-gpui/README.md:2875`, screenshots `workspaces.png` and `review-split-collapsed.png`.

## 2. Navigation should preserve a place, not only a page

Herdr UI's focus history has Back/Forward, skips destinations that no longer exist and scopes runtime identities to a daemon boot. Canban currently remembers whether focused work came from Board or Overview; that solves one return path but not a longer investigation.

A useful Canban trail could be:

```text
Overview: Needs you, row A
    -> Card A / Terminal @developer
    -> linked Note
    -> Card B / Review
    -> Back: Note
    -> Back: Card A / Terminal
    -> Back: Overview with row/filter/scroll restored
```

Store typed product destinations with their view state: Project/Card/Note IDs, profile/tool, filters, selection, scroll and split widths. Keep unsaved document drafts separately so history entries do not overwrite newer edits. Closed runtime viewers become an explicit resume/recovery state rather than reviving an obsolete terminal ID. Ordinary background polling should not add history entries or move the user's focus.

Reference: `crates/herdr-gpui/src/endpoint/history.rs:1`, `crates/herdr-gpui/src/titlebar/navigation.rs:1`. Canban baseline: `crates/ui/src/presenter.rs`, `crates/ui/src/workspace.rs`.

## 3. Fuzzy Quick Open with a candidate list

User-selected direction: one search field and a fuzzy-matched candidate list, with no context preview pane. Herdr UI is a reference for matching and keyboard candidate navigation; its preview and ranking machinery do not define Canban's scope.

Search across all Projects in the current Canban home:

- Projects by name and directory.
- Cards by number and title, with Project identity available for disambiguation.
- Notes by title, with their owning Project.
- Agents as existing Card/profile conversations, searchable by profile and associated Card/Project. This does not add a provider session model or an independent Agent workspace.

Each candidate shows its type/icon, primary name, and concise owning Project/Card information. Agent candidates can also show runtime/closed state. These labels distinguish results; they do not form a task/report/screen preview.

Provide a search icon and a keyboard shortcut (user-selected default: Cmd+O). Typing updates the local candidate list; Up/Down chooses, Enter opens, Escape dismisses. Opening an empty query can show recent destinations. An empty result has a clear message. Search and selection never launch an Agent or call Herdr for a preview.

Opening a Project navigates to its Board, a Card to its detail, and a Note to its editor. Opening an Agent navigates to its existing Card/profile context/live Terminal; exited or unavailable invocations retain explicit Resume/recovery handling. Reuse current navigation, exact identity and operation gates. Preserve the originating Board/Overview place using the existing return behavior; general Back/Forward history remains a separate recommendation.

Use a shared result model and reusable search/list component with deterministic Gallery fixtures. Match prepared local metadata; do not perform render-time disk, Git or Core work. Archived conversation results must respect the Card's existing archive/recovery restrictions.

Reference: `crates/herdr-gpui/src/palette/search.rs:1`, `palette/render.rs:9`, `palette/keys.rs:1`. Preview remains an existing independent Overview action, outside Quick Open.

## 4. Make feedback carry its source context

This is the most useful product idea beyond navigation. Herdr UI lets the user select terminal text, annotate Diff lines, or select a page element/region, then write feedback beside that material. The comments are collected with their source and sent to an Agent.

The inspected Herdr UI directly supports terminal, Diff and browser annotations. Quoting Canban reports and database-backed Notes into Card comments is a Canban adaptation, not an existing Herdr Card/Timeline feature.

For Canban, a lightweight first step is **Quote into Card comment**. The user selects part of a report, Note or terminal output, chooses the target profile and adds a question. The comment retains its source and appears in the existing Timeline. This builds on the current comment/@question model and needs no additional Session category.

```text
Selected material                    Card Timeline
+-----------------------+             +-------------------------------+
| "Search returns ..." | -- Quote -> | @developer Why this behavior? |
+-----------------------+             | Source: report / Note / screen|
                                      | Exact excerpt + user comment  |
                                      | pending / delivered / replied |
                                      +-------------------------------+
```

Potential reference fields include Card ID, profile/conversation, report or Note revision, file/line/revision for a Diff, and capture time plus exact Core/terminal/attempt for screen text. Retain the excerpt itself; a changing terminal screen is not a durable source address. A quote is evidence of what was observed, not a parsed semantic Agent reply.

Continue Canban's selected immediate-append policy. Runtime permission/question dialogs still require handling through the actual Terminal. The reference's delivery queue and workspace-Agent fallback are different policies; they do not replace Canban's exact profile association or uncertain-delivery handling. A future browser annotation UI can reuse the same feedback record after local preview becomes a concrete need.

Reference: `crates/herdr-gpui/README.md:1829`, `README.md:3078`, `README.md:3000`, `src/agent_notes.rs:1`, `src/terminal_notes.rs`, `src/review/notes.rs`. The README paths in this paragraph refer to `crates/herdr-gpui/`.

## 5. Make Review a reading workspace before a delivery form

The reference's Review screenshots put changed files on the left, code in the center and numbered feedback on the right. The primary reading path is easy to scan; commit/delivery details do not dominate the first viewport.

Canban can stage this work:

1. Lead with saved outcome, changed-file summary, validation and unresolved findings. Keep the exact initiating outcome visible when another invocation of the same profile is live.
2. Make the Diff/file list the main working area. Keep target branch and commit message near the final Accept action, in a compact delivery area.
3. Add source-linked feedback and review progress when the advanced editor is introduced. Retain drafts, selected file and reading position through rework.

Viewed/read state is separate from approval. The reference persists viewed marks using file paths and added/removed line counts. Canban should bind any future viewed mark to a real file/Diff revision: code can change without changing those counts. A changed snapshot must not silently retain human approval.

A later "changes since previous review" view could use durable Card comparison points. The reference exposes checkpoints, but automatic checkpoints, restore semantics and extra Git refs need a separate Canban decision. The existing base commit and delivery receipt are not a complete review-round history.

Reference: `crates/herdr-gpui/src/review/view/model.rs:1`, `review/viewed.rs:1`, `docs/cli.md:127`, screenshot `review-split-collapsed.png`. Full editor work should coordinate with the existing Shear plan rather than create two parallel review editors.

## 6. Borrow a consistent visual grammar

The screenshots offer several layouts, but Canban can use one design language with two deliberate densities:

| Surface | Suggested treatment |
| --- | --- |
| Board Cards | Comfortable spacing; title, meaningful activity and next human action |
| Overview/search/Agent lists | Compact rows; aligned status/time; selected and actionable rows carry emphasis |
| Focused toolbars | Small controls, one main action, secondary actions in a menu |
| Card/Note reading | Comfortable text and restrained separators; metadata disclosed on demand |
| Timeline | Comments/reports as readable content; minor lifecycle transitions as compact events |

Use fewer nested panel borders and repeated badges. Distinguish stage, runtime, attention and selection with their existing semantic tokens; color has accompanying text/icons. Keep the same hierarchy in light/dark appearances and narrow windows. Extend UI Kit with density, toolbar, list-row and disclosure variants used by actual consumers, with Gallery cases.

Herdr UI also centralizes reversible short motion for panels, tabs and disclosures. Canban can use small, consistent transitions when supported by NSH and reduced-motion settings. Typing, output and scrolling should remain immediate.

Reference: screenshot `layouts.png`, `crates/herdr-gpui/src/config/sidebar_style.rs:1`, `src/sidebar/tokens.rs:1`, `src/motion.rs:1`. Herdr's Unknown-to-Idle projection in one sidebar path is unsuitable for Canban's conservative runtime handling.

## Recommended sequence for discussion

1. **Shared workspace chrome:** compact identity/header, collapsible sidebar, consistent focus/expand/restore controls and a first navigation-history model.
2. **Card working surfaces:** a stable context pane plus Terminal/Changes/Report switching for the selected profile. Preserve existing lifecycle, exact-resume and workflow gates.
3. **Quick Open:** user selected fuzzy cross-Project Project/Card/Note/Agent search with a candidate list, without contextual previews.
4. **Contextual feedback:** quote source material into the existing Card Timeline/@question flow.
5. **Advanced Review:** file navigation, revision-aware reading progress and inline feedback, aligned with Shear integration.

Multi-host infrastructure, system-load dashboards, arbitrary workspace trees, multiple competing visual presets and a general docking framework can stay outside the near-term Canban scope. Existing database-backed Notes retain their full document model; Herdr's one-line worktree reminders are a different feature.

The source inspection was read-only. The selected rail/Quick Open slice is now implemented in Canban; the other recommendations still require product selection.

## Bounded first implementation scope for confirmation

The user-authorized implementation contains two features:

1. Default compact icon navigation replacing the large sidebar buttons. Retain Overview's actionable badge, selected/focus states, accessible labels and hover names; offer Project switching through a compact picker.
2. Fuzzy Quick Open for Projects, Cards, Notes and existing Card/profile conversations, using one search field and a candidate list with concise ownership labels. Provide keyboard selection/activation/dismissal and reuse current destinations and lifecycle rules.

General navigation history, Terminal/Changes/Report tool tabs, quote-to-comment feedback and advanced Review remain separate proposals. Their presence in this research does not authorize their implementation in this slice.

Acceptance should cover wide/narrow and light/dark layouts, rail hit targets/tooltips/selection/badges, Project switching, cross-Project fuzzy results, duplicate names and Card numbers, keyboard behavior, empty results, exact Agent association and closed/unavailable handling. Searching or choosing a candidate must not approve a gate, start work implicitly or fetch a terminal preview. Use production Gallery components and an isolated native application/Core for integration checks.
