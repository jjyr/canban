# Projects, Boards and workflow definitions

Status: reconciled 2026-11-11. The [unified engine](workflow-engine.md) now separates Board position from progress and interprets converted stage templates/custom definitions through one runtime. See [workflow implementation](../workflow-implementation.md) for actual authoring, activation, API and verification limits; Project source editing and Edit with agent are implemented; onboarding remains open.

## Project and Board

A Project has a stable identity, one required directory/current execution location and its own Board. Create its directory if absent; Git is common but not mandatory. Project creation does not imply cloning or initializing Git, and directory failures never make existing files disposable rollback data. A Project does not require a Herdr Space.

Keep the current Board at three columns: Backlog / Develop / Review. Archive retains completed work outside the active Board. Earlier support for configurable runtime stages does not require adding a Design column for planning.

```text
Project
  +-- directory / execution location
  +-- Board: Backlog / Develop / Review
  +-- declarative workflow definitions
  +-- event subscriptions / entry bindings
  +-- Cards
       +-- Board position
       +-- workflow progress, independent of column
       +-- conversations / execution / results / decisions
```

Column IDs are stable; names are presentation. Backlog entry has no built-in Agent task, but a manual or subscribed planning entry can execute while a Card remains there. Board filtering does not affect workflow admission.

## One workflow model

A definition describes entries, steps, transitions, inputs/outputs, Agent references, approval and workspace policy. A WorkflowRun captures one invocation and definition revision. The same engine handles manual starts, subscribed facts and external column entry. There is no separate stage-callback runtime.

The default behavior remains externally moving to Develop -> developer -> move to Review -> reviewer -> human decision. The planning entry adds read/design and optional approval before that sequence, without adding a column. Workflow-owned moves carry run/step causality and do not start duplicate work.

Explicit completion advances only its owning request/attempt and workflow step. A response to a direct mention is not completion of the main delivery process. Profiles retain separate Card conversations; physical execution continues through the existing Scheduler and Herdr. Idle/process exit is not completion, and the next business step does not require old-process drain or independent quality verification.

Required human decisions persist and bind exact result/input versions. Reading notifications, unrelated replies and another Agent's completion cannot release them. Rework, stop, stale-result handling and external moves follow the [engine rules](workflow-engine.md), retaining existing history.

## Authoring and display

Agents should author/modify workflows through the same declarative model that users can manually edit. Use shared schema validation, useful diagnostics, expected revisions and immutable published revisions. Invalid drafts do not replace active valid configuration; new edits do not silently rewrite active runs or captured assignments.

Project Workflows displays and edits multiple named YAML sources, with immutable SQL version history. Original YAML and active revision references live only in SQLite; files are optional submit/export inputs and no Workflow copies are written under ~/.canban. Edit with agent previews one Project-wide Prompt and opens a Project-owned configuration conversation in Terminal. Column trigger icons show shared-library descriptions of active rules. Cards retain execution/approval controls. See [the concrete contract](../workflow-implementation.md) for validation, conflicts, activation and UI behavior. A graph editor is not required; onboarding remains open.

## Migration and compatibility

Convert existing Stage `execute/profile/instruction`, `on_completed` and `require_human_review` into Agent steps, transitions and approval steps. Legacy format parsing may remain as conversion; only one engine executes the result. Preserve existing template/workspace behavior during conversion.

Migrate active/queued assignments and human gates into run/step checkpoints without relaunching Agents, recreating worktrees, changing provider IDs or replaying completed callbacks. Reject/roll back an incompatible conversion rather than activating partial state. Configuration revisions and database changes must follow the [update/migration contract](../updates.md).

## Workspaces and lifecycle

Worktree lifecycle defines when a task checkout is prepared, reused and handled for cleanup. It does not create a new checkout for every step/conversation.

```text
Planning: explicit execution/workspace requirements
                  |
Develop: prepare/reuse task checkout when applicable
                  |
Review / rework: retain checkout and exact change association
                  |
Archive: handle managed-worktree cleanup; preserve Card history
```

Planning can be read-only and must not create a worktree merely because a Card entered Backlog. Each Agent step needs an explicit/inherited execution location policy. New policy syntax and read-only enforcement details remain implementation work; preserve existing saved workspace/provider bindings.

Implemented workflow workspace settings are currently:

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

Fresh defaults enable worktree/base synchronization. An omitted base uses the Project target/current branch; older templates without the object retain local-only preparation. Non-Git Projects can turn both off. Sync requires a committed Git base and worktree creation.

Before the first checkout, fetch and fast-forward the configured base. Already containing the remote is acceptable; divergence, a dirty checked-out base or fetch failure prevents unsafe preparation. Do not silently push/rebase. Review/rework/resume reuse the Card workspace without another initial base synchronization. Existing detached worktrees are adopted without discarding edits. Current readable branch naming and saved location behavior are documented in [workspace and scheduling](../workspace-and-scheduling.md).

Archive removes clean managed worktrees while retaining branch/Card refs; dirty worktrees remain. Restore/open can recreate from retained history without automatically dispatching. Do not remove the Project directory or Card history as execution cleanup.

Review reports, human acceptance, commit/merge and archive remain distinct facts. The default flow does not auto-merge or auto-archive. See [Review delivery](review-delivery.md).
