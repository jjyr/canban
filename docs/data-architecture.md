# Data architecture

Status: updated 2026-11-06. The selected ~/.canban/, SQLite WAL, file-backed execution content and compressed Card archive direction now has a first implementation using checksummed tar+zstd packages. See [implementation and acceptance](implementation.md) for actual behavior and verification limits. Broader mechanisms below remain design references where not implemented.

## Recommended storage boundary

Use ~/.canban/ as the application home and SQLite with WAL for structured storage. Recommend one database containing all Projects, with execution input/report bodies and attachments stored as managed files. Archived Card details and related execution content move into compressed packages, while SQLite retains their lookup records. Keep repository files and worktrees in their own directories. Herdr continues to own its runtime state.

```text
NSH UI ------ typed Rust calls ----+
                                  |
Agent CLI --- local RPC -----------+--> AppServer
                                        |
                       domain crates own rules and operations
                                        |
                               shared storage boundary
                                        |
                 +----------------------+---------------------+
                 |                      |                     |
          canban.sqlite            managed files         Herdr adapter
          current records         images / reports            |
          history/revisions       captured resources      herdr-core
          execution ledger                                terminals
                                                         holders / PTYs

Project directory / Git worktrees: code being worked on, outside the database
```

A single database makes global scheduling, cross-Project references, notifications, and consistent state/history writes straightforward. Separate crates do not need separate databases. The database belongs to AppServer's storage boundary; UI and agents use application operations rather than writing tables.

Per-Project databases would make individual Projects easier to move as a unit, but would require coordinating global scheduling and references across stores. Prefer an explicit Project export/import later if portability becomes a requirement. This is a local single-installation proposal; collaboration and multi-device sync are outside this draft.

## SQLite WAL

Enable and verify PRAGMA journal_mode=WAL when initializing/opening the database. WAL allows readers alongside a writer; SQLite still serializes writers. Keep transactions short and perform Herdr calls, file IO, and archive compression outside write transactions. Connection settings, busy handling, checkpoint policy, and synchronous mode remain implementation refinements; WAL alone does not select the power-loss durability policy.

The live database may have canban.sqlite-wal and canban.sqlite-shm sidecars. Let SQLite manage them; never delete them as disposable application cache. Use a supported consistent backup procedure as described below.

## What belongs where

| Data | Durable owner | Suggested representation |
| --- | --- | --- |
| Project, directory binding, Board | Kanban | Relational records; stable IDs independent of directory paths |
| Cards, stage, attention, archive marker, timing attributes | Kanban | Current-state rows, project-local card number, revision |
| Workflow templates and routing configuration | Workflow Engine / Scheduler | User-editable JSON inputs and saved validated revisions |
| Agent profile definitions and overrides | Profiles / AppServer | Editable command/preset configuration; Requests retain resolved snapshots for Scheduler execution |
| Scheduling requests and execution attempts | Scheduler | Persistent queue/intent and attempt records |
| Core/terminal association and last observation | Herdr adapter via Scheduler | Optional runtime binding and observed status/time |
| Descriptions, comments, resource references; retained legacy Notes | Context / Timeline, according to content ownership | Stable content IDs and immutable revisions; current-revision pointers |
| Captured execution input | Context | Managed prompt/context files and snapshot manifest; database stores identity, revisions, and file references |
| Card history, execution milestones, report links | Timeline | Append-only typed entries with per-card ordering |
| Result declaration and report | Scheduler / Timeline | Structured declaration and report reference in the database; report body in a managed file |
| Internal notifications and read state | Notifications | Durable notice records; optional Project/Card/event references |
| Pasted images, retained reports and captured files | Context / shared storage | Managed bytes plus database metadata and references |
| Terminal screen, PTY/process lifecycle, runtime registry | Herdr | Core-owned runtime data; Canban saves selected observations/results |
| Repository files, branches, worktrees | Git / filesystem; Workspaces tracks bindings | Paths and Git references in Canban; working files outside SQLite |

These are logical groups, not a final SQL schema or a requirement for one table per row. Owner crates define business rules; canban-storage supplies connection/migration, file-storage, and transaction support. AppServer composes operations that cross owners without moving their rules into storage.

Use relational columns for identity, foreign keys, stage, state, ordering, and frequently filtered properties. JSON fits versioned template/config snapshots and typed event details. Avoid putting an entire Board into one opaque JSON document or duplicating every large body in every history entry.

## Kanban and content relationships

```text
Project ---- directory binding
   |
   +---- Board ---- workflow revision ---- stage definitions
   |        |
   |        +---- Card #42
   |                +---- current stage / attention / archived_at
   |                +---- description revisions
   |                +---- comments ---- comment revisions
   |                +---- Timeline entries ---- actors / content / resources
   |                +---- scheduling requests ---- execution attempts
   |
   +---- Notes ---- Note revisions

Content revision ---- resource references ---- attachment / URL / Card / file
```

Recommend one Board per Project for the initial model. Card identity is a stable internal ID plus a unique (project_id, number) reference for humans. Renaming a Project, moving its directory, changing a title, or archiving a card does not change its identity. Archive is saved state; it retains the number, content, execution records, and Timeline.

The current card row answers "where is this card now?" without replaying history. A Timeline entry answers "what happened, who caused it, and which content/execution did it involve?" Comments have their own content records/revisions, referenced by Timeline entries. Editing a comment saves a new revision and an edit event. One UI Timeline can combine several record kinds without forcing all content into event payloads.

Resource links and captured content are distinct. A live file or URL reference can change or disappear. Preserve bytes/revisions that were actually captured for an execution; a bare path alone does not prove historical file contents. Capturing a context manifest does not imply copying the whole repository or guaranteeing a complete provider transcript.

## Agent data means configuration, execution, and result

```text
Agent profile / existing runnable configuration
                 |
Card trigger --> scheduling request
                 |  trigger + task + callback + routing choice
                 |
                 +--> attempt 1 = one Session / Run
                 |      +---- effective profile/config revision
                 |      +---- context snapshot
                 |      +---- execution directory / worktree binding
                 |      +---- optional (core.id, terminal.address)
                 |      +---- observed runtime state
                 |      +---- agent report / declared outcome
                 |
                 +--> attempt 2, if a fresh retry is requested
```

Current implementation retains one Conversation per (Card ID, profile ID), not one shared conversation for an Agent name across Cards. Profiles are reusable configuration; each Request/Attempt retains its own task, callback and runtime. Attempts are identified by (request_id, attempt_number). The Conversation stores an opaque exact provider-history ID; Herdr does not supply a universal provider ID. This supersedes the earlier attempt-only Session proposal.

Save request and attempt identity before starting external work. A queued request has no terminal yet. Once known, save the owning core.id and owner-local terminal.address; save the connection route separately. The core tuple links to a live runtime, while the Canban execution record survives terminal removal. See [runtime identity research](research/herdr-identifiers.md).

Keep operational state, observed terminal status, and agent-declared completion separate. A terminal becoming idle or disappearing does not constitute a completed report. A completed report invokes the callback saved on its initiating request; an @mention reply therefore does not inherit another request's stage-progression callback. No separate Session types are needed.

Save the launch configuration and input that were actually used, rather than only linking to mutable current defaults. A snapshot can reference immutable revisions to avoid repeated copies. Historical profile/config snapshots remain readable after a profile changes.

Prefer file-backed execution bodies from the start rather than switching between database and files at a size threshold. Save the assembled launch prompt and captured context as files; save each submitted report as a separate immutable file. The database holds the execution key, declared outcome, timestamps, optional short summary, and artifact IDs. A file-backed snapshot manifest can describe captured revisions/resources and their roles without putting the full input into a Timeline payload. This storage convention applies equally to @mention and stage-triggered Sessions.

Persist milestones, useful output, and final reports in Canban. A live terminal viewer can read Herdr directly. Do not turn every terminal byte or render update into a Timeline entry. Long-term full terminal recording is a separate feature, not an implied MVP guarantee.

## Durable history and in-memory events

```text
Accepted operation
       |
       v
SQLite transaction
  update current state
  append associated Timeline entry / content revision
  save required scheduling intent or notification
       |
     COMMIT
       |
       +--> emit Rust enum over in-memory channel
                 +--> UI refresh
                 +--> interested component wakes and reads saved work
```

State and the history describing it must commit together. For example, moving #42 to Develop saves the new stage, its Timeline event, and its scheduling request in one short transaction. Starting Herdr occurs after commit, outside that transaction. Receiving a completed report saves the result and report linkage, advances according to the template, and records any next request consistently. An accepted duplicate report returns the prior result rather than advancing twice.

Modules can contribute to a shared transaction through AppServer composition and storage interfaces. Timeline durability must not depend on a best-effort in-memory subscriber. Persist required follow-up intent and unread notifications; channel messages can wake their owners after commit. The database is the recovery basis if the process exits before sending the wake-up.

This uses ordinary transactional state plus durable business history. It does not require reconstructing current state by replaying all events, a durable copy of every Rust enum, or a new event-sourcing framework. Detailed event topics/channel/subscriber design remains deferred until after MVP. See the existing [Timeline envelope](product/timeline.md) for proposed sequence, actor, causation, and revision fields.

## Files and configuration

The home directory ~/.canban/ is user-selected. The subdirectory and file names below are a proposed layout:

```text
~/.canban/
  canban.sqlite          active state/history and archived Card lookup records
  config/
    workflows/*.json     editable template inputs
    agents.json          Canban routing/profile references and overrides
  projects/
    <project-id>/
      cards/
        <card-id>/
          resources/    Card-owned attachments and retained artifacts
          runs/
            <request-id>/
              <attempt-number>/
                prompt.md
                context.json
                inputs/
                reports/
                  <report-id>.md
  attachments/           attachments shared across Cards
  artifacts/             other shared retained artifacts
  archives/
    <project-id>/
      <card-id>/...      compressed Card packages; format/version recorded in DB
  worktrees/
    <project-id>/...     suggested location for Canban-managed Git worktrees

<project-directory>/     user's source files, usually a Git repository
```

Organize managed execution files by Project -> Card -> request -> attempt. This supersedes the earlier top-level runs/<request-id>/<attempt-number>/ proposal, which did not express Card ownership in the filesystem. Both @mention and stage-triggered executions for a Card belong under that Card. The request/attempt pair still identifies the unified Session; the parent directories express ownership rather than introducing another execution identity. Use stable internal Project/Card IDs in paths, not mutable titles or just the project-local display number.

The database retains explicit Project/Card/request associations and artifact records; it must not infer ownership merely by parsing paths. An attempt belongs to its saved request, and the request belongs to its Card. Artifact IDs resolve to managed files or archived entries. The context manifest records Project/Card/request/attempt identity and exact input/config revision references, while prompt.md contains the assembled launch prompt. Shared attachments can be referenced rather than copied into every run; Card-exclusive bytes belong in the Card's resources/ or execution inputs/ directories.

The projects/ subtree is Canban-managed data, not the user's source repository. Source Project directories remain at user-selected locations; only Canban-managed worktrees are proposed under ~/.canban/worktrees/. Keep worktrees separate from retained execution content. After successful archiving, the Card's execution content can live in archives/ instead of its expanded managed directory. These files are Canban's captured input/results, not Herdr's internal state or a provider's complete conversation store.

Config JSON is an editing/import surface. On successful validation, save an immutable revision in the database and activate it explicitly. Work already queued/launched keeps its bound revision. Invalid edits leave the previous active revision intact. Keep the source path/digest as provenance; do not let startup silently replace saved execution history with current file contents. Project-local config overrides and reload policy can be refined later.

Managed files have stable IDs and database metadata such as media type, byte size, relative storage path under ~/.canban/, and optionally a digest. IDs, rather than absolute paths, are exposed in business references and Timeline payloads. Saving bytes must succeed before a committed content record points to them. A crash can leave an unreferenced file; cleanup can remove it later. Attachment deletion must consider retained revisions and context snapshots, not just current card text.

SQLite and filesystem writes cannot share one transaction. For an input or report, write a temporary file, finish the durable write, and atomically publish it at a new managed path before committing its database reference. Commit the report's structured result, artifact reference, Timeline entry, and configured progression together; acknowledge acceptance after that commit. A failed file save must not produce an accepted report or advance the card. Unreferenced published files are recoverable orphans, not accepted results. Repeated report submissions use the existing request-key deduplication and must not overwrite a previously accepted file.

Timeline entries keep concise structured details and references. Card descriptions and ordinary comments remain in SQLite. Notes are [deferred](notes-deferred.md); their legacy bodies, immutable `note_revisions`, ordered `note_events`, links and Card associations remain stored without a product editor or automatic injection into new captures. Existing captured files remain historical input. The earlier [Note model](product/notes-and-workspace.md) and [implementation evidence](notes-and-workspace-implementation.md) describe that retained compatibility schema, not current product scope. Load full report/context content on demand rather than for every Board or Timeline query. Full-text search, attachment limits and very large report rendering remain refinements.

## Compressed Card archives

User decision: when a Card is archived, compress its related data, including Agent execution data and results, using zstd or lz4. History is retained in compressed storage rather than deleted. The following packaging and lifecycle are proposals.

```text
Active Card
  SQLite: content revisions / Timeline / requests / attempts
  Files: prompt / captured context / reports / attachments
                      |
                  Archive action
                      |
          build and verify compressed package
                      |
          publish package and commit archive location
                      |
  SQLite: Card identity + listing fields + archive index
  Archive: detailed Card history + related execution content
```

Include Card content/revisions, comments, Timeline, request/attempt records, captured effective configuration/context, report outcomes/bodies, and required resource bytes/revisions. Include only Agent runtime observations/output that Canban actually retained; this does not promise a backup of Herdr's internal state, live processes, or a provider's complete conversation. Project config and legacy Note records remain shared retained data; preserve any Note content already captured for historical executions, while new captures do not add Notes. Shared attachments can be copied into the package while keeping originals required elsewhere. A live external link stays a link unless its content was captured.

The Card-owned filesystem subtree supplies its execution/resource files directly. Also export that Card's database records from the consistent archive snapshot and include referenced shared resource bytes/revisions. Compressing only the directory would omit database-backed Timeline/comments and shared references, so it is not by itself a complete Card archive.

Retain a small SQLite record for Card ID, Project ID, project-local number, title, archive time, useful listing fields, and archive location/version/codec/checksum. Keep identity/reference and report-deduplication records required by other active data; pruning must preserve referential integrity and existing accepted-report semantics. Timeline/artifact IDs remain stable when their physical storage moves. Detailed archived history is loaded through storage access, not direct filesystem paths.

Recommend zstd as the default for cold Card history; lz4 can be an alternative when compression/decompression speed has higher priority. No codec has been selected between them yet. Both are compression codecs, so also define a versioned container with a manifest, structured record export, and resource files. A tar container compressed with either codec is one possible implementation, not a selected format. Persist codec and schema/container versions per package so readers can support both.

Keep logical Card archive state separate from packaging progress (for example, pending/ready/failed). Compress in background without blocking the UI. Build from a consistent record snapshot and immutable files; finish writing and verify the package before atomically publishing it. Then transactionally save its archive index and switch detailed-content resolution. Only afterward remove redundant expanded files or detailed database rows that are no longer needed. A failed/interrupted package operation keeps the original data and can retry. Database row deletion does not itself guarantee that the SQLite file immediately shrinks; space reclamation policy can be refined later.

Read archived details on demand, optionally through a disposable extraction cache. Reopening restores editable records/files with the same identities before the Card becomes active; it does not silently dispatch work. Worktree recreation/handling remains a separate concern. Packaging is not a new process-exit or old-Agent handover gate. If a late report is accepted after the captured archive snapshot, retain it durably as a later record/file outside the sealed package, resolve it alongside archived history, and include it in a later package revision. The exact mechanism can remain simple; never overwrite a sealed package or discard an accepted report.

## Restart, worktrees, and backup

On restart, load current cards, active config revisions, queued requests, saved timers, and unread notifications. Reconcile known active attempts with Herdr. Reconnecting to the same terminal keeps the same attempt; deliberately retrying creates a fresh attempt. A crash between a core launch and saving its reply can leave an uncertain launch: record that uncertainty and reconcile available labels/runtime data rather than assuming no work started. Core has no durable application launch-idempotency key.

Keep worktrees through development, Review, and rework. Archive triggers the chosen managed-worktree handling while card history and captured resources remain. Removing a worktree does not preserve its uncommitted files automatically; the archive policy for modified worktrees still needs refinement. A stored commit hash also does not retain Git objects by itself; code retention relies on retained repository references or an explicitly saved artifact.

An application-data backup needs a consistent SQLite snapshot and its referenced managed files, including compressed archives and configuration revisions. Coordinate file cleanup with backup so referenced content stays available while it is copied. Use SQLite's supported backup mechanism or a stopped-app copy; a running WAL database cannot be backed up reliably by copying just the main database file. Source repositories/worktrees are separate backup scope. This is not a full Project export or a backup of Herdr/provider conversations.

Use versioned database migrations and versioned structured payloads. Permanent deletion, large-output retention, Project export/import, and full transcript capture remain later product choices; archive alone does not delete history.

## Selected direction and remaining proposals

The selected baseline is ~/.canban/, SQLite with WAL, file-backed execution input/report bodies, and zstd/lz4-compressed Card archives including related execution data/results. One shared database, detailed schema/layout, archive container/default codec, publication/cleanup and restore mechanisms, and portable Project export remain proposals or implementation refinements. No database or ~/.canban/ directories have been initialized by this design work.

Implementation refinement: the MVP uses one shared WAL database and versioned, checksummed tar+zstd archives. Structured SQL history/reference/deduplication rows remain online; matching expanded file bytes are compacted after archive publication. SQL cold-storage pruning and lz4 encoding remain later options. Actual runtime homes are initialized on startup independently of the earlier design-only work.

## Implemented interactive conversation metadata (2026-11-07)

SQLite schema version 2 adds conversations and questions. A Conversation ID is Card ID + profile ID, with provider, exact provider_session_id and last_request_id. A Question references its comment, Card/profile, request/attempt, delivery/reply state and file-backed reply. Attempts additionally retain conversation_id, provider_session_id, close_state, turn_ended_at and response_path. These records are included in Card archive exports; structured rows remain online for references and deduplication. Captured prompts, question replies, reports and retained terminal output live below the Card's existing runs/<request>/<attempt> directory. Historical callback-derived final-response files remain readable; the current Herdr lifecycle does not create provider callback snapshots. Provider-owned full histories remain outside the Canban archive.

## Profile/lifecycle and delivery metadata update (2026-11-07)

Editable preset files live under config/presets; embedded JSON supplies default Codex/Claude recipes. Validated legacy migration writes exact original config bytes to config/agents.legacy-<uuid>.json before atomic publication. Request profile snapshots and historical conversations are not rewritten. Attempts additionally record completion time, report-time turn, close-request time and connection state. Runtime observation is distinct from the explicit outcome.

Project.target_branch captures the default local delivery branch. Card.delivery stores the latest accepted target/commit/request key/timestamp, while full delivery events remain in Timeline. review.merge persists an operations receipt with state unknown before Git effects, then replaces it with the observed result. This is deliberately not described as a transaction spanning SQLite and Git. Reusing the same operation key never blindly replays effects after a lost reply.

Archive retains the Card Git ref independently of the Project's current HEAD. Restore preserves that checkout association; reopening recreates the retained Card commit even if the Project branch has since advanced. Legacy restores with a cleared path also consult the retained Card ref. Reports/replies should be written under CANBAN_RUN_DIR so temporary reporting artifacts stay out of delivered code.


## Named Card workspaces and approval state (2026-11-07)

Card records now retain branch/worktree identity, the initial base commit and an optional human gate with related request/pending transition. Workflow snapshots include simple workspace settings; Requests snapshot whether completion requires human review. These use backward-compatible JSON fields in the existing SQLite records. Existing attention seeds a human gate on open. No file content or Git history is discarded during adoption; detached Card checkouts become named branches at their current HEAD on next use. See [implementation contract](workspace-and-scheduling.md).
