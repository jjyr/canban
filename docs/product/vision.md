# Canban: Human Context

Status: product direction, updated 2026-11-10 for Profile-only selection. See [implementation](../implementation.md) for actual behavior and acceptance; explicitly marked proposals remain open.

## Product thesis

Canban is a Kanban-centered workbench that preserves human intent and coordinates agent work. Human Context is the proposed name for the approach. A person describes work, moves it into an actionable stage, and follows its execution, discussion, and outcome in one card.

The automation model has two distinct questions: **the stage determines what workflow to execute; its Profile selects the Agent, falling back to the global default**. Scheduler manages when eligible work actually starts under configured limits.

## Confirmed product directions

- A Project can be created freely and must correspond to a directory. Create the directory if it does not exist. It is usually a Git repository, but a Git repository is not mandatory.
- Each Project has its own Board. Canban owns this organization; it does not need to inherit Herdr's Space/Tab/Pane model.
- A Board is a configurable set of workflow stages and triggers. The default template is Backlog, Develop, Review; completed cards are archived.
- Backlog is required and does not start a workflow on entry. Other stages can be customized.
- Entering Develop makes the card eligible for the development workflow; entering Review makes it eligible for the review workflow.
- Outcome transitions are template-configurable. The agent itself reports whether it has completed; in the default template that completed report moves the card into Review and triggers its ordinary report workflow. No independent quality/check/process-exit gate is required.
- Eligible Cards are scheduled automatically. Selection is explicit @profile, workflow stage.profile, then global default_agent. Routing tags and Profile aliases are removed; users choose automatic roles through workflow JSON and direct questions through explicit mentions.
- Each Project has project-local card numbers, comparable to issue numbers, for easy reference.
- Every card has a unified timeline of comments, state changes, agent executions, and other relevant updates. Users and agents can comment; users can mention agents.
- A submitted @profile request reuses its Card/profile conversation, delivering immediately when live or starting/resuming when absent/exited. Stage assignments keep independent Request/Attempt and callback bindings; different profiles retain separate histories.
- Use one Session model for all executions. The difference is the scheduling trigger/input, not a question/stage session type. Canban makes the scheduling decisions and controls the actual runtime through herdr-core.
- Cards can reference resources and accept pasted images. Local cards are the first backend; GitHub issue cards are a later extension with the same outward card experience.
- Notes are deferred pending further product design. They have no current UI/search/creation flow, and saved Notes are not automatically injected into new Agent executions. Preserve existing data and historical captures. Card content, comments, reports and attachments supply current Human Context. See [Notes deferred](../notes-deferred.md).
- Projects choose a Kanban template at creation. JSON-style configuration is the initial customization surface; template UI and deeper migration features follow later. Incompatible stage mappings must be made visible rather than silently applied.
- Custom workflows can be created by people or with agent assistance.
- Canban must expose an interface that agents can call directly.
- Modular Kanban and Scheduler remain the MVP core. Advanced review/editor functionality can follow.
- Main UI surfaces use native Rust/NSH; embed WebView only when a surface needs it. This supersedes the earlier main-workbench WebView suggestion. Reuse Herdr's existing runnable agent setup through core.
- Canban's AppServer is logically independent of NSH but co-hosted in the desktop process for MVP; its business logic can later be hosted in a daemon.
- Keep managed task worktrees through active development/Review/rework, and handle them at Archive.
- Accept exceptional old executions continuing in MVP; do not add process-drain, writer handover, or independent completion verification before normal progression.
- A blocked/input-needed agent marks the card needs attention and generates an internal notification event. Native macOS notifications follow later.
- Drive internal product activity with Rust enum events, an in-memory channel, and interested topic subscribers. Detailed event architecture is explicitly deferred until after MVP.

Earlier Plan/Process wording describes the general transition from planning to schedulable work. The current default stage names are Backlog/Develop/Review. Do not introduce mandatory Plan, Process, Ready, In Progress, or Done columns from earlier research proposals.

## Current product model

```text
Canban
  +-- Project: directory-backed context and work boundary
  |     +-- Directory (often a Git repository)
  |     +-- Board
  |     |     +-- Backlog -> Develop -> Review
  |     |     +-- custom stages and trigger bindings
  |     |     +-- Cards #1, #2, ...
  |     |           +-- description + resource references
  |     |           +-- unified timeline
  |     |           +-- dispatch requests -> run attempts
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
| Why did an agent start? | Stage-entry or mention event, workflow revision, routing decision, dispatch request |
| What did it receive? | Captured card/context revisions and effective workflow/profile configuration |
| What happened? | Each run, questions, comments, updates, artifacts, and check evidence in the timeline |
| What changed? | Repository/worktree and exact change identity where applicable |
| What did the human decide? | Review disposition and archive/completion history |

Current living context and the immutable input used for a particular run are separate. A later edit must not rewrite what a previous agent received.

## MVP boundary

The user has now resolved the main scope questions with explicit MVP simplifications. The updated [scope review](mvp-scope-review.md) records those decisions rather than keeping superseded hardening recommendations as release requirements.

The first useful loop is **Project -> local card in Backlog -> Develop -> automatic dispatch -> visible result and discussion -> human disposition -> archive**.

Include directory-backed Projects and Boards, captured Card context, the default workflow template, local cards and project-local numbers, rich descriptions/resource references/image paste, unified timeline/comments, agent mentions, stage-profile/default selection, run history, independent concurrent Sessions and recovery, and an agent-callable application interface. Notes require further design and are outside this scope.

Review produces an ordinary report through the existing generic Scheduler path; integrate the Shear interface later. The default next-stage callback trusts the agent's completion declaration. JSON templates provide customization without a template-editing GUI.

Defer the GitHub issue backend, GitHub write synchronization, advanced Shear-derived review UI, public executable widget/plugin ecosystem, and sophisticated schedule management. Local merge and simple recurring schedules remain product-scope choices rather than prerequisites for the core loop.

## Refinements during implementation

The main MVP policy decisions are settled: self-reported completion, no old-process coordination, worktree handling at Archive, ordinary Review reports, JSON templates, existing Herdr agent setup, internal attention notifications, and a minimal event-driven direction.

Refine small interface details as needed: completion/blockage encoding, deterministic routing defaults, attachment limits, notification display, archive handling of modified managed worktrees, and the actual coordinator lifetime. Do not turn these into an expanded first-release architecture program.

See [product log](product-log.md), [Projects/templates](projects-and-workflows.md), [Scheduler](scheduler.md), [timeline draft](timeline.md), [Session model](sessions.md), [agent interface](agent-interface.md), and [event direction](events-and-notifications.md).
