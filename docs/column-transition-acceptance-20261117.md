# Column transition dispatch acceptance — 2026-11-17

Card #105 fixes source-column automation being invoked when a user moves a Card
from Develop to Review. The implementation was verified on branch
`canban-0afd/105`, based on `c8deed42908f29512b386a92d76e17be8d56be1b`.

## Behavior

- Transition entry views match `to`; exit views match `from`. Historical events
  with the old column in `stage` retain the correct direction during replay.
- Creation/restoration enter `stage`; archive/removal leave `stage`. Same-column
  facts and reorders do not create entry/exit views. Mixed selector alternatives
  each retain their column constraint.
- Manual movement and Agent-return-to-Backlog facts are appended after saving
  the resulting Card, keeping `stage` and `card_revision` consistent. Workflow
  moves, adopted moves and column-removal moves already saved before appending.
- Desktop API advances from 61 to 62 for the corrected event metadata contract.
  Workflow YAML and stored historical events are unchanged.

## Verification

- `bu exec -- mise run test`: **575 passed, 1 configured skip**. Regression
  coverage includes stale event metadata, membership direction, mixed types,
  real `card.move` forward/reverse dispatch and returned-to-Backlog metadata.
- Standard `mise run build` completed after staging the current Bullet artifacts
  for the existing packager. Standard `mise run acceptance` passed with isolated
  application data and Herdr Core. The final LLVM CLI also passed the standard
  acceptance script (`ACCEPTANCE_PASSED /private/tmp/cb-sxy0urkn/home`).
- The extended workflow acceptance script completed using the LLVM comparison
  CLI described below: `WORKFLOW_ACCEPTANCE_OK /private/tmp/cb-workflows-ptaeshah`.
  A deterministic developer ran over a real isolated PTY, completed while the
  Card remained in Develop, and checkpointed its deliverable. A manual move to
  Review started only the delivery entry, merged locally and completed both
  runs. The developer remained at execution number 1; no resolver was launched.
- A signed native acceptance bundle copied from `.local/Canban.app` used that
  same isolated home/Core. A second Card was moved through the **Card stage
  menu** from Develop to Review. The UI showed Merging then Merged, its Timeline
  recorded the local merge, and the resulting file was verified on `main`.
  Backend evidence confirmed two completed workflow runs and one developer
  execution. The background drag attempt had no effect; physical drag/drop
  acceptance is not claimed. The isolated UI, AppServer and Core were stopped.

## Build-path limits

Bullet artifact discovery/packaging limitations are tracked by bullet Cards
#1/#2. Its Debug CLI also reproducibly crashed with SIGSEGV in
`execute_workflow_delivery` during single-card integration. This independent
build-path failure is tracked by **bullet Card #5**; its compiler/cache root
cause remains unproven.

Without source changes, this Bullet LLVM build passed the complete workflow
script and native menu-driven merge:

```sh
RUSTUP_AUTO_INSTALL=0 mise exec -- bu exec -- cargo build --release --config profile.release.opt-level=0 -p canban-cli
```

The exact output was staged as `target/debug/canban` and packaged without running
another Debug build that would replace it. Runtime acceptance invoked the
existing Python scripts after satisfying their build prerequisites. The final
local bundle combines the normal Debug UI with this LLVM CLI; this is a
validation workaround, not a change to the repository's build configuration.
No native Cargo comparison, installed-app update, live Card #102 recovery,
upstream push or live provider run was performed.

Evidence: `/private/tmp/canban-105-test.log`,
`/private/tmp/canban-105-acceptance.log`,
`/private/tmp/canban-105-acceptance-llvm.log`,
`/private/tmp/canban-105-acceptance-workflows-llvm.log`,
`/private/tmp/canban-105-native-evidence.json`, and
`/private/tmp/canban-105-native-merged.png`. Successful workflow metadata is in
`/private/tmp/cb-workflows-ptaeshah/evidence.json`.
