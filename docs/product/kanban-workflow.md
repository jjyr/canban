# Kanban card workflow

Status: current product requirements from the 2026-11-05 clarification. Recommendations and unresolved behavior are labeled. These are Canban concepts, not existing core APIs.

## The card is the coordination point

```text
Project: directory + default agent + template
  +-- Board
        +-- Backlog
        +-- Develop
        |     +-- Card #42
        |           +-- description / tags / resources
        |           +-- timeline: comments + events
        |           +-- development request -> run R1 -> terminal t7
        |           +-- revision request    -> run R2 -> terminal t9
        |           +-- change / checks / human decision
        +-- Review
  +-- Archive: completed cards, history retained
```

Every card belongs to a Project and its Board, and has a project-local number. Recommend a separate immutable internal ID and an atomic, increasing number allocation per Project; never recycle a number after archive or deletion. Within a Project use #42; cross-Project references need a stable Project identity plus the number. Display prefixes are an open UX choice.

## Stage, execution, and outcome

Keep these independent:

| Axis | Example | Meaning |
| --- | --- | --- |
| Stage | Backlog / Develop / Review / custom stage | Which workflow is relevant |
| Scheduler/run state | Queued / Preparing / Active / Input needed / Failed / Unknown | What is happening operationally |
| Task outcome | Awaiting decision / Accepted / Changes requested / Canceled | Human or configured workflow disposition |
| Archive | Active / Archived | Whether the card appears on the active Board |

These are proposed field categories, not finalized enums. A Develop card can be queued; a Review card can have a reviewer running. An agent's explicit completed declaration drives its request's workflow callback; it remains distinct from human acceptance or merge. Archive is a lifecycle action, not an implicit delete or proof that code was integrated.

## Default interaction

1. Create or select a Project, backed by its directory and Board template.
2. Create a local card in Backlog, receiving its project-local number.
3. Write the description, reference notes/files/URLs/other cards, and paste images where useful. Add optional routing tags.
4. Move the card into Develop. This commits a stage-entry event and makes the development workflow automatically eligible for Scheduler; no second routine Start/Scheduler click is required.
5. Scheduler selects a matching agent, or the default for an untagged card, and queues it for launch without a concurrency limit. The card remains in Develop with a Queued reason until execution begins.
6. The workflow runs preparation callbacks, such as creating a worktree where applicable, then starts the agent. Its attempt and runtime observations appear in the same card timeline.
7. Users and agents comment; users can mention agents. Questions, comments, results, checks, and relevant code changes remain linked to the card and attempt.
8. In the default template, the agent declaring development completed automatically moves the card into Review. No independent quality/check/process-exit gate is added. Review produces an ordinary report; the Shear interface follows later. Other JSON templates can configure different outcome transitions.
9. The human or configured completion policy records an outcome; completed work is archived. Human acceptance, Git commit, merge, and archive remain distinguishable facts.

Backlog entry never automatically starts a stage workflow. Stage preparation and review behavior depend on the selected template; the advanced review editor is optional for MVP.

## Routing and mentions

Tags select which agent may execute a stage's workflow. They do not by themselves define development versus review. No-tag routing uses the configured default; a tagged request must not silently run on a different agent.

A submitted @profile comment resolves the Card/profile conversation. If live, it is immediately appended to the current invocation; otherwise Scheduler starts/resumes it. It does not automatically interrupt, change routing tags or replace the original task/callback.

Develop/Review independently select their configured profile and queue a task with the stage callback. Distinct profiles retain separate histories; repeated work for the same Card/profile resumes the same provider ID. All invocations use one request/attempt model. See [Sessions](sessions.md).

A CLI reply is linked to its Question/comment in Timeline without completing the current assignment. An explicit completed report invokes its original callback unless a human gate pauses progression; required human review must be approved first. Different Cards/profiles run concurrently within the configurable global budget (default eight); duplicate live starts of one conversation are avoided.

Saving the description, typing a mention, and submitting a comment are different actions. Trigger a mention from a submitted, structured mention rather than every keystroke or autosave. Mentions in quoted text and code blocks remain content. These are interaction recommendations to validate.

## Progression, attention, and worktree retention

The normal rule is simple: the agent says whether it has completed; completed invokes the configured Kanban progression. Moving normally follows that report. MVP does not wait for terminal exit or independently verify the work.

The user explicitly defers exceptional old-execution coordination. If a process or mentioned agent continues after logical completion, do not block the next stage on drain, writer quiescence, or a lease/handover framework. Keep normal request/callback association and avoid repeating the same accepted completion.

When an agent reports blockage/needs input, mark the card needs attention and generate an internal notification event, with timeline context. Needs attention is separate from the Board stage. Native macOS notifications are later.

Keep the managed task worktree through development, Review, and rework. Handle it at Archive. A fresh Session need not create a new worktree, and card history/the Project directory are not disposable execution resources.

Templates are JSON-configurable for MVP. A customization GUI and complex migration of active Boards are deferred. Existing mismatch-warning direction remains relevant when configuration is incompatible.

## Unified timeline and resources

Comments are timeline entries alongside structural events. Each agent execution is visible with its input identity, workflow, routing decision, run/terminal association, output summary, and result. Relevant code/commit/check events can be recorded like issue/PR activity while raw terminal output remains a separate bounded stream.

The proposed event envelope, event catalog, ordering, comment revisions, and attachment model are in [Timeline](timeline.md). All state and content updates must have history; UI-only focus/hover changes do not need card events.

Cards support arbitrary resource references through extensible typed links: note, card, file, URL, artifact, change, image, and unknown future kinds. Image paste should store durable attachment bytes and a reference, not a temporary clipboard path. Report upload/storage failures without showing an image as saved. File reference behavior when files move, attachment limits, and export/retention policy remain open.

## Local-first backends

Confirmed: local cards first. GitHub Issue Cards use the same workflow and visual language, with source-specific content and editing. The [2026-11-09 GitHub Card design](github-cards.md) records the confirmed unified Add Card form, automatic Open-Issue intake modes and local removal/re-addition. See [implementation and acceptance](../github-integration.md) for current code and live-acceptance limits.

Proposed seam:

```text
Card presentation + local workflow + unified timeline
                          |
                   backend/source binding
                    /                 \
               Local card       GitHub issue (later)
```

A remote source binding stores provider/repository/issue identity independently of the internal Card ID and Project-local number. A GitHub Issue Card requires one source Issue; a Local Card may merely link to an Issue. The approved creation form previews remote title/body and edits local Additional instructions. Source identity, related Issue/PR links and delivery PRs have distinct meanings. Remote publication/conflict behavior still needs an explicit contract; do not promise offline GitHub writes. See the GitHub design for proposed caching and shared-core/content-enum organization.

## Custom card blocks

Keep the earlier direction of declarative layouts: title, tags, stage/status, resource previews, attention, change summary, and timeline excerpts. Research and implementation cards may show different blocks without changing card identity or automation semantics. Render unknown block kinds with preserved data and a fallback. Arbitrary executable widget plugins are outside MVP.

## Acceptance scenarios

1. Create a Project with a nonexistent directory and obtain its Board without requiring Git initialization.
2. Create cards in two Projects; each has independent numbering and unambiguous scoped references.
3. Move an untagged card to Develop: automatic default routing, preparation, and one execution are visible without another launch click.
4. Move a claude-tagged card to Develop: only the configured matching profile may run it.
5. Run multiple Boards with the same profile: each request launches independently without a concurrency cap.
6. Replay a move or reconnect after an uncertain launch: no blind duplicate, and ambiguity remains visible.
7. Submit user and agent comments, a structured mention, and a pasted image: all persist in the unified timeline. The mention appends or starts/resumes its Card/profile conversation; its reply cannot be mistaken for completion of the active development task.
8. The development agent reports completed: the default template enters Review and schedules an ordinary report without independent checks or waiting for process exit. Normal repeated report handling does not create another transition.
9. Archive completed work: retain card number, comments, events, and artifacts; reopening cannot silently dispatch.
10. Load a JSON template; show a useful mismatch if a change cannot apply. A visual template editor and complex active-template migration are deferred.
11. Edit card/context after queuing or launching: retain the previous input; a deliberate refresh creates the appropriate new request/history.
12. Call card operations through the agent interface: the same validation and timeline behavior apply as in the UI.

See [Projects and templates](projects-and-workflows.md), [Scheduler](scheduler.md), and [agent interface](agent-interface.md).
