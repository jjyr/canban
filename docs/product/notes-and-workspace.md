# Notes and the Kanban workspace

Status: implemented 2026-11-07. Kanban remains central, using the calm native workspace direction (A). A Note behaves like an editable document but its content is stored in SQLite. Related events, Card/Agent associations and bidirectional links share the application operation boundary. This supersedes both the earlier Note discussion/composer proposal and the physical-file interpretation. See [implementation and acceptance](../notes-and-workspace-implementation.md) for contracts and validation limits.

## Storage and ownership

A Note belongs to a Project and has a stable ID, editable name, Markdown body and optimistic revision. Content, immutable saved revisions and related-event history live in SQLite WAL. File-like means create/read/write/rename through Canban, rather than a watched filesystem path. Agents use the same AppServer operations through `canban note`; they do not edit SQLite directly.

Cards select Project Notes for execution context. The existing default remains all Project Notes for Cards created directly; creating a Card from a Note selects that source Note explicitly. Each execution captures the selected content. Later saves never rewrite existing execution captures.

## Confirmed product role

Use a Project Note as a file-like editable document supporting both emerging ideas and reusable project context. Continue with Markdown as the initial editing format. Examples include a feature exploration, architecture rationale, constraints or a product decision. These uses share one document model without mandatory document types or a folder taxonomy.

SQLite holds the content. Canban adds a related-event Timeline, Card/Agent associations and bidirectional navigation between Notes. An Agent reads and writes through the Canban CLI; a dedicated Note chat surface and Note-specific conversation lifecycle are outside this simplified scope.

A Card represents a scoped piece of work with a workflow, execution history and human disposition. A Note can remain useful before any Card exists and after several related Cards are archived. One Note may inform multiple Cards; one Card may use multiple Notes. A Note need not be converted or removed when a Card is created from it.

```text
Project
  |
  +-- Board -------------------- Cards and workflow
  |                               |
  |                               +-- Task / Timeline / Agents / Review
  |                               +-- Selected context ------+
  |                                                         |
  +-- Notes ------------------------------------------> SQLite documents
        |
        +-- Content <---- human editing / Agent read-write
        +-- Timeline <--- edits / Card relations / Agent activity
        +-- Links <-----> other Notes
        +-- Select a scope -> create Card in Backlog

Note edits -> future execution captures
             existing captures retain the content they originally used
```

Keep the Board as the default destination. Surface Notes through a secondary Project destination and through the selected Card's Context section. This preserves a short path from an idea to work and from work back to its rationale.

## Workspace hierarchy

The first reading priority is Project/Board and work requiring attention; the second is the selected Card's current state and next action; the third is task content, activity and discussion; reference material and execution diagnostics remain accessible with less emphasis.

### Board overview with an optional Card pane

```text
+-------------+---------------------------------------+--------------------------+
| CANBAN      | Project / Board          Filter  +Card | #42 Add search       ... |
|             |                                       | Review   @developer done |
| Project  v  | Needs attention 2   Running 3          |                          |
|             |                                       | Task summary             |
| > Board     | Backlog       Develop       Review    | Search local Cards...    |
|   Notes     | 4             3             2         |                          |
|   Agents    | +-----------+ +-----------+ +-------+ | Latest report            |
|   Archive   | | #51 ...   | | #48 ...   | | #42 <| | Changes / validation...  |
|             | +-----------+ | @dev live | +-------+ | [Review changes]         |
|             |               +-----------+         |                          |
|             |                                       | Timeline                 |
|             |                                       | Agent submitted report   |
|             |                                       |                          |
|             |                                       | Context 2 / Resources 1  |
|             |                                       | [collapse / expand]      |
|-------------|                                       |--------------------------|
| Notices 2   |                                       | Comment... @profile      |
| Settings    |                                       |                   [Send] |
+-------------+---------------------------------------+--------------------------+
```

The Card pane is optional. An unselected Board uses the whole content width. At a content width below 900 points, the workspace uses one main surface with a clear back action and retained selection/scroll/drafts. Above that threshold, Board/Card and Card/work splits use NSH resizable panels. The Gallery includes both full-width and constrained viewport scenes; further density refinements can use those same compositions.

Card tiles should emphasize title, Agent/activity and attention, with secondary metadata below. Workflow stage is visible through the column; runtime and attention remain independent. An attention summary should lead to the Cards that need action.

### Focused Card with a Terminal or Review surface

```text
+-------------+-------------------------------+---------------------------------+
| Project  v  | <- Board   #42 Add search ... | @developer     Live / Connected |
|             | Review                        | Interrupt   End   Expand  Close |
| > Board     |                               |---------------------------------|
|   Notes     | Readable task description     |                                 |
|   Agents    | [Edit]                        | Terminal                        |
|             |                               |                                 |
|             | Timeline                      | Agent output and direct input   |
|             | Reports / comments / events   |                                 |
|             |                               |                                 |
|             | Context / Resources [expand]  |                                 |
|             |-------------------------------|                                 |
|             | Comment... @profile    [Send] |                                 |
+-------------+-------------------------------+---------------------------------+
```

Clicking a Card opens Board plus Card detail. Clicking its Agent entry starts, attaches or resumes that Card/profile conversation and opens focused work. Clicking Review changes opens the same focused layout with Review instead of Terminal. The Board is hidden while global navigation remains visible.

`Expand work` gives Terminal/Review the full content region. `Show Card` switches to task/discussion; `Restore split` restores both panels. In narrow mode, `Show Card` / `Show work` switches surfaces. `← Board` and `Close panel` return to the Board plus selected Card (or the Board alone at narrow widths), retaining Board scroll and unsaved drafts. Closing the viewer never interrupts or ends the Agent; those remain explicit controls.

The Card opens in reading mode, with its latest useful result and activity accessible. Editing is deliberate, with dirty/save/error feedback near the editor. Context and resource details can expand without making every visit a form-filling task. The composer stays reachable while reading history. Action emphasis changes with the actual state: answer a question, inspect Review, resume an exited conversation, or recover from failure.

### Notes workspace

```text
+------------+----------------+---------------------------+------------------------+
| Project v  | Notes     +New | search.md             ... | Related                |
|            | Search...      | Saved   Edit / Preview    | Cards: #42, #51         |
|   Board    |                |---------------------------|                        |
| > Notes    | > search.md    | Goal                      | Links                  |
|   Agents   |   arch.md      | Find Cards quickly...     | -> constraints.md      |
|   Archive  |   constraints  |                           | <- arch.md             |
|            |                | Options / decisions       |------------------------|
|            |                | ...                       | Timeline               |
|            |                |                           | User edited Note       |
|            |                |                           | Linked Card #42        |
|            |                |                           | @developer updated     |
|            |                |                           | Note while doing #42   |
+------------+----------------+---------------------------+------------------------+
```

The primary experience is a file list and editor/reader. A secondary region shows related Cards, Note links/backlinks and the event Timeline; it can collapse when reading width is limited. There is no Note chat composer. The diagram shows information grouping, not a physical filesystem location.

For example, a search Note can inform search-UI and indexing Cards. An Agent working on one of those Cards reads and updates the relevant document through the CLI, and the Note Timeline exposes that association when known. The Note remains available for later decisions.

Creating a Card should preview its title, task description and selected context, then place it in Backlog. Moving it into an executable stage remains the scheduling action. The implemented Create Card action opens an editable title/body preview; it does not schedule work. The user can scope that body before creating the Card, then move it into Develop when ready.

## Timeline and bidirectional links

The Note Timeline records related events, including content changes, Card associations and Agent modifications/associations. Examples of useful rows are a human save, a Card linked to this Note, and an identified Agent updating it while working on a Card. Event rows link back to the relevant Card or execution where known. The Timeline provides provenance; it is not a separate Agent reply thread.

When Note A contains `[label](note:NOTE_ID)`, A exposes the outgoing link and B derives a backlink from A. Reference-style Markdown links also work; code examples do not create links. A single reference is sufficient for navigation in both directions. Stable IDs preserve references after rename; missing targets appear as unavailable references. Links stay within a Project. The editor provides link insertion and the Related pane provides navigation; no reciprocal body edit is required.

## Agent read/write and remaining details

Agent interaction is ordinary Note reading and writing. Existing Card/Agent execution controls can remain the entry to Agent work. No dedicated Note-owned conversation or @Agent chat UI is required by this model. A Note edit itself does not move a Card or start a workflow.

A save atomically updates current content, an immutable revision, parsed links and Note events. Existing documents receive a baseline revision on schema upgrade without invented edit history. An outdated write is rejected with its draft retained; the user can inspect the saved revision and explicitly replace the draft. Metadata-only refreshes do not discard an unsaved edit.

Canban attributes CLI writes using the injected request/attempt binding, records profile/Card/execution provenance and adds a corresponding Card Timeline event. Unbound writes are recorded as user operations. Reading does not create Timeline noise. Raw terminal text is not treated as a semantic Note update or Agent reply.

Separately, current automatic inclusion of every Project Note should be reconsidered as the library grows. Source Notes selected when creating a Card and deliberately included shared references would be more predictable. No default-selection policy change is decided here.

## Design-system implications

Develop reusable NavigationItem, CardTile, ConversationSummary, TimelineEntry, CommentComposer, ContextReference, file list/editor shell and split layout as needs become concrete, using the existing UI Kit and suitable NSH controls. Document readers in Cards and Notes share typography and Markdown behavior. Note and Card event presentation should reuse appropriate Timeline/link components with their owner and available actions explicit; the Note surface does not need a discussion composer.

The standalone Gallery imports these production Board/Card, focused Review and Notes components. It includes links/backlinks, long content and retained drafts, plus full/narrow viewport and light/dark controls. Native application acceptance exercises actual saves, Agent writes, workflow progression and conflicts; see the linked implementation report. Draft retention covers navigation during the current app process, not crash recovery.
