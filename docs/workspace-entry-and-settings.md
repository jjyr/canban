# Project entry, grouped Overview and Settings

Implemented 2026-11-08. This refines the [compact navigation](quick-open-and-navigation.md) contract without changing workflow, scheduler admission or Agent lifecycle policy.

## Project entry

Without a valid selected Project, the Board shows only its Project selector and a **Create a Project** entry card. It does not render scheduling statistics, task filters, New Card or workflow columns. The entire entry card is one accessible NSH Button, with shared hover/pressed/focus styling; there is no second creation button below it. Selecting/creating a Project restores the normal Board's controls and filters.

```text
No selected Project                       Selected Project
Board                                     Board
[Choose Project v]                        [Project name v]    [+ New Card]
[ + Create a Project                    ]  execution summary
                                          All / Needs attention / Running / Review
                                          Backlog / Develop / Review
```

The Project-name selector still offers existing Projects and New Project in its footer. Clicking the entry opens the same creation form. The form requires a name and source directory; Cancel returns to Board without creating anything.

## Source directory

The shared `ui-kit::directory_field` combines an editable path with **Choose…**. It uses NSH `pick_paths(OpenDialog::folders())`, so macOS displays its native folder-selection sheet; no new native bridge or dialog dependency is introduced.

- Choosing a folder fills the path. A blank Project name is suggested from its basename; an existing user-supplied name is preserved.
- Cancelling preserves the path and name. The chooser disables its own launch action while open, reports picker failures inline and ignores answers after its owning field has been disposed.
- If a path has already been entered, the chooser starts at its nearest existing absolute directory. Typed/pasted paths remain supported, including a directory that will be created on submit.
- Choosing a folder does not create a Project. The explicit Create Project action remains the commit point.

The UI Kit emits the chosen path; Project-name policy and `project.create` stay in product UI. Field values and submit validation are controlled by the form. The existing backend remains responsible for directory creation and Project validation.

## Overview

Overview renders all nonempty groups in the existing projection order: **Needs you**, **Ready for review**, **Working**, **Queued**, **Idle**, then collapsed **Closed**. There is no All/Needs you/Working filter toolbar or filter state. Each conversation/recovery row appears once. The internal `Needs attention` group key remains unchanged; only its section heading reads Needs you. The global actionable count includes human Review decisions as before, so it can exceed the first group's count.

The context inspector, exact Card/Terminal destinations, retained selection/scroll and Closed expansion continue to work. Wide layouts show list and inspector; narrow layouts show the inspector with a return to Overview. Empty groups are omitted; an empty Overview shows one contextual empty state. There is no duplicate All group.

## Settings composition

One screen-owned vertical scroll contains every group, with a maximum content width of 880 points. Shared `settings_section`/`settings_field` components provide 16-point group headings, consistent surfaces, borders, spacing and field/helper alignment. Numeric fields are bounded to 160 points; descriptions wrap in narrow layouts. The scheduling section has no independent scroll or pinned region.

| Group | Existing capabilities exposed |
| --- | --- |
| Agent scheduling | Execution summary, global concurrency, idle timeout and Save scheduling settings; validation stays inside the group |
| Agent profiles | Configuration path, profile/role list and Reload configuration |
| Workflow | Default workflow JSON location and its scope for new Projects |
| Herdr | Runtime executable/session and explicit Check connection |
| Storage | Application-home location |

This is a grouped presentation of current capabilities, not a new template/profile configuration GUI. Save scheduling settings persists both fields; concurrency must be positive, idle timeout nonnegative, and 0 disables idle reclamation.

## Components and Gallery

| Owner | Contract |
| --- | --- |
| `ui-kit/entry_card.rs` | Generic icon, title, supporting text and press intent; one NSH Button |
| `ui-kit/directory_field.rs` | Controlled directory string, native chooser/cancellation/error/pending behavior and path-selected intent |
| `ui-kit/settings.rs` | Section/field visual composition; parent owns scrolling and persistence |
| `ui/board.rs`, `ui/screens.rs` | Project availability, form/autoname policy and application operations |
| `ui/overview.rs` | Grouped projection and context-inspector composition; no filtering |
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
