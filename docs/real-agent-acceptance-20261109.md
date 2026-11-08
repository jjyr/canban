# Real Agent functional acceptance — 2026-11-09

Status: the local Codex workflow passed. Claude reached its actual provider API
but was blocked by `401 User not found`; Claude task/exit/Resume is not accepted.
These results use actual model processes in Herdr PTYs, not fixture Agents.

## Scope and isolation

- Codex CLI 0.167.0; Claude Code 2.1.319; Herdr Core 0.1.18.
- Two disposable Git Projects, separate Canban homes, Core sessions, sockets,
  state and configuration. Existing model login/history was reused for real
  provider sessions; no credentials were copied, changed or reauthenticated.
- The user's normal Canban data/Core and source repository were not test targets.
  The code delivered by model tasks was merged only into the scratch repositories.
- Code baseline: `759ab76` plus the uncommitted first-Terminal fix and the
  managed-worktree Resume repair found during this acceptance.

## Executed flows

| Scenario | Real result |
| --- | --- |
| Two Boards execute concurrently | Observed two bound Codex PTYs active across different Projects; distinct worktrees and Card context |
| Backlog → Develop → Review | Agents declared completion through the injected Canban CLI; Reviewer used its independent Card/profile conversation |
| Human input | Unicode slug developer reported blocked before changing code; Card/notice retained the question |
| Live @developer reply | Test operator chose the hyphen separator; the running Agent acknowledged with a Question reply and implemented the task |
| Human gate | After the blocked Agent completed, the Card stayed in Develop until explicit `card.continue`; only then was Review queued |
| Database Note | Agent read the marker, used revision-aware Note write, preserved the marker and appended the human decision; Note events retained execution provenance |
| Review → rework | Integer-sum requirement changed to reject any negative value; moving back to Develop reused the same Developer history; second Review checked the new requirement |
| Follow-up after normal exit | @developer resumed the original provider session in a new Terminal, recalled the marker/choice and completed without advancing the Card from Review |
| Code delivery | Actual Diff reviewed; explicit commit/merge updated each scratch source main; repeating the acceptance request returned the same receipt |
| Archive / Restore | Reports remained readable from zstd archives; clean worktrees were removed; Restore preserved source heads and did not dispatch work by itself |
| Archive → Restore → Resume | Found and repaired missing managed-worktree reconstruction; real Codex then resumed its original provider ID/cwd |
| Native Terminal follow-up | Typed a direct question into libghostty; Codex recalled the marker/choice, wrote its report and exited normally; tracked code hashes and Notes were unchanged |

The main workflow had seven successful Codex invocations across four provider
histories. The post-Archive native follow-up added an eighth successful
invocation in the original Developer history. All eight declared completion and
closed normally. The failed pre-fix post-Archive launch never reached a model
process; its failure and explicit retry remain in scratch history.

## Independent code checks and delivery

| Task | Checked behavior | Final checks | Scratch main commit |
| --- | --- | --- | --- |
| Unicode slug | Unicode lowercase, whitespace trimming/collapse, human-selected hyphen, punctuation preservation | 7 unittest cases | `73316febe4eba18247e47ef8603b832cf61f06a9` |
| Integer total | One-pass iterable sum, empty iterable = 0, reject every negative value including cancellation/generator cases | 8 unittest cases plus a direct negative-input assertion | `6904544c6783a20bc74a5ff7d32bbb7f37ec9d07` |

Tests ran independently in the model worktrees and again in the accepted source
checkouts. Source checkouts were unchanged before acceptance. The post-Archive
native question also reran the slug tests and checked the exact tracked file
hashes and database Notes against a pre-question baseline.

## Defect found: post-Archive Resume

Before repair, Archive removed a clean managed worktree, Restore restored the
Card/files, and Resume failed with:

```text
Original execution directory is missing; Resume cannot move to another location
```

The original machine/cwd fence was correct, but there was no reconstruction step.
`canban-workspaces::Ops::resume` now recreates a missing managed Card worktree at
its exact saved path using its retained branch/commit. It does not fetch a new
base, switch machines, or fall back to a source directory. Existing directories
are reused. Missing arbitrary/manual directories, unknown branches and lost
retained refs still fail visibly instead of silently starting unrelated work.

The Git regression covers reconstruction with the ordinary branch removed but
Archive's retained ref present, a changed template, no usable origin, and refusal
to recreate from a fresh base when both saved refs are absent. The real repaired
run restored provider session `01a1bc10-b539-7ad2-84dc-1245a38108a7` in a new PTY at
its original Card worktree and completed a direct native Terminal question.

## Claude boundary

The local auth-status command reported an API-key login. Actual Claude execution
passed its scratch-folder trust prompt and then returned:

```text
401 User not found
```

Presence of local credentials did not establish valid provider authentication.
Retries were stopped; credentials and account settings were left unchanged.
Claude Note/reply, normal exit and exact Resume remain unverified until the
provider accepts a real request. The first probe also exposed a harness issue:
Claude's trust menu defaults to exit; the harness now explicitly selects the
visible trust choice for its own disposable folder and detects this 401 response.

## Reproduction and evidence

```sh
mise run acceptance-real-workflows
mise run acceptance-real-claude
```

Both commands intentionally invoke real providers and can consume account quota.
The workflow harness now includes the blocked-completion human gate, live Note
writes/replies, rework, exact Resume, merge/receipt checks, compressed reads,
Restore and post-Archive Resume. Claude uses a separate isolated Core/home and
stops on recognized credential/billing errors.

Executed Codex evidence root:
`/private/tmp/cb-real-workflows-hxgn6ksj`.

- `evidence.json`: successful main workflow snapshot, checks, overlap observations
  and accepted commits/archives.
- `restore-resume-before.json`: reproduced missing-worktree failure.
- `native-restore-before.json` and `native-restore-evidence.json`: restored real
  session binding, file/Note baseline and successful native outcome.
- `metadata.json`: scoped test IDs and isolated runtime environment.

Claude evidence roots:
`/private/tmp/cb-real-claude-1lfvxg9a` (initial trust-menu probe) and
`/private/tmp/cb-real-claude-rp_hkkq3` (actual API 401, retained terminal screen and
`provider-error.json`). Temporary artifacts are diagnostic evidence, not portable
provider-history backups.

Final code checks: 90 workspace Rust tests, Clippy with warnings denied, native
build/package and diff checks passed. The full main paid workflow preceded the
Resume fix; the affected recovery path was rerun afterward using the same real
provider history through the native UI. The script's post-Archive continuation
was added to retain this scenario for future complete runs.

These are bounded local-model checks. They do not establish real model access on
remote machines, all provider permission dialogs, a second physical UI client,
or Chinese IME behavior. Task-owned UIs, AppServers, Cores and PTYs were stopped
after testing; reports and model-owned session history were retained.
