# Card and comment submission latency acceptance

Executed 2026-11-16 on macOS, Card #101, branch `canban-0afd/101`, based on
`d2572f49d2ec6f8f834e09ce95f5de4a62973a46` plus this change.

## Change and regression boundary

Card creation (local and GitHub intake) and comment Send complete on the write
RPC acknowledgement. The presenter projects the returned Card/Event before
calling the form's success handler, then requests a coalesced background refresh.
The existing per-method pending guards and draft identity/content comparisons
remain in place. Failed writes never enter the projection. A failed refresh is
reported separately and cannot turn an acknowledged submission into a failed one.

Confirmed writes survive older/cached snapshots until authoritative state
includes them. Newer Card revisions and edited/deleted comments win on
reconciliation; a confirmed Card removal clears its creation receipt. Refreshes
started before a later acknowledged mutation are discarded and retried. A refresh
requested while another is running is retained. Other operation callbacks keep
their existing full-refresh path.

Host summaries reuse the fetched snapshot. Remote submission acknowledgement
does not synchronously fetch the full remote state; its existing supervisor is
woken, and new Card ownership is retained for routing while the cache catches up.
Server admission checks return early when no capacity gates are waiting, and
otherwise load only scheduler inputs, including Questions needed for idle-session
eligibility. No RPC payload, schema, workflow definition or prompt changed.

## Automated checks

- `mise run build`: passed, packaged `.local/Canban.app`.
- `mise run test -- --test-threads 4 --no-fail-fast`: 564 passed, one deliberately
  ignored provider test skipped. An earlier run, concurrent with compilation and
  native checks, hit the existing background Git push fixture's five-second
  startup deadline. That test passed alone in 2.62 seconds and in the full rerun;
  its assertions/timeouts were not changed.
- After the final removal-receipt cleanup, `mise run test -- -E
  'package(canban-ui)' --test-threads 4` passed all 151 UI tests; the final
  packaged build and changed-crate Clippy also completed.
- New real-socket presenter regression: Card creation and a subsequent comment
  callback finish while snapshot reads are blocked; releasing them as errors
  neither repeats writes nor loses the coalesced follow-up refresh.
- Projection regression: stale remote state, duplicate-free comments, host
  ownership, authoritative edits, and removal before the first refresh.
- Admission projection equivalence and the existing capacity/ready-gate tests
  passed; the multi-host test checks immediate routing ownership followed by
  explicit supervisor reconciliation.
- `mise run acceptance`: passed with disposable home/Core.
- Changed-file Rust formatting and `git diff --check`: passed.
- Clippy of the changed crates with all targets completed. Strict workspace
  Clippy is not clean at the base: `ui-kit/src/button.rs` has
  `items_after_test_module`, and `app-server/src/github.rs:569,583` have
  `needless_borrow`. These unrelated files were not changed.

Logs are under the Card run directory, including `test-final.log`,
`test-delivery.log`, `acceptance.log`, `clippy.log`, and `clippy-final.log`.

## Native interaction evidence

The packaged App was copied to uniquely identified disposable bundles, using
independent Canban homes and Herdr Core sockets. A scratch Unix-socket proxy
forwarded real AppServer requests while selectively blocking writes/snapshots or
returning fixture errors. Fixture setup and assertions used the CLI, never direct
database access. No paid Agent or installed-user data was used.

`/private/tmp/cb-submit-cft1w6ku` records the expanded interaction run:

- Two consecutive native Create actions finished while all snapshot replies
  remained blocked. Both Cards appeared and each submitted form reset.
- Two consecutive Send actions appeared once each in Activity with the composer
  cleared, despite the blocked snapshots.
- An acknowledged Send followed by refresh errors remained successful and visible.
- A rejected write retained the draft and showed its error; retry saved it once.
- Typing after submission, then switching Cards while the write was blocked,
  preserved the newer draft and did not change the selected Card on completion.
- Creating while typing the next title saved the submitted title and kept the
  newer title in a fresh New Card draft. `create-preserved-draft.png` shows both.
- CLI assertions found exactly five Cards, five distinct submitted comments and
  zero Agent terminals. `results.json`, `drafts.json`, `rpc.jsonl` and PNGs retain
  the evidence.

`/private/tmp/cb-submit-nkeqb2xu` repeated blocked-snapshot consecutive creation
and blocked/failed-refresh Send on the freshly packaged UI/CLI; `results.json`
records binary hashes and equality with that `.local/Canban.app`. It found four
Cards, two distinct comments and zero Agent terminals. The final removal-receipt
cleanup is covered by the projection regression.

Observed native postcondition waits in the latter run were 245–281 ms for Create
and 161–164 ms for Send. These are Cua polling observations on small fixtures,
not frame/input latency benchmarks or measurements against the installed dataset.
The proof is that completion occurred while snapshot replies were unavailable.
All fixture UI/server/Core processes were stopped. Chinese IME, paid providers
and remote-network performance were not exercised.
