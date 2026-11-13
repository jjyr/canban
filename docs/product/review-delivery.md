# Review and code delivery

Status: implemented 2026-11-07. Basic native Diff and explicit commit/merge are included; full Shear integration is deferred. Review reports remain ordinary Card-owned Markdown reports.

## Human disposition

From Card details choose **Review changes**. Refresh the diff, read report/Timeline context, and choose one of these paths:

- Ask a configured Agent through the existing Card composer. A live question is appended immediately; an exited conversation resumes exactly.
- Return to Develop using the workflow stage action. Add feedback to the Card/comment first. The workflow schedules that stage with the existing profile conversation and a new invocation binding.
- Accept using **Accept · commit and merge**. This is a deliberate Git operation, separate from Archive.

### Quick merge from the Card header

Status: implemented 2026-11-13 (Card #22; user selected the one-click option and no Board-tile button).

The Card header shows **Merge** next to `Diff` / `Review` for an active local-delivery Card that has a worktree, no Local delivery receipt and no PR binding (effective delivery mode is not GitHub PR, and the Card is not a GitHub PR source). It is hidden on the Review page, whose own merge control remains, and on Board tiles.

Pressing it calls the read-only `review.preview` for the Project target branch without rendering the diff, then:

- `merge_allowed == false`: shows the preview reason as an operation error and changes nothing.
- A preview `warning` (for example a still-running or queued Card Agent): opens a confirmation dialog with the Card branch, target, changed-file count and warning. **Merge anyway** continues; **Terminals ↗** and **Cancel** do not merge.
- Otherwise: immediately calls `review.merge` with the preview token/revision and the default message `Canban #<number>: <title>`.

The merge is skipped if the Card revision changed while previewing. AppServer still revalidates the token, target cleanliness and conflicts, records the delivery receipt, completes waiting delivery steps and attempts the normal merged-Card auto-archive. No RPC or schema changed. Gallery scenes: `card-quick-merge` and `card-quick-merge-warning`.

Project creation captures its current local branch as `target_branch`; callers can specify it explicitly. Review can choose another existing local branch. The Project checkout must already be on that target branch and clean. Canban does not silently switch branches, discard local edits, push, or publish a PR. Non-Git/unborn Projects or Cards without a separate Git worktree do not support this merge path.

The diff includes tracked changes since the merge base and untracked files. Acceptance includes the complete reviewed Card checkout, including untracked source files; there is no partial staging/hunk selector. The preview indicates truncation beyond 2 MiB; inspect the complete checkout in the terminal when truncated. Binary patches are represented as Git binary diffs.

## Execution boundary

```text
Refresh diff
   |
   +-> target HEAD + Card HEAD + working changes + Card revision
   |                      |
   |                immutable preview token
   v                      v
Human accepts -> persist unknown delivery intent / request key
                           |
                    revalidate preview
                           |
                    commit Card changes (if dirty)
                           |
                  merge target HEAD INTO Card
                           |
             +-------------+------------------+
             |                                |
          conflict                          success
             |                                |
      retain in Card worktree        recheck clean/unchanged target
      target branch unchanged                 |
      attention + failure receipt       fast-forward target checkout
                                              |
                                     Delivery + Timeline receipt
                                              |
                                         Archive separately
```

Git readiness (`merge_allowed` and `reason`) governs whether a diff can be accepted. Active Card Agent work or queued/uncertain requests never disable `Merge` or `Publish PR`: `review.preview` exposes an informational `warning` only when Herdr reports the Agent is still running/disconnected or when a Request is `queued`/`starting`/`unknown`. When a retained interactive Agent terminal is connected and Herdr reports `status.state` as `idle` or `done` (or the PTY has `finished`), no warning is shown at all. Git hooks remain active; a hook changing the committed tree or leaving checkout edits stops delivery for inspection. Divergent non-conflicting target changes are integrated in the Card worktree first. Conflicts stay there; the Project source checkout is not left in a half-merged state. Resolve/abort them in the Card terminal, rerun validation and refresh the diff before another acceptance.

Runtime activity advisories use the shared Attempt lifecycle (`Attempt::agent_stopped`). A `continued` assignment is historical: its terminal belongs to the subsequent assignment. Connected retained sessions sitting at their TUI prompt (`idle` or `done` in Herdr) are treated as stopped. Actively working/blocked sessions, disconnected terminals, and `queued`/`starting`/`unknown` requests surface a non-blocking `warning` in Review while still permitting explicit human merge; preview token verification continues to reject stale diffs if files change concurrently.

Card and target changes are checked again, but Canban does not lock arbitrary external editors/Git commands. Profiles on one Card still share a worktree. Keep that checkout quiescent during acceptance. External Git hooks, permissions, identity/signing failures and concurrent source changes surface as attention; any created Card commit remains available for inspection. A global delivery mutex prevents two Canban merges from updating checkouts concurrently; it is not an Agent concurrency cap.

## API and retry contract

```sh
canban call review.preview '{"card_id":"CARD_ID","target_branch":"main"}'
canban call review.merge '{"card_id":"CARD_ID","revision":7,"target_branch":"main","token":"PREVIEW_TOKEN","message":"Implement the reviewed task","request_key":"unique-acceptance-id"}'
```

`review.preview` is read-only. It returns revision, heads, target/source branch, worktree, file paths, patch, truncation/dirty/conflict flags, token, merge_allowed, reason and optional non-blocking warning. The token fingerprints full content/status/heads even if the displayed patch is truncated. A stale token or revision rejects acceptance before staging/committing.

`review.merge` requires the request key, revision and preview token. It returns `state: merged`, `failed` or `unknown`. Reusing a key with the identical payload returns its stored receipt before any Git action; changing its payload is a conflict. The intent is saved before Git effects. A crash/lost result can leave unknown; inspect the target and Card Git heads and Timeline before a new acceptance. There is no blind replay of an uncertain physical operation. The native UI clears the preview after a returned result, requiring a fresh inspection.

Success saves `Card.delivery` with target branch, commit, request key and merged timestamp. Timeline events are review.merge_started, review.merged or review.merge_failed. Failed operations request attention. Delivery metadata does not change the workflow stage or imply Archive. Subsequent rework retains the earlier delivery as history until a newer successful acceptance replaces the Card's latest-delivery summary.

## Archive and remote PR merge

Card work now uses a named branch plus worktree. A successful human merge (`review.merge` or `github.pr.merge`) clears the review gate, applies any configured completion steps, records `Card.delivery` (`Merged` status), and automatically archives the Card once no active Agent terminal, active workflow run or uncommitted worktree changes remain.

When a delivery PR (`card.pull_request`) or a GitHub PR source Card (`CardContent::GitHubPr`) is merged directly on GitHub, Canban reconciles the remote merge state (`Card.delivery` + `Merged` badge), attempts a best-effort fast-forward of the local Project target branch when its checkout is clean (`sync_target`), and automatically archives the Card if no live Agent terminal or uncommitted worktree edits are present. If a live Agent terminal or dirty worktree exists at the time of remote merge, the Card stays in `Review` with the `Merged` badge and automatically archives when the terminal finishes (or when archived manually). Restoring an archived Card retains its `Merged` badge without re-archiving it automatically.

Archive packages Card-owned reports/context/replies/output and exported records with zstd. Clean managed worktrees are removed after retaining their HEAD under the Card ref and retaining the named branch; dirty worktrees remain with a notice. Restore expands content without scheduling work. Provider-owned conversation files are outside this portable archive boundary.
