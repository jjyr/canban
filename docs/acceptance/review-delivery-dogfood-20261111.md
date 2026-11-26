# Live Review and delivery exercise, 2026-11-11

## Scope and outcome

The user requested review and merge of two completed development Cards through
the already-running application, together with feedback on its interaction and
visual design. The UI remained the existing `/Applications/Canban.app` process
(PID 90787). It was not replaced with the newly committed UI build. Observations
below therefore describe that installed UI, not fresh native acceptance of every
change in the resulting source tree.

The preceding Workflow/Column refinements were committed as `2ce76c9` before
review began. The selected Project was `canban` and both Cards were in Review.
Their descriptions, development reports, review reports and actual diffs were
inspected. The existing reports included isolated App/Gallery acceptance and
231-test runs for each Card; those earlier native checks were read as evidence,
not presented as newly repeated interactions.

Both deliveries used the application's **Accept · commit and merge** control.
The App displayed a `Merged into main` event and matching accepted-delivery hash
for each operation. Git inspection confirmed clean target and Card checkouts.

| Card | Accepted change | Card commit | Integrated delivery |
| --- | --- | --- | --- |
| #12 | Board-only Drafts entry and bottom-pinned Settings | `aadbd94` | `91ab162` |
| #11 | Archive/Restore for local Cards, without Deleted Cards recovery | `ad4e565` | `0cd356b` |

Both Cards remain unarchived, with their reports and conversation history intact.
Delivery is separate from Archive. No push was performed.

## Reproduced delivery blocker and repair

The original installed backend rejected both previews with **End active Card
Agent terminals before accepting code**. Card #11's Timeline already said its
idle Agent had exited. Card #12 was normally closed using its terminal close
button during this exercise; refreshing its preview still produced the blocker.

The read-only runtime snapshot explained the contradiction: Develop and Review
reused the same Core/terminal binding. Earlier assignments were marked
`continued`, retaining their last `live` observation. The latest assignment was
finished, and the terminal list correctly omitted the exited conversation, but
`app-server/review.rs` inspected all historical observations without the shared
Attempt lifecycle rule. The UI offered no remaining live terminal to close for
Card #11.

Fix `ce2650e` uses `Attempt::finished()` for the shared preview/merge blocker.
Historical continued assignments no longer claim runtime ownership. Current
live/disconnected assignments and queued, starting or unknown requests continue
to block delivery. An isolated regression first failed with the original check,
then passed through preview, merge, idempotent retry, restart and Archive
separation. Additional assertions retain the active/uncertain-work protections.

After explicit user approval and isolated verification, only the AppServer was
restarted: PID 90783 was shut down with an expected-PID guard, and fixed PID
95788 started from the stable local bundle
`.local/review-delivery-20261111/Canban.app`, using the existing `~/.canban` home.
The existing UI reconnected and both deliveries then succeeded. Herdr Core and
unrelated processes were not stopped. The installed `/Applications` bundle was
not modified; this running fixed backend is a local-build override, not an
installed release update.

## Interaction and visual findings

These are follow-up recommendations, not additional UI changes made in this
exercise. The false delivery blocker above was the only blocking defect found.

| Priority | Observed friction | Suggested direction |
| --- | --- | --- |
| P2 | A disabled acceptance button's reason is in the lower diff panel, detached from the button; the original terminal blocker did not identify the responsible assignment or offer a usable next action. | Put the reason beside the acceptance control and link to the exact owning conversation; distinguish historical records from live runtime. |
| P2 | The diff is one long Markdown code block with a plain joined file list. Reading several files requires extensive vertical and horizontal scrolling. | Add file navigation, per-file sections and clear added/removed-line styling. Keep the patch accessible without letting the delivery form dominate the reader. |
| P2 | After successful delivery the form remains, acceptance becomes disabled, and the reader again says to refresh before accepting. Success is a muted full-hash line plus Timeline activity. | Show an explicit successful delivery receipt with a short commit and target; make reviewing subsequent changes a separate clear action. Preserve explicit Archive. |
| P3 | Long reports and old runtime input-wait events compete with the current outcome. The narrow Timeline split makes review reports expensive to scan. | Expose the latest development/review outcomes directly and collapse resolved runtime chatter while retaining history. |
| P3 | The header says `fiber` while the Board says `canban`. Source inspection confirms the header is an intentional shortcut to another Project, not stale selection. Its visual role is ambiguous. | Make other-Project shortcuts visually distinguishable from current-Project identity. |

The Card #12 reviewer separately observed the current Project label disappearing
at 440 logical points, with actions still reachable. That is prior review
evidence, not a newly repeated narrow-window check or an established regression
introduced by this patch. Reserve readable Project identity in a compact header.

Card #11's P3 finding about contradictory lifecycle/Gallery documentation was
confirmed after integration. The present-tense contracts in
`card-conversation.md` and `ui-components.md` were reconciled with Archive/Restore,
GitHub Remove from Board and the renamed Archived Cards scene. Earlier dated
Delete/recovery checks remain explicitly historical.

## Integrated verification

- `mise run test`: 232 workspace tests passed after both deliveries.
- `mise run acceptance`: rebuild/package plus disposable-home/Core acceptance
  passed (`/private/tmp/cb-l5x513m6/home`).
- `mise run build-gallery`: integrated production-component Gallery compiled and
  packaged; this check did not launch it or repeat the earlier native scenarios.
- `git diff --check`: passed.

Final test and acceptance logs are saved in
`/private/tmp/canban-review-final-tests.log` and
`/private/tmp/canban-review-final-acceptance.log`.
Read-only delivery/history checks are captured in
`/private/tmp/canban-review-final-deliveries.json`.
The real interaction evidence is the existing App's successful delivery controls
and receipts. The isolated automated acceptance does not imply authenticated
model, IME, or new native validation of all merged UI behavior.
