# MVP scope review and resolution

Review date: 2026-11-05. Status: updated after the user's explicit answers. The earlier review identified broad scope and open behavior; the decisions below now close the main gaps. This is product documentation, not proof of implementation readiness or a finished application.

## Current assessment

The major product-scope questions are sufficiently settled to proceed with focused feasibility and implementation planning. The default loop is Project/card -> Develop -> agent-reported completion -> Review report -> human disposition/Archive, with shared context, timeline, and one Session mechanism.

Do not retain earlier implementation recommendations as extra release gates when they conflict with the user's simplifications. In particular, MVP does not require independent completion verification, old-process coordination, a template GUI, or a detailed event framework.

## User resolutions

| Earlier question | Current decision |
| --- | --- |
| How is completion determined? | The agent explicitly answers/reports whether it has completed. A completed report invokes the configured Kanban progression |
| When does a card advance? | Normally after that completion report |
| What if an old execution continues? | Accept/ignore the exceptional residual-execution case for MVP, including mentioned agents; do not block progression waiting for coordinated termination |
| How long is the worktree retained? | Through development, review, and rework; handle it at Archive |
| What is Review in first release? | An ordinary report; integrate the Shear interface later |
| How are templates customized? | JSON-style configuration first; rich UI customization and deeper template features later |
| Which agents ship first? | Reuse Herdr's existing runnable agent setup through core |
| How is a blocked task handled? | Mark the card needs attention and generate an internal notification event |
| Are native notifications required? | No; macOS delivery is a later subscriber/integration |
| What is the event direction? | Rust enum + in-memory channel, with interested components subscribing by topic; discuss detailed architecture after MVP |

These decisions preserve the earlier requirements for local cards, project-local numbers, comments/resources/image paste, basic Notes/context, automatic routing, fresh conversations, timeline, and agent-callable operations.

## First-release depth

| Area | MVP | Later |
| --- | --- | --- |
| Kanban | Local Project/Board/card, default stages, archive, visible task/execution/attention status | GitHub issue backend |
| Scheduler | Automatic triggers, tag/default selection, configurable global concurrency (default eight), later-stage priority, human gates and generic Session execution via core | Complex orchestration and residual-process management |
| Completion | Agent's own completed declaration and workflow callback | Independent verification/quality gates |
| Context and discussion | Basic Notes/preview, captured references, comments, attachments, fresh @mentions | Full collaborative knowledge-base/chat product |
| Worktree | Prepare when applicable, reuse across stages, retain until Archive | Sophisticated lifetime/migration/cleanup automation |
| Review | Ordinary report and human outcome | Shear editor and advanced code-review UI |
| Template | JSON configuration | Visual editor and complex live migration |
| Agent compatibility | Existing Herdr launchable agents/configuration; shared Canban report interface | Separate provider support program |
| Attention | Card flag/status + internal notification event | Native macOS notification adapter |
| Events | Small enum/channel/topic-subscription implementation | Detailed event architecture and infrastructure |
| Interface | Minimal shared UI/agent operations for the core loop | Broad public extension ecosystem |

The minimal completion-report encoding is an implementation interface, not a new automated evaluation step. Output summaries, changes, and check results can remain useful context without becoming mandatory independent acceptance gates.

Herdr core itself has no agent-brand registry. Reusing its existing runnable setup means using available launch commands/templates/configuration through the same execution API; it does not mean core supplies provider conversation IDs or validates task completion.

## Remaining details, not new scope blockers

- Choose the simplest encoding/callback by which an agent explicitly reports completion or blockage.
- Keep default/tag resolution deterministic; show unavailable or ambiguous configurations rather than invent a successful launch.
- Decide the precise cleanup action for Canban-managed worktrees at Archive. Before Archive, retain them. Card/archive history and the Project directory are separate from disposable execution resources.
- Define needs-attention presentation and its internal notification body. The notification is an application event; native delivery is deferred.
- Pick small JSON configuration fields. Do not build a rich workflow editor or full live-migration system first.
- State actual coordinator/window lifetime behavior. Event architecture details remain deferred.
- Keep basic request association/replay protection; do not turn it into exhaustive old-run/resource handover logic.

The [Herdr identifier report](../research/herdr-identifiers.md) recommends (owning core.id, local terminal.address) for runtime binding and (dispatch_request_id, attempt_number) for logical execution identity. This does not change the agreed one-Session model.

## MVP acceptance focus

Demonstrate one useful task with real visible context and history: create a local card, trigger development automatically, receive the agent's completion declaration, follow the default Review progression, obtain a report, handle human attention when needed, and archive. Also show fresh @mention execution, JSON template loading, worktree retention until Archive, and the minimal shared agent interface.

Use harmless fixtures for protocol/event mechanics. A declared existing-agent launch path and the native Rust/NSH interactions and any specifically embedded WebView still require runtime acceptance; none has been run in this documentation work.

The user also selected native Rust/NSH main surfaces with WebView only as needed. The AppServer is logically independent while co-hosted in the desktop process for MVP; a future daemon is an extraction option, not another release prerequisite. See the updated [architecture](../architecture.md), [MVP plan](../mvp-plan.md), and [event direction](events-and-notifications.md). The product scope is now reasonably converged; implementation proof is the next gate.
