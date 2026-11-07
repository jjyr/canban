# Conversations, requests and attempts

Status: implemented 2026-11-07. This replaces the earlier fresh-conversation-per-mention proposal. See [Agent interaction](agent-interaction.md) for the accepted product behavior.

There is one execution model. Scheduling triggers differ; there are no question/stage session classes.

```text
Project -> Card -> profile conversation (Card ID + profile ID)
                     |       provider + exact provider_session_id
                     |
                     +-> Request (task, profile snapshot, callback)
                            +-> Attempt (request ID + number)
                                  core.id + terminal.address
                                  captured input / replies / report / output
```

A live @profile comment becomes a durable Question associated with its comment and current request/attempt. It is sent immediately through Herdr. canban reply records the answer and Timeline event without completing the original request. An absent/exited profile starts/resumes through the same Scheduler; its follow-up request has no stage callback.

Stage entry queues a request with the template's task and completion callback. If the same Card/profile is still live, the queued stage waits for that conversation boundary. Other Cards/profiles run concurrently, with no global, Board or profile capacity cap. Developer/Reviewer profiles have independent conversations even if both use Codex.

Clicking a Card Agent focuses its queued/live request; an exited conversation gets a new request/attempt/terminal using the exact saved provider ID. Repeated opens do not duplicate a live invocation. Provider/session bindings cannot be shared between different Cards/profiles.

```text
explicit completed report -> save outcome + apply callback once
                          -> wait for a later relevant Herdr root done
                          -> apply the configured normal-exit action -> observe exited
                          -> retain provider binding for Resume
```

Turn-end, idle, a reply and process exit are not completion. Exit without a report needs attention. A close failure/timeout remains inspectable; Canban does not automatically kill it. Interrupt targets the live PTY; End session explicitly kills it. Closing the terminal screen only detaches its viewer. Core identity is checked before sending input/control/exit.

SQLite WAL stores Conversation and Question metadata alongside requests/attempts. Long captured context, replies, reports and retained terminal output are Card-owned files. Historical final-response snapshots remain readable; the current lifecycle does not capture provider callback payloads. Archive compresses these files and record exports but does not back up provider-owned histories. Restore is required before invoking an archived Card; a removed clean worktree can be recreated from its retained Card ref. Missing provider history fails visibly; Start with retained Card context is an explicit fresh-context recovery action.

AppServer restart never blindly replays a launch without its receipt, an uncertain question delivery or a physical terminal control intent. These remain unknown/attention for inspection. Provider lifecycle/terminal acceptance limits are in [implementation](../implementation.md).
