# MVP progress and next steps

Assessment date: 2026-11-07. The user selected real daily-use workflows, Review/code delivery and Terminal/recovery as the next milestone. Implementation and bounded acceptance are recorded in [MVP acceptance](../mvp-acceptance.md).

```text
Notes + Card -> Backlog -> Develop -> explicit completion -> Review report
                              |                               |
                    native terminal / @reply         human disposition
                              |                       /             \
                       exact-ID Resume          Develop rework   Diff + Accept
                                                                     |
                                                             commit / local merge
                                                                     |
                                                               Archive / Restore
```

Profiles now use editable JSON presets and generic launch/resume/exit commands. Herdr supplies runtime status/control; Canban no longer injects duplicate notify/Stop lifecycle hooks. Real Codex acceptance passed for active replies, normal completion, human-input blockage, independent Reviewer context, repeated rework and exact-history Resume.

Basic native Diff and explicit commit/merge are implemented. Reviewed real work was merged into disposable target branches, independently tested there and archived/restored with readable reports. Accept commits the whole reviewed Card checkout; conflicts stay in the Card worktree. Source local edits and stale previews are protected. Archive remains separate. See [Review delivery](review-delivery.md) for policy and failure/retry behavior.

Native long-output validation found and fixed inverted scroll direction and fractional-scroll loss. Core outage, host restart, missing provider history and explicit fresh-context recovery have deterministic acceptance; selected paths were also operated in the production native UI. The user requested skipping Chinese IME verification. This is a focused local MVP, not exhaustive stress/recovery acceptance.

Next, use the build for ordinary repository tasks and record friction before expanding scope. Prioritize any failures in report clarity, review navigation, Git conflicts or resume recovery. Repeat real Claude acceptance after valid local authentication is available; earlier startup was blocked by Invalid API key. Full Shear review, GitHub backends, template GUI, advanced timers and daemon extraction remain deferred.
