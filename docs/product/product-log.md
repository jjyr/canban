# Product discussion and decisions

This living record preserves the Canban product discussion in English. Distinguish user directions, working proposals, and open questions. Discussion does not imply implementation authorization. Update the relevant [vision](vision.md), [workflow](kanban-workflow.md), or [dispatch rules](scheduler.md) when a discussion changes the design; identify superseded proposals so developers can find the current direction.

## 2026-11-05: initial direction

Status: **User-supplied direction**.

The product is Canban; the proposed approach is Human Context. Kanban is the management center, with modular Dispatch for agent execution through Herdr. Notes and discussions preserve intent; tags/presets and board transitions can initiate agent work. Tasks connect context, execution, code, and human review without requiring a separate hosted PR. Explore NSH + WebView. An advanced Shear-derived review editor can follow the Kanban + Dispatch MVP. Current consolidated requirements are in [vision and scope](vision.md#confirmed-product-directions).

The original idea used Space to describe organizing notes and work. That language does not require inheriting Herdr's object hierarchy.

## 2026-11-05: Canban owns its product organization (earlier discussion)

Status at that point: **Confirmed freedom of product design**; information architecture was open. The later Project clarification below resolves the top-level container.

The user confirmed that Canban can organize work in its own way rather than adopting Herdr's Space concept. Core's Terminal is the central execution object; labels and opaque documents allow client-defined associations. Core also supplies related runtime facilities such as status, parent/child execution, viewers, and machine connections.

Consequences:

- Do not require a Space merely because Herdr app has Spaces.
- Define board, note, repository, card, and run relationships from the Canban workflow.
- Keep Canban identities independent of terminal IDs and Herdr tabs/panes.
- Existing Space diagrams and one-Space-per-board/card recommendations are working proposals, not confirmed requirements or a finalized schema.
- Whether to keep a context container, rename it, or omit the layer remains undecided. Freedom to omit Space does not decide that all grouping disappears.

### Candidate: organize directly around boards and cards

Status: **Earlier product-manager proposal**, superseded by the directory-backed Project model below.

```text
Canban
  +-- Boards
  |     +-- Card
  |           +-- Notes / decisions / discussion references
  |           +-- repository + change references
  |           +-- dispatch requests -> runs -> core terminals
  |           +-- timeline + human outcomes
  +-- Notes / context library
  +-- Agents index
  +-- Dispatch settings / schedules
```

This candidate allows a board or note to exist without first creating a Space. It does not decide whether notes are global, board-owned, repository-owned, or grouped in another context container. Compare it with the existing container proposal through a feature walkthrough before fixing ownership or navigation.

## 2026-11-05: collaborative product development

Status: **User-requested working approach**.

The assistant should participate as a product manager: reason through ideas with the user, examine concrete scenarios and tradeoffs, identify gaps, and maintain documents for development. Go beyond transcription while avoiding premature conversion of exploratory ideas into implementation requirements.

For each substantive discussion:

1. Capture the user problem and desired behavior.
2. Separate explicit user direction from recommendations and unresolved alternatives.
3. Explain relevant implications for interaction, ownership, workflow, and MVP scope.
4. Update the affected design documents and this log when a direction changes or a material question emerges.
5. Preserve the rationale and clearly identify superseded decisions.

Use ASCII for product relationships and execution architecture when helpful, keeping those views distinct. Documentation stays in English; the conversation can remain in Chinese. These are repository documentation practices, not a request to write personal memory outside the project.

## 2026-11-05: directory-backed Projects and automatic Board workflows

Status: **User-supplied product direction**, superseding earlier Space/container and default-column proposals.

The user supplied a detailed earlier product note. Preserve these requirements:

- A freely created Project must correspond to a directory, created if absent; usually Git, but Git is not mandatory. Each Project has its own Board.
- Kanban is workflow stages plus triggers. Default template: Backlog, Develop, Review; completed work goes to archive. Only Backlog is required and does nothing on entry.
- Entering Develop selects the development workflow, including callbacks such as worktree preparation. Entering Review selects the review workflow, such as producing an explanation/report.
- Moving a card into an executable stage is scheduling intent. Dispatch starts work automatically under limits; no-tag cards use the default agent, tagged cards only a corresponding agent.
- Users define tag-to-agent mappings; initial tags can be agent names.
- Every Project has project-local card numbers for referencing work.
- Every card has one timeline combining comments, state/content updates, agent executions, and relevant code/resource activity. Users and agents can comment, and users can mention agents.
- Cards can reference arbitrary resources and accept pasted images.
- Local cards are the first backend. GitHub issue cards are a later source/backend with the same outward card experience.
- Projects select templates and can switch them; unmapped stages must be shown and resolved. People or agents can help create custom workflows.
- Canban exposes an interface for agents to call.
- Modular Kanban and Dispatch remain MVP core; advanced review is optional.

The earlier Plan/Process wording is interpreted as generic planning versus executable work, with Backlog/Develop/Review as the latest explicit default. Exact names beyond required Backlog are template-defined.

### Product-manager synthesis

Stage determines **what** workflow runs; routing tags determine **who** may run it; Dispatch determines **when** capacity permits. Keep those three decisions visible.

Recommended detailed policies, still proposals: archive as a separate lifecycle property; immutable per-request inputs; stable stage/entry identities; no dispatch on within-stage reorder or template migration by default; block unmatched/ambiguous routing rather than run an unrelated agent; scope mentions to a run or response request without spawning a competing writer.

At this point automatic development-to-review advancement was raised for discussion. The next clarification resolves its configuration/default. The subsequent unified Session clarification resolves the fresh-conversation rule for mentions and stage entry. Active-run moves, archive eligibility, and worktree retention still need choices; do not treat remaining recommendations as confirmed requirements.

Current design is in [vision](vision.md), [Projects/templates](projects-and-workflows.md), [Kanban workflow](kanban-workflow.md), [Dispatch](scheduler.md), [Timeline](timeline.md), and [agent interface](agent-interface.md).

## 2026-11-05: default automatic development-to-review transition

Status: **Explicitly confirmed by the user**.

When asked how a card enters Review after the development agent submits its result, the user selected: **Configurable by template, with automatic entry into Review by default**.

This replaces the earlier open question about the default transition. The default template records the development result, moves the card to Review, and creates that stage's configured workflow intent. It does not imply automatic human approval, merge, or archive.

Implementation proposals: commit causal result/transition/request linkage consistently, deduplicate repeated results, and reject workflow authority for obsolete run/stage entries. The precise criteria for a completed result and required check evidence remain open.

## 2026-11-05: unified Session and Canban-controlled scheduling

Status: **User direction**, refining the preceding mention discussion.

The user proposed that an @agent invocation starts a new conversation for the current question, while entering Develop independently assigns an agent to handle that stage. The assistant initially proposed question/stage session categories. The user explicitly rejected that distinction: sessions have no essential difference; their scheduling differs. The user further clarified that Canban performs this scheduling by controlling Herdr.

Current direction:

- One common Session model and lifecycle; no question/stage session classes, separate managers, or type discriminator.
- Submitted @mentions schedule fresh conversations, rather than appending into an active Run.
- Each executable stage entry independently selects an agent and starts a fresh conversation.
- A newly selected agent can use the same profile as before; the conversation/execution instance is fresh.
- Canban's scheduler resolves triggers, task/context, routing, resource constraints, and completion callbacks, then controls terminal execution through herdr-core.
- Trigger provenance and callbacks belong to the dispatch request. Completion of a comment response does not impersonate completion of the development stage.
- Human Context persists in card/Note/resource/timeline data and is captured for each new execution.

Recommendation: use the existing Run record/ID as the Session identity for MVP, one primary provider conversation and terminal per Run, without a separate session hierarchy. That cardinality and provider startup details are implementation proposals, not new user requirements.

The earlier assistant proposal to append normal @mentions to the active Run is superseded, and the two-session-type proposal is rejected. See [one Session model](sessions.md).

## 2026-11-05: runtime identity research and MVP readiness review

Status: **User-requested research and product assessment**; release scope is not frozen by this review.

The user requested one subagent to investigate IDs already supplied by Herdr and whether they can identify a Canban execution. Avoid assuming that separate session_id, run_id, and execution_id fields are all needed. Assess lifetime, queued/pre-launch intent, reconnect, process replacement, and historical traceability before choosing the minimal scheme. Findings belong in the runtime identifier research, separately from the single-Session product model.

The [completed source investigation](../research/herdr-identifiers.md) found an existing runtime identity, (owning core.id, local terminal.address), but no universal provider-conversation or pre-launch intent ID. Its minimal recommendation reuses dispatch_request_id plus attempt_number for logical execution and core's tuple for runtime binding. Names, mutable machine aliases, event sequences, and private process fields do not replace these. keep recovery may retain a terminal while replacing the program; core identity can change on detected cloned-machine state. No runtime tests or identity-scheme implementation occurred.

The user also asked whether the product design is documented, what remains incomplete, and whether MVP scope is complete. At that point the scope assessment identified result validation, active-run movement, resource/worktree handover, provider support, template configuration, and application lifetime as questions. The user's subsequent simplification decisions below resolve the main questions and supersede conflicting hardening recommendations. The [scope review](mvp-scope-review.md) now reflects those answers.

## 2026-11-05: MVP simplifications and event-driven direction

Status: **User decisions**, superseding conflicting earlier completion/coordination/template/review recommendations.

The user resolved the scope questions:

- Let the agent itself answer/report whether it has completed. A completed declaration follows the configured Kanban progression; do not add independent output/test/process-exit verification.
- Normal movement follows that report. Ignore exceptional old execution continuing, including mentioned agents, for MVP; do not build process-drain, exclusive-writer handover, or orphan coordination before progression.
- Keep the task worktree through active work and handle it at Archive.
- Review initially produces an ordinary report. Integrate the Shear UI next.
- Template depth and rich UI customization are deferred. Use JSON-style configuration first.
- Reuse Herdr's existing runnable agent setup for initial support.
- When work is blocked, mark the card needs attention and produce an internal notification event. macOS notification delivery follows later.
- Internal product activity produces events. Use a Rust enum and in-memory channel with interested components subscribing by topic.
- Discuss detailed event architecture after completing MVP; do not turn this direction into a pre-MVP framework design.

These decisions make the MVP scope reasonably converged. Exact report encoding, JSON fields, attachment limits, internal notification presentation, and managed-worktree cleanup details can be refined during implementation. Timeline persistence remains a requirement; the memory channel is transport, not a new durable event store.

Current scope and event direction are recorded in [MVP plan](../mvp-plan.md), [scope review](mvp-scope-review.md), [Projects/worktrees](projects-and-workflows.md), and [events/notifications](events-and-notifications.md).

## 2026-11-05: concrete MVP interface mechanisms

Status: **User-requested interface design**, with the concrete mechanism below still an **assistant proposal for review**.

The user asked how reporting works and clarified that recording architecture alone is insufficient: the concrete interface mechanisms should be designed. Define the critical MVP request/report path now while leaving detailed event architecture until after MVP as previously agreed.

The [proposed interface contract](../interfaces/mvp-contract.md) specifies a bundled report CLI over same-user local Unix-socket JSON-RPC, execution-context environment injection via core launch parameters, execution.report fields/state/receipt, normal failures/replay, and request-bound callbacks. It preserves self-reported completion, existing runnable agents, one Session model, and no process-exit or independent quality gate. The exact transport/command names are a recommendation, not an already approved implementation or shipped API.

## 2026-11-05: native Rust/NSH UI and the logical service boundary

Status: **User-selected UI direction and clarified architecture**, with detailed process/runtime wiring still to implement.

The user asked whether the overall architecture was clear enough and chose: main Board, card detail, Timeline, and Notes surfaces use Rust/NSH native UI; WebView is embedded only where needed. This supersedes the earlier main-workbench WebView proposal.

The user clarified that Canban can logically have a standalone server/service component without deploying it as a separate process yet, preserving the possibility of hosting it as a daemon later. The ApplicationService owns business operations/state independently of NSH, while MVP co-hosts it with the desktop. Native UI uses a typed Rust handle; external adapters share the same operation handlers. Application window-close/quit behavior must remain explicit.

This is modular separation, not a requirement for one crate/thread/process per responsibility. Service startup, shutdown, endpoint discovery, and subscriptions still need work if a daemon is extracted; business rules should remain reusable.

## 2026-11-05: CLI/RPC/MCP reference research

Status: **User-requested single-subagent research**, recommendation not yet an implemented adapter choice.

The user asked whether agents should receive CLI, RPC, or MCP and requested a Herdr comparison. The [completed report](../research/agent-interface-options.md) found CLI plus JSON-RPC, shared method/schema discovery, and agent help/manual support in current Herdr; no built-in MCP server was found in the inspected core/app source.

Recommendation: expose a thin CLI to shell-capable agents, use local RPC for its process boundary, call shared Rust operations directly from native NSH UI, and add MCP later only as another adapter when a target host benefits. All adapters preserve the same execution/report semantics. A shared MCP server must bind the execution explicitly; an MCP session ID or focused terminal does not identify the Canban execution.

## 2026-11-05: internal module architecture comparison

Status: **User-requested ASCII comparison**, with a recommended organization still **Proposed**.

The user asked to compare Canban's internal architecture. The [comparison](../architecture-internal-options.md) uses the same components across three alternatives: explicit service/coordinator calls, modules with lightweight event-driven reactions, and independently running component actors.

Recommendation: module ownership plus minimal event reactions, with ApplicationService as the shared entry/composition boundary. Kanban owns card transitions, Dispatch owns execution, Workflow reads JSON rules and requests configured actions, and Timeline/notifications/UI consume relevant changes. Modules share a persistence boundary and need not become separate actors, threads, crates, or processes.

This records high-level responsibilities and a normal development/report path. It does not select channel/topic APIs or override the user's decision to defer detailed event infrastructure until after MVP. No alternative has been implemented or newly confirmed by the user.

## 2026-11-06: component naming, crate separation, and Timer ownership

Status: **User-requested names/crate direction**, with Timer placement an **architecture recommendation**.

Current component names are **AppServer**, **Workflow Engine**, and **Scheduler**. AppServer replaces the earlier ApplicationService name; Scheduler replaces Dispatch. Workflow Engine is responsible for advancing workflow according to JSON templates. Scheduler owns routing, ordinary queue/capacity, execution, and time readiness.

The user requested separate Rust crates for the major responsibilities. This supersedes the earlier suggestion to leave crate separation unspecified. Co-hosting in one process remains unchanged; crate separation does not introduce actors or separate services.

The user asked where a Timer component for cards belongs. Recommendation: Kanban owns saved card timing attributes, canban-scheduler::timers owns waiting/due-trigger readiness, Workflow Engine applies timed workflow actions, Notifications handles reminders, and the native UI owns the date/countdown editor. Reminders, execution triggers, and due dates are explicit intents rather than one ambiguous time field.

The [crate and timer document](../architecture-crates-and-timers.md) proposes the workspace and a dependency direction that avoids cyclic Cargo imports between Workflow Engine, Scheduler, and Kanban. A shared-types/event/action boundary and AppServer composition can carry runtime reactions without mutual crate dependencies.

This does not select a timer library, cron/DST/missed-run policy, or detailed event framework, and it does not automatically expand MVP scheduled-task scope. Earlier log entries preserve the historical component names. Lowercase dispatch/request field notation in prior payload drafts does not imply another component or require a new identity.

## 2026-11-06: filesystem naming and Apache-2.0 license

Status: **User-selected project conventions**.

Cargo package names use canban-xxx, while crate directories and ordinary source/module filenames omit the prefix: for example, package canban-scheduler lives in crates/scheduler with source such as src/timers.rs.

The user selected Apache-2.0 for Canban, matching Herdr's project license. The root LICENSE contains the standard Apache License, Version 2.0 text. Future workspace Cargo metadata uses license = "Apache-2.0" and package inheritance. Existing third-party licenses/notices remain intact when code/assets are imported.

The user subsequently specified the project attribution as JJy <jjyruby@gmail.com>. The root LICENSE now starts with Copyright 2026 JJy <jjyruby@gmail.com>, followed by the complete unchanged Apache-2.0 text; README and development guidance carry the same attribution.

These conventions are recorded in [development conventions](../development-conventions.md) and the [crate directory mapping](../architecture-crates-and-timers.md). No implementation workspace/manifests have been created by recording these decisions.

## 2026-11-06: data architecture discussion

Status: **Architecture proposal for discussion**, not a user-selected database or implemented schema.

The user asked how Kanban, agent-related data, and Timeline should be saved. The [data architecture proposal](../data-architecture.md) recommends one Canban-owned SQLite database for all Projects, with attachments/larger captured artifacts stored as managed files outside source repositories. Domain crates own business rules while sharing the storage and transaction boundary.

The proposal separates current card state from durable Timeline/content revisions, reusable agent configuration from scheduling requests and execution attempts, and captured input/results from Herdr-owned live terminals. One attempt remains the unified Session; the request/attempt pair identifies it and the core/terminal tuple is an optional runtime binding.

State changes, associated history, and required scheduling intent should commit together before emitting in-memory notifications. This preserves history and restartable work without requiring event sourcing or advancing the deferred event-framework design. JSON configuration inputs have saved validated revisions; executions retain their actual input/configuration references. Archive retains card/history and triggers worktree handling.

The main open storage choice is application-managed data versus self-contained Project directories. The recommendation favors application-managed storage for MVP and explicit export/import later. Exact schema, attachment limits, retention/deletion, configuration reload, and modified-worktree cleanup remain refinements.

## 2026-11-06: Canban home and long execution content

Status: **User-selected home directory**, with file-backed execution content an **architecture recommendation** responding to the user's question.

The user selected ~/.canban/ for configuration and managed data, superseding the illustrative macOS Application Support path. They noted that actual agent input/context and submitted results can be long and asked whether files would be more suitable.

Recommendation: keep structured card/execution state, outcomes, history envelopes, and artifact references in SQLite. Store assembled prompts, captured context, and submitted report bodies as immutable managed files, consistently regardless of size. Proposed runs/<request-id>/<attempt-number>/ directories reuse the unified Session identity; shared attachments and managed worktrees live separately. Timeline loads report bodies on demand. File publication must succeed before the result/reference transaction commits; an accepted report must not reference an unfinished file.

The [data architecture](../data-architecture.md) records the revised layout and database/filesystem boundary. SQLite selection, exact filenames, and storage mechanisms remain proposals. No runtime data directories or databases were created.

## 2026-11-06: WAL and compressed Card archives

Status: **User-selected storage direction**, with detailed archive mechanisms still proposals.

The user accepted the SQLite/file-backed content direction, specified SQLite WAL, and selected zstd/lz4 compression when a Card is archived, including related Agent execution data and results. This supersedes the earlier pending SQLite selection and archive-as-only-a-storage-marker assumption.

The [data architecture](../data-architecture.md) now records these decisions. Proposed mechanics retain Card lookup/reference records in SQLite and package detailed Card history, execution records, captured context, reports, and required resource bytes. Publish and verify the package before switching references or removing redundant expanded data. Shared active resources remain available to other Cards. Archive details are readable on demand; restore keeps stable identities. Compression must not introduce a process-exit gate for exceptional old executions. Late accepted results remain durable alongside the sealed snapshot.

The choice between zstd and lz4, container format, detailed schema, cleanup and restore mechanics remain refinements. zstd as the default is a recommendation, not a selected codec. No runtime storage was initialized.

## 2026-11-06: make execution file ownership Card-centered

Status: **Revised layout recommendation responding to user feedback**.

The user pointed out that the execution context directory structure did not clearly associate files with a Card. Replace the earlier top-level runs/<request-id>/<attempt-number>/ layout with projects/<project-id>/cards/<card-id>/runs/<request-id>/<attempt-number>/. Both @mention and stage-triggered executions for the same Card share this ownership tree; one Session model and the request/attempt execution identity remain unchanged.

Card-owned resources live with the Card; resources shared across Cards can remain in shared managed storage. Database relationships remain explicit, and artifact IDs hide physical storage locations. The projects/ subtree contains Canban data, while the user's actual source directory remains separate. Archive combines the Card subtree, a consistent export of its database history/content, and required shared resources. The [data architecture](../data-architecture.md#files-and-configuration) records the revised proposal; no runtime directories were created.

## Remaining implementation refinements
The earlier completion, old-execution handover, worktree-lifetime, Review-depth, template-UI, provider-scope, and notification questions are resolved by the user's decisions above. Do not keep them open as product blockers.

Small refinements remain: the common completion/blockage report encoding, deterministic routing configuration, JSON file/schema/reload details, Note ownership/selection, attachment limits, archive treatment of modified managed worktrees, internal notification display, and actual coordinator lifetime. Detailed event architecture is intentionally postponed until after MVP.
