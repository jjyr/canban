# Canban: Human Context

Status: current product direction from the user's 2026-11-05 clarification. Implementation details marked as proposals remain open. No application behavior is implemented yet.

## Product thesis

Canban is a Kanban-centered workbench that preserves human intent and coordinates agent work. Human Context is the proposed name for the approach. A person describes work, moves it into an actionable stage, and follows its execution, discussion, and outcome in one card.

The automation model has two distinct questions: **the stage determines what workflow to execute; tags determine which agent may execute it**. Scheduler manages when eligible work actually starts under configured limits.

## Confirmed product directions

- A Project can be created freely and must correspond to a directory. Create the directory if it does not exist. It is usually a Git repository, but a Git repository is not mandatory.
- Each Project has its own Board. Canban owns this organization; it does not need to inherit Herdr's Space/Tab/Pane model.
- A Board is a configurable set of workflow stages and triggers. The default template is Backlog, Develop, Review; completed cards are archived.
- Backlog is required and does not start a workflow on entry. Other stages can be customized.
- Entering Develop makes the card eligible for the development workflow; entering Review makes it eligible for the review workflow.
- Outcome transitions are template-configurable. The agent itself reports whether it has completed; in the default template that completed report moves the card into Review and triggers its ordinary report workflow. No independent quality/check/process-exit gate is required.
- Eligible cards are dispatched automatically. An untagged card uses the default agent; a routing-tagged card is handled only by a matching agent.
- Users can define tags and which agents handle them. MVP tags can simply be agent names, such as claude or codex.
- Each Project has project-local card numbers, comparable to issue numbers, for easy reference.
- Every card has a unified timeline of comments, state changes, agent executions, and other relevant updates. Users and agents can comment; users can mention agents.
- A submitted @agent request starts a fresh conversation; entering an executable stage independently assigns an agent and starts a fresh conversation for its task.
- Use one Session model for all executions. The difference is the scheduling trigger/input, not a question/stage session type. Canban makes the scheduling decisions and controls the actual runtime through herdr-core.
- Cards can reference resources and accept pasted images. Local cards are the first backend; GitHub issue cards are a later extension with the same outward card experience.
- Preserve the original Notes direction: strong editing and preview, ideas/discussion, and selecting context or a preset to turn an idea into actionable work. Card creation can also begin directly on the Board.
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
  |     |           +-- tags
  |     |           +-- unified timeline
  |     |           +-- dispatch requests -> run attempts
  |     |           +-- changes / artifacts / human outcomes
  |     +-- Archive
  |     +-- Notes / context references
  |     +-- workflow template + agent routing defaults
  +-- Agents index
  +-- Scheduler / schedules
```

Project is now the chosen product term. The exact relationship and storage of Notes remain open; the diagram preserves the earlier note-editing and discussion direction without deciding a full document hierarchy.

## Human Context contract

From a card, a person should be able to answer:

| Question | Evidence |
| --- | --- |
| Why does this work exist? | Description, notes, discussions, decisions, and resource references |
| Why did an agent start? | Stage-entry or mention event, workflow revision, routing decision, dispatch request |
| What did it receive? | Captured card/context revisions and effective workflow/profile configuration |
| What happened? | Each run, questions, comments, updates, artifacts, and check evidence in the timeline |
| What changed? | Repository/worktree and exact change identity where applicable |
| What did the human decide? | Review disposition and archive/completion history |

Current living context and the immutable input used for a particular run are separate. A later edit must not rewrite what a previous agent received.

## MVP boundary

The user has now resolved the main scope questions with explicit MVP simplifications. The updated [scope review](mvp-scope-review.md) records those decisions rather than keeping superseded hardening recommendations as release requirements.

The first useful loop is **Project -> local card in Backlog -> Develop -> automatic dispatch -> visible result and discussion -> human disposition -> archive**.

Include directory-backed Projects and Boards, basic Notes editing/preview and captured context, the default workflow template, local cards and project-local numbers, rich descriptions/resource references/image paste, unified timeline/comments, agent mentions, tag/default routing, run history, concurrency/recovery controls, and an agent-callable application interface. Rich conversational UI remains a separate scope choice.

Review produces an ordinary report through the existing generic Scheduler path; integrate the Shear interface later. The default next-stage callback trusts the agent's completion declaration. JSON templates provide customization without a template-editing GUI.

Defer the GitHub issue backend, GitHub write synchronization, advanced Shear-derived review UI, public executable widget/plugin ecosystem, and sophisticated schedule management. Local merge and simple recurring schedules remain product-scope choices rather than prerequisites for the core loop.

## Refinements during implementation

The main MVP policy decisions are settled: self-reported completion, no old-process coordination, worktree handling at Archive, ordinary Review reports, JSON templates, existing Herdr agent setup, internal attention notifications, and a minimal event-driven direction.

Refine small interface details as needed: completion/blockage encoding, deterministic routing defaults, attachment limits, Note ownership/selection, notification display, archive handling of modified managed worktrees, and the actual coordinator lifetime. Do not turn these into an expanded first-release architecture program.

See [product log](product-log.md), [Projects/templates](projects-and-workflows.md), [Scheduler](scheduler.md), [timeline draft](timeline.md), [Session model](sessions.md), [agent interface](agent-interface.md), and [event direction](events-and-notifications.md).
