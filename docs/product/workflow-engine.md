# Unified workflow engine, Studio and implementation

Status: implemented and active (reconciled 2026-11-12; SQLite schema 12, desktop API 10). This is the canonical product and engineering contract for Canban Workflows, consolidating the unified engine rules, Project/Board workflow ownership, Visual Studio UX, YAML schema, RPC/CLI operations, workspace lifecycle, and acceptance evidence. [Events and subscriptions](events-and-notifications.md) owns the durable journal envelope and trigger delivery.

## Core design principles

1. **Single source of truth and one runtime:** Declarative YAML stored in SQLite (`workflow_sources` + immutable `workflow_revisions`) is the sole workflow definition and authoring model. There is no secondary `Stage`-callback executor, JSON `WorkflowDefinition` authoring RPC, or `legacy_definition` runtime fallback; all programmatic and interactive workflow submissions pass YAML directly (`workflow.check`, `workflow.apply`, `project.create.workflow_yaml`). Manual YAML edits, Visual Studio option toggles, and **Edit with agent** submissions all pass through the same schema validation, optimistic concurrency revision checks, and transactional publication path.
2. **Graph-derived, non-destructive UI:** Visual Studio renders one Stage lane per Board column directly from the active YAML graph (`canban_workflow_engine::stages`); there are no column-specific preset panels. Every step is configured inline on its card, and structural edits (add/move/disable/delete steps, set Stage exits, add/rename/reorder/remove columns) splice only the affected edges while preserving custom columns, custom steps, prompts, rework/branch references and unrelated rules. The older option helpers (`source::derive_options` / `set_*`) remain for CLI/tests but no longer drive Studio.
3. **Pragmatic default flow (human-controlled intake, automated delivery):** New Projects default to three Board columns (`Backlog -> Develop -> Review`, extensible via YAML `columns`):
   - **Backlog (quiet intake):** Joining Backlog does **not** automatically analyze or move Cards by default, avoiding unintended compute spend on rough ideas. Teams wanting hands-free pipelines can enable **Backlog Auto-Analyze** (`agent:plan`) and/or **Backlog Auto-Start Develop** (which waits for analysis to finish when both are enabled). Auto-Start Develop can be **Immediately** or **When Agent free**: the latter inserts `admit:develop` (`uses: admit`, `agent_slot: true`) so planned Cards stay in Backlog until a global automatic Agent slot is free, and—when analysis is enabled—`admit:plan` (`agent_slot: false`, `max_ready: K`, default 2) so at most K planned Cards wait in Backlog. Waiting Cards are admitted in Board order (drag to prioritize).
   - **Develop (automated implementation):** Entering Develop automatically runs `@developer` (`agent:develop`) in an isolated managed worktree and moves the Card to `Review` (`move:develop`).
   - **Review (Queue Merge & upstream sync):** By default Review omits a redundant `@reviewer` Agent pass (users can add one via YAML or Studio) and enters **Queue Merge (`deliver:review`)**, which commits/merges the clean worktree into the target branch and syncs upstream (`sync_upstream: true` by default). Teams can switch Review to **Manual Review** (`approval:review`) or **Merge Only (No Sync)** (`sync_upstream: false`).
4. **Safe-by-default delivery and human override:**
   - **Source-trust branching:** Even when Queue Merge is active, untrusted external PRs (`card.source_trust == "external_pr"`) are routed by `condition:external_pr` to a required human approval gate (`approval:review`) by default.
   - **Per-Card hold and graceful fallback:** Users can toggle **Hold auto-merge** (`card.hold_auto_merge`) on any Card at any time. Whenever a Card is held, has active/uncertain Agent terminals, encounters a dirty target checkout, or fails upstream remote push (`review.upstream_sync_failed`), `deliver` never forces an unsafe operation—it pauses as an inspectable human decision gate (`waiting_approval`), allowing manual resolution or rework feedback to the Agent.
   - **Merge-queue algorithm:** Deliveries run one at a time (global review gate). Each attempt syncs the target from upstream, commits the reviewed Card worktree, merges the target into the Card worktree, then fast-forwards the Project target branch and pushes. A clean (fast-forward or conflict-free three-way) merge delivers immediately. A textual **merge conflict** leaves the Card worktree mid-merge and routes the run to the step's `on_changes_requested` Agent (`review.merge_conflict`) with the conflicted files and target head, up to `max_conflict_attempts` (default 3, `0` disables) per run; the Agent resolves/commits and the run re-enters Queue Merge. Beyond the budget, or without a rework target, it pauses for a human. If the target branch advances during delivery (`review.merge_retry`), the run simply stays queued and retries.
5. **Board stage decoupled from workflow progress:** Columns represent business stage position; `WorkflowRun` steps represent execution progress. Planning and approvals can run while a Card stays in `Backlog`. Workflow-owned `card.move` transitions carry causal ancestry (`origin: "workflow"`, `workflow_run_id`) and continue the active `WorkflowRun` without re-triggering duplicate column-entry Agents.
6. **Deterministic boundaries (Engine vs. Scheduler, explicit completion, frozen revisions):**
   - **Engine vs. Scheduler:** Workflow Engine determines *what* happens next and enqueues `Request`s; Scheduler owns *when* terminals launch under the global concurrency budget and conversation reuse rules.
   - **Explicit completion only:** Only an Agent's bound CLI `completed` report advances its owning step. Terminal idle, root `done`, process exit, or `@profile` mention replies never advance workflow steps.
   - **Frozen run revisions:** Each `WorkflowRun` and queued `EventDelivery` captures the published definition revision at creation. Publishing new YAML takes effect immediately for future events without mutating in-flight runs or pending human gates.
7. **Agent-native authoring:** Users do not need a complex drag-and-drop node-wire canvas. Everyday adjustments use graph-derived Studio switches and inline Step Inspectors, while multi-column or custom branching designs use **Edit with agent** in a Project-owned configuration conversation in Terminal.

---

## Project, Board and workspace lifecycle

A Project has a stable identity, an execution directory/machine location, and its own Board. Git is common but optional; Project creation never clones or initializes Git implicitly, and directory errors never treat existing user files as disposable rollback data.

```text
Project
  +-- directory / execution location
  +-- Board: Backlog / Develop / Review (or custom YAML `columns`)
  +-- SQLite workflow sources & immutable revisions
  +-- durable event subscriptions
  +-- Cards
       +-- Board stage (column ID)
       +-- WorkflowRun (captured revision, step visits, outputs, decisions)
       +-- Agent step -> Request -> Attempt -> Card/profile Conversation -> Herdr PTY
```

### Worktrees and base synchronization

Worktree lifecycle governs when a task checkout is prepared, reused across stages/rework, and cleaned up on archive—never creating a fresh worktree per step:

```json
{
  "workspace": {
    "create_worktree": true,
    "base_branch": "main",
    "sync_base": true,
    "remote": "origin"
  }
}
```

- **Preparation:** Planning in `Backlog` can inspect the repository without forcing a branch checkout merely from Card creation. When an executing task prepares a Git worktree (`create_worktree: true`), it optionally fetches and fast-forwards the configured base branch (`sync_base: true`) if clean, then creates `<dir>-<proj4>/<card-number>`. Divergence, dirty checked-out bases, or fetch failures pause safely instead of rebasing or discarding edits. Non-Git projects can disable `create_worktree` and `sync_base`.
- **Delivery & upstream sync:** Queue Merge (`deliver`) integrates the Card worktree into the target branch. When `sync_upstream: true` (the default) and the repository has a push remote configured, Canban fast-forwards the target branch from upstream before previewing/merging and pushes `refs/heads/<target>:refs/heads/<target>` to the remote after merging (gracefully skipping push when no remote is configured, or holding with `review.upstream_sync_failed` if push fails). Setting `sync_upstream: false` merges locally into the target branch without fetching or pushing upstream.
- **Archive & restore:** Merged Cards with no active Agent terminals or dirty worktrees auto-archive cleanly. Archiving removes clean managed worktrees after preserving their commit under `refs/canban/cards/<card-id>`; dirty worktrees are retained with a notice. Restoring a Card expands archived artifacts without implicitly re-entering column workflows (`restoration` events are suppressed unless explicitly subscribed).

---

## Visual Studio, Step Inspector and storage

Open **Workflows** in the Project header or **Project Workflows** in Settings. All workflow sources live exclusively in SQLite:

```text
canban.sqlite
  workflow_templates       SQL bootstrap for legacy compatibility
  workflow_sources         Project + filename + active revision/hash/columns/rules
  workflow_revisions       immutable original YAML + compiled WorkflowDefinition
  workflow_runs            captured revision + visit checkpoints + inputs/outputs
  event_subscriptions      selectors + target + durable journal cursor
  event_deliveries         captured definition revision + receipt/backoff state
  configuration_sessions   Project + workflow-configuration purpose + Profile/history
```

No Workflow YAML mirror or revision file is created under `~/.canban`; external `.yaml` paths are used only when explicitly importing/exporting via CLI.

### Studio layout

```text
Project / Workflows                                                 [Edit with agent]
-------------------------------------------------------------------------------------
development.yaml       | [Visual Studio] [YAML Source]                        [History]
issue-planning.yaml  • |
[+ New workflow]       | Stage Execution Graph · Development [Active]   [New stage] [+ Add]
                       | +-------------------+-------------------+------------------------+
                       | | [1] Backlog       | [2] Develop       | [3] Review             |
                       | | Analyze:[Off|Auto]|                   | Delivery:[Queue|Manual]|
                       | | Start:[Stay|Auto] |                   | Sync:[Sync | Local]    |
                       | |                   |                   | Ext.PR:[Human|Same|Man]|
                       | | ----------------- | ----------------- | ---------------------- |
                       | | Quiet intake      | ⚡ on:develop      | ⚡ on:review            |
                       | |                   | Implement (@dev)  | External PR check      |
                       | |                   |        ↓          |           ↓            |
                       | |                   | Move to Review    | Queue merge (Sync)     |
                       | +-------------------+-------------------+------------------------+
                       |
                       | Step Inspector · Queue merge (deliver:review)
                       |   Upstream sync: [Merge & Sync Upstream] [Merge Only (Local)]
                       |   Target branch: [Project default]   Rework target: [Implement]
                       |                                                        [Save · ⌘S]
```

1. **Stage Execution Graph (layout-only reactive updates):** Groups steps into swimlanes matching the Project's Board stages (`columns`), showing triggers (`⚡ on:develop`), branching (`✓ Then` / `✕ Else`), rework loops (`↺ Rework`), and inline `+ Add stage` / column removal controls. Stage lanes, inline policy switches, keyed step cards, and the Step Inspector stay mounted across clicks, policy toggles, and prompt typing without global view regeneration.
2. **Inline Stage Automation Controls:** Graph-derived stage controls (`source::derive_options`) live directly inside the **Backlog** (`New Card analyze`, `Auto-start Develop`) and **Review** (`Review delivery`, `Upstream sync`, `External PR`) lanes, surgically updating `on.backlog`, `agent:plan`, `move:backlog`, `on.review`, `condition:external_pr`, `deliver:review`, and `approval:review` in memory while preserving custom columns, custom steps, and custom prompts.
3. **Interactive Step Inspector:** Clicking any node binds the persistent Step Inspector below the graph:
   - `agent`: Profile selector (`@developer`, `@reviewer`, `@codex`, etc.) + persistent NSH Prompt editor with curated built-in defaults and **Reset to default prompt**.
   - `deliver`: **Upstream sync** (`Merge & Sync Upstream` vs. `Merge Only (Local)`), **Target branch**, and **Rework target**.
   - `approval`: **Gate message** (`title`) and **Rework target**.
   - `card.move`: **Destination stage** selector.
4. **YAML Source & History:** Syntax-highlighted NSH editor (2-space indent, soft wrap, line numbers), immutable **History** modal (**Load as draft**), concurrent `revision_conflict` comparison view that preserves unsaved local keystrokes, and atomic **Save** (`Command+S`).

### Board column hints and Card stepper

- **Column headers** display concise client-derived badges (`@developer`, `Queue merge · Sync`, `Queue merge · Local`, `Review`, etc.) via `source::column_summary` and a **workflow-node** icon. Clicking opens a native Popover (`source::column_details`) listing active triggers and flow effects with a **View workflows** link.
- **Card detail** renders active/recent `WorkflowRun`s as a **Step Progress Stepper** (`✓ Analyze task → ● Implement → ○ Queue merge`), exposing clickable report links, an inline **Hold auto-merge / Resume auto-merge** button when a run includes `deliver`, and a **Request changes** feedback box that injects rework instructions into the next Agent step visit.

---

## YAML language and execution semantics

YAML 1.2 parsing uses `serde_yaml_ng` with `deny_unknown_fields`. Duplicate keys, unknown fields, missing step targets, undeclared column/Profile IDs, and automatic cycles fail validation before publication.

```yaml
schema: 1
id: delivery:example
name: Development
enabled: true
columns:
  - id: backlog
    name: Backlog
  - id: develop
    name: Develop
  - id: review
    name: Review
on:
  develop:
    event: card.column_entered
    start_at: agent:develop
    match:
      column: develop
  review:
    event: card.column_entered
    start_at: condition:external_pr
    match:
      column: review
steps:
  - id: agent:develop
    name: Implement
    uses: agent
    with:
      profile: developer
      instruction: Implement the card task in the supplied working directory. Explain your changes and validation in your report.
    next: move:develop
  - id: move:develop
    name: Move to Review
    uses: card.move
    with:
      column: review
    next: condition:external_pr
  - id: condition:external_pr
    name: External PR check
    uses: condition
    with:
      pointer: /card/source_trust
      equals: external_pr
      then: approval:review
      otherwise: deliver:review
  - id: deliver:review
    name: Queue merge
    uses: deliver
    with:
      sync_upstream: true
    next: __end
    on_changes_requested: agent:develop
  - id: approval:review
    name: Human approval
    uses: approval
    with:
      title: External PR; human approval is required before merging
    next: __end
    on_changes_requested: agent:develop
  - id: __end
    uses: complete
workspace:
  create_worktree: true
  sync_base: true
  remote: origin
max_steps: 100
```

### Stages, disabled steps and relative moves

Studio presents every Board column as a **Stage**: an ordered pipeline of step cards plus an exit. Three optional YAML annotations make that structure explicit (all compiled away; the runtime interpreter is unchanged):

- `stage: <column>` on a step records the Stage it belongs to. When absent, the Stage is inferred by walking each column's entry rule and switching lanes after `card.move` (Studio writes the annotation on the first edit).
- `enabled: false` skips a step at compile time. References (`next`, rework, condition branches, trigger starts) continue at its successor; a disabled `condition` follows `otherwise`; a skipped tail resolves to an implicit terminal.
- `card.move` accepts `column: $next` (the column after the step's Stage) and `next: $entry` (continue at the target column's entry rule start, or end the run). Studio exits default to `$next`/`$entry`, so reordering or inserting columns keeps exits pointing at the following Stage.

Structural edits (`canban_workflow_engine::stages`: insert/move/remove/disable steps, set exits, add/rename/reorder/remove columns) splice only the affected edges, preserving rework and branch references. Publishing a revision that **deletes the step a run is currently executing** marks the run `stop_after_step`: the step finishes, then the run ends as `cancelled` (`workflow.stopped`). **Removing a column** moves its active Cards to the first column (`card.stage_changed`, reason `column_removed`) instead of rejecting the publication.

Studio UI: Stage lanes are generated from the graph (no column-specific panels). Each step card is configured inline with a form derived from its `uses` (Agent profile, admission gate, Queue merge sync/branch/conflict attempts/rework, approval message, notify text, move target); prompts open in a dialog. Cards can be dragged by their ⋮⋮ handle within or across Stages, toggled On/Off, or deleted; Stage headers support rename, ← → reordering and two-step delete; `+ Add step` offers step templates; each Stage footer sets its exit (Next stage / Stay).

### Supported step types

| `uses` | `with` arguments and behavior |
| --- | --- |
| `agent` | optional `profile` (falls back to global `default_agent`), required `instruction`. Enqueues a `Request` bound to `run_id` and `<step_id>:<visit>`. |
| `card.move` | `column` (must exist in effective columns). Emits `card.stage_changed` with `origin: "workflow"`. |
| `condition` | JSON `pointer` into `{inputs, outputs, card}` (e.g. `/card/source_trust`), exact `equals`, `then`, and `otherwise` step IDs. |
| `approval` | `title`, optional `on_changes_requested` rework step ID. Pauses the run in `waiting_approval` and sets `Card.human_gate`. |
| `deliver` | optional `target_branch`, optional `sync_upstream` (boolean, default `true`), optional `max_conflict_attempts` (integer, default `3`), optional `on_changes_requested` rework step ID (also the automatic merge-conflict resolver). Enters `waiting_delivery` for Queue Merge, or pauses at `waiting_approval` when held/blocked. |
| `card.move` | `column` (column ID or `$next`), optional `next` (`$entry` continues at the target column's entry). |
| `admit` | optional `agent_slot` (boolean, default `true`: wait for a free global automatic Agent slot), optional `max_ready` (1..100: at most K Cards that passed this gate may remain in its column). Enters `waiting_capacity`; the AppServer admission pump releases runs in Board order before each scheduler dispatch. |
| `notify` | `message`. Appends a `workflow.notification` Timeline event. |
| `complete` | Terminal node ending the `WorkflowRun` (`completed`). |

### Overlap, approvals, and rework

- **Single active delivery run per Card:** While a Card has a running or waiting `WorkflowRun`, competing event deliveries wait in `subscription.deliveries` (`pending`) with their captured `definition_revision` without consuming failure retries. Manual starts conflict visibly until the active run finishes or is stopped.
- **Bound human decisions:** `workflow.decide` binds `run_id`, exact `visit`, and current `Card.revision`, requiring human provenance (Agent execution tokens are rejected). `rework` increments `visit`, records optional human `feedback`, and injects that feedback plus prior outputs into the target Agent step.
- **External column moves & stop:** Manually dragging/moving a Card to another column cancels stale queued workflow work from the superseded stage while preserving completed reports and history, then triggers any matching `origin: "local"` rule on the destination column. `workflow.stop` halts future workflow progression without killing an interactive PTY.

---

## Edit with agent

The Project Workflows header provides **Edit with agent**, opening a native modal with:

- Complete selectable Project-wide configuration Prompt (`workflow.prompt`), **Select all**, and explicit **Copy prompt** (opening never mutates the clipboard).
- Profile picker (`@codex`, `@claude`, custom profiles) and **Start / Continue / Resume in Terminal** or **New conversation**.

Configuration conversations are owned by **Project + `workflows` purpose + Profile** (`ConfigurationSession`). They reuse Profile command expansion, Scheduler admission, Herdr PTYs, and the native Terminal workspace (`Project / Configuration / Workflows · Profile`) with a **Back to Workflows** action, without creating fake business Cards or `Request`s, and cannot approve business human gates.

---

## API and CLI operations

All clients share the same AppServer validation and publication boundary (`canban call <method> '<json>'` or `@/path/to/params.json`):

| Operation | Parameters / purpose |
| --- | --- |
| `workflow.schema` | Discover YAML syntax, step types (`deliver.sync_upstream`, etc.), selectors, and operations |
| `workflow.sources` | `project_id` -> optimistic collection `revision`, `sources`, and active `WorkflowSource` rows |
| `workflow.check` | `project_id`, `revision`, `sources` -> validate YAML/columns/Profiles/cycles without publishing |
| `workflow.apply` | Same inputs -> atomically publish YAML revisions, sync `columns` to Project stages, and update subscriptions |
| `workflow.list` / `workflow.get` | Inspect immutable `WorkflowRevision` history (including `source_yaml`) |
| `workflow.prompt` / `configuration.open` | Generate Project-wide configuration Prompt and launch/resume configuration Terminal |
| `workflow.start` | `card_id`, `workflow_id`, `entry` -> explicit manual run start |
| `workflow.runs` / `workflow.run` | Inspect active and historical `WorkflowRun`s |
| `workflow.decide` | `run_id`, `visit`, `revision`, `decision: "approve" \| "rework"`, optional `feedback` |
| `card.hold_auto_merge` | `card_id`, `revision`, optional `hold: bool` -> pause or resume Queue Merge on a Card |
| `workflow.stop` | `run_id` -> cancel future run progression |
| `event.list` / `subscription.*` | Durable journal pagination and `save` / `list` / `deliveries` / `ack` / `retry` |

CLI file transport helpers (`canban workflow submit --check` / `submit` / `export`) read or write user-chosen external files against `workflow.sources` collection tokens without creating internal workflow directories.

---

## Migration and verification summary

- **Schema 11–12 migration:** Upgrades legacy Project `Workflow` stage templates, queued/running `Request`s, and pending human gates into SQLite `development.yaml` revisions and `WorkflowRun` checkpoints without relaunching Agents or recreating worktrees. Downgrade refuses to discard user-authored YAML sources or active configuration sessions.
- **Verification suite:**
  - `mise run test` & `cargo clippy --workspace --all-targets -- -D warnings`: Unit and integration coverage in `crates/workflow-engine/src/source.rs` and `crates/app-server/src/tests/workflows.rs`, including default `development.yaml` (`Backlog` quiet intake, `Develop` auto-run, `Review` Queue Merge with `sync_upstream: true` against bare Git remotes vs. `sync_upstream: false` local-only merge), Backlog Auto-Analyze + Auto-Start Develop sequencing, Manual Review gates, external PR trust branching, rework feedback, and 4-column custom workflow preservation.
  - `mise run acceptance` & `mise run acceptance-workflows`: Isolated Herdr Core + real PTY end-to-end execution, restart checkpointing, optimistic `revision_conflict` protection, and CLI export/submit checks.
  - Native UI & Gallery (`CanbanGallery.app` scenes `workflows`, `workflow-agent`, `workflows-conflict`, `column-automations`, plus isolated `CanbanDev.app`): Verified stage swimlanes, `deliver` / `agent` Step Inspectors, graph-derived Workflow Automation Options, YAML syntax highlighting, modal Escape focus restoration, and click-only Column automation popovers (see dated UI refinement log in [workflow-ui-acceptance-20261111.md](../workflow-ui-acceptance-20261111.md)).
