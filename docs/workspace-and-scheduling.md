# Workspace preparation, scheduling and human gates

Multi-machine update, 2026-11-09: see [current execution/location and service contract](multi-machine.md). Profile commands resolve on their execution host; Resume and Review retain their original workspace, while new work uses the Project's current location.


Status: implemented 2026-11-07. This update supersedes detached Card checkouts, unrestricted scheduling and attention-as-display-only behavior. The mechanism stays within existing crates; no operation graph or scripting engine was introduced.

## Workflow settings

```json
{
  "name": "Development",
  "workspace": {
    "create_worktree": true,
    "base_branch": "main",
    "sync_base": true,
    "remote": "origin"
  },
  "stages": [
    { "id": "backlog", "name": "Backlog", "execute": false, "instruction": "", "on_completed": null },
    { "id": "develop", "name": "Develop", "execute": true, "profile": "developer", "instruction": "Implement the Card", "on_completed": "review" },
    { "id": "review", "name": "Review", "execute": true, "profile": "reviewer", "instruction": "Review and report", "on_completed": null, "require_human_review": true }
  ]
}
```

Fresh default templates enable worktree creation, synchronization and human Review. `base_branch: null`/omission resolves to Project target/current local branch. Missing workspace settings in an existing saved template preserve local-only preparation (`create_worktree: true`, `sync_base: false`). Existing Project snapshots are not rewritten from a changed default file. A missing human-review flag defaults to false. For non-Git work use `create_worktree: false, sync_base: false`; attempting synchronized work without a committed Git base requests attention rather than silently running elsewhere.

A Card stores optional `branch`, `base_commit`, `worktree` and `human_gate` fields in its existing JSON record. No destructive SQL migration is required. Old attention is converted into a persistent gate on application open; legacy detached worktrees are adopted when next used, preserving current HEAD and edits.

## Git preparation and delivery

```text
First Card invocation
  -> reserve a global starting slot
  -> fetch configured remote/base
  -> fast-forward local base (or retain local-ahead commits)
  -> allocate <directory-name>-<project-id-first-4>/<card-number>
  -> create branch and ~/.canban/worktrees/<allocated-name>
  -> persist workspace identity/base commit + Timeline event
  -> capture context and start Agent

Review / rework / resume
  -> reuse Card branch/worktree (no implicit fetch/reset)

Human Accept
  -> validate preview and stopped Agents
  -> commit pending Card changes
  -> integrate target into Card
  -> fast-forward target checkout
  -> receipt + human approval

Archive
  -> retain branch + Card ref
  -> remove only a clean managed worktree
```

Fetch updates the remote-tracking ref. If the remote is ahead, update the local base by fast-forward only; if local already contains remote, retain local commits. Divergence, fetch failure and a dirty checked-out base that needs updating stop preparation. If base is checked out in another worktree, update through that checkout; otherwise update its ref with the expected previous commit. Preserve ignored local data during fast-forward. No automatic push, forced reset or rebase occurs.

A saved existing workspace bypasses synchronization. A retained Card branch/ref can recreate an archived checkout without fetching. The actual branch/path are persisted before Agent launch; subsequent stages, Project display-name changes, Card title changes and Resume use that binding. The initial base commit remains fixed when later invocations reuse the directory. Card numbers are Project-local Canban numbers, not necessarily a linked GitHub Issue number.

### Readable workspace names

For a Project whose execution directory is `/workspace/canban` and internal ID begins `4e5r`, new Card #42 uses:

```text
branch:    canban-4e5r/42
worktree:  <execution-home>/worktrees/canban-4e5r/42
```

The directory basename comes from the execution location admitted for this invocation, including remote machines. The Project display name and Card title do not participate. Directory components are lowercased; letters/numbers in any language and `_`/`-` are retained, other runs become `-`, and leading/trailing hyphens are removed. The directory component is limited to 80 UTF-8 bytes. Empty names fall back to `project`. The namespace includes the first four characters of the internal Project ID. Generated branch names are validated by `git check-ref-format --branch`.

A four-character ID is a readable disambiguator, not a uniqueness guarantee. Local branch collisions (including ancestor/descendant refs), existing files/directories/symlinks and registered missing worktree paths select matching leaf suffixes: `canban-4e5r/42-2`, then `42-3`, for both branch and path. Branch checks conservatively fold case for macOS portability. If the namespace itself is an existing branch or a filesystem file, use `canban-4e5r-2/42`. An existing directory may hold other Cards in that namespace and is reused as a parent. No unrelated branch/path is adopted or overwritten.

An exclusive directory reservation and Git's atomic ref creation protect concurrent creators, including separate Canban homes sharing one repository. A lost branch race reloads refs and retries; only an empty directory reserved by this invocation may be removed. Permissions, Git/transport errors and incomplete nonempty checkouts remain explicit failures rather than being treated as ordinary name collisions. Allocation is bounded to 1024 candidates.

Existing `canban/<number>`, `canban/<project-id>/<number>` branches and UUID/numeric worktree directories retain their saved identities and restore behavior. Existing readable bindings also restore from their saved branch/path, independently of the current directory basename. This change does not rename or migrate any existing workspace.

Card details show branch/target, path, base commit, latest delivery and read-only Git status. Refresh detects missing workspaces, dirty state and merge conflicts. Long values use compact visual labels; **Copy branch** and **Copy workspace path** retain exact values. Board tiles show a compact branch label. These controls are shared production components used by the Gallery.

## Scheduler configuration

`config/agents.json` adds:

```json
{ "scheduler": { "max_agents": 8 } }
```

Only positive integers are valid. Settings and `scheduler.configure` persist the same field without replacing presets/commands. Lowering the limit leaves existing Agents alive. `scheduler.status` returns `active`, `max_agents` and `queued`; Settings retains scheduler configuration and usage, while Overview provides global Agent/attention status. Board omits the Execution slots summary and task filter toolbar.

Slots now count starting, working and unknown/uncertain runtime. Verified blocked/idle/done/error statuses release execution capacity while retaining the terminal; stale/disconnected observations remain reserved. This supersedes the original all-live-terminal accounting. See [Agent Overview and lifecycle](agent-overview-and-lifecycle.md) for the configurable idle timeout, native activity tracking, global Overview and current acceptance.

```text
Persistent queue
   -> active usage below limit?
   -> automatic Card is not waiting for human / archived / in another stage?
   -> same Card/profile has no live or uncertain invocation?
   -> choose later stage index, then FIFO
   -> revalidate/reserve starting intent in the database
   -> prepare workspace and launch through Herdr
```

Priority follows each Project template's stage order rather than a fixed stage name. It applies across Boards; equal depth uses creation time with insertion-order ties. Runtime reconciliation and closure keep operating when capacity is full. No additional Board/profile cap or preemption is added.

## Human decisions

`require_human_review: true` lets the stage Agent execute, then pauses its completion callback. `blocked`, `failed`, uncertain transport/startup and recovery attention also pause automatic work. The gate survives app restart and empty queues; notice acknowledgement does not approve it. A later Agent completed report can record the pending transition but cannot release a gate.

Explicit `@Agent` and open/resume can assist while paused, within capacity. They do not approve the gate. Paused automatic work cannot swallow a manual request and prevent that assistance.

`card.continue` requires the current Card revision. It applies the completed initiating request's saved callback once, or clears a resolved non-transition gate. If routing failed before a request existed, human continuation retries queue creation after the routing configuration is corrected. If the Agent is still blocked/live, the user must answer it or end it before retrying. Unknown launch receipts require inspection and explicit retry. Moving stages for rework and retrying an ended failed execution are also explicit human decisions. Successful code acceptance approves the gate and applies a pending callback; the default Review has none, so it does not auto-Archive or rerun Review.

Agent-bound CLI calls carry their execution context for human-only approval actions and are rejected by AppServer. This is a local invocation/provenance rule rather than a security boundary against arbitrary same-user scripts.

```sh
canban call card.workspace '{"card_id":"CARD_ID"}'
canban call scheduler.status '{}'
canban call scheduler.configure '{"max_agents":8}'
canban call card.continue '{"card_id":"CARD_ID","revision":7,"request_key":"human-approval"}'
```

## Ownership

- `types`: small workspace/settings/gate contracts; backward-compatible defaults.
- `workspaces`: branch/worktree preparation, base synchronization, status and retained archive history.
- `scheduler`: slot counting, eligibility and queue priority.
- `app-server`: transactional reservation, reports, explicit human approval, configuration persistence and Timeline.
- `ui/components/git_workspace.rs` and `scheduling.rs`: reusable Card metadata, copy/status and global limit controls.

## Validation

Rust coverage includes remote fast-forward, checked-out bases in another worktree, dirty/divergent/failing remotes, detached adoption preserving edits, retained branches across archive/restore, global slot release, closing/unknown reservations, cross-Board custom-stage priority/FIFO, persisted settings, blocked-to-completed gates and approval idempotency. Existing delivery, Notes and Terminal regressions remain in the workspace suite.

`mise run acceptance-scheduling` uses a disposable local bare remote, two Boards and real Herdr PTYs with deterministic Agent fixtures. It verified eight live Agents plus one queued, remote updates captured into named worktrees, Review starting ahead of the ninth Develop, no progression after blocked-then-completed until human approval, restart preserving the gate, and limit edits without interrupting Agents. Evidence: `/private/tmp/cb-scheduling-p5mngfjj/evidence.json` (final full integration rerun).

`mise run acceptance-interactive` passed with the new explicit approval after Core outage/reconnection, plus Note CLI/provenance, active replies, exact resume, missing-history recovery and archive. Evidence home: `/private/tmp/cb-interactive-icti3cp0/home`.

Native UI acceptance used `/private/tmp/cb-native-8l1r1hne`, its own Canban home/Core and a packaged application. Settings rejected zero, saved a limit of two, and updated Board usage. Developer completion launched Reviewer; Reviewer completion showed **Waiting for human** with no live Agents; clicking **Reviewed · continue workflow** cleared the gate without rerunning the stage. After a packaged-build restart, compact Git labels and both copy actions remained visible. Copying the workspace path and pasting into another Card composer produced the exact full path. The saved `scheduling-ui-evidence.json` verifies the persisted limit of two, named branch/base commit, two completed/closed attempts, Review stage and cleared gate without another invocation. The standalone Gallery at `/private/tmp/cb-scheduler-gallery-e8abwdbs` exercised the shared settings validation and constrained workspace layout.

Final checks passed: `mise run test` (52 Rust tests), all-target Clippy with `-D warnings`, formatting, `git diff --check`, main/Gallery builds and Python syntax checks. `mise run acceptance` also passed at `/private/tmp/cb-wwbrmmav/home`, retaining the Develop/Review, blocked report, archive/restore, restart and three-Board legacy-config regression. The old outage/notice harness expectations were updated to require an explicit human recovery and to wait for the actual blocked report rather than an earlier Review notification.

These are deterministic Agent/real-transport checks, not new live Claude/model acceptance. Chinese IME remains excluded at the user's request. Arbitrary external Git commands are not locked; the existing stale-preview/checkout checks still apply.


### Readable naming acceptance (2026-11-10)

- `mise run test`: 169 workspace tests passed. Disposable real-Git coverage includes multilingual/long/invalid directory components, short-ID collisions, occupied refs and paths, prefix/descendant refs, dangling symlinks, registered missing checkouts, concurrent creators in one home and across different homes, legacy numeric/UUID restore, retained history and Review/PR publication.
- `mise run build` and workspace/all-target Clippy with warnings denied passed.
- `mise run acceptance` passed against isolated Herdr Core 0.1.18, including Develop/Review, compressed archive reads/restore, service restart and cross-Board admission. Evidence home: `/private/tmp/cb-tj4heva9/home`.
- `mise run acceptance-scheduling` passed at `/private/tmp/cb-scheduling-rpzoq03u/evidence.json`. Two Projects share a repository but use their own directory/short-ID namespaces. For the first Card, a preexisting default branch plus an unrelated `-2` file forced allocation of `source-a520/1-3`, with an identical relative worktree path; the unrelated file remained intact. Eight-Agent admission, synchronized base, Review priority, human gates and limit updates also passed.
- The packaged native App was opened against that scratch home/Core. Board tiles and the opened Card header displayed `source-a520/1-3`; existing completed Review history remained readable after restart. This reused deterministic PTY-provider data; it is not a new authenticated model run or remote-machine acceptance. Test windows/Core were closed afterward.

No UI components or RPC/storage schemas change for workspace naming. The earlier Gallery acceptance applies to the separate Archive redesign; the naming check uses the actual packaged App's existing Card/Board components.
