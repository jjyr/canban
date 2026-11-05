# MVP interface contracts and reporting mechanism

Status: concrete interface proposal for review, 2026-11-05. These commands, environment variables, RPC methods, and Rust boundaries are not implemented. Product decisions already confirmed by the user remain authoritative: one Session model, agent-declared completion, ordinary review reports, JSON templates, and a minimal internal event direction. This proposal does not design the post-MVP event infrastructure.

## Scope and recommendation

Architecture needs executable boundaries, not only responsibility boxes. Define the normal request/report flow now: caller, entry point, execution association, payload, acknowledgment, side effect, and failure behavior.

The [Herdr comparison](../research/agent-interface-options.md) recommends a thin Canban CLI backed by shared application operations. The current transport proposal is same-user local Unix-socket JSON-RPC for macOS MVP. Native Rust/NSH UI invokes the same handlers through typed Rust calls, without loopback RPC. The logical AppServer is co-hosted in the desktop process and can be extracted into a daemon later. Herdr-core remains a separate runtime.

```text
Agent in a core Terminal
       |
       | canban report --state completed
       v
Canban CLI
       | local JSON-RPC / Unix socket
       v
Canban report handler
       | associate execution + record agent declaration
       v
small internal Rust event
       |
       +-- workflow callback -> next stage / Scheduler
       +-- card + timeline updates
       +-- needs-attention / internal notification, if blocked

Core status subscription ------> runtime status observations / UI
```

The CLI avoids requiring different reporting plugins in Claude, Codex, or other existing runnable agents. Supply the report instruction with the task. It does not independently determine whether the agent's work is good enough.

## Boundary contracts

| Boundary | Caller -> receiver | Minimum contract |
| --- | --- | --- |
| Product command | Native UI typed Rust call or agent CLI/IPC -> Canban handler | Versioned typed operation, explicit target, normal request key/revision handling, recorded product effect |
| Agent report | Launched agent -> CLI -> report handler | Execution reference, report state, optional message/body; receipt means recorded, not next stage already finished |
| Launch | Scheduler -> core adapter -> terminal.run | Existing launch profile/template, task/context, cwd, report environment, correlation labels |
| Runtime observation | Core events/read/attach -> Canban adapter | Preserve actual terminal binding/status/output; do not infer whole-task completion from arbitrary text |
| Workflow outcome | Recorded execution report -> stored request callback | Apply configured next action after an agent declaration; no independent quality or process-exit gate |
| Product events | Application operations -> small Rust enum/channel -> interested components | Minimal publication/subscription needed by MVP; full topics/delivery framework deferred |

## Minimum application operations

The following is a proposed command boundary for the core loop. Operation schemas share normal request-key handling; updates include an expected revision where appropriate. Full optional fields can be added during implementation without changing the reporting mechanism.

| Operation | Essential input | Observable result/effect |
| --- | --- | --- |
| project.create | Name, directory, selected JSON template | Project/Board identity; create directory if missing |
| card.create | Project, title/body, tags/context references | Internal card identity, project-local number, initial history |
| card.update | Card, expected revision, changed content/tags/references | New revision and recorded update |
| card.move | Card, expected revision, target stage | Recorded stage event; configured automatic request, except no-op Backlog |
| comment.add | Card, body/resources, structured mentions | Comment identity; new request identities for actionable mentions |
| execution.report | Execution reference, state, request key, optional message/body | Recorded receipt and callback/event processing described below |
| execution.get | Execution reference | Logical state, runtime binding when known, reports/results |
| timeline.list | Card, after-sequence/cursor, limit | Ordered bounded history page and next cursor |
| card.archive | Card, expected revision | Archive/history event and managed-worktree handling; Project directory/history retained |

Native UI handlers and CLI RPC calls use these same service operations. An optional WebView bridge, if a specific component needs one, is another narrow adapter rather than the primary UI path. The existing [card.move example](../product/agent-interface.md#example-move) illustrates the command envelope. Resource upload and detailed configuration operations remain bounded follow-on schemas, not a requirement for a second application authority.

## Execution reference

Use the proposed logical execution key:

```json
{
  "request_id": "request_123",
  "attempt_number": 1
}
```

This is not another Session UUID. A scalar run_id may serialize this pair when needed. Store core's (owning core.id, local terminal.address) as the runtime binding after launch.

The report CLI reads execution context injected by Canban rather than asking the agent to copy card/terminal IDs. For the local MVP, propose:

| Environment variable | Purpose |
| --- | --- |
| CANBAN_ENDPOINT | Local Canban service socket |
| CANBAN_REQUEST_ID | Request being executed |
| CANBAN_ATTEMPT | Attempt number |
| CANBAN_CLI | Absolute path to the bundled CLI executable |

Pass them using core terminal.run's supported env field, alongside cwd/argv/template and labels. Core separately supplies HC_TERMINAL_ID; that is useful runtime metadata, not the logical request key.

These Canban variables are proposed names. They provide local execution association, not proof of a separate security identity. The MVP endpoint is same-user local IPC, not a network API or a new per-agent permission broker.

## How the agent learns to report

Scheduler composes the initial task/context with a short shared instruction, for example:

```text
Work on the supplied card task.

Report your own outcome using the provided Canban CLI:
- completed: you have finished this assigned task.
- blocked: you need a human response or cannot proceed.
- working: you are still working and want to post an update.
- failed: this execution could not complete.

Do not move the card merely to report completion.
Canban applies the workflow's configured callback after your report.
```

The agent runs the instruction using its existing command/tool capability. It does not need to know the next stage or call Herdr's UI commands. A question, development task, and review task all use this same report interface.

If the agent does not send a product report, do not invent one from silence or natural-language scraping. Keep the existing runtime observation visible; there is no automatic completed declaration. This does not add an independent completion evaluator.

## CLI examples

Proposed commands, not currently available executables:

```sh
"$CANBAN_CLI" report --state completed --message "Implemented the requested change."
"$CANBAN_CLI" report --state blocked --message "Which compatibility behavior should I preserve?"
"$CANBAN_CLI" report --state working --message "Implementation is complete; I am writing the summary."
"$CANBAN_CLI" report --state completed --file review-report.md
```

The CLI reads the injected execution reference and endpoint. For --file, it reads the UTF-8 content locally and includes it as the report body; sending only a temporary file path is insufficient. message and body are optional context, not mandatory quality/check gates.

A completed report is about this assigned execution. It is not human approval, merge, or archive. The request callback chooses the appropriate action.

## Agent report RPC

Proposed method: execution.report.

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "execution.report",
  "params": {
    "request_key": "completion-request_123-1",
    "execution": {
      "request_id": "request_123",
      "attempt_number": 1
    },
    "state": "completed",
    "message": "Implemented the requested change.",
    "body": null
  }
}
```

Required: execution reference, state, and a request key produced by the CLI. Optional: message and a Markdown body string. No required test result, commit, artifact-quality proof, or observed process exit.

| State | Meaning | Product action |
| --- | --- | --- |
| working | Not completed; progress/update | Record/update runtime/task presentation; no completed callback |
| completed | Agent declares its assigned task finished | Finish logical work and invoke its stored on-completed binding |
| blocked | Agent needs input/cannot proceed | Mark card needs attention and produce internal notification event |
| failed | Agent declares execution unsuccessful | Record failure and make it visible; proposed default also requests attention; no completed callback |

The state vocabulary is an interface proposal, not a new core enum or Session type.

Illustrative acknowledgment:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "accepted": true,
    "duplicate": false,
    "execution": {
      "request_id": "request_123",
      "attempt_number": 1
    },
    "recorded_state": "completed",
    "workflow_action": "pending"
  }
}
```

accepted means the service recorded the report. workflow_action pending means the configured callback may still be processed. It does not claim that Review has started or finished. The UI observes subsequent application events/state.

## Normal report handling

1. CLI resolves endpoint/execution from its launch context and builds a report request.
2. Handler checks only ordinary protocol/association requirements: parseable supported state and an existing known execution. Do not inspect code/test quality or wait for the old process.
3. Record the declaration and timeline association; apply ordinary duplicate handling.
4. Publish the minimal execution-reported application event.
5. Workflow uses that execution's request callback. Card/timeline/notification consumers handle their relevant state.
6. Return an accepted receipt for the recorded declaration; subsequent callback/start failures are visible through card/notification state.

For default development, on-completed moves to Review and schedules the ordinary report task. For a submitted @mention, its callback posts the response against the initiating comment. The same Session/report implementation serves both.

This is a normal-path interface contract. Exact ordering/transaction details between storage and the internal memory channel belong to implementation and the deferred detailed event design; do not turn this into a required durable event-bus/outbox project.

## Replay and failures

- A completed declaration is handled once for the same execution. Repeating it does not create another Review transition.
- JSON-RPC id correlates a response; request_key identifies a submitted operation. The CLI retains that key for retries of the same submission.
- A normal completed command can derive a stable completion key from the execution reference. Progress/blocked submissions can have individual keys. None of these is another Session identity.
- If acknowledgment is lost, retry the report against the same execution; do not launch a new Session or rerun the task just to report.
- Reusing a request key with a different payload returns a structured request-key conflict rather than silently changing recorded history.
- If endpoint/context is absent, CLI returns a nonzero result with a useful error; do not guess the focused card or a default execution.
- Unknown execution and invalid state return structured errors such as execution_not_found and invalid_report.
- CLI success means accepted/already accepted, not successful code verification. The service's recorded card state exposes later workflow/start problems.
- Exceptional old execution continuing after logical completion remains deliberately outside MVP coordination scope.

CLI transport/service errors and business follow-up errors are different. A failed callback after a recorded report should create card attention/notification rather than falsely telling the agent its original report never existed.

## Core adapter contract

Scheduler prepares one launch specification with selected existing agent/template, task/context/report instruction, cwd, env, and labels. The adapter translates it into core terminal.run, records returned terminal identity and initial-message delivery outcome, and subscribes to terminal events as needed.

If core did not deliver the initial task or launch failed, show the situation as requiring attention; do not wait as if an informed agent were working. Exact provider-specific startup behavior is inherited from the existing runnable configuration and still requires runtime validation.

Core status reports through OSC 7501 already expose working/done/blocked/error observations. They are useful for presentation and blockage/attention integration. They are not a universal business-result body, provider conversation ID, or permission to treat every agent turn end as completion of the whole assigned card task. Explicit Canban reporting makes the product contract uniform without scraping terminal prose.

Sources: core [run parameters](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api/types.rs#L198-L285), [injected terminal environment](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api/run.rs#L211-L239), [status reports](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/status.rs#L12-L63), [template readiness/delivery](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/docs/templates.md).

## What is decided now versus later

Proposed now: common report entry point, launch context injection, local CLI/service path, minimal payload/state/receipt, normal replay behavior, request-bound completion action, and core adapter responsibilities.

Deferred: full application event enum, topic hierarchy, channel library, subscriber lifecycle, global delivery/replay guarantees, distributed reporting, provider-specific integrations, template UI, and old-process coordination. The [event direction](../product/events-and-notifications.md) remains deliberately small.

Before claiming the mechanism works, implement and validate a harmless fixture calling the CLI, one existing runnable agent following the instruction, blocked notification, default completion-to-Review, and an acknowledgment retry. No such implementation or runtime validation has been performed.
