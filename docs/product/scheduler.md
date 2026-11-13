# Scheduler, capacity and human approval

Status: current runtime implemented 2026-11-07, with accepted next architecture recorded 2026-11-11 below. Default global limit is eight Agents; earlier unrestricted concurrency is superseded and old `capacity` fields remain ignored. The [unified engine](workflow-engine.md) is implemented in the initial slice; see [workflow/event implementation](workflow-engine.md) for exact coverage.

## Unified workflow admission direction

All workflow Agent steps use this queue/budget. Workflow Engine owns entries, progression and approval; Scheduler owns readiness/admission/execution. Subscribers target the engine rather than launching terminals or creating another scheduler.

Planning in Backlog is valid when an explicit step requests it. Workflow-owned Requests need run/step execution and applicable entry/input identities; they cannot be discarded merely because the legacy stage predicate assumes Backlog cannot execute. Revalidate run/step eligibility, lifecycle and gates at admission. Manual assistance remains available under existing Conversation/Question rules.

Retain current later-column/FIFO priority initially; graph depth does not silently redefine priority. Stage callback fields convert into steps/approvals; Scheduler does not retain a callback executor. The runtime policies below remain applicable; callback configuration is compatibility input converted by the new engine.

## One path and one global budget

Stage entry, explicit mentions and opening/resuming a conversation use the same persistent request queue. Profile selection is explicit mention, stage profile, then global default_agent. Routing tags and alias matching were removed on 2026-11-10; old stored tags cannot select or block an Agent. Queued/running Requests keep their captured Profile. Different Boards share one Scheduler and budget; there are no additional Project or profile caps.

```text
Queue -> reject archived/stale-stage automatic work
      -> exclude automatic work for Cards waiting for human
      -> check global Agent slots
      -> later template stage first, FIFO within a stage
      -> reserve starting intent
      -> prepare/reuse workspace
      -> start/resume through Herdr
```

`config/agents.json` contains `scheduler.max_agents` (positive integer, default 8). Settings exposes the same value. `scheduler.configure` saves it without replacing user profile commands/presets. Reload and app restart preserve it. Lowering the value does not interrupt existing Agents; new launches wait until usage drops below the limit.

Slots cover starting, working, closing and unknown/uncertain runtime. A pending close reserves its slot until resolved, even when the last root status was idle/done. Verified root blocked/idle/done/error statuses release execution capacity while retaining their terminals; disconnected or stale observations reserve it conservatively. A completed business report alone does not free a still-working/unknown runtime. Queued requests reserve no slot. A user may resume retained waiting Agents above the budget; this stops further launches without killing existing work. An exited conversation's resume is queued and needs admission. See [Agent Overview and lifecycle](../agent-overview-and-lifecycle.md) for idle reclamation and status details.

The limit covers Agents managed by this Canban home/AppServer. Unrelated Herdr terminals are outside it. Herdr status and normal closure continue while the queue is full.

## Workflow admission (`admit`)

Scheduler admission budgets launches; workflow `admit` gates budget *starting work*. Before each dispatch the AppServer evaluates `canban_scheduler::admissions`: runs in `waiting_capacity` are ordered by their Card's Board position within its column (then age). An `agent_slot` gate is admitted only while `max_agents - active - queued eligible requests > 0`, so already-admitted Cards whose Agent has not launched still count. A `max_ready: K` gate keeps at most K Cards that passed it in the gate's column (for Backlog: planning, awaiting plan approval, or waiting for a slot). Cards with a human gate/attention are skipped. Studio exposes this as Backlog **Auto-start Develop → When Agent free** plus **Plan ahead (ready Cards)**.

## Depth-first priority

The stage's index in its Project's saved template defines depth. A later stage wins over an earlier stage across Boards; requests at the same depth use enqueue time/insertion order. This is the agreed simple priority policy, not a graph execution engine or a hardcoded `review` name. Multiple slots can advance several Cards concurrently.

For example, a newly queued Reviewer takes the next available slot ahead of an older Develop request. The normal workflow may move into Review immediately on the completed report, but its new Agent still waits for a physical slot if execution capacity remains occupied.

## Human approval is persistent

`Card.human_gate` records why work is paused, the related request when known, and a pending next stage. `attention` remains explanatory text and notifications remain discoverability/read state. Existing saved attention is upgraded into a gate on open.

```text
Agent completed -> require_human_review?
                  | no                  | yes
                  v                     v
             next stage           wait for human

explicit blocked/failed report / uncertain recovery -> wait for human

wait for human -> no automatic dispatch/progression
               -> explicit user decision -> continue / retry / rework
```

An executing stage can set `require_human_review: true`. Its Agent runs automatically, but the completed report waits before applying `on_completed`. The fresh default Review stage enables this flag. Explicit blocked/failed reports and uncertain recovery also pause automatic work. A confirmed unreported process exit or preparation/launch failure records Failed history and does not manufacture a task gate; it stays in its stage without automatic retry/progression. Existing independent gates remain. See [runtime observation](../runtime-observation.md). Empty slots, reconnection, restart, notifications being read, ordinary comments and later Agent completion never approve the gate.

Explicit `@Agent` or open/resume remains available to help resolve the issue, subject to capacity. These invocations cannot approve the workflow through their completed report. A paused automatic request does not absorb a manual mention and prevent assistance.

The user can choose **Reviewed · continue workflow**, move to another stage for rework, retry a failed execution after its prior terminal has ended, or accept code through Review. Approval applies a completed initiating request's saved callback once. Continuing an unresolved live blocked Agent is rejected: answer it, or end it before retrying. Unknown starts require inspection and explicit retry rather than blind replay. Without a pending callback, approval clears the pause and does not launch another run of the finished stage.

The Agent-bound CLI marks approval/move/retry/merge/archive calls with its execution context; human-only decisions are rejected where appropriate. This is a local invocation/provenance boundary, not isolation from arbitrary same-user programs that can discard their environment.

## API

```sh
canban call scheduler.status '{}'
canban call scheduler.configure '{"max_agents":8,"idle_minutes":15}'
canban call card.continue '{"card_id":"CARD_ID","revision":7,"request_key":"approval-id"}'
```

The status contains active usage, limit and queued count. Card approval requires its current revision and supports normal request-key idempotency. Read acknowledgements do not clear the gate. Reports retain request/attempt binding and the existing normal-exit contract.

## Timers and deferred scope

Scheduler owns readiness/time waiting. Advanced schedules, fairness, per-profile caps and arbitrary scripting remain deferred. Declarative entries/steps/conditions/approvals now belong to the accepted unified workflow design and use this Scheduler. See [workspace and acceptance](../workspace-and-scheduling.md) for current validation, and [Agent profiles](agent-profiles.md) for routing configuration.
