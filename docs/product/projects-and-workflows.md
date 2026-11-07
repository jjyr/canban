# Projects, JSON workflow templates, and worktrees

Status: updated 2026-11-07. The workspace settings and human-review gate below are implemented. Older illustrative workflow examples in this document remain conceptual; use the current types and [implementation contract](../workspace-and-scheduling.md) for actual JSON.

## Project

A user-created Project has one required directory and its own Board. Create the directory if absent. It is usually a Git repository, but Git is not mandatory. A Canban Project does not need a corresponding Herdr Space.

Creating a Project does not imply cloning a repository or initializing Git. Directory failures should be visible; existing files are not disposable rollback data. Stable Project identity is independent of its path.

The main creation experience is name/directory, template/default agent configuration, then the Board. A missing directory later does not erase card history.

## Board, stage, and workflow

```text
Project
  +-- directory
  +-- Board
        +-- Backlog -> no automatic entry workflow
        +-- Develop -> development task
        +-- Review  -> ordinary review report
        +-- custom stages from JSON configuration
```

A card's stage is a property; stage entry produces an event and can trigger a configured workflow. The workflow defines the task and callbacks. Tags/defaults choose the agent. Scheduler starts a generic fresh Session through herdr-core.

All Sessions have the same structure. An @mention and a stage entry differ in their scheduling input/callback, not in session type. See [Sessions](sessions.md).

## Completion and progression

The agent decides/reports whether it has completed. When it reports completed, Canban applies the workflow's configured next action. In the default template, completion of development moves the card to Review, which starts its review workflow.

Do not add an independent test/artifact-quality gate or wait for terminal exit before this normal progression. Check output and change references can be recorded as context, but they are not a separate Canban verification requirement.

The user explicitly accepts exceptional old executions continuing during MVP. Do not require process drain, writer quiescence, or coordinated shutdown before stage progression. The ordinary assumption is that the agent reports completion before the card moves.

Keep simple request/callback association and avoid processing an identical completed report twice. A response to an @mention uses that request's callback; it does not become the stage's completion merely because the same profile was used.

When the agent reports blockage/needs input, mark the card needs attention and publish an internal notification event. This is independent of its Board stage. See [events and notifications](events-and-notifications.md).

## JSON templates first

Confirmed: users can customize templates via JSON-style configuration; a template-editing UI is deferred. Rich template depth and complex live migration can follow MVP.

Illustrative configuration:

```json
{
  "id": "default-development",
  "revision": 1,
  "stages": [
    { "id": "backlog", "title": "Backlog", "on_enter": null },
    { "id": "develop", "title": "Develop", "on_enter": "development" },
    { "id": "review", "title": "Review", "on_enter": "review" }
  ],
  "workflows": {
    "development": {
      "task": "implement-card",
      "prepare": "prepare-development-checkout",
      "on_completed": { "enter_stage": "review" }
    },
    "review": {
      "task": "produce-review-report",
      "on_completed": { "record_report": true }
    }
  }
}
```

Only Backlog is mandatory and has no automatic entry workflow. Stable stage IDs determine bindings, not display labels. Validate that referenced stages/workflows exist and retain enough configuration identity to explain an execution. The exact file path/schema/reload mechanism remains an implementation choice, not a new product-design project.

The earlier direction of selecting templates and explaining unmapped stages remains relevant. For MVP, configuration can reject an incompatible change with a useful mismatch message instead of implementing complex live remapping of active Boards. Agent-assisted configuration can use the same JSON and application interface. A visual designer is not required.

## What worktree lifecycle means

A worktree lifecycle describes **when a task checkout is created, which stages reuse it, and when it is handled for cleanup**. It does not mean repeatedly creating a new directory for every agent conversation.

The user chose:

```text
Develop: prepare task worktree when applicable
                  |
Review: reuse it and its change
                  |
Rework: keep using the task checkout as appropriate
                  |
Archive: handle managed-worktree cleanup
```

Retain the worktree throughout active card work. A fresh Session does not require a new worktree. Record the checkout/change reference so development and review operate on the intended task. A non-Git Project can use its configured directory without a worktree.

Archive is the cleanup boundary. Exact removal/retention treatment of modified managed worktrees is an implementation detail still to specify. It does not authorize deleting the Project directory or discarding unrelated files/card history. Do not clean an active card's worktree merely because one Session ends.

## Preparation and result callbacks

Keep the MVP callbacks small: prepare/reuse the execution directory, supply the task/context, record agent reports/results, and apply configured stage progression. Preparation failures should appear on the card rather than launch in an unintended directory.

Callbacks and all other internal product operations participate in the event-driven direction. Do not design a script editor, plugin SDK, lease/handover framework, or detailed event infrastructure before completing MVP.

## Review and archive

Review produces an ordinary report in the card/timeline. The Shear review interface is a later integration. Agent-reported completion of a workflow is distinct from a human accepting the code, merging it, or archiving the card.

Archive removes finished work from the active Board while retaining its number and history, and is the point at which Canban handles its managed worktree. A mandatory Done column is unnecessary.


## Implemented workspace settings (2026-11-07)

A Workflow has one `workspace` object rather than an operation graph:

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

Fresh default templates enable worktree creation and base synchronization. An omitted `base_branch` uses the Project target/current local branch. Templates lacking the new object retain local-only preparation for compatibility; existing Project snapshots are not silently rewritten. Set both worktree and synchronization off for non-Git Projects. Sync requires worktree creation and a committed Git base.

Before the first checkout, fetch the configured remote base and fast-forward the local base. Local commits already containing the remote are retained; divergence, dirty checked-out base or fetch failure stops preparation and requests human attention. A checked-out base is updated through that checkout; an unoccupied base uses a checked ref update. No automatic push or rebase occurs.

Each Card gets `canban/<project-id>/<card-number>` plus its own managed worktree. Review, rework and resume reuse both without another base synchronization. Existing detached worktrees are adopted at their current HEAD without discarding edits. Card stores branch/base commit/path, and its UI exposes copies and a read-only status refresh. Archive removes clean worktrees while retaining branch and Card ref; dirty worktrees remain. Restore/open can recreate from retained history.

Each stage can set `require_human_review`. It pauses after its Agent finishes and before the next callback. The default Review stage requires human approval. See [Scheduler](scheduler.md) for the global limit and persistent human gate.
