# Canban: Human Context

Status: product direction reconciled 2026-11-11 for unified workflows, durable subscriptions and Agent authoring. The initial engine/subscription slice is implemented; see [concrete operations and limits](../workflow-implementation.md) for current behavior/evidence. Explicitly open UX/schema choices remain open.

## Product thesis

Canban is a Kanban-centered workbench that preserves human intent and coordinates agent work. Human Context is the proposed name for the approach. A person describes work, moves it into an actionable stage, and follows its execution, discussion, and outcome in one card.

The automation model separates **Board position, workflow progress and execution admission**. Manual starts, event subscriptions and column entries address one Workflow Engine. Its Agent steps reference Profiles; Scheduler admits their Requests under the existing global budget. Planning/approval can run in Backlog. See [unified workflow rules](workflow-engine.md).

## Confirmed product directions

- A Project can be created freely and must correspond to a directory. Create the directory if it does not exist. It is usually a Git repository, but a Git repository is not mandatory.
- Each Project has its own Board. Canban owns this organization; it does not need to inherit Herdr's Space/Tab/Pane model.
- Keep the current Board at Backlog / Develop / Review; planning is a process step, not a Design column. Completed Cards are archived.
- Backlog entry has no built-in Agent action; an enabled manual/event entry can plan and wait for approval there.
- External Develop/Review entry retains current delivery behavior through the same engine. Workflow-owned movement does not start duplicate runs. Earlier configurable-stage support does not require new columns for this design.
- Outcome transitions are template-configurable. The agent itself reports whether it has completed; in the default template that completed report moves the card into Review and triggers its ordinary report workflow. No independent quality/check/process-exit gate is required.
- Ready Agent steps use the existing Scheduler/Profile system. Definitions reference Agent Profiles; legacy stage Profile references convert to step references. Direct @profile questions preserve their independent binding. Routing tags and aliases remain removed.
- Each Project has project-local card numbers, comparable to issue numbers, for easy reference.
- Every card has a unified timeline of comments, state changes, agent executions, and other relevant updates. Users and agents can comment; users can mention agents.
- A submitted @profile request reuses its Card/profile conversation, delivering immediately when live or starting/resuming when absent/exited. Stage assignments keep independent Request/Attempt and callback bindings; different profiles retain separate histories.
- Use one Session model for all executions. The difference is the scheduling trigger/input, not a question/stage session type. Canban makes the scheduling decisions and controls the actual runtime through herdr-core.
- Cards can reference resources and accept pasted images. Local cards are the first backend; GitHub issue cards are a later extension with the same outward card experience.
- Notes are deferred pending further product design. They have no current UI/search/creation flow, and saved Notes are not automatically injected into new Agent executions. Preserve existing data and historical captures. Card content, comments, reports and attachments supply current Human Context. See [Notes deferred](../notes-deferred.md).
- Workflow definitions are declarative, primarily Agent-authored and also manually editable through shared validation/revisions. A generic UI displays any supported definition/run; a graph editor is not required.
- Replace existing stage callbacks through one engine migration, retaining queued/running work and human gates without relaunch. Compatibility parsing does not justify a second executor.
- Default Profile onboarding and settings Agent conversations are the next UX topic; exact setup, ownership, layout and apply/activation behavior remain open.
- Canban must expose an interface that agents can call directly.
- Modular Kanban and Scheduler remain the MVP core. Advanced review/editor functionality can follow.
- Main UI surfaces use native Rust/NSH; embed WebView only when a surface needs it. This supersedes the earlier main-workbench WebView suggestion. Reuse Herdr's existing runnable agent setup through core.
- AppServer is an independent service shared by UI clients and local/remote executions; UI exit does not stop scheduling.
- Keep managed task worktrees through active development/Review/rework, and handle them at Archive.
- Accept exceptional old executions continuing in MVP; do not add process-drain, writer handover, or independent completion verification before normal progression.
- A blocked/input-needed agent marks the card needs attention and generates an internal notification event. Native macOS notifications follow later.
- Use canonical committed typed events, durable scoped subscriptions and consumer progress for automation; memory channels provide advisory wake-ups/invalidation. Support broad/specific-Card and GitHub object events with source provenance, polling gaps and deduplication. See [event rules](events-and-notifications.md).

Earlier Plan/Process wording describes the general transition from planning to schedulable work. The current default stage names are Backlog/Develop/Review. Do not introduce mandatory Plan, Process, Ready, In Progress, or Done columns from earlier research proposals.

## Current product model

```text
Canban
  +-- Project: directory-backed context and work boundary
  |     +-- Directory (often a Git repository)
  |     +-- Board
  |     |     +-- Backlog -> Develop -> Review
  |     |     +-- workflow entry bindings
  |     |     +-- Cards #1, #2, ...
  |     |           +-- description + resource references
  |     |           +-- unified timeline
  |     |           +-- workflow runs -> step executions -> requests/attempts
  |     |           +-- changes / artifacts / human outcomes
  |     +-- Archive
  |     +-- Card content / comments / reports / attachments
  |     +-- workflow template + agent routing defaults
  +-- Agents index
  +-- Scheduler / schedules
```

Project is the chosen product term. Legacy Note records remain stored for compatibility; a future Note model requires a separate product decision.

## Human Context contract

From a card, a person should be able to answer:

| Question | Evidence |
| --- | --- |
| Why does this work exist? | Description, discussions, decisions, and resource references |
| Why did an agent start? | Manual/event/column entry, workflow/run/step revision, Profile and execution request |
| What did it receive? | Captured card/context revisions and effective workflow/profile configuration |
| What happened? | Each run, questions, comments, updates, artifacts, and check evidence in the timeline |
| What changed? | Repository/worktree and exact change identity where applicable |
| What did the human decide? | Review disposition and archive/completion history |

Current living context and the immutable input used for a particular run are separate. A later edit must not rewrite what a previous agent received.

## Historical first-MVP boundary

The following records the initial MVP scope, not a deferral of the 2026-11-11 workflow/event design. GitHub integration and independent AppServer execution have since shipped; see [implementation](../implementation.md). The earlier [scope review](mvp-scope-review.md) preserves the original release decisions.

The first useful loop is **Project -> local card in Backlog -> Develop -> automatic dispatch -> visible result and discussion -> human disposition -> archive**.

Include directory-backed Projects and Boards, captured Card context, the default workflow template, local cards and project-local numbers, rich descriptions/resource references/image paste, unified timeline/comments, agent mentions, stage-profile/default selection, run history, independent concurrent Sessions and recovery, and an agent-callable application interface. Notes require further design and are outside this scope.

Review produces an ordinary report through the existing generic Scheduler path; integrate the Shear interface later. The default next-stage callback trusts the agent's completion declaration. JSON templates provide customization without a template-editing GUI.

Defer the GitHub issue backend, GitHub write synchronization, advanced Shear-derived review UI, public executable widget/plugin ecosystem, and sophisticated schedule management. Local merge and simple recurring schedules remain product-scope choices rather than prerequisites for the core loop.

## Historical MVP simplifications

The main MVP policy decisions are settled: self-reported completion, no old-process coordination, worktree handling at Archive, ordinary Review reports, JSON templates, existing Herdr agent setup, internal attention notifications, and a minimal event-driven direction.

Refine small interface details as needed: completion/blockage encoding, deterministic routing defaults, attachment limits, notification display, archive handling of modified managed worktrees, and the actual coordinator lifetime. Do not turn these into an expanded first-release architecture program.

See [product log](product-log.md), [Projects/templates](projects-and-workflows.md), [Scheduler](scheduler.md), [timeline draft](timeline.md), [Session model](sessions.md), [agent interface](agent-interface.md), and [event direction](events-and-notifications.md).
