# Replies to @mentions during an active Agent invocation

Status: source-backed comparison and recommendation for discussion, 2026-11-07. The user requested comparison of explicit Canban CLI replies versus reading replies from Herdr. They subsequently selected immediate delivery of active @mentions, after considering task-completion-based scheduling. No reply interface or terminal parsing implementation is added by this research.

## The question to resolve

A development invocation can be working on the Card while a user asks a question through @developer. The same provider conversation remains active. Canban needs to distinguish a reply to that question from completion of the development assignment, and to decide whether the reply belongs only in the terminal or also in the durable Card Timeline.

Delivery timing and reply publication are separate decisions. Immediate append is now selected for active @mentions; it does not establish a structured reply signal or permission to replace/complete the original assignment.

## What Herdr actually provides

Inspected core revision c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a and the installed hc 0.1.18 schema:

- terminal.read returns text plus complete/how/why capture information. Full-screen applications may expose a stitched view or only their current screen with complete=false. This is terminal capture completeness, not a guarantee of a complete assistant message or conversation.
- terminal.attach exposes a first screen paint and subsequent terminal bytes. Bytes include terminal UI rendering, repaint/control sequences, input echoes and tool output; they are not typed assistant/user message events.
- status facts are program-reported OSC 7501 records. Their msg field is free-form status text, not a standard answer payload with a Canban question reference.
- The root turn counter increments whenever status leaves working, including interruption, blockage, clear and exit. It is not a successful-answer or task-completion receipt.
- terminal.send delivers text to a terminal. Its sent/started result concerns delivery/runtime observation, not semantic acknowledgment or a returned answer to that particular Canban question.

No inspected Herdr interface returns assistant-message records keyed by a Canban mention/comment ID. An Agent-specific integration could expose more structured output, but that is additional provider integration rather than a facility supplied by Herdr's generic terminal protocol.

## Option A: Agent explicitly replies through Canban CLI

Canban persists the user's question, assigns it an existing comment/event ID, and includes that reference and a concise reply instruction in the message sent to the active Agent. The Agent calls an application operation with that reference and its answer.

Implemented command shape:

```sh
canban reply --to COMMENT_ID --file answer.md
```

The handler resolves the Card, profile and active invocation from the saved question/execution association and injected reporting context. It stores the answer as a reply/Timeline item and returns a receipt. Repeated delivery of the same reply operation can reuse a request key. A long answer is sent as file contents through the CLI, preserving the existing long-report ergonomics.

This is a message operation, not a new Session type or a new scheduled Agent. Its receipt means the answer was stored; it does not mean the original development assignment completed.

Advantages:

- Exact question/answer association, authorship and file-backed durable content.
- Reuses the existing CLI/local RPC/AppServer/Timeline boundary across providers.
- Can record an answer while the original task continues, without invoking its stage callback or closing the process.

Limitations:

- Depends on the Agent following the instruction and having access to the CLI/endpoint.
- The Agent can answer in its terminal but forget or fail to publish a Canban reply.
- A local endpoint outage can delay publication even though the human sees an answer.

Keep delivery and reply status visible. A missing reply is not proof the Agent has not answered in its terminal. Do not invent a successful reply receipt or auto-complete the stage to compensate.

## Option B: Canban reads output from Herdr

The application displays terminal output through the native libghostty viewer. A user can read the Agent's real response without another Agent tool call. This is a strong path for the requested direct terminal interaction.

For an automatic Timeline reply, however, Canban must additionally infer where a particular assistant answer starts/ends and which outstanding question it answers. Tool logs, multiple questions, in-turn steering, redraws and partial captures make text slices ambiguous. A stopped turn does not solve this association problem.

A recorded output segment or terminal link can be attached to a question as observed execution evidence. It should be labeled terminal output rather than a precisely attributed assistant reply unless the association is established independently. Persisting every terminal byte still does not turn it into a semantic conversation protocol.

Advantages:

- Directly shows the actual Agent UI, including progress and answers outside Canban tool calls.
- Does not require an explicit reply command for the human to see a response.

Limitations:

- Generic Herdr APIs do not provide reliable assistant-message/question boundaries.
- Screen snapshots may be incomplete; a live stream must be recorded while attached and still contains rendering artifacts.
- Parsing terminal text is sensitive to CLI version, theme and UI changes.

Adding mandatory reply markers or custom OSC payloads is another explicit Agent reporting protocol. It should not be presented as obtaining structured replies from Herdr automatically. A provider-specific transcript/event adapter is also additional integration work and should be compared separately if required.

## Comparison

| Need | Explicit CLI reply | Herdr terminal read/attach |
| --- | --- | --- |
| Watch work and read a response immediately | Does not replace terminal rendering | Appropriate |
| Associate an answer with a particular @comment | Explicit reference | Not supplied by generic terminal APIs |
| Durable clean Markdown answer in Timeline | Straightforward after accepted call | Requires extraction/association beyond terminal capture |
| Works when the Agent omits the reply call | No stored reply, terminal can still show it | Human can see output if available |
| Distinguish an answer from stage completion | Separate message operation | Cannot infer it from turn/status alone |
| Integration cost | Small shared operation plus Agent instruction | Viewer is already needed; reliable semantic extraction is additional work |

## Combined boundary (selected and implemented)

```text
User @developer question Q
          |
     store comment Q
          |
     Herdr send to existing invocation
          |
     Agent works / answers
          +---- terminal stream ----> native TerminalView
          |
          +---- Canban reply(Q) ----> durable Timeline reply
          |
          +---- later report(completed)
                           |
                   original task callback + graceful exit
```

Use Herdr as the live terminal/observation channel and explicit CLI replies as the durable semantic answer channel. Do not scrape terminal text to drive workflow. This makes the two channels complementary rather than requiring either one to be the entire conversation system.

The existing execution.report operation is not a substitute for reply: completed can advance a stage and request exit in the target lifecycle. Answering Q must not accidentally complete that original invocation. A follow-up invocation whose entire task is answering Q may publish the answer and separately report its own completion; no question/stage Session classes are needed.

Question references and reply operations were implemented on 2026-11-07 and accepted with real Codex. The user selected immediate append to an active Card/profile conversation, without waiting for its original task to complete. Do not add application-level deferred scheduling for this case. The combined reply-channel boundary above is implemented, with input delivery distinct from original-task completion. Validate with an actual interactive Agent before promising that it reliably replies through the CLI or that Herdr captures a complete assistant response.

## Sources and verification boundary

- Core [terminal read implementation](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api.rs), [send/read result types](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/api/types.rs), [status/turn handling](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/status.rs) and [framed protocol/events/status](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/schema/README.md).
- Current Canban [outcome CLI](../../crates/cli/src/main.rs), [report/callback handler](../../crates/app-server/src/reports.rs) and [Timeline/report presentation](../../crates/ui/src/components.rs).

Read-only source inspection establishes API semantics. No Agent was launched and no model, reply publication, terminal extraction or new native interaction was exercised during this comparison.


## Delivery timing decision, 2026-11-07

The user considered a deferred variant: keep the @question pending, then let Scheduler run a follow-up after the current Agent completed, publishing its answer through the same CLI operation. They chose immediate append for simplicity. The deferred alternative is not selected for MVP. Both could have used the same reply operation; reply persistence did not require either scheduling choice.

Implement the timing decision by preserving the comment/question identity and delivering to the existing active invocation. Keep the original task and completion callback in place. Explicit answer publication, if adopted, records a reply without completing that task. Ordinary provider input handling and Herdr's send/blocked/uncertain-delivery behavior still apply.

## Implementation follow-up, 2026-11-07

The comparison above preceded implementation. agent.reply and canban reply --to QUESTION_ID --file PATH now persist an answer without invoking a stage callback. Real Codex and deterministic Herdr acceptance cover a live question followed by original task completion. Native TerminalView supplies the separate observation/input channel. See implementation.md for evidence and provider limits; no semantic answer parser was added to terminal.read.
