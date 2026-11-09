# GitHub Cards and unified creation

Status: product decisions recorded on 2026-11-09. See [implementation and acceptance](../github-integration.md) for the implemented code, compatibility model and current live-acceptance limits. Earlier implementation recommendations below retain their design context; this document is not runtime acceptance evidence.

## Card identity, relationships and delivery

Confirmed direction: Local Cards and GitHub Issue Cards use similar visual components but may have different content structures. A GitHub Issue Card must identify one source GitHub Issue and display the official GitHub icon. A Local Card remains local even when it links to an Issue.

```text
Card
  +-- Local content
  |     +-- editable title and description
  |
  +-- GitHub Issue content
        +-- required source Issue
        +-- remote title and description
        +-- editable local instructions

Both share workflow, routing tags, Notes, Timeline, Agent interactions,
workspaces, Review and delivery.
```

Both kinds can reference Issues and PRs. Keep these meanings separate:

- Source Issue: the identity of a GitHub Issue Card.
- Related Issue/PR: supporting context, references or dependencies.
- Delivery PR: the PR delivering the Card's current work.

Source identity is independent of Canban's internal Card ID and Project-local number. A related Issue does not convert a Local Card into a GitHub Issue Card. A PR link alone does not prove delivery. Linking an Issue does not implicitly authorize closing it on GitHub.

## One Add Card form

Confirmed interaction: Add Card opens the form immediately, defaulting to Local. Do not insert a preliminary Local/GitHub menu or a separate import wizard. Select the type inside the same form.

```text
Add Card
   |
   v
+-- New Card ----------------------------------------------+
| Type       [Local v]                                     |
| Title      [                                           ] |
| Description                                             |
| [                                                     ] |
|                                       [Cancel] [Create] |
+---------------------------------------------------------+
                     |
                change type
                     v
+-- New Card ----------------------------------------------+
| Type       [GitHub Issue v]          owner/repo           |
| Search     [title keywords or #number                   ] |
| +-----------------------------------------------------+ |
| | #186 Fix reconnect failure                    Open   | |
| | #153 Reconnect after timeout                  Closed | |
| +-----------------------------------------------------+ |
|                                          [Cancel] [Add] |
+---------------------------------------------------------+
                     |
                select Issue
                     v
+-- New Card ----------------------------------------------+
| Type       [GitHub Issue v]                              |
| [GitHub] owner/repo#186                         [Change] |
| Fix reconnect failure                                   |
| Remote description preview                              |
| Additional instructions                                 |
| [Local execution requirements                         ] |
|                                          [Cancel] [Add] |
+---------------------------------------------------------+
```

- Search the Project's connected repository. A number or `#number` prioritizes exact lookup; title text searches Issues. Search may return Open and Closed Issues.
- Selecting a result binds the source and shows its preview. Add submits the Card to Backlog. This supersedes the earlier proposal that clicking a search result immediately added the Card.
- An already-managed Issue offers Open existing instead of creating a duplicate. A locally removed Issue can be explicitly re-added, restoring its retained record and history.
- Local content is editable; GitHub title/body are source previews and the editable text is local Additional instructions. No content is silently published to GitHub.
- Preserve drafts when changing type or changing the selected Issue. Keep Local description and GitHub instructions separately; do not overwrite either with remote content.
- Reuse the form shell, fields, validation, focus/error behavior and submission controls. The type-specific content is the part that changes.

## Automatic intake

Confirmed: a Project's GitHub source offers two automatic intake modes:

1. All Open Issues, regardless of author.
2. Open Issues created by repository collaborators.

Both initial intake and subsequent intake use Open Issues. Closed Issues remain available for manual search/add. A tracked Issue becoming Closed does not remove its Card.

```text
Open GitHub Issues
        |
  selected author rule
        |
  local removal exclusions
        |
  ensure one source-bound Card per Project
        |
  Backlog: no Agent/session/worktree launch
```

Manual addition is not restricted by the automatic author rule. Automatic intake does not interpret GitHub Labels as Scheduler routing tags or bypass human gates. It creates only the lightweight local task record; ordinary workflow entry determines execution.

## Local removal and re-addition

Confirmed: removing a GitHub Issue Card affects Canban only. It does not delete, close or edit the GitHub Issue. Retain an exclusion for the Project/source Issue so synchronization does not recreate the removed Card. Explicit manual re-addition clears that exclusion.

Recommended implementation: retain a removed Card and its existing execution/Timeline/delivery history, hide it from the Board and exclude it from future automatic dispatch. Re-addition restores that record. Removal is distinct from Archive, which preserves completed work under the existing archive lifecycle.

The behavior of removing a Card with active executions still needs a concrete interaction and safety rule before implementation; removal must not silently terminate terminals or erase running work.

## Persistence and shared code

Recommended design: persist Canban work state plus source identity, cache remote content separately, and snapshot actual Agent inputs through the existing execution context storage.

| Data | Ownership and retention |
| --- | --- |
| Card ID/number, Project, stage, routing tags, local instructions and human gates | Durable Canban state |
| Source host/repository/Issue identity | Durable binding, independent of the display number/URL |
| Timeline, local comments, sessions, attempts, workspaces and delivery | Existing durable Card-owned history |
| Current remote title/body/state/labels and sync metadata | Refreshable GitHub cache |
| Remote comments | Fetch/cache as needed, with stable external identities |
| Exact content supplied to an Agent | Retained execution context, independent of later cache refreshes |

Browsing/searching Issues does not itself create Cards. Automatic intake or explicit Add establishes local workflow ownership. Losing access to an Issue does not remove that ownership or its history.

Use a shared Card core with a Local/GitHubIssue content enum, rather than two independent workflow implementations. Resolve content centrally for UI presentation and Agent context. With two content kinds, an enum is sufficient; a general backend plugin framework is unnecessary.

```text
Local create ------------------------+
                                     +--> shared Card creation
GitHub manual add --> source dedup ---+    ID/number, Backlog, creation event
GitHub auto intake --> exclusions ----+
                                               |
                           existing workflow / Scheduler / human gates

GitHub sync --> source cache + source Timeline events
               (does not run interactive comment or stage-entry commands)
```

The existing `card.create` command in `crates/app-server/src/commands.rs` combines creation and event recording. Extract its shared operation for the type-specific entry points, preserving Note association, numbering and correct event actors. Source binding, deduplication and restore must be atomic so manual Add and synchronization cannot create two Cards for the same source Issue in a Project.

Do not feed imported remote comments into the existing interactive `comment.add` command: it recognizes @profile mentions and may launch or contact an Agent. Remote comments need source events and external-ID deduplication, not local user-command semantics.

## Collaborator API feasibility

Researched against GitHub's official REST documentation and public GraphQL schema on 2026-11-09. This verifies API contracts, not a live account's access to any particular repository.

- `GET /repos/{owner}/{repo}/collaborators` lists collaborators, with pagination. Cache a successful complete list and match Issue authors by stable user ID.
- `GET /repos/{owner}/{repo}/collaborators/{username}` checks one user: 204 means collaborator; 404 means not a collaborator only after repository visibility/authentication are established. Access failures must not become a negative membership result.
- The list includes direct access, team access, organization default permissions and organization owners. It is not limited to individually invited users or write-level collaborators.
- The list endpoint documents write/maintain/admin privileges and organization membership for organization repositories. The individual check requires push access. Classic OAuth/PAT scopes include `read:org` and `repo`. The chosen authentication mechanism must be verified against these requirements.
- Issue `author_association` is not an exact replacement for the list: COLLABORATOR, MEMBER and OWNER describe different relationships, and CONTRIBUTOR means previous commits rather than repository access.

Recommended behavior: validate lookup capability when enabling collaborator-only intake. If unavailable, show the limitation and pause that intake rather than silently broadening it to all Issues. Manual search/add remains independent of collaborator lookup. Preserve cached membership on transient failures and show stale/error state; do not treat a failed/incomplete fetch as an empty successful list.

The simple rule checks current collaborator membership. Historical membership at Issue creation is not established by these endpoints. Recommended first-version semantics: refresh membership periodically, re-evaluate eligible Open Issues, and do not eject already-managed Cards if an author loses access. This temporal behavior remains an implementation recommendation.

Sources:

- [List repository collaborators](https://docs.github.com/en/rest/collaborators/collaborators#list-repository-collaborators)
- [Check if a user is a repository collaborator](https://docs.github.com/en/rest/collaborators/collaborators#check-if-a-user-is-a-repository-collaborator)
- [List repository issues](https://docs.github.com/en/rest/issues/issues#list-repository-issues): REST Issue listings can also contain PRs; exclude PR entries when implementing Issue intake/search.
- [CommentAuthorAssociation](https://docs.github.com/en/graphql/reference/enums#commentauthorassociation)

## Review delivery direction

The user requests GitHub PR publication as an alternative to the existing local merge. Either Card kind can use either delivery path; source kind does not determine delivery mode.

```text
Backlog --> Develop --> Review
                          |
               +----------+----------+
               |                     |
           local merge          push + publish PR
               |                     |
               |              checks / review / rework
               |                     |
               |                GitHub merge
               +----------+----------+
                          |
                      delivered --> Archive
```

Recommended details, pending implementation design: keep the Card in Review while a delivery PR is open; synchronize its status and record delivery after merge; return to Develop to update an open PR; preserve earlier delivery records when later work needs another PR. A closed unmerged PR is not delivery. Do not run local merge and GitHub merge for the same delivery. Keep GitHub Issue state, Canban stage and delivery state separate.

AppServer should own GitHub API authentication/synchronization; Git push runs on the saved workspace machine using that machine's Git credentials. Project repository identity is independent of its mutable execution location. Existing executions and deliveries retain their original binding.

## Crate and module boundaries

User-selected direction, 2026-11-09: introduce only the provider crate, `canban-github`, and put Canban-specific GitHub integration in the existing `canban-kanban::github` module. This supersedes the proposed separate `canban-integrations` crate. This boundary is now implemented; see the implementation document for exact ownership and acceptance.

| Package / directory | Responsibility |
| --- | --- |
| `canban-github` / `crates/github` | GitHub protocol models, HTTP/GraphQL, Device Flow, secure credential access/refresh, installation access, collaborator lookup, Issue search/listing, PR API actions and protocol errors |
| `canban-kanban::github` / `crates/kanban/src/github` | Project source bindings, intake rules, sync reconciliation/checkpoints, source-Card deduplication, local exclusions/restoration, source event mapping and PR delivery policy |

`canban-github` does not know Cards, Projects, Backlog, workflow, SQLite or NSH. Its internal modules can be `client`, `models`, `auth`, `credentials`, `repositories`, `collaborators`, `issues` and `pull_requests`. Keep provider response models here; only cross-product IDs, bindings, commands and UI-safe account states belong in `canban-types`. Never put tokens in shared UI snapshots or Card DTOs.

Start with ordinary modules under `crates/kanban/src/github`, such as `sources`, `sync`, `intake`, `cards` and `delivery`. Keep shared Card creation/removal/restoration outside the source-specific module so both Local and GitHub entry points use the same rules. The module may use `canban-github` and the existing storage/Timeline boundaries where needed; it must not depend on AppServer or UI. Do not build a registry, dynamic plugins or a general backend framework.

```text
UI / CLI
    |
AppServer: commands, service lifetime, component wiring
    |
    +--> Kanban
    |      +-- shared Card lifecycle and validation
    |      +-- github module: sources, sync, intake, bindings, delivery policy
    |                 |
    |                 +--> canban-github: authentication and provider API
    |
    +--> Storage / Timeline: one DB and durable product events
    +--> Workspaces: existing Git operations on the saved machine

Workflow Engine / Scheduler do not depend on GitHub.
Kanban and GitHub do not depend back on AppServer.
```

Storage continues to own migrations, queries, source uniqueness and atomic Card/source/exclusion updates in the existing SQLite database. `kanban::github` owns reconciliation and intake decisions, not a second database. Run network requests outside storage transactions/locks; validate source/account generations when committing results. AppServer hosts the service lifecycle and polling workers; source polling/reconciliation behavior belongs to the GitHub module rather than ad hoc command handlers. Source polling is not Agent scheduling and does not add another Scheduler.

Kanban owns common Card creation/removal/restoration rules; its `github` module adds source binding, remote eligibility and exclusion behavior. Context owns the assembled/snapshotted Agent input. Workspaces owns commit/push/Git operations; `canban-github` owns remote PR mutations; `kanban::github::delivery` owns delivery decisions, validation and state transitions. AppServer composes existing workspace operations with provider calls according to those decisions, without moving Git/Herdr execution into Kanban or embedding intake/delivery policy in RPC handlers. Workflow Engine continues to advance templates, and Scheduler continues to admit work under existing human gates and limits.

UI calls AppServer and consumes shared presentation/account state. Keep the unified creation form in `canban-ui`, with reusable Local and GitHub source sections, and generic form/search controls in `canban-ui-kit`. Neither UI layer links directly to `canban-github` or owns authorization tokens/network polling. This dependency rule applies to new GitHub code; it is not a claim that existing UI manifests already depend only on AppServer.

## Remaining implementation decisions

- App registration and authentication implementation follow the [Device Flow plan](github-authentication.md); repository connection UI and collaborator permission failures still need implementation design.
- Concrete database migration, command schemas and draft behavior when switching types.
- Active-execution removal behavior and dispatch exclusion checks.
- PR publish/merge controls, reviewed-head validation and unknown-write reconciliation.
- Explicit remote editing/comment publication and closing-keyword behavior.

Broad bidirectional editing, GitHub Projects boards and a separate GitHub Scheduler are outside this design.
