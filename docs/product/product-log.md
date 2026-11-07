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

## 2026-11-06: UI architecture and design-system discussion

Status: **User-requested quality/reuse direction**, with architecture and process recommendations for discussion.

The user wants each needed UI component developed independently or reused from NSH, supported by an overall design system so components are reused and UI defects do not recur across screens. They asked to discuss how to achieve this before implementation.

The [UI architecture proposal](../ui-architecture.md) separates app-owned tokens/styled controls in a proposed canban-ui-kit from reusable product components, presenters and screen composition in canban-ui. NSH's inspected component documentation assigns interaction behavior to nsh-base and styling to the application. The proposal therefore reuses base behavior, centralizes Canban presentation/interaction contracts, and avoids independent screen implementations of shared controls.

Proposed development practice: inventory components and NSH reuse routes; build a native gallery using actual production components and deterministic fixtures; validate meaningful component and composition states; fix defects in their owning layer and retain reproductions. Start with a real Board/Card/Timeline/composer slice rather than an exhaustive speculative library. These proposals are not yet approved crate/convention changes and do not claim native acceptance or a guarantee of zero defects.

## 2026-11-06: UI direction approved and development readiness

Status: **User-approved UI architecture and conventions**; readiness assessment is an engineering recommendation.

The user approved the preceding UI direction: separate canban-ui-kit from product UI, reuse suitable NSH behavior, define shared visual/interaction contracts, maintain a native gallery using production components, and fix defects at their owning layer with meaningful regression cases. The architecture/crate plan, development conventions and AGENTS guide now record this as selected direction. Palette, exact component inventory/metrics and test harness details remain refinements.

The user then asked whether development could begin. Assessment: product scope and ownership boundaries are sufficiently defined to start MVP implementation. The remaining high-risk areas are native NSH input/editing, Board drag/scroll/focus composition, application state updates, and real Herdr startup/report/reconnect integration. Start with workspace boundaries, a small native component/gallery slice, and the local Card -> queued request -> execution report -> Timeline/Review path. Resolve detailed schemas and UI APIs through those slices rather than delaying all work for a complete upfront specification. This assessment does not claim that runtime feasibility or native acceptance has already been demonstrated and does not itself start implementation.

## 2026-11-06: first MVP implementation and live acceptance

Status: **User-authorized implementation**, with scoped verification evidence.

The user requested implementation through MVP, then confirmation of native interactions and Herdr connectivity. The repository now contains the Rust workspace, shared UI Kit/Gallery, local Board/Card/Notes/context selection, SQLite WAL and file-backed captures/reports, Workflow Engine/Scheduler, installed-hc JSON-RPC adapter, explicit report CLI/IPC, attention notices, worktree lifecycle, and checksummed tar+zstd Card archives.

An isolated real Herdr core/PTY harness verifies development-to-review, fresh mentions, blocked notices, report storage/archiving/restoration and restart without duplicate dispatch. A real existing Codex Agent also read captured context and successfully reported completed through the bundled CLI. Live integration found and fixed obsolete Codex --full-auto startup arguments and large JSON frame truncation from macOS accepted-socket nonblocking inheritance. Native checks include creation/editing, Unicode/Markdown, stage drag, comments, Notes, keyboard/disabled behavior, themes and image clipboard persistence.

The current [implementation guide](../implementation.md) distinguishes actual behavior and limits from earlier design proposals. Structured SQL history stays online in this MVP; complete record exports plus long execution/resource files are compressed, and matching expanded bytes compacted. Deferred features remain deferred. This does not claim exhaustive acceptance of every provider, IME composition path or large-data interaction.

## Remaining implementation refinements

The earlier completion, old-execution handover, worktree-lifetime, Review-depth, template-UI, provider-scope, and notification questions are resolved by the user's decisions above. Do not keep them open as product blockers.

Small refinements remain: the common completion/blockage report encoding, deterministic routing configuration, JSON file/schema/reload details, Note ownership/selection, attachment limits, archive treatment of modified managed worktrees, internal notification display, and actual coordinator lifetime. Detailed event architecture is intentionally postponed until after MVP.


## 2026-11-06 — Simplify concurrency and inspect direct Agent interaction

User direction: remove default concurrency limits and avoid speculative scheduling machinery. Scheduler now launches eligible requests without global, Project or profile caps. Obsolete capacity configuration fields are ignored, including saved request profiles. This supersedes earlier capacity/admission decisions in this log and the original research.

The user also asked to inspect direct Agent interaction: add information while an Agent works, ask follow-up questions, and continue through Herdr operations. The current UI only reads output and retries with a fresh Session. Herdr exposes terminal.send/interrupt/keys/attach, but the built-in batch-mode commands do not provide an interactive conversation merely because a PTY exists. The current gap and recommended minimal next step are documented in [Agent interaction](agent-interaction.md); interactive support is not yet implemented or accepted.


## 2026-11-06 — Interactive terminal, automatic exit and explicit resume

The user rejected codex exec / claude -p as the intended execution experience. Start an interactive Agent terminal for work; users can view it, type and interrupt during execution. After completion, exit the Agent, and allow a later explicit resume for follow-up.

The technical/product proposal preserves a single Session model: request/attempt is an invocation, provider conversation is resumable history, and core/terminal is the current runtime binding. New stage entry/@profile remains fresh; explicit resume creates a new request reusing the provider conversation, with no stage callback by default. Exact ID binding, graceful post-report exit and the renderer are not yet accepted in the runtime. The earlier minimal text-composer-only recommendation is superseded by the requirement for a real terminal.

See [Agent interaction](agent-interaction.md) and [technical research](../research/interactive-agent-lifecycle.md). WebView plus a mature terminal emulator, normal-close failure handling and Restore-before-archived-resume are recommendations, not additional user decisions.


## 2026-11-06 — Native libghostty and Card/configuration context reuse (discussion)

The user selected libghostty instead of the proposed WebView/xterm.js terminal. Use libghostty-vt as the initial concrete reuse target; the NSH native renderer/input host remains work to implement. Herdr's inspected API has no provider-conversation resume operation; Canban resolves provider references and invokes the provider's resume command through terminal.run.

The user then proposed reusing Agent context within each Card, followed by the clarification that genuinely different contexts such as Review should remain separate. The working recommendation is one provider conversation per (Card, Agent configuration identity). Developer and Reviewer can be distinct named configurations using the same Codex provider. Repeat mentions/clicks reuse the corresponding role's conversation; execution/request/attempt/report/callback records remain per invocation. This is a proposal under discussion, not a finalized new runtime contract or an implemented migration. It supersedes the preceding execution-specific mention-picker recommendation and avoids equating provider brand with conversation identity.

See [Agent interaction](agent-interaction.md), [interactive lifecycle](../research/interactive-agent-lifecycle.md) and [native terminal](../research/native-terminal.md). No Agent was launched, provider setup changed or native library imported during this discussion.


## 2026-11-06 — Profile identity accepted; definition ownership proposed

The user agreed that selecting a profile through @ uses that configuration and its Card-scoped conversation. Distinct configurations such as Developer and Reviewer retain distinct context even if their provider is the same.

Recommendation in response to who defines profiles: the application-wide Agent registry defines reusable named profiles (starter profiles supplied by Canban, user customization through JSON); Kanban templates reference those IDs for automatic stage tasks and define stage instructions/transitions. Templates do not define live sessions. Direct mentions can use any configured profile without changing the workflow. Reuse the existing config/agents.json and workflow configuration boundaries. New profile role fields, stage references and tag/default precedence are not implemented or finalized by this documentation update.


## 2026-11-06 — Implement independent profiles and stage references

The user approved the profile/template ownership boundary. Implemented optional profile.role_instruction and stage.profile, starter developer/reviewer roles on fresh homes, role/task snapshots and validation of global references at startup/Project creation/reload. Explicit mentions select their profile; named automatic stage profiles take precedence over Card routing tags, while old stages preserve tag/default selection. Existing files/Projects are retained.

Settings and Card mention controls share registry data; Board headers expose stage selection. Workspace tests and isolated real-Herdr fixture acceptance passed, including actual distinct Developer/Reviewer launches and native @reviewer submission. This implements the configuration/routing slice; provider conversation binding/reuse, native libghostty terminal and interactive graceful exit/resume are still unimplemented and must not be described as delivered. See [implemented profile configuration](agent-profiles.md).


## 2026-11-07 — Reply channel comparison and simpler profile loading

The user requested comparing Agent-initiated Canban CLI replies against reading Herdr terminal output before selecting how active @questions are handled. Generic Herdr APIs provide terminal capture/streams/status, not semantic assistant-message records linked to a Canban question. [Reply path comparison](../research/agent-reply-options.md) records both options and a combined recommendation, still pending a decision; no reply API or scraping logic was implemented.

The user selected simpler profile behavior: changed configuration applies on next load; no strict new profile IDs/versioning/reset policy. Running invocations retain their loaded setup and saved execution captures remain immutable. The user also accepted Restore before resuming archived Cards and explicit fresh-conversation recovery when provider history is missing.


## 2026-11-07 — Immediate append selected for active @mentions

The user considered scheduling @questions after an Agent finished its current task, then explicitly selected immediate append for simplicity. Persist the question and deliver it now to the existing Card/profile invocation. Do not add a task-completion-gated follow-up queue for active mentions. The original task/callback remains active; answering the question does not by itself complete it or request Agent exit.

Absent/exited conversations still use the agreed first-start/exact-resume path, with an ordinary follow-up invocation. Closing and blocked/uncertain-delivery cases preserve the message and show its state. The delivery decision does not imply Herdr has semantic reply extraction; [reply-channel comparison](../research/agent-reply-options.md) remains the reference for that separate boundary. No runtime behavior was changed in this documentation update.

## 2026-11-07 — Interactive Card/profile conversations implemented

Implemented the accepted single conversation per Card/profile model with immediate live mentions, CLI question replies independent of completion, explicit provider binding, exact-ID resume, turn-boundary normal exit and a native libghostty-vt terminal. Real Codex workflow/resume acceptance and deterministic Herdr/native UI acceptance are recorded in implementation.md. Claude's lifecycle adapter remains pending live-provider acceptance; no new product session classes or concurrency caps were introduced.

## 2026-11-07 — Next MVP priorities and command-defined Agent expectations

The user accepted daily-use validation, the Review/code-delivery loop and native Terminal/recovery acceptance as the next priorities. Review stays simple: Accept means merge; rework can use a move back to a previous stage or an @Agent instruction. Merge target/commit/conflict details remain implementation decisions to discuss; no new review state machine is required.

The user expects Agent start/resume behavior to be supplied through editable profile commands, with presets for common Herdr-runnable Agents and user-provided CLI/script commands for others. The [CLI and extensibility discussion](agent-extensibility.md) separates this proposed target from the current Codex/Claude-specific adapters and documents the shared session-ID/reply/report/turn-end/exit contract. This turn changes documentation and upgrades NSH; it does not implement the proposed profile refactor or merge action.

## 2026-11-07 — Unified profile invocation interface confirmed

The user explicitly agreed that Codex and Claude should use the same interface as custom profiles, with optional supplied presets. Rust owns configuration resolution and generic invocation/lifecycle handling; provider-specific command and callback recipes belong to profiles/preset integrations. The current seven-field Profile schema is documented separately from that target. Preset resolution, generic exit/lifecycle fields and migration are still being discussed; no runtime refactor was performed by this clarification.

## 2026-11-07 — Reuse Herdr lifecycle observations; avoid duplicate hooks

The user selected adjusting Profile to reuse capabilities already supplied by Herdr. Runtime status/turn/wait/interrupt use the Herdr adapter; a separate Canban turn-end callback is not a requirement of the new profile contract. Canban retains business reports/replies and provider session binding, while Profile supplies start/resume and normal-exit behavior. Automatic close still requires an explicit completed report plus a later relevant root completion observation; idle, blockage and a bare turn increase are insufficient. Agents without usable status remain manually controllable. Current notify/Stop code has not yet been refactored; this entry records the design decision, not runtime acceptance.

## 2026-11-07 implementation and acceptance follow-through

The approved profile simplification is implemented: editable JSON presets/whole-command overrides, generic invocation, Herdr root status/control, no mandatory notify/Stop channel, explicit report/reply/session binding retained. Recognized legacy configuration is backed up and migrated; opaque Herdr templates fail without rewriting their original configuration.

The user authorized implementation followed by real-workflow, Review-delivery and Terminal/recovery verification. Real Codex human-input and rework tasks passed. Basic native Diff and explicit commit/merge now close the code-delivery gap; Archive remains separate. The implementation defaults target the Project's captured local branch, require a clean matching source checkout, integrate divergent target changes in the Card first, retain conflicts there and fast-forward source only after success. See review-delivery.md for the concrete policy. Native and deterministic recovery checks are recorded in ../mvp-acceptance.md. Chinese IME verification was explicitly skipped by the user. Claude live acceptance remains blocked by local authentication; Shear, GitHub backend and template GUI remain deferred.

## Standalone component Gallery

The user requested removing Gallery from the production program and moving it to examples as the UI-component development/check surface. The entry and deterministic fixtures now live in examples/gallery; mise run gallery opens a separate native app using the exact production UI Kit/product components. It does not initialize AppServer, application storage or Agent/Core connections. Production navigation and startup no longer contain a Gallery mode.

## 2026-11-07 — Notes and workspace direction accepted

The user identifies the calm native workspace (A) as closest to the Kanban-centered product, then accepts the proposed Note role and ASCII workspace hierarchy. Notes retain emerging ideas, independent Agent discussion and reusable Project context. Explicitly creating Cards from a Note places scoped work in Backlog while preserving the source document; multiple Cards may reference one Note and a Card may use several Notes. Note discussion does not automatically advance a Board workflow.

The accepted workspace direction uses Board as the default, an optional selected-Card pane, focused Card plus Terminal/Review when needed, and a Notes document list/editor with discussion opened on demand. See [Notes and the Kanban workspace](notes-and-workspace.md). Detailed Note conversation ownership, context/reply behavior, responsive dimensions and component contracts still need design. Default inclusion of all Project Notes has not been changed or approved as a new policy. This acceptance updates documentation only; it does not authorize or claim completion of the UI redesign.

## 2026-11-07 — Note simplified to file, related events and bidirectional links

The user clarified that a Note should initially be an editable file, augmented with a Timeline of related events (including Card relations and Agent modifications/associations) and bidirectional links between Notes. Agents interact with Notes by reading and writing. This supersedes the independent Note discussion/composer direction in the preceding entry; a Note-owned Agent conversation is not required by the simplified scope.

The Notes ASCII layout now shows file editing/preview, related Cards, links/backlinks and Timeline, with no chat pane. File location, identity/link syntax, refresh/conflict handling and reliable attribution of external writes remain details to design. Current SQLite Note bodies have not been migrated, and no UI/runtime implementation is claimed by this documentation update.


## 2026-11-07 — Database Notes and focused workspace implemented

The user clarified that file-like Notes are actually stored in the database, so Agent and product operations can record associations. This supersedes the physical-file interpretation above. No Note filesystem watcher or content migration is required. Notes now expose shared create/read/write/rename operations, immutable revisions, optimistic conflicts, Card/Agent provenance and stable bidirectional links. Agents use the Canban CLI; Notes do not own another chat/session model.

The user approved the calm Kanban workspace and confirmed focused work behavior: clicking an Agent or Review hides the Board while retaining global navigation, with Card left and work right. Back/Close returns to the original Board/selection; drafts and scroll survive route changes. Closing the viewer does not end execution. Production components are also imported by the standalone Gallery. See [implemented contracts and validation](../notes-and-workspace-implementation.md); drag polish, full design-state coverage and crash-persistent drafts remain separate refinements.


## 2026-11-07 — Named workspaces, synchronized base and bounded scheduling

The user approved simple Workflow workspace settings rather than a Git-operation engine: optional worktree creation, base branch, remote and pre-creation synchronization. New Card work uses a named branch and worktree; Card details expose their identity/status. Fetch and fast-forward failures stop preparation. Existing active workspaces are reused, and archive retains branch history.

The user replaced the previous no-cap policy with a configurable global default of eight Agents, across Boards. Later template stages get priority, FIFO within the same depth. Live blocked/idle/closing Agents still count; the queue waits without killing existing work when limits change.

Human handling is a persistent stop condition. A required Review, blocked/error or uncertain recovery cannot resume automatically because a slot opens, the app restarts, a notice is read, or an Agent subsequently reports completion. Explicit human continue/rework/retry/acceptance releases the appropriate gate. Manual Agent assistance remains available without approving the workflow. Implementation and isolated/native evidence are recorded in [workspace/scheduling](../workspace-and-scheduling.md).


## 2026-11-07 — Global human-attention workspace design

The user requests distinct names for Canban task blockage and Herdr runtime waits, one highlighted in-app entry, and a Project-independent list of current human actions. The context must identify Project, Card, Agent and actual assignment, show Herdr-reported context when available, and link directly to Card or the exact terminal. macOS notifications remain out of scope.

The [Needs attention proposal](needs-attention.md) recommends Execution waiting (Permission/Sign-in/Answer required), Task needs decision, Review requested and Recovery needed, with a persistent global unresolved count. It defines a list/context layout, source-specific actions, a bounded terminal preview fallback, independent runtime/task resolution, per-episode deduplication and return navigation. Labels/layout/mechanics are recommendations for review; this document update does not implement the global UI, status-based slot release or idle auto-close, and does not claim acceptance of those proposals.


## 2026-11-07 — Broaden attention into a Herdr-style Overview

The user supplied a Herdr UI screenshot and pointed to `~/Workspace/herdr-gpui`, requesting a global page covering all Agents, including work needing human action. Read-only source inspection at `a6db99b4` confirmed the grouped Overview, summary, attention emphasis, branch/host/time metadata and collapsed closed history. This replaces the proposed separate attention-only top-level destination.

The [Overview proposal](overview.md) combines Agent activity and attention into one global entry while retaining Board as the workflow center. It recommends Needs attention, Ready for review, Working, Queued, Idle and Closed groups, a selected Card/assignment/terminal-context inspector, per-conversation identity, and preserved return navigation. Unlike generic runtime done, Canban review readiness still requires its own outcome/gate. Host metrics, pin/snooze/settle controls and remote-machine management remain deferred. These are documented recommendations, not an implemented UI or a new acceptance result.


## 2026-11-07 — Implement status-aware admission, idle reclamation and Overview

User authorized the discussed Agent limit and global Overview. Verified blocked/idle/done/error runtime releases execution slots; unknown/disconnected/starting work reserves them. Manual resumption can exceed the automatic budget without preemption. This supersedes the earlier all-live-terminal slot rule. Default admission remains eight and resumable idle closure defaults to fifteen minutes, configurable through Settings/JSON. Viewed terminals, outstanding questions, unfinished automatic assignments and unresolved decisions are protected. Normal exit retains exact provider history and does not complete/advance a Card.

Overview replaces Agent activity and the proposed separate attention page. Its pure projection groups all Projects, counts actionable conversations once, separates execution input from task decisions, provides bounded read-only context and explicit Card/Terminal/Review/exact-Resume actions, and preserves the return destination. Native UI and isolated Herdr evidence, API and known protocol race boundary are recorded in [Agent Overview and lifecycle](../agent-overview-and-lifecycle.md).


## 2026-11-07 — Compact icon sidebar direction

The user prefers compact icon navigation in the sidebar instead of the current large buttons. Treat the icon rail as the default direction, with selection/focus affordances, accessible names, hover labels and actionable badges preserved. Project switching and any expanded-label option remain layout details; the other Herdr UI research recommendations are not implicitly approved.

Source verification confirms Herdr UI implements terminal selection annotations, Diff line notes and browser annotations. Extending source-linked quotations to Canban reports/Notes and durable Card comments is a Canban product proposal. See [Herdr UI references](../research/herdr-ui-product-and-layout.md).


## 2026-11-07 — Quick Open candidate list and bounded UI scope

The user selected fuzzy search across Projects, Cards, Notes and Agents, presented as a search field with a candidate list. Context preview is unnecessary and is removed from the proposed Quick Open. Candidate type and concise Project/Card ownership remain visible to distinguish similarly named results. Existing Overview preview is independent.

The requested scope reconfirmation pairs this with the compact icon sidebar direction. Project-picker presentation, a proposed Cmd+K shortcut, destination semantics and Gallery/native acceptance are documented in [Herdr UI references](../research/herdr-ui-product-and-layout.md). General Back/Forward navigation, tool tabs, contextual quotations and advanced Review remain separate recommendations.


## 2026-11-07 — Implement compact rail and Command+O Quick Open

The user authorized the two-feature slice and selected Command+O instead of the proposed Command+K, with outside-click dismissal. The default compact icon rail and cross-Project Project/Card/Note/Agent fuzzy candidate list are implemented. Quick Open has no preview; Escape closes it in one press. Agent activation attaches the exact live conversation or opens closed/unavailable context with explicit recovery actions, without automatically resuming or approving work. Production Gallery components and isolated native/Core checks are documented in [Quick Open and navigation](../quick-open-and-navigation.md). Other UI research proposals remain deferred.
