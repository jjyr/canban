# Global Overview

Current implementation: 2026-11-11 refinement. Overview is the global handling and execution view across Projects. Board remains the management workspace; Terminals owns direct PTY interaction. The [refinement design](../overview-and-terminal-navigation-proposal.md) records the selected direction and entry inventory. See [refinement acceptance](../overview-terminal-acceptance-20261111.md) for executed checks and limits.

## Information and layout

Overview has no default inspector or Terminal preview. Compact production rows show task identity, Project/profile, and a source-derived reason or reported activity. Missing activity uses a factual state, never an invented summary. Branch and raw execution IDs are not permanent row content. Waiting duration is shown only when an actual runtime wait timestamp exists.

Sections are Needs you, Updates, Running, Queued, Idle and Closed. Needs you combines task decisions, runtime input, recovery and human Review, preserving each source's independent resolution. Idle and Closed are collapsed by default and retain expansion state. Empty groups are omitted. Running includes Project-owned Workflow configuration Terminals as well as Card Agents; unrelated manual Shells remain in Terminals.

Rows retain Card/profile conversation identity. Multiple profiles on one Card remain distinct; Card-only recovery stays visible without fabricating an Agent. Stable Project/Card/identity ordering avoids reordering on every activity timestamp. The heading reports Scheduler execution slots and queued requests; section counts describe their items. The rail badge continues to count unique Cards with unread or pending reminders, not execution slots.

Updates displays unread Card summaries and opens the corresponding Timeline event. Cards already represented in Needs you do not repeat in Updates; their actionable rows retain unread counts. Running and Updates can represent different facts about the same Card. Visiting Overview never marks replies read or approves a gate.

## Direct navigation

Row activation and its visible primary action share a destination: Review for a human review result, Card for task decisions/recovery, exact Terminal for a live runtime interaction, or saved Card/request history for queued/closed work. Keyboard focus alone has no effects. Rows with both task and runtime concerns expose a separate Terminal action. Resume Agent is explicit and subject to exact-provider-history eligibility and Scheduler admission.

Opening a destination retains Overview selection, scroll and expansion for Back. Background status changes never redirect the user. Card-level Needs attention now handles its actual reason directly or offers a compact menu when several independent reasons exist; it no longer routes through an Overview inspector.

See [Terminal navigation](../terminal-workspace.md#unified-terminal-navigation) for multiple profiles, ended history, disconnected bindings and explicit execution effects. Runtime recovery cannot approve a task gate; Herdr done alone does not constitute Ready for review. Unknown connection state is not Idle.

## Ownership and validation

`crates/notifications/src/overview.rs` retains the authoritative Card conversation/reason projection. `crates/ui/src/overview.rs` composes the global display and Project configuration entries from prepared snapshot data, without render-time runtime reads. `crates/ui/src/terminals/navigation.rs` owns shared UI destination selection; AppServer owns durable execution and eligibility.

The Gallery uses the production screen with multi-reason, multiple-profile, long-title, queued, disconnected, idle and closed fixtures. Native acceptance must separately exercise direct actions, nested Board buttons, Back/Forward, narrow layouts, exact resume and unread positioning using an isolated Core. Historical lifecycle/backend evidence remains in [Agent Overview and lifecycle](../agent-overview-and-lifecycle.md).
