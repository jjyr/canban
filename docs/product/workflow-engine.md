# Unified workflow engine, Studio and implementation

Status: implemented and active (reconciled 2026-11-15; SQLite schema 20, workflow schema 8, desktop API 55). This is the canonical product and engineering contract for Canban Workflows, consolidating the unified engine rules, Project/Board workflow ownership, Visual Studio UX, Scheduled Tasks (`Schedules`), YAML schema, RPC/CLI operations, workspace lifecycle, and acceptance evidence. [Events and subscriptions](events-and-notifications.md) owns the durable journal envelope and trigger delivery.

## Core design principles

1. **Single source of truth and one runtime:** Declarative YAML stored in SQLite (`workflow_sources` + immutable `workflow_revisions`) is the sole workflow definition and authoring model. There is no secondary `Stage`-callback executor, JSON `WorkflowDefinition` authoring RPC, or `legacy_definition` runtime fallback; all programmatic and interactive workflow submissions pass YAML directly (`workflow.check`, `workflow.apply`, `project.create.workflow_yaml`). Manual YAML edits, Visual Studio option toggles, and **Edit with agent** submissions all pass through the same schema validation, optimistic concurrency revision checks, and transactional publication path.
   - **Unified `WorkflowRun` & thin `ScheduledRun`:** Every actual workflow execution—Card-bound runs, cardless GitHub event runs (`script -> condition -> agent` with `$CANBAN_OUTPUT`), and cardless scheduled runs—executes as a single first-class **`WorkflowRun`** (`card_id: Option<String>`, optional `trigger_event_id`) in `canban-workflow-engine`. Card-only steps (`card.move`, `approval`, `deliver`, `admit`) return a clear error when `card_id` is `None`, while `script`, `condition`, `agent`, `notify`, and `complete` run identically with or without a Card. `ScheduledRun` serves strictly as the thin cron firing, overlap (`skip` / `queue` / `allow`), and bounded 50-row audit ledger pointing to `workflow_run_id` for both Card-creating (`creates_card: true`) and cardless (`creates_card: false`) schedules.
2. **Graph-derived, non-destructive UI:** Visual Studio renders one Stage lane per Board column directly from the active YAML graph (`canban_workflow_engine::stages`); there are no column-specific preset panels. Every step is configured inline on its card, and structural edits (add/move/disable/delete steps, set Stage exits, add/rename/reorder/remove columns) splice only the affected edges while preserving custom columns, custom steps, prompts, rework/branch references and unrelated rules. The older option helpers (`source::derive_options` / `set_*`) remain for CLI/tests but no longer drive Studio.
3. **Pragmatic default flow (human-controlled intake, automated delivery):** New Projects default to three Board columns (`Backlog -> Develop -> Review`, extensible via YAML `columns`):
   - **Backlog (quiet intake):** Joining Backlog does **not** automatically analyze or move Cards by default, avoiding unintended compute spend on rough ideas. Teams wanting hands-free pipelines can enable **Backlog Auto-Analyze** (`agent:plan`) and/or **Backlog Auto-Start Develop** (which waits for analysis to finish when both are enabled). Auto-Start Develop can be **Immediately** or **When Agent free**: the latter inserts `admit:develop` (`uses: admit`, `agent_slot: true`) so planned Cards stay in Backlog until a global automatic Agent slot is free, and—when analysis is enabled—`admit:plan` (`agent_slot: false`, `max_ready: K`, default 2) so at most K planned Cards wait in Backlog. Waiting Cards are admitted in Board order (drag to prioritize).
   - **Develop (automated implementation):** Entering Develop automatically runs `@developer` (`agent:develop`) in an isolated managed worktree and moves the Card to `Review` (`move:develop`).
   - **Review (Queue Merge & upstream sync):** By default Review omits a redundant `@reviewer` Agent pass (users can add one via YAML or Studio) and enters **Queue Merge (`deliver:review`)**, which commits/merges the clean worktree into the target branch and syncs upstream (`sync_upstream: true` by default). Teams can switch Review to **Manual Review** (`approval:review`) or **Merge Only (No Sync)** (`sync_upstream: false`).
4. **Safe-by-default delivery and human override:**
   - **Source-trust branching:** Even when Queue Merge is active, untrusted external PRs (`card.source_trust == "external_pr"`) are routed by `condition:external_pr` to a required human approval gate (`approval:review`) by default.
   - **Per-Card hold and graceful fallback:** Users can toggle **Hold auto-merge** (`card.hold_auto_merge`) on any Card at any time. Whenever a Card is held, has active/uncertain Agent terminals, encounters a dirty target checkout, or fails upstream remote push (`review.upstream_sync_failed`), `deliver` never forces an unsafe operation—it pauses as an inspectable human decision gate (`waiting_approval`), allowing manual resolution or rework feedback to the Agent.
   - **Batch merge-queue algorithm:** Deliveries drain in strict FIFO queue order (`inputs.queued_at`), grouping up to `deliver.with.batch_size` (default `5`, clamped `1..=50`) compatible queue-head Cards (`waiting_delivery` with matching `target_branch`, `strategy`, `sync_upstream`, `test_command`, and `conflict_profile`) into an isolated Project batch worktree (`<home>/worktrees/<project-ns>/_merge_queue`) with `0` artificial batch wait delay. When a connected Card Agent invoked `canban report --state completed` mid-turn (while Herdr still observes `status.state == "working"`), that Card stays excluded in `waiting_delivery` until the Agent finishes its turn (`idle` / `done` / `blocked` / `error` or terminal exit, bounded by a 3-minute settle grace window) so post-report turn cleanup completes before Git integration. Each batch commits clean Card worktrees, integrates up to `batch_size` Cards onto the target branch using `deliver.with.strategy` (`rebase` by default, `squash`, or forward `--first-parent` `merge`), and—only when `deliver.with.test_command` is configured—runs that shell test command once against the integrated batch worktree (skipping tests completely when unset). Clean batches fast-forward the Project target branch once and push once (if `sync_upstream: true` and a remote exists) without spawning any `@merger` session. Any merge conflict or batch test failure persists a `MergeBatch` (`merge-queue:<pid>:<short_id>`) and `TerminalEntry`, writes Git safety refs `refs/canban/batch/<short_id>/{target,card-<N>}`, transitions participating runs to `waiting_batch`, and dispatches a single Project-scoped `@merger` session in `_merge_queue` (with `_merge_queue`, the Project root, batch log directory, and all batch Card worktrees automatically trusted for `agy`); `@merger` may resolve in place or exclude incompatible Cards via `canban batch report --state completed --evict <card_number>` before re-verifying (once) and completing the remaining batch. When `batch_size == 1 && test_command.is_none()` (or a single Card already has an in-progress worktree rebase/conflict), single-Card conflict resolution runs in the Card worktree up to `max_conflict_attempts` (default 3, `0` disables). Target advancement (`review.merge_retry`) keeps runs queued for automatic retry.
5. **Board stage decoupled from workflow progress:** Columns represent business stage position; `WorkflowRun` steps represent execution progress. Planning and approvals can run while a Card stays in `Backlog`. Workflow-owned `card.move` transitions carry causal ancestry (`origin: "workflow"`, `workflow_run_id`) and continue the active `WorkflowRun` without re-triggering duplicate column-entry Agents.
6. **Deterministic boundaries (Engine vs. Scheduler, explicit completion, frozen revisions):**
   - **Engine vs. Scheduler:** Workflow Engine determines *what* happens next and enqueues assignments onto the Card/profile `TerminalEntry` (`AgentRun`); Scheduler owns *when* terminals launch under the global concurrency budget and conversation reuse rules.
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
       +-- Agent step -> TerminalEntry (`{card_id}:{profile_id}`) + Conversation -> Herdr PTY
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
  workflow_templates       saved global reusable workflow collections (e.g. `id = "default"` for new Projects)
  workflow_sources         Project + filename + active revision/hash/columns/rules
  workflow_revisions       immutable original YAML + compiled WorkflowDefinition
  workflow_runs            captured revision + optional `card_id`/`trigger_event_id` + visit checkpoints + inputs/outputs
  scheduled_runs           thin cron firing/overlap/50-row history ledger pointing to `workflow_run_id`
  event_subscriptions      selectors (`project_id`, `repository`, `exclude_self`) + target + durable journal cursor
  event_deliveries         captured definition revision + `workflow_run_id` + receipt/backoff state
  configuration_sessions   Project + workflow-configuration purpose + Profile/history
```

No Workflow YAML mirror or revision file is created under `~/.canban`; external `.yaml` paths are used only when explicitly importing/exporting via CLI.

### Reusing workflows across new Projects

To avoid repeating workflow adjustments on every new Project:

- **Set as default (`workflow.default.set` / `workflow.default.reset`):** In `Project / Workflows`, once all changes are saved, clicking **Set as default** (or running `Set Current Project Workflow as Default` from the Command Palette / `canban workflow set-default --project-id <ID>`) validates and stores the Project's published YAML source collection in `workflow_templates` (`id = "default"`, exposed via `Snapshot.workflow_templates`). When the current Project's saved workflows already match the global default, Studio displays `Default for new Projects` with a **Reset default** action to revert to the built-in default.
- **New Project workflow selector (`project.create`):** `New Project` provides a collapsible `▸ Workflow · ...` section (alongside `Execution machine` and `Project Agents`). By default (`workflow_mode: "default"` or omitted), `project.create` automatically instantiates the saved global default template if present, falling back to the built-in `development.yaml`. Users can also explicitly pick **Built-in default** (`workflow_mode: "builtin"`) or **Copy from `<Project>`** (`workflow_mode: "project"`, `workflow_source_project_id: "<pid>"`) to clone an existing Project's active workflow collection—automatically rewriting `doc.id` (`delivery:<new_pid>` / `<base>:<new_pid>`), clearing project-specific selectors, stripping deleted Profile references, and syncing custom Board columns and workspace settings (`sync_base`, `create_worktree`, `base_branch`, `remote`).

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
   - `agent`: Profile selector (`@developer`, `@reviewer`, `@codex`, etc.) + persistent NSH Prompt editor with curated built-in defaults, **Reset to default prompt**, and per-step **Environment** (`env`) editor.
   - `script`: Editable script body dialog (**Edit script**), **Environment** (`env`), **Shell** (`sh`/`bash` default, `zsh`, `python3`, etc.), **Working dir** (relative to Card worktree or Project directory), **Timeout** (seconds / minutes), **Continue on error** toggle, and optional **On failure →** step fallback target.
   - `deliver`: **Strategy** (`Rebase` default, `Squash`, or `Merge`), **Upstream sync** (`Merge & Sync Upstream` vs. `Merge Only (Local)`), **Target branch**, **Batch size** (`×1`, `×3`, `×5`, `×10`), **Test command** (optional; skip tests when empty), **Conflict attempts**, **Conflict Agent**, and **Rework target**.
   - `approval`: **Gate message** (`title`) and **Rework target**.
   - `card.move`: **Destination stage** selector.
   - Workflow-level **Environment** (`env`) dialog in Studio configures plaintext key/value defaults inherited by all `script` and `agent` steps.
4. **YAML Source & History:** Syntax-highlighted NSH editor (2-space indent, soft wrap, line numbers), immutable **History** modal (**Load as draft**), concurrent `revision_conflict` comparison view that preserves unsaved local keystrokes, and atomic **Save** (`Command+S`).

### Board column hints and Card stepper

- **Column headers** display a read-only, client-derived automation pill (`⚡ Auto develop`, `⚡ Auto merge`, `⚡ Auto plan`, `⚡ Auto start`, `⚡ Auto @<profile>`, etc.) on the right side via `source::column_automation` (`Icon::Zap`). Hovering shows a styled tooltip (`tooltip_box`) summarizing the active workflow file, step actions, external-PR gating, upstream sync, and conflict retries; clicking does not navigate. Quiet/manual columns without automatic actions (including approval-only manual Review or default Backlog) stay blank. Compact single-stage mode (`stage_tabs`) omits the pill to preserve switcher width.
- **Card detail** renders active/recent `WorkflowRun`s as a **Step Progress Stepper** (`✓ plan (@planner) · 4m → ● check (Script · 12s) → ○ Queue merge · Sync`), displaying compact elapsed execution time on the active workflow status line (`Agent running… · 1h10m`, `Running script… · 12s`, `Resolving merge conflict (@merger)… · 3m`) as well as per-step durations on completed and active steps (matching the compact runtime duration on Board Card Agent capsules `@developer · 12m`), exposing clickable report and **Script log** links, an inline **Retry script** button when a script pauses in human recovery (`waiting_approval`), an inline **Hold auto-merge / Resume auto-merge** button when a run includes `deliver`, and a **Request changes** feedback box that injects rework instructions into the next Agent step visit.

---

## YAML language and execution semantics

YAML 1.2 parsing uses `serde_yaml_ng` with `deny_unknown_fields`. Duplicate keys, unknown fields, missing step targets, undeclared column/Profile IDs, and automatic cycles fail validation before publication.

```yaml
schema: 6
id: delivery:example
name: Development
enabled: true
env:
  CI: "true"
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
    start_at: script:check
    match:
      column: review
steps:
  - id: agent:develop
    name: Implement
    uses: agent
    with:
      profile: developer
    next: move:develop
  - id: move:develop
    name: Move to Review
    uses: card.move
    with:
      column: review
    next: script:check
  - id: script:check
    name: Run checks
    run: |
      cargo test --workspace
      echo "gate=ok" >> "$CANBAN_OUTPUT"
    on_failure: agent:develop
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
| `agent` | optional `profile` (falls back to global `default_agent`), optional `instruction` (resolves to the built-in step/role default prompt at compile time when omitted), optional `env` (`KEY: VALUE` map). Enqueues a `Request` bound to `run_id` and `<step_id>:<visit>` and injects merged workflow/step/`$CANBAN_ENV` variables into the Agent session (replacing a warm PTY cleanly if custom env values changed). |
| `script` (or step-level `run:`) | Custom shell/interpreter script (`uses: script` + `with.run`, or shorthand step-level `run:` without `uses:`). Optional `shell` (defaults to `bash` with `-eo pipefail`, falling back to `sh -e`; supports `zsh`, `python3`, etc.), `working_directory` (relative to Card worktree or Project directory), `timeout_minutes` / `timeout_seconds`, `continue_on_error` (default `false`), step-level `env`, and `on_failure` fallback step ID. Runs via Herdr Core (local or remote Machine) without consuming `scheduler.max_agents` slots, persists logs under `cards/<card_id>/scripts/`, reads `$CANBAN_ENV` into `WorkflowRun.inputs.env` and `$CANBAN_OUTPUT` into `WorkflowRun.outputs.<step_id>`, and supports **at most 1 automatic fallback** via `on_failure` (with exit code + log tail feedback) before pausing in `waiting_approval` for Retry/Stop/Rework. |
| `card.move` | `column` (must exist in effective columns). Emits `card.stage_changed` with `origin: "workflow"`. |
| `condition` | JSON `pointer` into `{inputs, outputs, card}` (e.g. `/card/source_trust` or `/outputs/<step_id>/<key>`), exact `equals`, `then`, and `otherwise` step IDs. |
| `approval` | `title`, optional `on_changes_requested` rework step ID. Pauses the run in `waiting_approval` and sets `Card.human_gate`. |
| `deliver` | optional `strategy` (`rebase` default \| `squash` \| `merge`), optional `profile` (conflict-resolution Agent Profile ID, defaulting to `@merger` when enabled), optional `target_branch`, optional `sync_upstream` (boolean, default `true`), optional `batch_size` (integer `1..=50`, default `5`), optional `test_command` (shell verification command run once per integrated batch; skipped when omitted), optional `max_conflict_attempts` (integer, default `3`), optional `on_changes_requested` human rework step ID. Enters the Project's **FIFO Batch Merge Queue** (`waiting_delivery`, ordered by `inputs.queued_at`, transitioning to `waiting_batch` while a `MergeBatch` `@merger` session handles a batch failure): up to `batch_size` compatible queue-head Cards integrate in `<home>/worktrees/<project-ns>/_merge_queue`, run `test_command` once if configured, and fast-forward + push once cleanly with zero Agent sessions; conflicts or test failures dispatch one Project-scoped `@merger` terminal (`merge-queue:<pid>:<short_id>`) capable of resolving or evicting failing Cards via `--evict`. Single-Card runs with no `test_command` use per-Card conflict handling directly. Transient main-checkout blockers pause non-destructively at the head of the queue; permanent blockers and exhausted retry budgets step aside to `waiting_approval`. |
| `card.move` | `column` (column ID or `$next`), optional `next` (`$entry` continues at the target column's entry). |
| `admit` | optional `agent_slot` (boolean, default `true`: wait for a free global automatic Agent slot), optional `max_ready` (1..100: at most K Cards that passed this gate may remain in its column). Enters `waiting_capacity`; the AppServer admission pump releases runs in Board order before each scheduler dispatch. |
| `notify` | `message`. Appends a `workflow.notification` Timeline event. |
| `complete` | Terminal node ending the `WorkflowRun` (`completed`). |

### Environment variables and script context (`env`, `$CANBAN_ENV`, `$CANBAN_OUTPUT`)

Plaintext environment variables can be configured at the Workflow root (`env:`) and per `agent` or `script` step (`env:` / `with.env`), and extended dynamically by preceding scripts via `$CANBAN_ENV` (both `KEY=VALUE` lines and GitHub Actions-style `KEY<<EOF` heredoc delimiters). Merging applies in ascending precedence across every `script` and `agent` step:

1. Trust-scoped host environment + built-in `CANBAN_*` context (`CANBAN_CLI`, `CANBAN_HOME`, `CANBAN_ENDPOINT`, `CANBAN_PROJECT`, `CANBAN_PROJECT_DIR`, `CANBAN_WORKSPACE`, `CANBAN_CARD`, `CARD_ID`, `CANBAN_CARD_TITLE`, `CANBAN_BRANCH`, `CANBAN_BASE_BRANCH`, `CANBAN_SOURCE_TRUST`, `CANBAN_WORKFLOW`, `CANBAN_RUN_ID`, `CANBAN_STEP_ID`, `CANBAN_STAGE`, `CANBAN_RUN_DIR`, plus GitHub context such as `CANBAN_GITHUB_REPO` and—only for PR Cards/PR triggers—`CANBAN_PR_NUMBER` / `CANBAN_PR_URL`, and `$CANBAN_ENV` / `$CANBAN_OUTPUT` inside `script` steps).
2. Workflow top-level `env`.
3. Accumulated dynamic exports from earlier scripts' `$CANBAN_ENV` (`WorkflowRun.inputs.env`).
4. Step-level `env`.

Script outputs written to `$CANBAN_OUTPUT` (`key=value` or `key<<EOF`) are stored in `WorkflowRun.outputs[step_id]`, making them directly readable by `condition` pointers (for example `/outputs/script:check/gate` or `/outputs/script:filter/matched`) across both Card-bound and cardless `WorkflowRun`s, and injected into downstream Agent instructions alongside workflow outputs.

#### Trust-scoped Host Terminal Environment policy

Host Settings provides an **Inherit User Terminal Environment** mode (`Inherit` by default, with a `Minimal` fallback and an explicit **Refresh** action to re-probe the user's shell environment). When `Inherit` is active, Canban captures a filtered shell environment snapshot (excluding dangerous or session-local variables such as `DYLD_*`, `LD_PRELOAD`, `LD_LIBRARY_PATH`, `CANBAN_*`, `GIT_*`, `HERDR_*`, `PWD`, `OLDPWD`, `SHLVL`, `_`, and `TERM_SESSION_ID`). Environment exposure is strictly scoped by trust boundary:

- **Interactive Card Terminals (Shell mode) & code-controlled AppServer actions (`canban review publish` / `github.remote.review`, `deliver`, `github.pr.publish`):** Always **unrestricted**. Interactive shells run the user's normal login shell in Herdr, and AppServer actions execute in trusted Rust code using Canban's configured GitHub/Git credentials.
- **Unattended `Agent` / `Script` steps on `local` or `collaborator` trust:** Inherit the Host Terminal Environment snapshot when Host Settings is set to `Inherit` (or minimal/`PATH`-only when set to `Minimal`), merged beneath explicit workflow/step `env:` and `$CANBAN_ENV`.
- **Unattended `Agent` / `Script` steps on `external_pr` (`source_trust == "external_pr"`) or untrusted external cardless GitHub events:** Default to **minimal / `PATH`-only** host environment so malicious fork code or prompt-injected PR/Issue content cannot exfiltrate personal host secrets (`AWS_*`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GH_TOKEN`, `SSH_AUTH_SOCK`, etc.). Users may still pass explicit variables deliberately via workflow/step `env:`.
- **Codex adapter secret safety:** Inherited host environment variables are forwarded via process environment rather than dumped onto CLI `argv` (`-c shell_environment_policy.set.*`), preventing secret leakage in `ps` output and avoiding `ARG_MAX` failures.

### GitHub PR review auto-publish and cardless event workflows

- **Decoupled `@reviewer` draft + optional publish script:** The built-in `@reviewer` Agent step continues to inspect the PR worktree and write a local draft (`review_draft.json` via `canban review draft`), keeping the Agent itself decoupled from direct remote publication. Workflows that want automated PR review publishing add an optional, toggleable (`enabled: true` / `false`) post-review `script` step invoking `canban review publish` (with optional `--event-override COMMENT`).
- **Step-authorized publication boundary:** `canban review publish` wraps the `github.remote.review` path (which also clears `review-draft:{cid}` on success) and is serialized through `github_action_gate`. Rather than granting arbitrary Agents unrestricted access to `github.remote.*` RPCs, AppServer authorizes workflow-driven remote publication via server-verified `workflow_run_id` + step capability checks, and records published remote `review_id` / `comment_id` receipts in `source_observations` for cross-GitHub loop suppression.
- **Cardless GitHub event rules (`script -> condition -> agent`):** Project workflow `on:` rules can target repository-level GitHub events (such as `github.issue.opened` or `github.issue.comment_created`) with `selector.repository` (bound from `project.github.repository`) and `exclude_self: true`. Matching events atomically queue a cardless `WorkflowRun` (`card_id: None`, `trigger_event_id`) that runs lightweight filter scripts (`$CANBAN_OUTPUT`) and `condition` gates before lazily admitting an `agent` step under `scheduler.max_agents`.

### Overlap, approvals, and rework

- **Single active delivery run per Card:** While a Card has a running or waiting `WorkflowRun`, competing event deliveries wait in `subscription.deliveries` (`pending`) with their captured `definition_revision` without consuming failure retries. Manual starts conflict visibly until the active run finishes or is stopped.
- **Bound human decisions:** `workflow.decide` binds `run_id`, exact `visit`, and current `Card.revision`, requiring human provenance (Agent execution tokens are rejected). `rework` increments `visit`, records optional human `feedback`, and injects that feedback plus prior outputs into the target Agent step.
- **External column moves & stop:** Manually dragging/moving a Card to another column cancels stale queued workflow work from the superseded stage while preserving completed reports and history, then triggers any matching `origin: "local"` rule on the destination column. `workflow.stop` halts future workflow progression without killing an interactive PTY.
- **Interactive stage adoption (`canban card adopt` / `card.adopt`):** When a user discusses requirements with a live interactive or `@mention` comment Agent in `Backlog` and authorizes starting implementation in that same session, the Agent updates the Card only with user-confirmed design points (`canban card update` / `card.update`) and invokes `canban card adopt` (default `--stage develop`). `card.adopt` transitions the Card to the target column with `reason: "agent_adopted_stage"` (suppressing duplicate `card.column_entered` trigger dispatch), resets any prior completed/terminal-turn `AgentRun` state on the live terminal back to `"running"` (switching a completed `@mention` trigger to `interactive`), and binds the live terminal directly to the target column's `WorkflowRun` `agent` step (`start_adopted`) so subsequent `canban report --state completed` seamlessly advances the remaining workflow (`Move to Review -> Queue Merge`).

---

## Edit with agent

The Project Workflows header (and Command Palette `Edit Workflows with Default Agent`) provides **Edit with agent**, which directly opens (or continues/resumes) a configuration conversation in Terminal using the configured default Agent (`config.default_agent`, respecting Project overrides via `Project.agent_overrides`) without showing a secondary Agent picker.

- Generated Project-wide configuration Prompt (`workflow.prompt`) is written completely in English, instructing the Agent to inspect the current Workflow schema, Profiles, and all Project YAML sources, present a read-only ASCII overview, wait for user modification instructions, and validate/publish via `workflow.check` / `workflow.apply`.
- If an ended session for the default Agent has exact provider history and a resume command, `Edit with agent` resumes it automatically; if exact resume is unavailable (`auto_fresh: true`), it starts a fresh conversation cleanly.
- `workflow.prompt` and the read-only Prompt preview (`workflow-agent` Gallery scene) remain available without any profile switcher.

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
| `workflow.default.get` / `set` / `reset` | Inspect, save (`project_id` or `sources`), or clear the global default `WorkflowTemplate` (`id = "default"`) for new Projects (`canban workflow set-default` / `reset-default`) |
| `project.create` (workflow options) | Optional `workflow_mode` (`"default"` \| `"builtin"` \| `"project"`), `workflow_source_project_id`, or explicit `workflow_yaml` |
| `workflow.list` / `workflow.get` | Inspect immutable `WorkflowRevision` history (including `source_yaml`) |
| `workflow.prompt` / `configuration.open` | Generate Project-wide configuration Prompt and launch/resume configuration Terminal |
| `workflow.start` | `card_id`, `workflow_id`, `entry` -> explicit manual run start |
| `workflow.runs` / `workflow.run` | Inspect active and historical `WorkflowRun`s |
| `workflow.decide` | `run_id`, `visit`, `revision`, `decision: "approve" \| "rework"`, optional `feedback` |
| `workflow.deliver` | `card_id`, `revision` -> manually trigger or retry Queue Merge (re-queueing a paused, failed, or blocked `deliver` step at the tail of the FIFO merge queue, or starting a fresh run at the Project's active `deliver` step) |
| `batch.bind` / `batch.report` (`canban batch report [--evict <card>]`) | Binds a Project-scoped `merge-queue:` `@merger` session (`CANBAN_BATCH_ID`) and reports completion (`completed` / `working` / `blocked` / `failed`, optionally evicting specific Card numbers to `waiting_approval` before re-running `test_command` once and fast-forwarding/pushing the remaining batch) |
| `card.hold_auto_merge` | `card_id`, `revision`, optional `hold: bool` -> pause or resume Queue Merge on a Card |
| `card.adopt` | `execution`, optional `stage` (default `"develop"`) -> transition the active Agent's own Card into `stage` and adopt its `WorkflowRun` without re-triggering scheduling |
| `workflow.stop` | `run_id` -> cancel future run progression |
| `schedule.list` / `schedule.runs` | `project_id`, optional `schedule_id`, `limit` -> inspect active `ScheduleState` timers and `ScheduledRun` execution history (`canban schedule list` / `runs`) |
| `schedule.trigger` / `schedule.toggle` | `schedule_id` (or `project_id` + `workflow_id`/`filename` + `rule_id`), `enabled` -> manually trigger a schedule immediately (`Run now`) or toggle a schedule rule On/Off in place (`canban schedule trigger` / `toggle`) |
| `schedule.bind` / `schedule.report` / `schedule.read_artifact` | Bind a Cardless scheduled Terminal Agent (`CANBAN_SCHEDULE_RUN_ID`), record its `canban report` outcome before automatic PTY reclamation, and read stored Markdown reports or captured output |
| `event.list` / `subscription.*` | Durable journal pagination and `save` / `list` / `deliveries` / `ack` / `retry` |

CLI file transport helpers (`canban workflow submit --check` / `submit` / `export`) read or write user-chosen external files against `workflow.sources` collection tokens without creating internal workflow directories.

---

## Migration and verification summary

- **SQLite schema 11–12 migration:** Upgrades legacy Project `Workflow` stage templates, queued/running `Request`s, and pending human gates into SQLite `development.yaml` revisions and `WorkflowRun` checkpoints without relaunching Agents or recreating worktrees. Downgrade refuses to discard user-authored YAML sources or active configuration sessions.
- **SQLite schema 16–17 migrations:** Schema `16` shortens newly generated record IDs to 8-character lowercase hex strings (`canban_types::id()`), migrates existing 32-char UUID hex IDs while recording reversible `id_migrations` and `path_migrations`, and moves Card asset directories from `projects/<pid>/cards/<card_id>` to `projects/<pid>/cards/<card.number>` (and `archives/<pid>/<card.number>`). Schema `17` adds the `merge_batches` table for batch merge-queue persistence (`Snapshot.merge_batches`).
- **SQLite schema 20 migration:** Adds `schedule_states` and `scheduled_runs` tables (`Snapshot.schedule_states`, `Snapshot.scheduled_runs`); rollback (`schedules_down`) refuses to drop tables while any scheduled run is active.
- **Linear Workflow YAML schema migrations (`crates/workflow-engine/src/migrations.rs`, `BASELINE = 1`, `CURRENT = 11`):**
  - Authoring, compilation, and Studio UI target `CURRENT` exclusively instead of retaining legacy branches across modules.
  - `source::parse` upgrades older supported `Document.schema` inputs (`1..=CURRENT`) in memory, `workflow.apply` / `project.create` canonicalize older submitted YAML to `CURRENT`, and `AppServer` startup (`authoring::initialize`) automatically publishes a migrated `WorkflowRevision` for any active `WorkflowSource` still on an older `schema`—keeping historical revisions intact and leaving active `WorkflowRun`s pinned to their captured `definition_revision`.
  - **Workflow schema 2:** Strips unmodified v1 built-in Agent prompts from `with.instruction` so built-in prompts resolve at compile time while user-customized prompts stay explicit in YAML.
  - **Workflow schemas 3–4:** Canonicalize admission gates and `@merger` conflict retry defaults (`max_conflict_attempts: 3`) on `deliver` steps.
  - **Workflow schema 5:** Introduces `deliver.with.strategy` (`rebase` default, `squash`, `merge`), omitting redundant explicit `strategy: rebase` so default linear delivery stays concise in YAML.
  - **Workflow schema 6:** Adds top-level and step-level `env` (`script` and `agent` steps), dual `script` step syntax (shorthand `run:` and explicit `uses: script` + `with.run`), `$CANBAN_ENV` / `$CANBAN_OUTPUT` context propagation, and single-attempt `on_failure` recovery.
  - **Workflow schema 7:** Canonicalizes `deliver.with.batch_size` (omitting redundant explicit `batch_size: 5` default, clamped `1..=50`) and optional `deliver.with.test_command` (omitting blank values).
  - **Workflow schema 8:** Adds scheduled cron triggers (`on.<rule>.cron` and GitHub Actions `on.schedule: [{ cron: ... }]` array shorthand), optional `timezone`, `overlap` (`skip` default, `queue`, `allow`), and optional `card` (`column`, `title`, `body`) for Card-creating schedules vs. Cardless Terminal Agent schedules with automatic post-report PTY reclamation and offline sleep-skip reconciliation.
  - **Workflow schema 11+ (`v11_cardless_and_event_selectors`, Card #150 unified cardless & GitHub workflow evolution):** Adds `exclude_self` and repository-scoped cardless event triggers (`selector.repository`) on `on:` rules, supports `WorkflowRun.card_id: Option<String>` with nullable `workflow_runs.card_id` in SQLite and thin `ScheduledRun` delegation to `workflow_run_id`, enables toggleable (`enabled: true/false`) post-review `script` steps calling step-authorized `canban review publish`, and advances `DESKTOP_API` alongside sequential migrations in `crates/workflow-engine/src/migrations.rs`.
- **Verification suite:**
  - `mise run test` & `cargo clippy --workspace --all-targets -- -D warnings`: Unit and integration coverage in `crates/workflow-engine/src/source.rs` and `crates/app-server/src/tests/workflows.rs`, including default `development.yaml` (`Backlog` quiet intake, `Develop` auto-run, `Review` Queue Merge with `sync_upstream: true` against bare Git remotes vs. `sync_upstream: false` local-only merge), Backlog Auto-Analyze + Auto-Start Develop sequencing, Manual Review gates, external PR trust branching, rework feedback, and 4-column custom workflow preservation.
  - `mise run sim` & `mise run acceptance`: Full deterministic workflow/fault/delivery simulation (`card_automation`, `workflow_faults`, `workspaces_delivery`, `scheduler_cross_contention`, `chaos`) plus isolated Herdr Core + real PTY end-to-end execution.
  - Native UI & Gallery (`CanbanGallery.app` scenes `workflows`, `workflow-agent`, `workflows-conflict`, `column-automations`, plus isolated `CanbanDev.app`): Verified stage swimlanes, `deliver` / `agent` Step Inspectors, graph-derived Workflow Automation Options, YAML syntax highlighting, modal Escape focus restoration, and click-only Column automation popovers (see dated UI refinement log in [workflow-ui-acceptance-20261111.md](../workflow-ui-acceptance-20261111.md)).
