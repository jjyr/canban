# Unified Card reminders

Status: implemented, 2026-11-10. This supersedes the action-only Project and
Overview badge contracts. Native macOS notifications remain deferred.

## Product contract

Projects count **Cards with reminders**, not messages or Agent rows. A Card
counts **reminder items**: unread Agent updates and unresolved user actions,
deduplicated when they describe the same outcome.

```text
Project [2]                 two Cards with reminders
  Card #42 [3]              two unread replies + one permission wait
  Card #51 [1]              one unread report awaiting Review
  Card #63                  no badge

read report on #51          [1] remains: Review still needs a decision
approve/rework #51          its pending item ends; Project count updates
```

An unread reply is information, never a Card attention flag, workflow gate or
Scheduler restriction. Reading cannot grant permission, continue a task, accept
Review, retry an Agent or launch a Terminal.

Board Cards show a compact number beside their grip icon. Current Project and
other Project buttons use the same projection; Quick Open's Project results
inherit their counts. Zero is hidden. Warm badges indicate at least one pending
action; unread-only badges use a quiet neutral tone. The Overview rail counts
Cards with either kind of reminder across Projects, while its execution summary
continues to describe actual Agent/action state.

Overview keeps existing runtime groups. **Unread updates** lists unread-only
Cards across all Projects with a message excerpt and direct Card navigation.
Cards already awaiting an action stay in their existing Needs you/Review rows,
with unread context there. Opening Overview does not mark messages read.
The Card's **Unread updates (N) ↓** action reuses its existing jump-to-latest
control, making replies below a long description reachable without automatic
scrolling on entry.

## Ownership and event identity

```text
Agent CLI reply / final report       Herdr / task / Review state
             |                                  |
        saved Timeline                     current user action
             +-----------------+----------------+
                               v
                  canban-notifications projection
                  stable item ID / unread / pending
                               |
          +--------------------+---------------------+
          v                    v                     v
      Card numbers        Project/Overview       UI sound delivery
          ^
          |
    SQLite CardRead cursor <--- foreground visible Timeline rows
```

`canban-notifications::reminders` owns classification, stable identities and
deduplication, without UI/runtime IO. Explicit `comment.replied` Agent events and
final `execution.reported` events (completed/backlog/blocked/failed) qualify as messages.
Progress, user comments, ordinary runtime changes and terminal output do not.

A report and its generated question replies share the report ID, including when
there is no report file. Older file-backed echoes can associate by request and
resource path. A human gate for that outcome attaches to the same report item.
Independent permission waits use their own wait episode ID. Reading clears the
unread flag; the pending flag follows the authoritative condition.

Archived/removed Cards are excluded. Restoring a Card preserves its read cursor;
archive exports include that cursor. Legacy `Notice.read` is retained for its
existing callers, but does not determine unread replies or approve a gate.

## Durable reading

SQLite schema 8 adds a `card_reads` record per Card, using a Card-local Timeline
sequence. It does not change Card revisions or write another Timeline event.
Migration baselines existing history as read to avoid an upgrade notification
flood; pending actions remain visible. New replies after that baseline are unread.

The reader uses existing row layout/scroll callbacks and macOS foreground state.
After a short event-triggered dwell, it advances the cursor through message rows
actually intersecting the viewport. Opening a long Card above its replies,
keeping the App inactive/background, or covering it with an application dialog
does not mark those replies read. Scrolling forward advances a read-through
position, not individual message receipts. No independent visibility poller or
per-row Herdr/Git query is introduced.

AppServer exposes `notifications.list` and `notifications.read`:

```json
{"card_id":"...","sequence":42}
```

The read operation is monotonic and clamps to published history. It rejects an
Agent execution context; it cannot mutate a Card, request or gate. Client read
responses also merge monotonically so delayed responses cannot move backwards.
Desktop API version 6 requires the matching AppServer when updating a running
development installation.

## Sound delivery

The UI loads a bundled original 240ms chime through NSH's native audio API. Audio
is local to the viewing computer, independent of the Agent's execution machine.
It requests no notification permission and does not call macOS notification APIs.

Audible notifications (`canban-notifications::sound_items`, updated 2026-11-14)
are decoupled from visual unread badges so automated multi-step pipelines do not
chime on routine stage transitions, and runtime `idle` / `done` status transitions
stay silent so transient Agent pauses do not trigger false notification chimes.
Sounds play only for four categories:

1. **Human intervention and `@agent` replies:** runtime permission/input waits
   (`wait:*`), independent CLI terminal `blocked` waits (`terminal-attn:*:blocked`),
   workflow approval/human gates and stale-input holds, Agent `blocked` or
   `backlog` reports, and explicit `comment.replied` answers to `@agent`
   questions. Routine automated `completed` stage reports without a human gate or
   `@agent` question remain unread badges without playing a sound; interactive
   runtime `idle` / `done` turns (`idle:*` and CLI terminal `idle`/`done`) keep
   their visual reminder/attention indicator without playing a sound, while
   automatic stage/workflow runs pausing in `idle` / `done` during background
   subagent or tool work stay suppressed until they have remained stalled without
   status change, user interaction, or task report for at least 1 hour
   (`UNREPORTED_IDLE_ALERT_MS`, surfacing `stalled:*` in `Needs you` with a chime).
2. **Agent failures:** `failed` reports, unreported exits (`execution.failed`),
   missing terminals (`execution.missing`), 1-hour unreported automatic stalls
   (`stalled:*`), runtime errors (`terminal-attn:*:error`), recovery states and
   uncertain question deliveries.
3. **Merge failures / paused deliveries:** `review.merge_failed`,
   `review.upstream_sync_failed`, `github.pr_merge_attention`, and deliveries
   held for a human after exhausting `max_conflict_attempts` or encountering
   non-reworkable conflicts. Automatic in-budget `review.merge_conflict` rework
   routed back to an Agent stays silent.
4. **Completed merges:** background Queue merge deliveries (`review.merged` by
   `system`) and remote GitHub PR merge reconciliation (`github.pr_delivered` /
   non-user `github.pr_merged`). Synchronous manual merges initiated directly by
   the user (`actor == "user"`) rely on immediate UI toast feedback without a
   redundant notification chime.

New audible item identities enter one 650ms delivery batch. Immediately before playback,
the UI rechecks whether any batch item remains active in `sound_items` and whether sounds
are enabled. Thus promptly read visible replies and already-resolved waits need
no sound. Repeated snapshots or message edits do not replay a delivered item.
Initial connection and reconnect establish a silent baseline. Re-enabling sounds
does not replay muted deliveries. **Settings → Notification sounds** persists in
the existing AppServer-home `config/desktop.json`, independently of Theme and the
last Project. It is a desktop preference, not a setting on each execution machine;
clients sharing that AppServer also share its saved desktop preferences.

This reuses the existing App subscription/refresh path. Its current client
transport compares AppServer snapshots; this feature does not introduce another
background scanner or claim to replace that transport with a push protocol.

## Regression and acceptance

- `CANBAN_GALLERY_SCENE=reminders mise run gallery` uses production components for
  multiple Projects, pending/unread badges, long context, live/batched replies and
  Overview. **Focus window** requests native foreground activation; the reader
  still follows actual OS lifecycle, without a foreground override.
- `mise run acceptance-notifications` checks structured replies, permission
  episodes, report/gate deduplication, persistent monotonic read state, sound
  preference and restart over deterministic providers in real isolated Herdr PTYs.
  `--native-bundle PATH` can hand its prepared data to an existing test App bundle.
- `CANBAN_NOTIFICATION_TRACE=1` logs native audio readiness/play requests and
  reader geometry/lifecycle, without logging message bodies. It is disabled normally.

### Acceptance on 2026-11-10

The application/Gallery builds, strict all-target Clippy and 158 workspace tests
passed. A final report-echo regression then passed with all 30 UI package tests.
The standard workflow acceptance passed at `/private/tmp/cb-z93ctkvx/home`.
The reminder-specific real-PTY harness passed at
`/private/tmp/cb-notifications-npj0bjoa/home`; its native handoff rerun passed at
`/private/tmp/cb-notifications-hny1beo4/home`.

Native Gallery checks covered multiple replies increasing one Card's count without
increasing its Project's Card count, one sound request for a two-reply burst,
quiet initial loading, pending/unread Overview groups and foreground/inactive
read handling. A real `Active` lifecycle transition cleared the visible short
Card reply; inactive reading and below-fold messages retained their badges.
Trace evidence is in `/private/tmp/canban-reminders-gallery-verified.log`.

The final packaged App ran from the previously approved native acceptance bundle
with fresh isolated home/Core data. Its Board showed three affected Cards in one
Project plus one in another; Overview showed the task gate independently of its
three unread-only Cards. The unread jump opened the actual saved Agent answer.
The native sound checkbox persisted its change: a real structured reply received
while muted updated counts without playback; re-enabling did not replay it, and
the next reply produced exactly one native sound request. The native sound decoded
successfully as 240ms. The App's read checks remained correctly inactive during
the automated window inspection; persistent read/gate behavior and restart were
additionally exercised through the production API, not represented as native
foreground clicks. The native run log lives in that acceptance directory.

These use deterministic providers over real PTYs, not new Codex/Claude model
evaluations. Playback evidence establishes the native audio load/play path;
physical speaker volume or audibility was not measured. Chinese IME was excluded
as previously requested. No foreground override or macOS notification adapter was
added for acceptance.
