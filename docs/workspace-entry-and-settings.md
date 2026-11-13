# Project entry, grouped Overview and Settings

Implemented 2026-11-08. This refines the [compact navigation](quick-open-and-navigation.md) contract without changing workflow, scheduler admission or Agent lifecycle policy.

## Project entry

Without a valid selected Project, the Board shows only its Project selector and a **Create a Project** entry card. It does not render New Card or workflow columns. The entire entry card is one accessible NSH Button, with shared hover/pressed/focus styling; there is no second creation button below it. Selecting/creating a Project restores the Project/work-location/Add Card header and stage columns. Board has no Execution slots summary or task filter toolbar.

```text
No selected Project                       Selected Project
Board                                     Board
[Choose Project v]                        [Project name v]    [+ New Card]
[ + Create a Project                    ]
                                          Backlog / Develop / Review
```

The Project-name selector still offers existing Projects and New Project in its footer. Clicking the entry opens the same creation form. Cancel returns to Board without creating anything.

### Simplified New Project form (2026-11-09)

The creation form now shows Directory first, then Name, a collapsed Execution machine disclosure, and Cancel / Create Project. The page subtitle, Project details section heading and routine field descriptions are removed. Fields retain accessible labels, shared focus/disabled styling and necessary error feedback.

```text
New Project

Directory
[/absolute/path/to/canban                 ] [Choose…]

Name
[canban                                            ]

▸ Execution machine

                         [Cancel] [Create Project]
```

The default execution machine is local (the AppServer host); users do not need to open or fill the machine setting. Expanding it reveals the existing machine picker. A selected remote machine remains named in the collapsed summary. Switching machines clears the previous directory, resets browser results and invalidates pending responses; a custom Project name is retained.

Name follows the directory basename, including typed/pasted paths and either chooser. It keeps following directory changes until the user explicitly edits Name; later directory changes preserve that custom text. Trailing directory separators are handled. Empty names/directories disable Create Project. New paths are still created by the existing server operation.

There is one Choose action beside the path: directly local UI uses the macOS folder picker, while remote machines and explicit AppServer endpoints use the server-side browser. The duplicate Browse on machine action is removed. Work location settings reuse this same path control while retaining their execution-change explanation.

## Source directory

The shared `ui-kit::directory_field` combines an editable path with **Choose…**. It uses NSH `pick_paths(OpenDialog::folders())`, so macOS displays its native folder-selection sheet; no new native bridge or dialog dependency is introduced.

- Choosing a folder fills the path. The New Project presenter generates its name from the basename until the user edits it; a user-supplied name is preserved.
- Cancelling preserves the path and name. The chooser disables its own launch action while open, reports picker failures inline and ignores answers after its owning field has been disposed.
- If a path has already been entered, the chooser starts at its nearest existing absolute directory. Typed/pasted paths remain supported, including a directory that will be created on submit.
- Choosing a folder does not create a Project. The explicit Create Project action remains the commit point.

The UI Kit emits the chosen path; Project-name policy and `project.create` stay in product UI. Field values and submit validation are controlled by the form. The existing backend remains responsible for directory creation and Project validation.

## Overview

Overview now renders compact Needs you, Updates, Running and Queued groups, with Idle and Closed collapsed by default. No filter toolbar is required. Rows directly open Card, Review, Terminal or saved history; the previous context inspector has been removed. Selection, scroll, expansion and row keyboard focus survive Back. See [current Overview](product/overview.md) and [2026-11-11 refinement acceptance](overview-terminal-acceptance-20261111.md). Dated acceptance below records the earlier inspector implementation.

## Settings composition

Settings pairs a section jump column (208 points wide on desktop viewports, collapsing into a horizontal top jump strip when `narrow < 950` points without unmounting the content scroll) with one screen-owned vertical scroll (`max_w` 880 points). Clicking a jump entry scrolls directly to that section while pinning its active highlight across smooth-scroll animation and clamped bottom offsets until the user scrolls away; manual scrolling updates the active item via scroll-spy. Shared `settings_section`, `settings_field`, `settings_nav_heading` and `settings_nav_item` components provide consistent group headings, surfaces, borders, spacing and focus/hover states. Numeric fields are bounded to 160 points; descriptions wrap in narrow layouts.

| Nav group | Section | Capabilities exposed |
| --- | --- | --- |
| Interface | Theme and support | Notification sound toggle, theme picker, shortcuts palette, issue reporting and diagnostics copy |
| Interface | Terminal | Installed monospaced font family picker, point size controls (9–32 pt) and reset |
| Interface | Updates | Current build version, channel check and install/restart |
| Execution | Agent scheduling | Execution summary, global concurrency, idle timeout and Save scheduling settings; validation stays inside the group |
| Execution | Machines | Local host and SSH-configured remote execution machines |
| Execution | Agents & Identities | Enabled Herdr CLI Agents, `@identities`, default interactive Agent, Setup wizard and configuration reload |
| Execution | Herdr | Runtime executable/session and explicit Check connection |
| System | Storage | Application-home location and path copy |
| *(Bottom divider)* | Acknowledgements & Licenses | Canban Apache-2.0 attribution, core/runtime credits (`Herdr`, `sheprd` / formerly `herdr-gpui`, `NSH`, `Ghostty`), asset/upstream credits (`CodexBar`, `iTerm2-Color-Schemes`, `Lucide`, `Lobe Icons`, `Primer Octicons`) with license badges and repository links, plus a collapsible bundled-notices and full license viewer |

Save scheduling settings persists both fields; concurrency must be positive, idle timeout nonnegative, and 0 disables idle reclamation.

## Components and Gallery

| Owner | Contract |
| --- | --- |
| `ui-kit/entry_card.rs` | Generic icon, title, supporting text and press intent; one NSH Button |
| `ui-kit/directory_field.rs` | Controlled directory string, native chooser/cancellation/error/pending behavior and path-selected intent |
| `ui-kit/settings.rs` | Section/field visual composition plus `settings_nav_heading` / `settings_nav_item` jump controls; parent owns scrolling and persistence |
| `ui/settings.rs` | Settings middle jump rail, narrow horizontal jump strip, scroll-spy/pin model, and `Acknowledgements & Licenses` section |
| `ui/board.rs`, `ui/screens.rs` | Project availability, form/autoname policy and application operations |
| `ui/overview.rs` | Compact global rows and direct destinations; no filtering |
| `ui/components/scheduling.rs`, `ui/profiles.rs` | Shared settings section and content-only profile projection |

The production Gallery includes Empty Board, Settings, existing all-state Overview fixtures, and shared entry/directory controls. Narrow viewport now constrains the workspace to 720 points, crossing the native narrow-layout breakpoint. These scenarios reuse production components and require no application data or Agent.

## Validation

Native acceptance used the isolated packaged app and configuration at `/private/tmp/canban-settings-1rlm3roy`. The task-owned home/Core endpoint is separate from the user's live Canban/Herdr. No Agent was launched.

Verified through actual native UI interactions:

- Empty Board has just the Project selector and one clickable entry; it has no task filters, summary or New Card.
- Clicking the entry opens the form. Blank fields disable submit. Choose opens the macOS folder sheet; selection fills the directory and blank name, cancellation retains both, and a second folder selection preserves a custom name.
- Creating the Project from the selected folder returns to its Board and restores task controls.
- Invalid scheduling concurrency shows inline validation; saving concurrency 6 and idle timeout 0 updates the live summary and persists both values after leaving/reopening Settings. The single Settings scroll moves scheduling out of view and reaches Workflow, Herdr and Storage.
- Gallery Overview simultaneously exposes Needs you, Ready for review, Working, Queued and Idle; Closed starts collapsed and expands independently. Selecting a permission row opens its correct context; narrow layouts return from the inspector to the complete grouped list.
- Settings group styling, bounded inputs, wrapping descriptions and action reachability were checked in light/dark appearance at 720-point constrained width.

The scratch `native-checks.json` records the checked interactions; `final-snapshot.json` retains one Project and zero Cards, requests or attempts, and `agents.json` retains concurrency 6 / idle timeout 0. The task-owned acceptance app and Gallery were closed after verification.

All 62 Rust tests, all-target Clippy with `-D warnings`, application/Gallery builds, formatting and whitespace checks passed. Chinese IME remains excluded at the user's request. Provider execution, new workflow behavior and macOS notifications are outside this UI refinement's acceptance scope.


## Simplified form acceptance (2026-11-09)

An independent `Canban Auto Acceptance` app used the disposable home `/private/tmp/cb-utilities-zl_w4pdq/home`; the user's running Canban window and default data were left alone. No Agent was launched.

- Native form showed only New Project, Directory/Choose, Name, collapsed Execution machine and Cancel/Create. Typed directory changes updated the generated name, including a trailing slash; explicit name edits survived subsequent path changes.
- Native Choose opened the macOS folder sheet. Cancel preserved path/name. Selecting `directory-picked` generated that name; Create opened its empty Board. AppServer snapshot confirmed the saved name, directory and local machine.
- A second launch with an explicit isolated AppServer endpoint used the same Choose action to open the server browser. Navigating into `directory-picked` and selecting it updated both path and generated name. This exercises the server browser path, not live SSH-machine acceptance.
- Gallery's 720-point New Project scene retained a manual name while switching to Build machine, cleared the old directory, disabled creation until a new path was entered and kept Build machine visible after collapse. No duplicate Browse action remained.
- Application/Gallery builds, 113 workspace Rust tests, UI/all-target Clippy with warnings denied, and whitespace checks passed.
