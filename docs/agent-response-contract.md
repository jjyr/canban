# Agent RPC and CLI response contract

The RPC and CLI are Agent-first. Return the smallest useful response by default;
request task text and complete business objects explicitly. AppServer owns field
selection, filtering, pagination and receipts. CLI formatting must not implement a
second set of domain projections. This is a breaking response-contract change,
introduced with desktop API 65; upgrade owned clients together.

## Content and formatting

- Default query/mutation CLI stdout is one compact JSON value followed by a newline, independent
  of whether stdout is a terminal. Errors are JSON on stderr with a nonzero exit
  status. Clap usage errors retain their standard diagnostics.
- `--human` pretty-prints the same selected JSON. It does not request more fields.
- `--fields number,title,body,revision` sends `fields` as an array to RPC.
- `--full` sends `view: "full"`. It requests the complete business view, not an
  old wire format. There is no legacy `--raw` mode.
- `fields` and `view` are mutually exclusive. This version supports explicit
  top-level field names, not arbitrary expressions or nested paths. Invalid
  selections fail before dispatch, mutation or durable handoff enqueue; valid
  optional fields that are absent are represented as null.
- Paginated response control fields and mutation identity/status/revision fields
  are not removable through projection. Response options are stripped before
  business execution and idempotency fingerprinting. Existing operations records
  are not rewritten; replay returns the same committed result through the
  requested view.

Text-oriented commands (`skill`, legacy Note text reads and exported Workflow
files) keep their content contracts. Unknown/unprojected methods retain their
method-specific result; their CLI JSON is still compact. Do not recursively
delete keys named `body` or `message` from arbitrary results.

## Current method defaults

| Method | Default result | Explicit expansion |
| --- | --- | --- |
| `card.list` | `{items,next_cursor}`; each item has `id,number,title,stage,revision` | `fields`, `view: "full"`; always paginated |
| `card.get`, `card.show` | `id,number,title,stage,revision` | `fields`, `view: "full"` |
| `card.create`, `card.update`, `card.move`, `card.restore`, `card.context.update`, `card.hold_auto_merge` | `accepted,id,number,stage,revision` and request key when supplied (optional `assignee` and `stage` supported on `card.create`; `card.update` binds `assignee` without waking) | `fields` adds selected business data; `view: "full"` returns the business object |
| `card.adopt` | `accepted,adopted,id,number,stage,revision,workflow_run_id` and request key | `view: "full"` includes Card/workflow/workspace details |
| `comment.add`, `comment.update` | `accepted,id,card_id,sequence,card_revision` and request key | `fields`, `view: "full"`; text only in `body` |
| `timeline.list` | `{events,next_cursor,last_sequence,highwater}`; event identity, sequence, kind, actor, timestamp and comment body | `fields`, `view: "full"`; non-comment payloads are opt-in |
| `report`, `reply`, `batch report`, scheduled reports | Existing small, method-specific acknowledgement plus request key | Preserve distinct accepted/queued/recorded-state semantics |

Archive/remove/delete and other already-small receipts retain their method
contracts. Snapshot/state synchronization and durable `event.list`/subscriptions
retain their domain payloads. The desktop still needs complete state for its
read model; the routine Agent inventory path is `card.list`, not `app.snapshot`.

## Card pagination

`card.list` supports `project_id`, `stage`, `include_removed`, `limit` (default
50, range 1–500) and an opaque string `cursor`. SQL applies project/stage/removal
filters before keyset pagination, ordered by `(number,id)`. Only the selected
page is decoded as business objects. A metadata-only membership/revision scan
provides the consistency generation; this is not a constant-cost query for an
arbitrarily large project.

Continue until `next_cursor` is null, including with `view: "full"`. The cursor
binds the resolved filters and matching Card IDs/revisions. Changes affecting
that membership or those revisions return `cursor_stale`; restart with the same
filters and deduplicate by ID. Changing filters returns `cursor_scope_mismatch`.
This deliberately rejects inconsistent scans rather than silently skipping Cards
that move between columns. A completed page sequence describes one generation;
it does not freeze subsequent Card activity.

```sh
"$CANBAN_CLI" call card.list \
  '{"project_id":"<PROJECT>","stage":"backlog","limit":50}'
"$CANBAN_CLI" card show --card 111 --fields number,title,body,stage,revision
```

## Timeline and comments

`timeline.list` supports `kinds` (array of exact event kinds), `after_sequence`
and `limit` (default 50, range 1–500). Omitted `kinds` means all kinds; an empty
array matches none. SQL filters kinds before selecting a page.

Continue with `next_cursor` while it is non-null. At the end, persist
`last_sequence` for the next incremental read. It advances to the Card's scanned
highwater even when no event matches. A full page ending the scan can also have
a null next cursor. Each new call can include newly committed events; the scan
is not a historical snapshot across all calls.

Comment read views normalize text to `body`, omitting `message` and `data.body`.
Full views retain other event metadata. Stored Timeline events, snapshot events,
durable journal payloads and subscription consumers are unchanged. Typed desktop
consumers accept the normalized `body` alias, and immediate Card/comment writes
explicitly request full views to populate their committed-write overlay.

`event.list` remains a different contract: it pages the journal **before**
selector filtering. An empty page is not completion; continue its global cursor
to highwater. Do not apply Timeline's termination rule to it.

## Receipts, skills and migration

An accepted write receipt confirms the write. It does not establish workflow
execution, provider completion or human acceptance. A handoff `queued` receipt
must never be presented as a recorded report. Concurrency-sensitive writes retain
their expected revision checks; receipts must not be assembled by reading the
Card again after the transaction.

Reuse task context and already-loaded data when sufficient. Request known-needed
fields together. Use file-backed JSON parameters for long text, explicit comment
recipients, stable request keys for retries and incremental Timeline positions.
Do not routinely read back entire objects after acknowledged writes. Business
acceptance and ambiguous outcomes still require targeted verification.

The canonical skill is `skills/canban/SKILL.md`, embedded by canban-context and
materialized under the selected application home. `canban skill` prints that
build's skill. Installed skill copies must be refreshed through their installation
path; a stale global copy is not evidence of the new contract.

Migrate clients that deserialize Cards/Events to explicit full/field requests;
`card.list` consumers must use the new envelope and traverse all pages. Do not
claim default-wire compatibility for old clients. Advance `DESKTOP_API` with
contract changes so installed desktop/backend activation follows the project
upgrade path. Remote clients/daemons must also be upgraded together.

Measure RPC response bytes, CLI stdout bytes and content actually submitted to
the model separately. Repeat the same operation set before comparing strategies
that remove calls. UTF-8 bytes are not token counts; only report tokens when the
actual tokenizer/provider measurement is available.
