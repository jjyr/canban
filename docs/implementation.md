# MVP implementation and development

[Compact navigation and Quick Open](quick-open-and-navigation.md) covers the implemented icon rail, Command+O fuzzy destination picker and native interaction checks.

[Agent Overview and lifecycle](agent-overview-and-lifecycle.md) documents the implemented status-aware execution budget, configurable resumable idle exit and global attention/Agent workspace.

Status: local macOS MVP updated 2026-11-07. This is the current implementation reference; earlier design documents contain broader proposals and dated pre-implementation statements.

## Build and launch

Use the pinned mise Rust toolchain. NSH 0.4.0 is pinned to aca7b8f92a33af9d83c3693a516f649f7ea5c5e5 in Cargo.toml and Cargo.lock. Install macOS Xcode command-line tools and a compatible hc separately. This implementation was verified with hc 0.1.18 and Codex CLI 0.166.0.

```sh
mise run build
mise run run
```

Build creates target/debug/canban, target/debug/canban-ui and a local .local/Canban.app containing both executables. The development package is unsigned and has not been notarized or released. The Gallery is a separate development example: `mise run gallery` builds/opens `.local/CanbanGallery.app`. It is not part of the production window or its startup modes; it uses local fixtures without starting AppServer or reading application data. See [examples](../examples/README.md). Launch one instance per application home; a second AppServer is rejected while the first owns its socket.

CANBAN_HOME selects an alternate absolute application-data directory. Its default is ~/.canban/. Native UI calls the co-hosted AppServer directly through background operations. A same-user, mode-0600 Unix socket exposes JSON-RPC to the CLI. The home directory is mode 0700.

```text
canban-ui -> product components/presenters -> AppServer
     |                                         |
canban-ui-kit -> nsh-base                 owner crates / storage
                                               |
                         +---------------------+--------------------+
                         |                     |                    |
                    SQLite WAL             managed files      hc connect
                                                              JSON-RPC
                                                                  |
                                                             herdr-core
```

The UI process hosts scheduling and the endpoint. Quit or closing the standalone window stops those functions (the MVP uses NSH's default keep_alive_on_close=false); Herdr-owned programs may continue. An Agent cannot report while AppServer is absent and must retry its CLI submission after the app returns. Launching a second UI is not an attach-to-existing-server mode. The CLI's serve command is an alternative headless host using the same operations, not an additional production daemon.

## Configuration

First startup creates config/agents.json and config/workflows/default.json. Installed hc/codex/claude paths are resolved when available so Finder launch is less dependent on PATH. Edit JSON and choose Reload agent configuration in Settings, or call config.reload. Invalid configuration leaves the previous in-memory configuration active. Stage profile IDs are validated at startup, Project creation and configuration reload against the global registry; removing a profile referenced by the default/saved Project workflow is rejected. Requests already queued keep their profile/instruction/callback snapshot.

Fresh homes receive developer and reviewer starter profiles (both initially using the existing Codex launch setup), plus codex/claude profiles for direct mentions. The default workflow references developer from Develop and reviewer from Review. Existing registries/workflow files and saved Project workflows are preserved without injecting or rewriting roles. If a workflow file is missing under an older registry, generated stages omit references to unavailable starter profiles and keep legacy tag/default routing.

The default workflow is Backlog -> Develop -> Review. Backlog does nothing; Develop completion enters Review and queues its ordinary review-report task. Review completion awaits human disposition. New Projects load the current default workflow JSON; existing Projects retain their saved workflow. Template replacement/migration UI is deferred.

Agent configuration has hc, hc_session, hc_env, default_agent and profiles. Profiles have id, optional preset, role_instruction, optional argv/resume_argv, exit, session_instruction and tags. canban-profiles resolves editable JSON defaults and explicit whole-field overrides. Commands expand {prompt}, {home}, {cli} and {session_id} as individual arguments. The runtime has no provider-specific invocation branches or Herdr launch-template mode. See [profile schema and migration](product/agent-profiles.md).

Codex and Claude presets configure interactive launch, exact-ID Resume and normal /exit input. Neither injects notify/Stop hooks. Codex binds its actual tool-environment ID through bind-session; Claude's fresh command receives a preallocated UUID. Business completion requires an explicit Canban report. Automatic exit additionally requires a later relevant Herdr root done, after the report; idle/blocked/error/turn increments do not suffice. A provider without usable status keeps manual terminal control. Legacy config conversion writes an exact-byte backup before publication; historical Requests/conversations are preserved. Opaque legacy launch templates require explicit command conversion.
Routing precedence is explicit @profile -> stage.profile -> Card tags -> default_agent. A named stage profile pins the automatic role; Card tags only route stages that omit profile. Tagged legacy work must match a configured profile; unmatched or ambiguous routing requests attention rather than starting an unrelated Agent. A submitted @profile mention immediately appends a Question to the live Card/profile invocation, or starts/resumes it if absent/exited. Stage-entry execution queues a request using that same provider conversation and a new callback binding. Retry is explicit and uses a new attempt; reconnect does not create one.

Boards share one Scheduler with a configurable global Agent limit (default eight). Live/starting/closing and uncertain invocations reserve slots; queued work waits. Later template stages run first, with FIFO ties. Automatic requests and progression pause on persisted human gates until an explicit user decision. Settings and JSON share the same configuration. See [workspace/scheduling implementation and acceptance](workspace-and-scheduling.md); earlier unlimited-concurrency evidence describes the previous policy.

## Product behavior

- Create a Project in an existing or new absolute directory; Git is optional. Each Project has a Board and increasing local Card numbers.
- Create/edit Cards, route with tags, move with stage buttons or the Board's dedicated drag handle, and inspect current execution/attention status.
- Create/read/write/rename SQLite Markdown Notes with native multiline editing/preview, saved revisions, links/backlinks and related Card/Agent Timeline. Create a scoped Backlog Card through an editable Note-to-Card preview. Card context controls select which Project Notes enter new execution snapshots; default is all Project Notes. Saved captures do not follow subsequent Note edits.
- Comment and mention configured Agents. Reports and state changes join the same ordered Timeline. Pasted images in Card/comment editors save bytes as Card resources and insert an attachment reference into the draft; save/post the draft to retain that text reference. Resources also support file-path import and native opening.
- AppServer returns structured errors for stale Card/Note revisions. A failed save remains visible in the status area. In-flight UI operations do not block render/input paths; stale resource-load results cannot replace a newly selected Card's output. Incoming state updates do not rebuild active Card editor drafts.
- Card Agents and Agent activity open the shared native terminal screen. Users type/paste, interrupt, end, detach or resume exact provider history. libghostty-vt owns VT state/input encoding; AppKit renders through an NSH native component; Herdr owns the PTY. Runtime state never implies business completion.
- The Agent's explicit working/completed/blocked/failed declaration is recorded through the CLI. Completed invokes only its originating request's callback. Blocked/failed creates a durable attention notice. Completion receipts and commands with request keys have duplicate/conflict handling.
- For a Git repository with HEAD, task execution creates a detached managed worktree and reuses it across stages. Non-Git/unborn repositories use their Project directory. Uncommitted source-directory edits are not silently copied into a new worktree. Archive retains dirty worktrees with a notice; clean worktrees are removed after preserving their HEAD under refs/canban/cards/<card-id>.
- Review offers a native basic Diff and explicit commit/merge into a clean target checkout. Stale previews and active/queued Card work block acceptance. Conflicts remain in the Card checkout, with durable failure/attention receipts. Accept does not archive. See [Review delivery](product/review-delivery.md).
- Archive creates a checksummed zstd-compressed tar package with manifest.json, records.json and Card-owned files. It verifies the package before publishing its index and removing matching expanded files. Report/attachment/output access can resolve compressed content on demand. Restore expands files and retains identity without silently scheduling work.

Structured database records remain online in this MVP for lookup, history, references and report deduplication, even though a complete Card record export is also compressed in the archive. This is an intentional refinement of the earlier proposed SQL-detail pruning. Large file bytes are compacted; SQL cold-storage pruning/space reclamation and lz4 encoding are later work. Late accepted reports remain durable outside an existing sealed package.

## Agent interface

While the UI or headless host is running:

```sh
target/debug/canban status
target/debug/canban snapshot
target/debug/canban call project.create '{"name":"Example","directory":"/absolute/path/to/source"}'
target/debug/canban call card.create '{"project_id":"PROJECT_ID","title":"Task","body":"Intent and context"}'
target/debug/canban call card.move '{"card_id":"CARD_ID","revision":1,"stage":"develop"}'
```

See [Agent profile configuration](product/agent-profiles.md) for loadable JSON examples, schema, routing precedence and backward compatibility. Settings displays the shared registry and roles, Board headers display stage profile references, and the Card composer offers Ask @profile actions.

Public operations include app.ping/snapshot, project.create/list, card.create/get/list/update/move/context.update/archive/restore/attention.clear/continue/workspace, comment.add, note.list/get/read/save/write/rename/history/revision, timeline.list, execution.get/report/retry/output, attachment.add, resource.read/materialize, config.get/reload, scheduler.status/configure, herdr.status/templates, profile.presets, review.preview/merge and agent.open/bind/reply/interrupt/end. agent.turn_ended is a deprecated ignored compatibility endpoint. For timeline.list supply card_id, optional after_sequence and limit; next_cursor provides pagination. Current code and shared types are authoritative for exact payloads; the [interface design](interfaces/mvp-contract.md) remains a broader contract reference.

The launcher injects CANBAN_CLI, CANBAN_HOME, CANBAN_ENDPOINT, CANBAN_REQUEST_ID, CANBAN_ATTEMPT, CANBAN_SESSION_ID, CANBAN_PROFILE_ID and CANBAN_RUN_DIR and supplies reporting instructions in the actual prompt.

```sh
"$CANBAN_CLI" report --state working --message "Investigating"
"$CANBAN_CLI" report --state completed --message "Finished" --file "$CANBAN_RUN_DIR/report.md"
"$CANBAN_CLI" report --state blocked --message "Need a human decision"
```

The CLI sends file contents, not just a temporary path. Completed gets a stable default request key for receipt retry; changing the payload under that key is a conflict. Pass --request-key for an explicit operation identity where needed. Terminal addresses bind to owning core.id; Conversation stores the distinct exact provider session ID. Runtime observations omit argv/env copies of potentially long prompts.

## Verification performed

```sh
mise run test
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise run acceptance
mise run acceptance-interactive
# Explicitly opt in to real-model usage:
mise run acceptance-real-interactive
mise run acceptance-real-workflows
# Deliver that real-workflow output without another model call:
mise exec -- python3 scripts/acceptance-delivery.py /private/tmp/cb-real-workflows-BASE
```

The acceptance script creates a fresh temporary Canban home and isolated HC_SESSION/HC_SOCKET/state/config. It starts a deterministic fixture program through the real installed Herdr core and PTY, and checks queued launch, work product, captured profile role instructions, distinct template-selected Developer/Reviewer executions, explicit CLI outcomes, automatic Develop -> Review, fresh mention, blocked notice, WAL, report compression/read/restore and restart without duplicate scheduling. It stops only that scratch core. It does not call a paid model.

Separately, an existing Codex 0.166.0 Agent was launched through the isolated core with normal workspace-write permissions. It read the captured context and invoked the bundled CLI with a Markdown report; Canban recorded completed. This establishes the actual provider/report path, beyond fixture-only success. It does not establish live acceptance for every installed Agent or template.

Subsequent live acceptance also completed two real coding tasks concurrently on separate Boards, automatically launched two fresh Codex review Sessions, and archived both Cards after independent validation. Four real Sessions and their report/context/worktree associations were checked; reports remained readable after compression. Run mise run acceptance-real explicitly to reproduce. The [evidence summary](real-agent-acceptance.md) distinguishes this from fixture acceptance and from cross-provider/security isolation claims.

Native Canban.app interaction checks covered Card creation, multiline Unicode paste/edit/save and preview, comment mention, cross-stage drag, Notes save/preview, light/dark Gallery rendering, keyboard Tab/Space activation and skipping disabled controls, copying an image in Preview then pasting its bytes into a Card, opening the retained attachment in Preview, per-Card Note checkbox selection, and closing/relaunching the standalone window. Image references/resources persisted and active editor text survived the resulting state refresh. These were real native interactions, not a browser mockup or screenshots alone.

Rust regression coverage includes state/history/request consistency and duplicate operations/reports, stale revisions, fresh mention callback isolation, blocking notices, routing failures, compressed resource restore, path isolation, workflow cycle rejection, large Unix-socket JSON frames, selected Note snapshots, and managed-worktree reuse/dirty retention/commit preservation. The large-frame case caught and prevents a macOS nonblocking accepted-socket truncation bug found during live integration.

## Remaining boundaries

This is the local macOS MVP with interactive Card/profile conversations. Shear review, GitHub backends, native notifications, template designer/migration UI, advanced timers, multi-device sync and release distribution remain deferred. Claude's preset is implemented and configuration-tested, but live Claude acceptance has not been established. Provider startup/trust dialogs remain interactive; Canban does not automatically authorize them. Notes use native Markdown input/preview. Terminal presentation does not yet implement mouse-reporting, clickable links or kitty graphics; Chinese IME/candidate positioning (skipped at the user's request), every provider startup dialog and long-duration recovery matrices are not exhaustively accepted. Bounded 12,000-line output, native scroll/zoom, Core outage and live-host restart have acceptance in the latest update. Saved output is bounded terminal observation; explicit reports are authoritative outcomes.

## Earlier interactive acceptance (before the profile refactor)

Real Codex 0.166.0 completed a Unicode greeting task in a managed worktree, replied to an active @developer question from the selected Note, ran its tests, and triggered a separate Reviewer conversation. Both exited normally after their final turn. A subsequent @developer resumed the exact original provider ID in a new terminal, recalled the Note marker and completed without replaying Develop's callback. Archive retained readable replies. Evidence: /private/tmp/cb-real-interactive-xf_mcgnu/evidence.json (temporary local artifact).

The deterministic interactive harness uses real Herdr/PTYS and a fixture provider to exercise active replies without task completion, interrupt/continue, duplicate opens, independent roles, normal exit, exact-ID resume, restart and archive/restore. Native UI acceptance also exercised Unicode input, Ctrl+C/continue, Card comment -> separate Timeline reply, detach/reattach, explicit completion -> NORMAL_EXIT, and the same session UUID on resumed terminal. Fixture evidence is not real-model evidence.

Historical integration at that point: schema version 2 added conversations/questions. Attempt records retain provider ID, conversation ID, close state, last turn boundary and final-response path. The old CLI helpers bind-session, reply --to QUESTION_ID --file PATH and agent-notify used the current invocation's environment. Codex notify supplies thread-id and last-assistant-message; Claude accepts only its root Stop callback. Physical control operations commit their request-key intent before sending, so retrying an uncertain action does not replay it.

A final real-Codex acceptance rerun passed after isolating inherited CODEX_THREAD_ID. The production native view also rendered the actual Codex TUI, accepted Unicode input, reattached after the desktop host restarted, bound its exact thread and completed/closed normally. Native acceptance caught and fixed a hidden-cursor NSNull crash and drawing outside terminal bounds. Selection/copy and bracketed paste were exercised; an automation clipboard conflict was observed and cleared before submission. This does not establish a full IME matrix.

Claude Code 1.0.108 smoke check: the production adapter launched its interactive TUI in the isolated native app and accepted the owned test-folder trust prompt. The model request returned Invalid API key / Please run /login. This is a concrete local authentication blocker: actual Claude report/normal-exit/resume acceptance is not claimed. The later refactor removed the Stop-hook design. Existing credentials/settings were not changed.

Final native Gallery acceptance verified the production hidden-cursor fixture after scrolling into view without clicking it. NSH's layer-transform scroll can precede its AppKit bounds commit; the terminal tracks both geometries and schedules redraw across that transition. Final workspace validation: 19 AppServer tests, 2 terminal tests and 1 worktree test passed; workspace Clippy with warnings denied passed; both deterministic acceptance tasks passed. The final development bundle is .local/Canban.app. Scratch hosts/cores were stopped after validation.

## NSH upgrade (2026-11-07)

Upgraded the three direct NSH dependencies together from 0.3.0 at b7a69186d3c20a59690f2ac834dc527bee7f8438 to 0.4.0 at aca7b8f92a33af9d83c3693a516f649f7ea5c5e5, the upstream main HEAD observed during this upgrade. Cargo.lock pins the resolved graph. No Canban compatibility code changes were required. Workspace check, all 22 tests, Clippy with warnings denied, acceptance and acceptance-interactive passed against this revision. The packaged native app launched with an isolated fixture home and displayed the Board, Card editor and Agent controls. The user then requested stopping further checks and continuing the design discussion; further native interaction/IME/Terminal acceptance was not performed for this dependency upgrade. The scratch app was stopped.

## Latest profile/delivery/recovery acceptance

See [MVP acceptance](mvp-acceptance.md) for the executed post-refactor matrix and exact temporary evidence. Real Codex completed human-input and Review/rework tasks; both actual work products were explicitly committed/merged, tested in their target repositories, archived and restored. Native Review was operated against real Git changes, including stale-preview rejection. Terminal checks found/fixed scroll direction and attachment-layout races, and verified same-binding reconnect, input discard while disconnected and native-host restart. History deletion and explicit fresh-context recovery passed in real-Herdr fixture acceptance.

Use `mise exec -- python3 scripts/acceptance-native.py` to prepare an isolated production-native app for CUA/manual checks. Metadata is written under `.local/native-acceptance.json` and the temporary base. Creating a `restart` marker there before quitting reloads the host while keeping its scratch Core alive; normal exit stops only that test Core. `CANBAN_APP_ID` allows isolated development bundles to coexist with the regular app. This harness does not establish acceptance without performing and observing the interactions.


## Notes and Kanban workspace update (2026-11-07)

The approved calm native composition is implemented: Board with a selected-Card pane, focused Card/Terminal or Review, explicit return/expand controls and retained navigation drafts/Board scroll. Notes remain database-backed documents with file-like Agent CLI operations, immutable revisions, stable links/backlinks and transactionally associated history. The [implementation and acceptance report](notes-and-workspace-implementation.md) records contracts, real Herdr fixture integration, native interaction checks and remaining limits.
