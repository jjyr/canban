# Interactive Agent execution, exit and resume

Status: original technical research (2026-11-06), followed by implementation and real Codex/native-terminal acceptance on 2026-11-07. The historical investigation below retains its original boundaries; [implementation](../implementation.md) is authoritative for current behavior. Codex uses notify plus CODEX_THREAD_ID binding, not an untrusted SessionStart hook. Claude launch is verified but live model/report/Stop/resume acceptance is blocked by local Invalid API key.

## User direction and recommendation

Launch an interactive Agent inside a Herdr terminal for each assignment. While it runs, expose the actual terminal so the user can type, steer the work, answer permission dialogs and interrupt. Once the Agent explicitly reports completion, close that invocation normally. Later, resume the saved provider conversation in a new Herdr terminal.

Use the existing Scheduler/request/attempt path. Add a small provider launch/resume/lifecycle adapter and a reusable terminal component; do not add another scheduler, Session class, concurrency limiter, or custom chat protocol. Latest proposal: resolve (Card, Agent configuration) to a conversation. First use creates it; stage assignments, clicks and @mentions reuse it while keeping separate request/attempt callback bindings. Developer and Reviewer configurations can use the same provider but keep different context. This supersedes the earlier fresh-stage/fresh-mention rule and execution-specific mention-picker proposal.

## Three identities

```text
Card
  |
  +-- Develop request D1 / attempt 1
  |      +-- provider conversation: codex / S1
  |      +-- runtime: core C / terminal t1 -> exited
  |
  +-- user Resume request F1 / attempt 1 (resume_from D1/1)
  |      +-- provider conversation: codex / S1       SAME history
  |      +-- runtime: core C / terminal t5          NEW process
  |      +-- on_completed: null                    no replay of D1 callback
  |
  +-- Review request R1 / attempt 1
         +-- provider conversation: codex / S2       FRESH history
         +-- runtime: core C / terminal t2
```

- The Canban request/attempt identifies one invocation and its outcome/callback. Keep the existing key.
- The provider conversation ID identifies resumable Agent history. Save provider, exact ID and the provider configuration/storage location needed to resolve it. The ID is opaque; avoid requiring every provider to use UUIDs.
- The owning core ID and terminal address identify a live PTY. Resume creates a new terminal binding; reattach to a surviving terminal does not create a new Agent invocation.

An Agent configuration is a named identity/role, not merely a provider/model name. The proposed developer/reviewer split is under discussion. A provider conversation can have several sequential Canban invocations. No extra synthetic Session UUID or question/stage Session types are required. Persist the Card/configuration -> provider-reference association, with request/attempt history and optional resume_from causality. Avoid simultaneously resuming the same provider conversation: when its invocation is still live, Open terminal focuses it. This is a duplicate-conversation guard, not a cross-Board concurrency cap.

## Herdr boundary: no provider-neutral resume API

The installed hc schema was checked again on 2026-11-06: no method name contains resume. The inspected core method table exposes terminal.run/attach/send/interrupt/keys/kill/keep, templates, events and documents, but no provider conversation registry or Agent completion-close policy.

- terminal.attach reconnects a viewer to a live terminal. Its resumed parameter concerns attachment behavior, not a provider conversation.
- terminal.keep restores a lost terminal as a shell; it does not rerun the original Agent command or restore model history.
- terminal.run executes whatever fresh/resume argv Canban prepares. Templates can package such commands but do not supply session discovery or a universal provider ID.

Canban provides the unified product operation: resolve Card participation -> exact provider reference -> reuse live terminal or build resume argv -> terminal.run. Codex/Claude retain and restore their own histories. This requires small command/lifecycle adapters, not an implementation of those providers' conversation engines.

## Confirmed command capabilities

Inspected installed help: Codex CLI 0.166.0; Claude Code 1.0.108. Claude help/version were read using a disposable CLAUDE_CONFIG_DIR because even those commands attempted to write its normal configuration. No authentication was copied or changed and no model request was made.

| Agent | Fresh interactive launch | Explicit resume | Conversation binding |
| --- | --- | --- | --- |
| Codex | codex with a positional prompt, without exec | codex resume <session-id> [prompt] | Official SessionStart hook input has session_id; validate scoped hook loading/trust on the installed version |
| Claude | claude with a positional prompt, without -p | claude --resume <session-id> [prompt] | Installed --session-id accepts a supplied UUID; SessionStart can confirm the running ID |

Codex interactive sandbox/add-dir/cd flags can preserve the current worktree and writable Canban reporting home. Do not carry exec-only flags, such as skip-git-repo-check, into an interactive launch without checking their support. Keep existing authentication, model selection, sandboxing and approval behavior. Startup trust/permission dialogs are visible states for the user to answer in the terminal.

Use explicit saved IDs. Never use codex resume --last, claude --continue, most-recent files or directory timestamps to associate a Card with a conversation. Multiple Boards and the user's independent Agents may share provider history storage.

### Hook and binding boundaries

Official Codex hooks expose session_id and SessionStart sources including startup/resume. Non-managed hooks require review/trust; skipped hooks cannot be assumed to have installed a binding. Transcript formats are explicitly not stable APIs. Use an explicit hook/launch handshake instead of scraping rendered terminal text or private provider databases.

Current Claude documentation describes session selection and lifecycle hooks. The installed 1.0.108 help independently confirms resume, session-id and settings flags, and its installed code contains SessionStart, Stop, SessionEnd and the /exit command. Newer documented fields or environment variables must not be assumed present in that older binary.

A minimal Canban callback would bind the current launch to a provider ID, using the injected request/attempt environment. It must reject a binding for another invocation and ignore child-Agent lifecycle events. Capture root turn identifiers where the provider supplies them. Expose a missing binding as Resume unavailable rather than guessing. If the user switches provider conversations inside the CLI, follow the verified root lifecycle binding or mark the saved reference stale; do not leave Resume pointing at a previously displayed conversation. Use per-launch configuration and ordinary provider hook trust; do not silently overwrite global user hooks or disable trust checks.

## Completion, turn stopping and process exit

```text
interactive Agent works <------ user terminal input / interrupt
         |
         +-- idle / question / permission dialog -> keep terminal available
         |
         +-- explicit completed report
                      |
           AppServer stores report + callback once
                      |
           request graceful close for THIS invocation
                      |
           root turn settles / provider reaches input boundary
                      |
           normal provider exit; preserve provider history
                      |
           observe Herdr finished + keep Canban report/output references
```

The explicit report remains authoritative. Ordinary Stop/idle events are observations of a turn, not a task-completion declaration. Interrupt does not complete a Card. Blocked work remains available for human input; resubmission can later report working/completed under that invocation's reporting contract.

Do not kill the process from inside the reporting CLI: the CLI may run as an Agent tool before the final response and provider persistence have finished. Return the report acknowledgment first. A lifecycle worker can request normal exit after the root turn settles, using the provider's supported quit action. Both investigated CLIs expose /exit, but safe automated delivery at the correct input boundary still needs a prototype. A Stop hook is not by itself proof that the TUI is ready, that every hook finished, or that its transcript has flushed. Hooks should signal the worker and return, not synchronously wait for their own parent to exit.

Normal exit is the default cleanup path. If it fails, show Completed / closing failed and let the user inspect or explicitly terminate. Do not label an interrupt/kill as a graceful close or claim history was saved without verification. Preserve final output/report references before Herdr's bounded history expires.

Workflow progression still happens after accepted completion and need not wait for terminal shutdown. This preserves the user's existing MVP simplification: closing the old process is cleanup, not a new workflow barrier. A user resuming that conversation before its previous process exits should first focus the live invocation or wait for the close result.

### Follow-up invocation

Resume creates a new Canban request, captures current follow-up input and report instructions, reuses the exact provider conversation, and launches a new interactive terminal. The original completed request/report stays immutable. Inject the new request/attempt/endpoint into the resumed process and explicitly tell the Agent to use that current binding; old conversation history may contain obsolete reporting commands.

Default on_completed is null. Asking the earlier development Agent why it made a change must not rerun Develop -> Review or move the Card backwards. If the user intends another development cycle, explicitly move the Card to Develop under the workflow. This is a callback difference, not a second Session model.

Separate Developer and Reviewer conversations can share the same Card worktree. Their separate terminal identities do not isolate file writes; keep the Card/stage context visible and retain the accepted MVP policy without introducing writer leases.

Resumption restores conversation history, not an old filesystem snapshot. Reuse the Card worktree where it still exists, and include current stage/relevant new reports as a small continuation context. Provider account/profile and history storage must remain compatible. If the provider history is absent, offer a clearly labeled fresh conversation with Canban context; it is not a successful Resume.

## Terminal integration

herdr-core is a terminal runtime. It does not provide an embeddable NSH terminal widget. The current read-only terminal.read snapshot cannot display or control an interactive TUI faithfully.

```text
Rust / NSH: Board, Card, Timeline, Agent list, native action toolbar
                              |
                 reusable AgentSessionPanel
                  +-----------+--------------------+
                  |                                |
             AppServer actions              Native TerminalView
            start / resume / interrupt       (NSH host + libghostty-vt)
                  |                                |
                  +----- Rust terminal controller--+
                                   |
                        persistent hc connect
                        hello { framed: true }
                                   |
                         herdr-core terminal.attach
                                   |
                                holder / PTY
                                   |
                           interactive Agent CLI
```

Normal keyboard/paste input uses the attached stream, preserving the Agent's own input editor, keybindings and dialogs. A native Interrupt action uses terminal.interrupt and displays its observed result. terminal.send is useful for a deliberate programmatic message or initial template task; it should not be called per keystroke. Closing the terminal view detaches the viewer and does not end the Agent. Closing the Agent is a separate action.

The framed transport needs a persistent reader/writer, request IDs on channel 0, per-terminal stream routing, input/resize/focus, bounded buffering, credit acknowledgments and close/reconnect handling. The present one-shot client kills hc connect after a reply, so it cannot own an attached stream. Existing control calls can remain simple while a streaming client is added in canban-herdr.

Herdr returns an initial authoritative screen paint, then bytes. Reattach after connection loss to the same surviving binding; do not launch/resume on an uncertain disconnect. Core events can resynchronize runtime state. Check core identity before routing controls to a saved address.

### Selected native renderer

The user selected libghostty. The concrete reuse target is libghostty-vt, as used by the inspected Herdr app: terminal emulation, screen state and key/mouse/paste encoding behind a C API. It is not an embeddable NSH UI or another PTY owner. Herdr continues to own the process/PTY; Canban owns the native viewer.

Use one reusable native TerminalView for AgentSessionPanel in Card details and Agent activity. Adapt Herdr's FFI/VT logic where suitable; port the presentation to an NSH-registered native macOS component. Drawing, text shaping/fonts, cursor, selection, scrolling, clipboard, focus and NSTextInputClient/IME integration remain implementation work. NSH native-component registration is a viable host seam; its existence is not terminal acceptance.

The pinned Herdr app uses ghostty submodule 291fd050b9e9c293c8adb6a140742649b7e42d3a and a Rust ghostty-sys wrapper that builds libghostty-vt with Zig (its build script requires 0.16) and generates C bindings with bindgen. Pin compatible source/API/build versions rather than combining current upstream headers with that historical wrapper. Its VT wrapper also references Herdr settings, links, sprites and key helpers; extract deliberate dependencies rather than copying it unchanged. Preserve Apache-2.0 notices and Ghostty's applicable upstream/third-party licenses when importing.

For the first native slice, establish output/input/resize and TUI behavior, then IME/paste/selection/scroll/reconnect using the same component. Credit is returned after bytes have been safely consumed by the terminal model, with bounded rendering queues. Distinguish generated terminal replies from human input using negotiated automatic_input support. Raw output/keystrokes are not SQLite Timeline events. See [native terminal architecture](native-terminal.md) for ownership and the proposed crate.

The previous WebView/xterm.js recommendation is superseded; no new WebView terminal dependency is proposed. Full Ghostty-application embedding is not assumed necessary or compatible with an externally owned Herdr PTY.

## Working directory and Archive

Agent exit does not remove its worktree. Preserve the existing through-Archive lifetime. A follow-up while the Card is active can reuse that directory.

For the first implementation, recommend Restore before resuming an archived Card. Restore alone currently restores Card-owned files; it does not reconstruct a removed clean worktree. Add recreation from refs/canban/cards/<card-id> at the expected path, then use an explicit provider cwd override where supported. Do not silently resume into the source repository or a different HEAD. Retained dirty worktrees need no recreation.

A Canban archive currently includes its context/reports and runtime references, not the provider's entire conversation store. Saving the exact provider ID enables resume only while that provider history is available. Offline/portable provider-history archival is a separate scope decision; do not promise it based on the current tar+zstd package.

## Small implementation boundary

- canban-types/storage: optional provider reference, resume_from and close-request/result metadata. Preserve older records with absent fields.
- canban-scheduler: fresh/resume launch intent through the same queue, with no concurrency caps.
- canban-herdr: persistent framed transport, attachment and terminal controls.
- canban-app-server: bind provider ID, resume, interrupt and request close; existing report transaction/callback remains authoritative.
- canban-workspaces: reuse/recreate the retained Card checkout for resume.
- canban-terminal (proposed crates/terminal): libghostty-vt wrapper, terminal model, native NSH host and input/rendering; no Card/workflow/provider dependencies.
- canban-ui: Card/configuration participation list and role-labeled mention picker, AgentSessionPanel and terminal controller; existing UI Kit owns shared actions/status styles.

Start with small modules for Codex and Claude command/binding/exit differences, rather than a general plugin framework. Custom command profiles can remain launch-only until their explicit resume mechanism is known.

## Acceptance before changing launch defaults

1. Launch an interactive Codex on a disposable Card/worktree and obtain the exact provider ID without output scraping.
2. Attach inside NSH, type supplemental instructions while it works, answer a dialog, interrupt, and continue. Verify visible output, IME/paste, focus and resizing.
3. Agent explicitly reports completion; verify exactly one workflow callback, final response retention and normal process exit. Stop/idle alone must not close it or advance the Card.
4. Resume that ID in a new terminal with the new report binding. Verify it recalls task-specific facts, uses the intended worktree, and does not replay the original callback.
5. Repeat with another Board active and double-click Resume; ensure routing selects the intended conversation and does not create duplicate live resumptions.
6. Repeat the full lifecycle for the installed Claude configuration. Newer official documentation is not proof of compatibility with 1.0.108.
7. Check lost attachment, app restart, missing provider history and archived clean/dirty worktrees. These should expose a clear recovery action, without guessing or fresh duplicate launches.

This research did not launch an interactive Agent, make a model call, create hooks, change provider configuration, or implement a renderer. Graceful automatic close, hook trust/onboarding and full resume are the most important unproven pieces.

## Sources

- Official OpenAI [CLI commands and interactive shortcuts](https://learn.chatgpt.com/docs/developer-commands) and [hooks](https://learn.chatgpt.com/docs/hooks), fetched 2026-11-06; installed Codex 0.166.0 help independently checked.
- Official Claude [CLI reference](https://code.claude.com/docs/en/cli-reference) and [hook reference](https://code.claude.com/docs/en/hooks), fetched 2026-11-06; installed 1.0.108 help and bounded installed-code inspection establish the local version boundary.
- Herdr core [transport/attach/events](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/schema/README.md), [templates](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/docs/templates.md), and [status handling](https://github.com/herdrdev/herdr-core/blob/c6b316f87c33e6f2ab1a1ceea3e3461e2b6bb61a/crates/herdr-core/src/status.rs).
- Herdr app [framed client](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/hc.rs), [GPUI terminal](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/terminal.rs), [VT wrapper](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/src/vt.rs), and [dependencies/license](https://github.com/herdrdev/herdr-app/blob/e77f112ac2ea136b84fe510efa42d41163bfd776/Cargo.toml), inspected local pinned snapshots.
- NSH [native component registration](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/docs/dev/embedding.md) and [macOS native component API](https://github.com/longbridge/nsh/blob/b7a69186d3c20a59690f2ac834dc527bee7f8438/crates/nsh-platform-macos/src/native.rs).
- Current Canban [report transaction](../../crates/app-server/src/reports.rs), [one-shot core client](../../crates/herdr/src/lib.rs), and [worktree lifetime](../../crates/workspaces/src/lib.rs).

## Implemented Codex lifecycle mechanism

Official [advanced configuration: notifications](https://learn.chatgpt.com/docs/config-file/config-advanced#notifications) documents notify with agent-turn-complete JSON including thread-id and last-assistant-message. The adapter injects an invocation-local notify command; root tool environment CODEX_THREAD_ID binds the provider ID before work, and notify records the final response/turn boundary. This avoids depending on non-managed Codex hook trust or private transcript formats. A completed report clears earlier turn timestamps; only a later turn-end permits normal /exit. Real-model acceptance confirms saved history remains resumable. Claude uses an invocation-local --settings Stop hook with a preallocated --session-id; its real lifecycle remains pending valid credentials.
