# Card conversation refinement acceptance — 2026-11-11

Scope: uncommitted Card UI changes on top of `0723969` in
`worktree-optimize-overview`. This report covers the implementation in the
associated diff, not a release or live GitHub acceptance.

## Implemented behavior

- Local, GitHub Issue and GitHub PR identity use the fixed header; remote source
  state and local Workflow stage are separate. Source description and editable
  local context are distinct. Archive/Restore and Stop workflow live in the menu.
- Removed the permanent GitHub URL/Add/Remove form and unused source panel.
  Saved content supplies the read-only reference summary, deduplicated and
  excluding self/delivery relations. Legacy explicit links remain supported.
- Shared human/Agent message presentation, visual-height report folding,
  adjacent routine execution groups, and contextual approval/rework controls.
- Selected historical execution details appear at their request in the Timeline.
  Measurement relative to the scroll content root fixes nested layout offsets
  for execution reveal and event/read positioning.
- Local quick replies remain fixed below the reader. Nonempty Markdown drafts
  stay expanded after blur. Source/Preview, explicit recipients and Cmd+Enter
  retain their existing ownership.
- Explicit hash completions retain stable targets. GitHub-owned Cards rank remote
  candidates first when available. Ambiguous bare hashes in connected Projects
  remain literal. Connected Issue/PR root links use native details; disconnected,
  fragment/query/subpath links retain browser destinations.

## Automated checks

- `mise run test`: full workspace passed, including 49 UI tests. Added reference
  extraction/deduplication/source exclusion and routine-group boundary coverage.
  Final rerun after the coordinate fix: `/tmp/card-test-final.log`.
- `mise run build`: final packaged app passed, `/tmp/card-build-final.log`.
- `mise exec -- cargo clippy -p canban-ui -p canban-ui-kit --all-targets -- -D warnings`:
  passed after the final fix, `/tmp/card-clippy-final.log`.
- `mise run acceptance`: passed using disposable home
  `/private/tmp/cb-_fcie0zf/home`, isolated Core
  `111cfffa1e4d1fb8c5ee6397efbde7ce`. Verified fixture Develop/Review, explicit
  results, mentions/blocking, Archive/Restore, restart and global-budget
  concurrency. This backend run preceded the final UI coordinate correction.
- `git diff --check`: passed.

## Native Gallery

Used `mise run gallery` with production components and deterministic
`card-local`, `card-issue`, `card-pr` scenes. Local scene: 760×820 points;
Issue/PR scenes: 560×780 points.

Observed local/Issue/PR identity, title wrapping, separate source/local context,
self-reference exclusion, compact reference summary and explicit local reply
scope. Clicked routine-event expand and full-report expand. Failures remained
individual entries. In the PR scene, clicked Details, checked exact completed
request content, and closed details.

The first Details check exposed parent-relative coordinate misuse: expansion
was correct but scrolling left the details below the viewport. Replaced the
position calculation with root measurements relative to the scroll content,
rebuilt, and repeated the interaction. The selected details then appeared
immediately below the fixed header. No Agent was started by inspecting history.

## Packaged native App

Used `mise run acceptance-editor`, disposable home
`/private/tmp/cb-editor-bb2_ghkg/home`, and its separately identified app/Core.
The user's installed home and live Core were not modified.

1. Opened local Card #1. No GitHub form or empty References section appeared.
2. Entered a reply with `https://github.com/acme/app/issues/42` and
   `https://github.com/acme/app/pull/57`. Neither appeared in References while
   still a draft. Clicked the reading area: the nonempty draft remained expanded.
3. Sent through the native Send action. Both references appeared once; the
   Timeline contained the reply, and the submitted composer cleared.
4. Typed `#2`, observed the local Card candidate, accepted with Tab, and sent
   using Cmd+Enter. AppServer recorded the stable `[Editor acceptance #2](card:...)`
   link. All three fixture Cards remained local and the snapshot had zero requests.
5. Quit the disposable app; its harness shut down the owned service/Core. Gallery
   processes were also stopped.

The packaged interaction run preceded the final isolated coordinate-only UI
fix; that fix was rechecked in the production-component Gallery and included in
final build/test/Clippy results.

## Limits

No live authenticated GitHub network acceptance, remote comment publication,
PR merge or paid model execution was performed. Issue/PR identity/layout was
verified with cached deterministic fixtures. Existing backend source/delivery
ownership is unchanged. References are a UI projection of saved content, not a
new persisted graph or automatic GitHub mutation. No database migration is needed.
