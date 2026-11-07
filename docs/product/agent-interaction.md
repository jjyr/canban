# Card-scoped Agent conversations and native terminals

Status: implemented on 2026-11-07. One provider conversation per (Card, profile), interactive start/resume, immediate active mentions with separate CLI replies, native libghostty-vt terminals and turn-boundary normal exit. Real Codex and deterministic Herdr acceptance passed; see [implementation and acceptance](../implementation.md). Claude's preset is implemented; live Claude acceptance still needs valid local authentication.

## Product model

One Card has one conversation per configured Agent profile. The first assignment creates the provider conversation; subsequent work, @mentions and direct interaction reuse it. A profile is the configured Agent identity/role, such as developer or reviewer, not the provider binary/model name, a terminal process or a routing tag. Both configurations may invoke Codex while retaining separate context.

```text
Card #42
  |
  +-- Developer config (Codex) -------------- conversation S1
  |     +-- Develop execution 1 ------------ terminal t1 -> exits
  |     +-- @developer follow-up 2 ---------- terminal t3 -> exits
  |
  +-- Reviewer config (Codex) --------------- conversation S2
        +-- Review execution 1 ------------- terminal t2 -> exits
        +-- @reviewer follow-up 2 ----------- terminal t4 -> exits

Same lifecycle and record structure; separate provider histories.
```

Different Cards never reuse a conversation merely because their profiles match. On one Card, different profiles retain separate conversations. Repeated invocations remain associated with the same Card/profile binding, using exact provider conversation IDs rather than latest-session searches.

There is one Session model. The enduring provider conversation differs from a short-lived invocation, but there are no question/stage Session classes. Each invocation still needs its request/attempt, task input, runtime binding, report and completion callback.

## Who defines profiles and what templates reference

Implemented: define profiles in the application-wide registry, independently of Kanban templates. Canban supplies useful starter configurations; users can add/edit named profiles in JSON. Reuse the existing ~/.canban/config/agents.json location for MVP rather than introducing Project-specific registries or a profile designer.

A profile owns its stable identity, provider launch/resume setup, model/options and durable role instructions. For example, developer and reviewer may both launch Codex while having distinct role instructions and IDs. An Agent profile is distinct from a Herdr launch template. Canban invokes resolved commands directly; legacy opaque templates require conversion. See [profile schema](agent-profiles.md).

A Kanban workflow template owns stage definitions, stage-specific task instructions, completion transitions and references to the profiles used for its automatic work. It does not own the reusable profile definition or a live conversation.

```text
Agent registry                          Kanban template
  developer: Codex + developer role <--- Develop uses developer
  reviewer:  Codex + reviewer role  <--- Review uses reviewer
                                                 |
                                    apply to a particular Card
                                                 |
                            (Card, profile ID) -> conversation
```

The template can select which profile handles automatic stage work. A direct @profile selects that profile for the user's question, independent of the current stage; it does not retag the Card or change the stage's configured automatic Agent. A profile can also be mentioned without appearing in the current workflow template: its first use on the Card creates that binding.

Templates and Boards can reference the same registry entry without sharing conversations between Cards. Conversely, two stages referencing the same profile deliberately share its context on the same Card; reference different profiles when context should be isolated. Renaming a display label should not create a different conversation identity.

Current implementation: role_instruction belongs to profiles and profile is an optional stage reference. Routing is explicit @profile -> stage.profile -> Card tags -> default_agent. Stage references pin automatic roles; legacy stages without a reference retain tag/default routing. Defaults, preservation of existing JSON and validation rules are documented in [Agent profiles](agent-profiles.md). The provider-conversation reuse lifecycle is implemented as described below.

## Simple entry points

| Action | No conversation yet | Conversation live | Conversation exited |
| --- | --- | --- | --- |
| Click Card's Agent entry | Create an interactive conversation, ready for input | Focus/attach existing terminal | Resume exact conversation and open new terminal, ready for input |
| Submit @profile question | Create conversation with question | Record comment and send question to that terminal | Resume conversation with question |
| Enter executable stage using that profile | Create conversation with stage task | Deliver stage task at the next valid invocation boundary | Resume conversation with stage task and new callback binding |

The mention picker only needs to identify a profile in the current Card. Show configuration/role names so two configurations using Codex are unambiguous. No execution-specific mention syntax, hidden choice between fresh/resume, or user-entered provider IDs is needed. The panel can show which profiles have already handled the Card and offer another configured profile when needed.

Clicking a historical execution from Timeline opens that execution's report/output. Continuing the conversation uses the Card/profile entry, which points to the current conversation and current live invocation, rather than resuming stale runtime addresses from old events.

If a conversation is closing, retain a submitted question/task until it can resume. Provider permission dialogs remain visible in the terminal; Canban does not classify arbitrary provider prompts or answer permission dialogs automatically. Uncertain delivery is shown, without blind automatic resends.

## Card and Agent panel

```text
+----------------------------------------------------------------+
| Card #42 | Review                                              |
| Agents: [Developer · Codex] [Reviewer · Codex] [+ Agent]             |
+----------------------------------------------------------------+
| Selected: Reviewer · Codex | current task: Review                          |
| [Interrupt] [End session] [Execution history]                    |
+----------------------------------------------------------------+
|                                                                |
| Native libghostty terminal                                      |
| Agent's own output, input editor and permission dialogs         |
|                                                                |
+----------------------------------------------------------------+
| Card comment: @developer explain why this approach was chosen       |
+----------------------------------------------------------------+
```

Use one AgentSessionPanel and native TerminalView in Card details and Agent activity. The Agent list can project existing request/attempt history; it does not need a new product entity for every appearance of an Agent. Show profile, current task/stage, working/needs-input/exited state and report links. The same profile's executions remain visible as history under its Card conversation.

Opening a live terminal does not start a process. Clicking an exited Agent does resume/start a process, as the label/status indicates. Read report/history is a separate read-only action. Closing the panel only detaches the view; End session explicitly ends the Agent invocation. If it ends before reporting completion, the Card does not advance.

Normal input, paste and supported in-turn steering go through the actual terminal. Interrupt asks Herdr to stop the current turn; the process remains available for more input when supported. No Canban Resume action is needed for an interrupted Agent that is still alive.

## Completion, automatic exit and continued work

Target behavior is automatic normal exit after an explicit completed report. It does not mean exit after every answer, idle observation, permission prompt or interrupt.

```text
Agent explicitly reports completed
                |
       save report + callback once
                +--------------------> Card enters configured next stage
                |
       finish response / persist history
                |
       normal provider exit
                |
       Card/profile conversation remains available for Resume
```

The reporting acknowledgment is returned first. A later Herdr root done with a later turn and fresh status timestamp establishes the boundary; Canban then applies the profile's normal-exit action through Herdr. No notify/Stop hooks are installed. An earlier ready/answer turn cannot close a later task. Exit delivery is recorded before sending, checked against the owning core, and never blindly repeated. Failure or a 30-second closing timeout is visible and does not kill the process automatically.

If shutdown fails, preserve Completed and show Closing failed with the still-live terminal. Do not pretend the history flushed or auto-start a second process into the same conversation. Workflow progression remains independent of cleanup, but a queued invocation reusing this same conversation waits for a usable invocation boundary. This is the minimum consequence of reusing provider history, not a global concurrency cap or generalized process-drain system.

Clicking an exited Agent to chat opens an input-ready conversation. It stays open while waiting for the user's question. Once the new assignment is explicitly reported completed, that invocation also exits normally. Closing its panel does not report completion.

## New invocation, same context, correct callback

A new stage/resumed follow-up gets a new request/attempt and fresh reporting environment. Do not reset the original completed request or reuse its callback. Provider history may contain old report commands; current instructions must direct the Agent to the current binding.

Stage-entry invocations carry the template callback. A user @question/click-to-chat invocation has no stage callback by default. Asking a question while a stage invocation is active is additional input to that invocation; it does not create another concurrent request or erase the stage's callback.

Provide current Card/stage/task, relevant changed Notes or report references and current reporting instructions as incremental context. Reusing conversation history does not mean all mutable Card data is already current. Historical captured context remains immutable.

Implemented starter default: Develop selects a developer configuration, Review selects a reviewer configuration. They may use the same provider but have separate conversations. Repeated questions to the same role/configuration reuse its conversation. Configuration identity determines the reuse boundary; stages describe tasks and select configurations. Do not assume a shared provider name means shared context. This keeps one Session model without a new general-purpose context subsystem.

## Persistence and lifetime

Persist one Card/profile -> provider reference binding. Each execution saves its own runtime, input, report and reference to that conversation. Reattach targets a live runtime; Resume launches a new runtime from the saved provider reference. Missing provider history requests attention rather than silently choosing another conversation. The terminal screen provides an explicit Start with retained Card context action.

The worktree remains Card-owned through Archive. The user confirmed Restore before reopening an archived Card, recreating a removed clean worktree from the retained Card ref before resuming. Provider history remains in the provider's own store; the current Canban archive does not contain a portable provider-history backup.

No global, Board or profile concurrency caps are introduced. Different Card/profile conversations run concurrently. Only duplicate starts of the same conversation are avoided; an active conversation is focused or receives input rather than being resumed twice.

## Delivery and acceptance

1. Prove provider-ID binding, interactive input/interrupt, explicit report, graceful exit and exact-ID resume with Codex on a disposable Card.
2. Build native libghostty-vt TerminalView in an NSH native host and validate input/rendering/IME/paste/selection/resize/reconnect.
3. Apply the same command/profile contract to Claude and verify its installed configuration after authentication is available.
4. Verify repeated mentions/click-to-resume of the same Card/configuration keep one provider conversation but distinct execution/report records. Developer and Reviewer configurations, including two Codex configurations, must get separate conversations. A different Card must also get another conversation.
5. Verify completion-close/new-stage races, blocked dialogs, stale reporting commands, missing history and Archive/Restore paths.

See [interactive lifecycle research](../research/interactive-agent-lifecycle.md) and [native terminal architecture](../research/native-terminal.md). These documents describe the implementation boundaries and provider-specific acceptance limits.


## 2026-11-07 interaction review: remaining product boundaries

The core direction is accepted. The following are recommended implementation defaults for discussion; this review does not mark them as new user decisions or implemented behavior.

1. **Questions during active work:** subsequently decided: immediately append/deliver to the existing Card/profile invocation. The user considered waiting for task completion and explicitly chose immediate delivery for simplicity. A question answer and original stage completion keep separate meanings. See [reply path comparison](../research/agent-reply-options.md).
2. **Editing a profile with existing conversations:** keep the existing Card/profile binding rather than silently resetting or replacing its history. The user rejected strict profile-identity/version/reset requirements in the subsequent decision below.
3. **Reopening archived or unrecoverable history:** recommend Restore before invoking an archived Card Agent. If the exact provider history is missing, show why Resume is unavailable and offer an explicit fresh conversation with retained Card context. Do not silently select another provider session or call a fresh conversation a successful resume.

Completion/exit behavior remains as already specified: explicit task completion triggers the callback and graceful close; idle/interrupt/needs-input do not. Preserve a question/draft submitted during closing, then associate it with a resumed follow-up after close rather than losing it or redirecting it to Review. This race handling and durable history after normal exit require acceptance rather than another Session type.

This historical review preceded implementation. Current acceptance evidence and remaining terminal/provider limits are recorded in implementation.md.


## 2026-11-07 user decisions and pending reply path

- **Profile edits:** apply changed configuration the next time it is loaded for execution/start/resume. Do not hot-change a running Agent. Keep the same profile ID and existing conversation association; no additional profile versioning, forced new IDs or mandatory resets. Existing queued/captured execution snapshots remain historical facts. A changed command cannot be assumed able to resume another provider's history; actual resume failure uses the ordinary visible recovery action rather than a configuration-identity gate.
- **Archive/recovery:** Restore an archived Card before resuming; if exact provider history is absent, show Resume unavailable and let the user explicitly choose a new conversation with retained Card context. Do not silently substitute another session.
- **Active @ delivery:** confirmed immediate append to the existing Card/profile invocation. No application-level wait-until-task-completion queue is selected. This does not automatically invoke interrupt or replace the current task/callback. Herdr delivery still follows its human-input quiet rule and actual CLI input handling.
- **Reply channel:** the [comparison](../research/agent-reply-options.md) recommends terminal observation plus an explicit CLI reply for durable question/answer association. Implemented: canban reply publishes a durable answer associated with the question; Herdr output remains a separate runtime observation.


## Confirmed active-mention flow

```text
User @profile question Q
          |
     persist comment Q
          |
     resolve the Card/profile conversation
          |
          +-- live ------> immediately send Q to the existing Agent
          +-- absent ----> start with Q
          +-- exited ----> resume exact conversation with Q
          +-- closing ---> preserve Q and resume after the close boundary
```

Immediate means Canban attempts delivery now, rather than waiting for the Agent's original task to finish. It does not mean bytes bypass Herdr input serialization, that the CLI necessarily consumes them immediately, or that an interrupt is sent automatically. Herdr governs input serialization. Canban does not detect every provider permission dialog; users inspect it in the terminal. Uncertain delivery is retained with attention instead of silently resending.

For a live invocation, the question keeps its comment reference and original task/callback remains active. If explicit CLI replies are used, replying to Q records the answer only; execution.report(completed) still refers to completion of the original assignment. There is no separate question Session type and no new scheduled invocation merely for appending Q to an already-active Agent.

Implemented: Card/profile reuse, live append and canban reply are verified with real Codex and the deterministic interactive fixture. An interrupted delivery becomes delivery_unknown with attention on restart rather than being resent.
