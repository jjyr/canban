[Dedicated Terminal workspace](terminal-workspace.md) (2026-11-08) is the current Terminal UI contract: its separate icon/tree, Card/Overview/Quick Open convergence and isolated multi-PTY acceptance supersede the older Card-plus-Terminal focused split. Review retains the Card-focused delivery layout.

Current runtime update (2026-11-09): [Event-driven runtime observation](runtime-observation.md) replaces periodic Terminal/Agent scans, adds bounded context discovery and changes automatic unreported-exit gates into Failed history.

# MVP implementation and development

Workflow/event implementation (2026-11-11): [concrete operations, source storage, UX and limits](workflow-implementation.md) implement the unified engine and durable subscriptions. Projects have multiple YAML sources, original text and immutable versions in SQLite; no Workflow files or revision mirrors are generated in the Canban home. Project Workflows supports manual editing/history and Edit with agent; Column trigger details are derived client-side. Configuration conversations have Project ownership and use the existing Terminal. Onboarding remains a separate topic; deterministic fixture acceptance does not imply live provider authentication.

[Unified Card reminders](reminders.md) (2026-11-10) defines the current unread /
pending counts, persistent visible-read positions, Overview updates and local
notification sounds. Current builds use SQLite schema 12 and desktop API version 10 (the earlier merged GitHub/editor/updater contract used schema 10/API 8). Upgrade
the installed AppServer together with its UI. `mise run dev` (also `mise run run`)
now manages a separate checkout/branch instance; it does not upgrade installed
`~/.canban`. Existing history is baselined quietly during migration.

[Compact navigation and Quick Open](quick-open-and-navigation.md) covers the implemented icon rail, Command+O fuzzy destination picker and native interaction checks.

[Agent Overview and lifecycle](agent-overview-and-lifecycle.md) documents the implemented status-aware execution budget, configurable resumable idle exit and global attention/Agent workspace.

Status: native macOS MVP with multi-machine execution; current contracts reconciled 2026-11-10. The internal updater has isolated/native acceptance recorded in [updates](updates.md); older documentation-only reconciliation added no acceptance. This is the current implementation reference; earlier design documents contain broader proposals and dated pre-implementation statements.

[Desktop utilities](desktop-utilities.md) covers the shared bottom bar, subscription Usage, Caffeine, persisted appearance, command catalog and cross-Project Card creation.

## GitHub workspace update

The [GitHub workspace and cache](github-workspace.md) extends the integration with a dedicated rail page, paginated notifications/Issues/PRs, PR source Cards, local GitHub references, Board source filters, repository rules and explicit remote comments/Review. Account controls moved out of Settings. Remote data uses disposable event-refreshed file caches instead of permanent business-table mirrors; schema 9 preserves bindings and local work. The GitHub/editor integration used schema 10/API 8, extended to schema 12/API 10 by Workflows; [combined acceptance](merge-integration-20261111.md) records migration and native verification.

## Build and launch

Use the pinned mise Rust toolchain. NSH 0.5.0 is pinned to f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1 (upstream tag `v0.5.0`) in Cargo.toml and Cargo.lock. See the [upgrade and acceptance record](nsh-050-upgrade-20261111.md). Install macOS Xcode command-line tools, librsvg (`brew install librsvg`, for app icon generation) and a compatible hc separately. This implementation was verified with hc 0.1.18 and Codex CLI 0.166.0.

```sh
mise run dev
mise run dev-stop
```

`mise run build` creates target/debug/canban, target/debug/canban-ui, target/debug/canban-updater and an ordinary installable `.local/Canban.app`. `mise run dev` builds the binaries and starts a checkout/branch-scoped Dev App under `.local/dev/<label>-<hash>/`, with its own home, native ID, bundle binding and process records. `mise run dev-stop` gracefully stops only that instance; `mise run run` is the same isolated start entry. Explicit homes override branch selection, and installed `~/.canban` is refused by the development manager. See [development startup](development-startup.md) and [remaining gaps](instance-isolation.md). All bundles are ad-hoc signed after assembly and pass strict/deep verification. Developer ID signing/notarization remains separate distribution work. The standalone `mise run gallery` uses fixtures without AppServer/data. Several compatible UI clients may still attach to one service; a second AppServer cannot own the same home.

All bundle variants generate the macOS app icon from [the original SVG](../crates/ui/assets/app-icon.svg), including standard and Retina sizes. Packaging installs `Contents/Resources/AppIcon.icns` and declares `CFBundleIconFile` before bundle signing. See [asset provenance and generation](../crates/ui/assets/README.md#app-icon). The release workflow installs librsvg; distributed apps need no SVG renderer at runtime.

`mise run test-package` exercises actual Mach-O fixtures and macOS signature checks: clean ordinary/development/Gallery packages, Release-to-Debug repackaging, stale resource seal repair and relocation into a temporary Applications directory. The release workflow runs it before building the distribution bundle.

Packaging acceptance (2026-11-11): the new regressions failed against the prior packager (all three local bundle variants plus both repackaging cases), then all three tests passed after the fix. `mise run build`, `mise run build-release`, `mise run test` (207 tests), `mise run acceptance` and `mise run acceptance-dev` passed. The newly built Debug `.local/Canban.app` and the repaired installed `/Applications/Canban.app` both opened native windows through LaunchServices with an isolated home/Core; the installed app also opened Settings. Logs are `/private/tmp/canban-package-before.log`, `/private/tmp/canban-package-after.log` and `/private/tmp/canban-signing-*.log`; native fixture metadata is under `/private/tmp/canban-signed-launch-2i7wmcy8`. This establishes local signing/launch behavior, not Developer ID notarization. Live application data was not used.

CANBAN_HOME selects an alternate absolute application-data directory. Its default is ~/.canban/. Native UI connects through AppClient RPC to a separate, persistent AppServer. A same-user, mode-0600 Unix socket exposes JSON-RPC to the CLI. The home directory is mode 0700.

```text
canban-ui -> components/presenters -> AppClient -- RPC --> AppServer
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


Current service lifecycle (2026-11-09): the desktop attaches to or starts an independent AppServer. Closing a window leaves scheduling active. Multiple UI clients share one authority; explicit CANBAN_ENDPOINT attaches only. Remote execution, directory browsing, saved workspace/Resume bindings and Terminal byte relay are implemented. See [multi-machine execution](multi-machine.md) and [acceptance](multi-machine-acceptance.md). Earlier same-process lifecycle descriptions are superseded.

## Configuration

First startup creates config/agents.json and stores the default Workflow bootstrap in SQLite. An existing config/workflows/default.json is imported once; no new Workflow file is created. The local hc path resolves when available. Profile commands remain portable and resolve on the selected execution host at launch. Edit JSON and choose Reload agent configuration in Settings, or call config.reload. Invalid configuration leaves the previous in-memory configuration active. Stage profile IDs are validated at startup, Project creation and configuration reload against the global registry; removing a profile referenced by the default/saved Project workflow is rejected. Requests already queued keep their profile/instruction/callback snapshot.

Fresh homes receive developer and reviewer starter profiles (both initially using the existing Codex launch setup), plus codex/claude profiles for direct mentions. The default workflow references developer from Develop and reviewer from Review. Existing registries/workflow files and saved Project workflows are preserved without injecting or rewriting roles. When creating the SQL bootstrap under an older registry, generated stages omit references to unavailable starter profiles and use the global default.

The default workflow is Backlog -> Develop -> Review. Backlog does nothing; Develop completion enters Review and queues its ordinary review-report task. Review completion awaits human disposition. New Projects use the SQL bootstrap and receive a visible development.yaml source document in Project Workflows. Existing Projects retain their configured behavior through conversion to SQL YAML sources. Source document names do not imply managed files.

Agent configuration has hc, hc_session, hc_env, default_agent and profiles. Profiles have id, optional preset, role_instruction, optional argv/resume_argv, exit and session_instruction. canban-profiles resolves editable JSON defaults and explicit whole-field overrides. Commands expand {prompt}, {home}, {cli} and {session_id} as individual arguments. The runtime has no provider-specific invocation branches or Herdr launch-template mode. See [profile schema and migration](product/agent-profiles.md).

Codex and Claude presets configure interactive launch, exact-ID Resume and normal /exit input. Neither injects notify/Stop hooks. Codex binds its actual tool-environment ID through bind-session; Claude's fresh command receives a preallocated UUID. Business completion requires an explicit Canban report. Completion keeps the interactive process available. A configured inactivity timeout later uses verified idle/done plus actual human-input timestamps to request normal exit; unfinished automatic work and pending input remain protected. Finished entries leave Terminals, while explicit Card Resume retains exact conversation history. See [current Terminal lifecycle/navigation](terminal-interaction.md). A provider without usable status keeps manual terminal control. Legacy config conversion writes an exact-byte backup before publication; historical Requests/conversations are preserved. Opaque legacy launch templates require explicit command conversion.
Routing precedence is explicit @profile -> stage.profile -> default_agent. Routing tags and Profile aliases have been removed; stages without a profile use the global default. Old stored tag fields are ignored when decoding; new Card mutations reject the removed field rather than silently choosing another Agent. Queued/running Requests retain their captured Profile. A submitted @profile mention immediately appends a Question to the live Card/profile invocation, or starts/resumes it if absent/exited. Stage-entry execution queues a request using that same provider conversation and a new callback binding. Retry is explicit and uses a new attempt; reconnect does not create one.

Boards share one Scheduler with a configurable global Agent limit (default eight). Starting/working/closing and uncertain invocations reserve slots; verified root blocked/idle/done/error states release capacity while retaining their Terminals. A completed report alone does not release a working runtime. Queued work waits without occupying a slot, and independent Shells are outside the Agent budget. Later template stages run first, with FIFO ties. Automatic requests and progression pause on persisted human gates until an explicit user decision. Settings and JSON share the same configuration. See [workspace/scheduling implementation and acceptance](workspace-and-scheduling.md); earlier unlimited-concurrency evidence describes the previous policy.

See [Card conversation workspace](card-conversation.md) for the current compact detail layout, explicit desktop recipients, comment shortcuts and native verification.

## Product behavior

- Create a Project in an existing or new absolute directory; Git is optional. Each Project has a Board and increasing local Card numbers.
- Create/edit Cards, move with stage buttons, the Card header stage switcher or Board card dragging, and inspect current execution/attention status.
- Notes are deferred pending product design. Their UI/editor/search/Card-selection flows are removed; new executions use Card content, Timeline, reports and attachments without automatically injecting legacy Notes. Existing Note records, associations and capture files remain intact. See [current scope and compatibility](notes-deferred.md).
- Comment and mention configured Agents. Reports and state changes join the same ordered Timeline. Pasted images in Card/comment editors save bytes as Card resources and insert an attachment reference into the draft; save/post the draft to retain that text reference. Resources also support file-path import and native opening.
- Card creation and task editing share a native Markdown editor in the Board right pane with retained expanded mode. Drafts persist independently in schema v9 and are restored through Drafts. `@` offers Profile references; `#` offers local Cards and connected GitHub Issues/PRs. Enter/Shift+Enter always insert newlines; Tab selects candidates and Command+Enter confirms. See [Card editor](card-editor.md) for current behavior and acceptance limits.
- AppServer returns structured errors for stale Card revisions. A failed save remains visible in the status area. In-flight UI operations do not block render/input paths; stale resource-load results cannot replace a newly selected Card's output. Incoming state updates do not rebuild active Card editor drafts.
- Card Agent links and Overview open the dedicated Terminal workspace. Users type/paste, interrupt, explicitly close or resume exact provider history; leaving the view only detaches it. X, Command+W and Close terminal share AppServer termination and receipt handling. Exited entries leave the list/tabs while Card/Overview retain saved history. `libghostty-vt` owns VT state, incremental render-state snapshots, selection gestures, text formatting, paste encoding, mouse encoding and key encoding (both macOS Option keys act as Alt, while Option-drag/Option-scroll override mouse reporting for local rectangular selection and scrollback). On macOS, AppKit + CoreText render directly from the binary snapshot through an NSH native component, with CJK/IME preedit support, procedural Box Drawing/Block/Braille glyphs, SGR underline styles/colors, Command-click hyperlinks (OSC 8 and HTTP/HTTPS URLs), OSC 52 clipboard writes, and reactive Terminal font family/size settings persisted in `config/desktop.json` (selected from a searchable picker of installed system/user monospaced fonts). Herdr owns the PTY, and runtime state never implies business completion.
- The Agent's explicit working/completed/blocked/failed declaration is recorded through the CLI. Completed invokes only its originating request's callback. Blocked/failed creates a durable attention notice. Completion receipts and commands with request keys have duplicate/conflict handling.
- For a Git repository with HEAD, task execution creates a named managed worktree and reuses it across stages. New branches use `<directory-name>-<project-id-first-4>/<card-number>`; managed paths mirror that name under `worktrees/`. Matching numeric suffixes resolve branch/path conflicts. Existing saved UUID/numeric/readable paths and branches remain unchanged. See [workspace naming](workspace-and-scheduling.md#readable-workspace-names). Non-Git repositories can use their Project directory with worktree/base synchronization disabled. Uncommitted source-directory edits are not silently copied into a new worktree. Archive retains dirty worktrees with a notice; clean worktrees are removed after preserving their HEAD under refs/canban/cards/<card-id>.
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

See [Agent profile configuration](product/agent-profiles.md) for loadable JSON examples, schema, routing precedence and backward compatibility. Settings displays the shared registry and roles, Board header tooltips display stage profile references, and the Card composer offers Ask @profile actions.

Public operations include update.state/check/install, app.ping/snapshot, project.create/list, card.create/get/list/update/move/archive/restore/attention.clear/continue/workspace, comment.add, timeline.list, execution.get/report/retry/output, attachment.add, resource.read/materialize, config.get/reload, scheduler.status/configure, herdr.status/templates, profile.presets, review.preview/merge and agent.open/bind/reply/interrupt/end. Note RPCs, card.context.update and the hidden canban note command remain legacy compatibility paths, outside the current product scope. agent.turn_ended is a deprecated ignored compatibility endpoint. For timeline.list supply card_id, optional after_sequence and limit; next_cursor provides pagination. Current code and shared types are authoritative for exact payloads; the [interface design](interfaces/mvp-contract.md) remains a broader contract reference.

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

Earlier native Canban.app interaction checks (before the named-theme update) covered Card creation, multiline Unicode paste/edit/save and preview, comment mention, cross-stage drag, Notes save/preview, light/dark Gallery rendering, keyboard Tab/Space activation and skipping disabled controls, copying an image in Preview then pasting its bytes into a Card, opening the retained attachment in Preview, per-Card Note checkbox selection, and closing/relaunching the standalone window. Image references/resources persisted and active editor text survived the resulting state refresh. These were real native interactions, not a browser mockup or screenshots alone.

Rust regression coverage includes state/history/request consistency and duplicate operations/reports, stale revisions, fresh mention callback isolation, blocking notices, routing failures, compressed resource restore, path isolation, workflow cycle rejection, large Unix-socket JSON frames, legacy Note retention/exclusion from new captures, and managed-worktree reuse/dirty retention/commit preservation. The large-frame case caught and prevents a macOS nonblocking accepted-socket truncation bug found during live integration.

## Remaining boundaries

This is the macOS MVP with interactive Card/profile conversations, multi-machine execution and the [implemented GitHub Issue/PR integration](github-integration.md). Shear review, native notifications, template designer/migration UI, advanced timers and multi-authority data sync remain deferred. Internal prerelease automation and desktop install/recovery are implemented with local fixture/native acceptance; actual GitHub publication and public signed distribution remain unverified. Claude's preset is implemented and configuration-tested, but live Claude acceptance has not been established. Provider startup/trust dialogs remain interactive; Canban does not automatically authorize them. Notes are deferred; legacy data remains stored without an editor or automatic capture injection. Terminal presentation supports SGR/pixel mouse-reporting, alternate-screen scroll, clickable OSC 8 / HTTP(S) links, procedural Box/Block/Braille rendering and live font configuration, while kitty graphics and Windows/Linux native terminal views remain deferred. Bounded 12,000-line output, native scroll/zoom, Core outage and live-host restart have acceptance in the latest update. Saved output is bounded terminal observation; explicit reports are authoritative outcomes.

## Earlier interactive acceptance (before the profile refactor)

Real Codex 0.166.0 completed a Unicode greeting task in a managed worktree, replied to an active @developer question from the selected Note, ran its tests, and triggered a separate Reviewer conversation. Both exited normally after their final turn. A subsequent @developer resumed the exact original provider ID in a new terminal, recalled the Note marker and completed without replaying Develop's callback. Archive retained readable replies. Evidence: /private/tmp/cb-real-interactive-xf_mcgnu/evidence.json (temporary local artifact).

The deterministic interactive harness uses real Herdr/PTYS and a fixture provider to exercise active replies without task completion, interrupt/continue, duplicate opens, independent roles, retained completed conversations, explicit/idle exit, exact-ID resume, restart and archive/restore. Earlier native UI acceptance also exercised Unicode input, Ctrl+C/continue, Card comment -> separate Timeline reply, detach/reattach, explicit completion -> NORMAL_EXIT, and the same session UUID on resumed terminal. Fixture evidence is not real-model evidence.

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

Historical update: the Note portion below was withdrawn from product scope on 2026-11-10; see [Notes deferred](notes-deferred.md). The dated acceptance remains historical.

The approved calm native composition is implemented: Board with a selected-Card pane, focused Card/Terminal or Review, explicit return/expand controls and retained navigation drafts/Board scroll. Notes remain database-backed documents with file-like Agent CLI operations, immutable revisions, stable links/backlinks and transactionally associated history. The [implementation and acceptance report](notes-and-workspace-implementation.md) records contracts, real Herdr fixture integration, native interaction checks and remaining limits.

Navigation refinement (2026-11-08): the rail contains only Quick Open, Board, Overview, Notes, Archive and Settings. Project switching/creation lives in the Board header; icons share vendored Lucide SVGs rendered by NSH. [Navigation behavior and native regression acceptance](quick-open-and-navigation.md#navigation-refinement-acceptance-2026-11-08) records the isolated two-Project run, modal Escape fix and long-title layout regression.

[Project entry, grouped Overview and Settings](workspace-entry-and-settings.md) (2026-11-08) documents the clickable empty Board, native directory chooser, Overview without filters and a single scrolling Settings page, including isolated native acceptance.

## Terminal workspace chrome and manual Shells (2026-11-08)

The [Terminal workspace](terminal-workspace.md) now shares a Project/Card inventory, numbered tabs and a single native renderer across Scheduler Agents and manually opened Shells. NSH's custom window titlebar is split at the same resizable boundary as the content; Card and branch stay in chrome, with secondary controls in native menus. New tabs open immediately, rename happens afterwards, and closing a tab terminates its associated live PTY before removing the tab. The technical status footer is removed; failures remain dismissible notices. SQLite schema v4 preserves entry names/numbers and manual Core/PTy bindings. Manual Shells do not consume Scheduler slots, create workflow Requests or inherit Agent idle reclamation.

### Independent Terminal context update (2026-11-08)

Terminal creation no longer requires Project: `terminal.create {}` starts at home; optional `cwd`, `project_id`, `card_id` or `inherit_terminal_id` selects the launch directory. Project/Card arguments do not persist ownership. `terminal.context` returns Herdr-observed cwd, optional Project/Card hints and branch; unavailable runtime returns null hints with an error. Schema v5 removes TerminalSource and stored Shell cwd, retaining optional Conversation association and independent runtime binding. In-memory snapshot contexts drive Project/Other grouping and chrome. See [Terminal contract](terminal-workspace.md) for migration, inference and acceptance details.

### Agent labels and independent Terminal sequence

Schema v6 adds a separate durable manual Terminal sequence. Agent inventory entries show two lines (Card/title + execution stage; provider/Profile + status), with one-line bounded tabs. Independent entries display Terminal N without a type icon. Custom names override the title portion and clearing them restores the generated title. Codex/Claude vector marks are shared UI Kit assets with preserved Herdr notices. Branch/directory labels shorten visually while full values remain in hover/actions. See [Terminal workspace](terminal-workspace.md) for contracts and current validation.


## Internal updater (updated 2026-11-11)

[Internal releases and updates](updates.md) defines the implementation, migration authoring rules and acceptance. The main-triggered release workflow reserves `YYYYMMDD-N`, builds one UI/CLI/updater App, and publishes only complete prereleases. Persisted rolling-24-hour and unrestricted manual checks feed the global bottom-rail Update action. Installation immediately replaces the initiating UI/AppServer without draining Herdr Agents; durable idempotent reports/replies survive the handoff. Paired database-only up/down batches are transactional, with no automatic database backup for release installation. Failed candidate startup rolls back schema/bundle and verifies the restored version; failed down stops with a standalone database-free Recovery window.

The earlier GitHub/editor integration used schema 10/API 8; Workflow sources and subscriptions now extend that to schema 12/API 10. The service lock is acquired before opening SQLite. The earlier integration's six isolated update/failure cases and native sidebar upgrade covered schema 10, including unsent-draft restoration and exact Agent terminal/Core/worktree preservation. The [Workflow/event-system merge acceptance](event-system-merge-acceptance-20261111.md) separately records schema-12 local backend activation and native Workflow checks; it does not rerun the release download/bundle-swap matrix. See the linked evidence for the simulated future-schema boundary and unverified GitHub publication.

### Local backend activation (2026-11-11)

Ordinary local Apps show **Update backend** only when the answering backend is
older. The action uses the installed App through the existing updater and never
downloads or replaces a bundle. Actual equal schema skips migration; pending
schema changes take a database snapshot for failure recovery. Equal/newer
backends permit normal entry; a newer backend shows a newer-App notice and is
never downgraded. Dev instances still restart via `mise run dev`. See
[the current update contract](updates.md) and
[executed backend acceptance](backend-update-acceptance-20261111.md).
