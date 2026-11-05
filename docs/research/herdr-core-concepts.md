# Herdr concepts and their relationship to Canban

Research date: 2026-11-05. This document explains the **product model**, separately from the [code architecture](herdr-core-architecture.md). Existing behavior is source-confirmed; sections marked **Proposal** describe Canban design, not an existing Herdr API.

Sources inspected:

- `herdr-core` 0.1.18, commit [`c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a`](https://github.com/herdrdev/herdr-core/tree/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a).
- `herdr-app`, commit [`e77f112ac2ea136b84fe510efa42d41163bfd776`](https://github.com/herdrdev/herdr-app/tree/e77f112ac2ea136b84fe510efa42d41163bfd776).
- Both research checkouts were clean. This is source inspection, not a running-client compatibility test. It does not describe the older Herdr 1 multiplexer.

## The essential distinction

**Space belongs to herdr-app. herdr-core runs terminals; it has no Space, Board, Card, Tab, Pane, repository, PR, or Kanban workflow object.** Core deliberately excludes workspace and layout features. Herdr app stores its Space structure in core's opaque JSON documents, but core does not understand that structure. Storage location and product ownership are different concerns. [C1, C6, A1, A4]

This matters for Canban: adopting core does not require adopting Herdr app's workspace hierarchy. Canban can own Human Context, notes, boards, cards, and dispatch rules while core owns the programs doing the work.

```text
HERDR APP: where a person looks           HERDR-CORE: what keeps running

Space "Payments"                         Core instance / runtime session
  folder: /src/payments                     terminal t1: interactive agent
  machine: local                           terminal t2: shell
  |                                        terminal t3: delegated worker
  +-- Tab "Design"                         terminal t4: another worker
  |     +-- Pane p1 ----------------------> t1
  |     +-- Pane p2 ----------------------> t2
  |
  +-- Tab "Notes"
        +-- Pane p3: a mod panel              t3/t4 can run without any pane

Space / Tab / Pane = app organization       Terminal = runtime identity
An app pane can disappear while a           A terminal does not need a Space
terminal remains running via parking.      or a visible viewer to exist.
```

The diagram expresses relationships, not process boundaries. Closing a normal pane is a destructive app action that ends its terminal; **parking** a terminal explicitly removes its pane while retaining the running program. Closing the app window leaves terminals running. [A1, A3]

## Core's actual concepts

| Concept | Meaning in current core | Consequence for Canban |
| --- | --- | --- |
| Core instance | Runtime with a permanent random `core.id`, a display name, terminal registry, events, documents, and machine connections. The ID resets when state is wiped. | Persist the core identity alongside terminal bindings; a terminal address alone is insufficient across runtime resets. |
| Session | `HC_SESSION` selects an isolated core socket and state namespace; default is `default`. It is not a project or conversation. Config and templates are not automatically separated by this name. | Use sessions for runtime separation, not one session per card. A session is not a permissions sandbox. |
| Terminal | Persistent runtime record addressed as `t7`, optionally with a human name, command, launch folder, parent, labels, lifecycle facts, and reported status. | Bind a dispatch attempt to its terminal address, not its mutable display name or PID. |
| Program | The shell, agent, server, build, or other command running inside a terminal. | A live terminal is not necessarily an agent. A recovered terminal may contain a fresh shell. |
| Agent | A program doing agent work. Core does not maintain a brand-specific agent registry or infer agent state from screen text. | Keep the configured agent profile in Canban; show program status only when observed. |
| Parent / child | A terminal may have a parent; one parent can have many children. Starting a terminal from inside an hc terminal makes it a child by default. Children normally end with the parent unless `outlive_parent` is set. | This is an execution/delegation relationship, not a Card/subtask hierarchy. An app-created run need not have a terminal parent. |
| Viewer | A client attached to a terminal's stream. Multiple viewers can watch one terminal; observe-only viewers cannot type or resize it. | A Kanban card, terminal panel, and Herdr window can refer to the same running terminal. Viewer state should not become task state. |
| Machine | A named connection to another core. The remote core owns its programs and files; the local core merges terminal lists/events. A name such as `box` is local to that connection. | Keep machine identity and repository paths explicit. `box/t7` is an address seen through the local core, not a globally permanent ID. |
| Labels | Free-form string key/value metadata on a terminal. Core can filter by them but does not interpret their values. Empty updates remove keys. | Use namespaced correlation labels; rule semantics and tag resolution stay in Canban. |
| Document | Opaque JSON addressed by name, with compare-and-set versioning and a 1 MiB value limit. Core persists it without interpreting it. | Useful for small shared client metadata; it is not a note editor, relational task store, or durable audit history. |
| Template | A Markdown file defining an agent command, opening role, readiness and input settings. Remote runs resolve templates on the destination machine. | A reusable launch mechanism; it does not provide Kanban triggers, quotas, retry policy, or a scheduler. |

Evidence: core identity and run parameters [C2]; session paths [C3]; terminal fields and retention [C4]; status [C5]; documents/viewers/events [C6]; templates [C7]; remote machines [C8].

```text
Machine A                                  Machine B
  Core session "canban"                      Core session "default"
    core.id = A                               core.id = B
    |
    +-- t1: lead agent                       +-- t7: worker
    |     +-- t2: child worker               |
    |     +-- t3: child build                +-- files and cwd live here
    |
    +-- t4: unrelated terminal
    +-- connection alias "box" ------------> B
         exposes remote terminal as box/t7

Canban bindings record the owning core / machine identity and terminal ID.
The connection alias is a route/display name, not task ownership.
```

Parentage can cross cores; the local nesting in this diagram is just the simplest case. A session separates state and terminals, not OS access privileges. [C2, C8]

## Herdr app's Space, Tab, Pane, and Layout

These concepts are useful product references even if Canban uses a different UI.

| App concept | Current meaning | What it does not imply |
| --- | --- | --- |
| Space | A place to work, holding a default folder, one machine, and tabs. Usually associated with a repository, but no required Git repository identity exists in `Space`. | It is not a core session, a Git worktree, or an enforced repository boundary. |
| Tab | A named piece of work in a Space, holding a split tree, focused pane, zoom, and settled state. | It is not a durable task record or an agent execution. |
| Pane | A visible block in a tab's arrangement. It can show a terminal or a mod. | A pane ID is not a terminal ID; some terminals have no pane. |
| Layout | The window arrangement and presentation, composed from blocks, rows, columns, stacks, active-tab content, and mods. Tabs also carry their own arrangement tree. | Changing window presentation need not change the underlying Spaces or runtime. |
| Mod | JavaScript code drawing app panels, dialogs, menus, or content panes. | It is not an independent core runtime or agent process. |
| Agents sidebar | An index of agents across Spaces, including delegated workers that may have no pane. | It is not a second container that owns those agents. |

The app's typed model stores `Model.spaces -> Space.tabs -> Tab.root`. A `Space` has one `machine: Option<String>`; `None` means this computer. Tabs and splits stay on that machine. The model is shared across window layouts. [A1, A2, A5]

Space and tab names can be explicit. An unnamed Space follows its first terminal's checkout/folder; an unnamed tab uses its position. This is presentation inference, not an authoritative Project ID. Current foreground folders and Git folder/branch information are separate observations used for display. [A2]

```text
Window layout: navigation + agents + active tab + panels
                            |
                            v
App work organization, independent of the chosen window layout

Machine local
  Space "API"       default folder /src/api
    Tab "Investigate"
      Row
        Pane p1 -> terminal t1
        Pane p2 -> mod "notes", props {file: ...}
    Tab "Implement"
      Pane p3 -> terminal t2, cwd /worktrees/api-fix

Machine box
  Space "Remote API" default folder /srv/api
    Tab "Build"
      Pane box/p4 -> terminal box/t7

Space is not enforced to be one checkout:
its default folder initializes work, while terminals can use other folders.
```

### Structure is shared; viewing choices are local

Herdr app serializes machine-owned Spaces, tabs, and pane trees into `doc.get` / `doc.set` with document name `herdr-app`. Its own code interprets this JSON. Each screen keeps local selection, focus, zoom, settled state, and recent Spaces through a separate `View`. [A4]

```text
herdr-core opaque document "herdr-app" on each machine
  { spaces: [ { id, name, folder, tabs: [...] } ] }
                  |
          shared structural data
           /                  \
Desktop A                     Desktop B
same Spaces/tabs               same Spaces/tabs
own selected tab               own selected tab
own focus / zoom               own focus / zoom

Core stores the JSON. The app defines and renders its meaning.
```

Canban should not rewrite the `herdr-app` document to introduce its own task data. Use Canban's own storage and optional separately named metadata documents. Sharing a core terminal with Herdr does not automatically create a Herdr pane or Space for it. Opening that terminal in Herdr is a separate app/UI action.

### Lifecycle carries different meanings

- Closing the Herdr window leaves programs running.
- Closing a pane, tab, or Space normally ends its terminals. Closing the last pane also removes its empty tab; removing the last tab removes its Space. [A2, A3]
- Parking removes a terminal pane and disables core keep, while its current program continues. Herdr app's `terminal.keep` means giving a pane-less terminal a pane; core's `terminal.keep` means recovery after program loss. The same action name belongs to different APIs. [A3, C6]
- Core `keep` recovery starts a **fresh shell**, never reruns the old task. A program exit or explicit kill remains final. The terminal record and its labels may survive recovery while the original execution is gone. [C6]
- Core finished-terminal retention is bounded: default one day, with parent-result visibility rules, and at most 500 finished records. It cannot be the complete lifetime timeline of a card. [C4]

For Canban, hiding a task panel, archiving a card, cancelling a run, and deleting a Space should be separate actions with explicit lifecycle policies.

## Program status, attention, and task completion

Core distinguishes terminal lifecycle (`live` / `finished`) from program-reported status (`idle`, `working`, `done`, `blocked`, `error`, or no status). It also has a `turn` counter that advances whenever the root status **leaves working**, including a block, interruption, clear, or exit. Child status records do not increment that counter. A turn is not proof of successful task completion. [C4, C5]

Herdr app adds a human-attention projection. Shared `herdr.seen=<turn>` and `herdr.pinned=1` labels support read/unread and pins across clients. In its attention rules, an idle/done agent can show as Done when the latest turn has not been seen, then Idle after a person views it. Core's reported state is not rewritten by this presentation. [A6]

```text
Observed runtime facts           UI attention              Canban task outcome

terminal live / finished         needs human attention     proposal awaiting review
program status / no status  -->  unread completion    -->  accepted / changes requested
exit code, turn                  seen / pinned             integrated / closed

Facts from core                  client projection         Canban-owned decisions
```

Canban should retain all three separately. `done`, exit code 0, a new turn, or looking at an agent must not independently mark a code change reviewed or integrated. A program that does not report OSC 7501 should show **status unavailable**, together with the observed lifecycle and launch/delivery facts, rather than fabricated Working/Idle transitions.

## Current Canban mapping: Project, Board, Card, and run

Updated after the user's product clarification. Earlier Canban Space suggestions are superseded by an app-owned, directory-backed **Project** with its own Board. This changes Canban's product design, not the verified Herdr concepts above.

```text
CANBAN PRODUCT DATA                            CORE EXECUTION DATA

Project: Payments
  +-- required directory (usually Git)
  +-- Notes / decisions / resources
  +-- Board: Backlog / Develop / Review / custom stages
  |     +-- Card #42: improve retry
  |           +-- description + routing tags
  |           +-- timeline: comments + updates + executions
  |           +-- development request
  |                  +-- attempt R1 ----------> terminal t7
  |                  +-- attempt R2 ----------> terminal t9
  |           +-- review request -------------> reviewer terminal
  |           +-- changes / outcomes
  +-- Archive
  +-- Template + workflow callbacks + agent routing

Product history persists after terminals and runtime records disappear.
```

| Canban concept | Mapping | Boundary |
| --- | --- | --- |
| Project | Canban-owned directory-backed record and Board | No required Herdr Space; Git optional |
| Board/stage/template | Canban-owned workflow organization and triggers | Backlog mandatory; stage decides what work is eligible |
| Card | Stable internal task ID plus project-local number | Not a tab or terminal; survives retries/archive |
| Timeline/comment/resource | Canban-owned durable history and references | Core events/output are observations, not the whole story |
| Request/run | Automatic workflow intent and attributable execution attempt | A retry gets a new execution identity; recovered shells do not resume the old task |
| Card tag / agent profile | Canban routing policy selecting the allowed agent | No-tag default and tagged matching belong to Canban |
| Runtime binding | Core/machine identity + terminal address + run labels | Labels aid reconciliation without becoming task authority |
| Directory/worktree/change | Project directory and per-workflow execution/artifact references | Core accepts cwd but does not own Git isolation or review |
| Issue/PR | Optional source/link; local cards first | GitHub issue backend is later work |

Canban's product Session is a generic agent conversation/execution, distinct from the core HC_SESSION namespace. Current user direction is a fresh conversation per submitted @mention or executable-stage request, with no question/stage session types. Canban schedules via core; the proposed MVP reuses Run identity for the Session. See [Session model](../product/sessions.md).

Use labels such as canban.project, canban.card, and canban.run as Canban conventions. A stage-entry workflow may prepare a worktree and then run a core terminal. Entering Review can schedule a review/report workflow without requiring the advanced editor.

The exact runtime/process route remains a recommendation; see [integration options](herdr-core-integration.md). Current product contracts and remaining questions are in [vision](../product/vision.md), [Projects/templates](../product/projects-and-workflows.md), and the [product log](../product/product-log.md).

## Source references

All links are pinned to the inspected commits.

- **C1** — [Core scope rule](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/AGENTS.md#L5-L36): runtime, no workspace/layout ownership, OSC-only status.
- **C2** — [API types](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api/types.rs#L12-L20), [run parameters and parent policy](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api/types.rs#L199-L299).
- **C3** — [Session, socket, state, and common config paths](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/paths.rs#L8-L35).
- **C4** — [Terminal identity and facts](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/terminals.rs#L9-L150), [finished-record expiration](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/terminals.rs#L372-L401).
- **C5** — [Status and turn semantics](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/status.rs#L1-L156).
- **C6** — [API guide: viewers, events, recovery, opaque documents, and status](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/schema/README.md).
- **C7** — [Template configuration and destination-machine resolution](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/docs/templates.md).
- **C8** — [Machines and session limitations](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/docs/machines.md).
- **A1** — [App product concepts and lifecycle](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/README.md#L40-L79).
- **A2** — [Space/Tab/Agent model](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/model.rs#L1-L89), [folder/name inference](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/model.rs#L236-L263), [container removal](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/model.rs#L327-L379).
- **A3** — [Space/Tab/Pane and terminal app actions](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/docs/commands.md#L20-L169), including app `terminal.keep` / `terminal.park`.
- **A4** — [Shared machine document versus local View](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/mirror.rs#L1-L153), [the `herdr-app` core-document adapter](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/mirror_sync.rs#L74-L109).
- **A5** — [Window layout content types](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/layout.rs#L1-L35).
- **A6** — [Shared attention and pin semantics](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/docs/labels.md).
