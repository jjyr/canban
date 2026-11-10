# GitHub Cards and unified creation

Status: current product decisions, updated 2026-11-10. The complete current interaction, cache and implementation contract is [GitHub workspace](../github-workspace.md). Historical App/live-delivery evidence remains in [GitHub integration](../github-integration.md).

## Card identity, relationships and delivery

Local, GitHub Issue and GitHub PR Cards share workflow, local instructions, context, Timeline, Agent interactions, workspaces, Review and delivery. A source Card represents one GitHub object and shows the GitHub icon. Card ID and Project-local number remain independent of source repository/object identity.

- Source Issue/PR establishes the identity of a GitHub Card.
- Related Issue/PR/commit references supply supporting context; adding them leaves a Local Card local.
- Delivery PR identifies the code delivery for a Card. A plain reference does not establish delivery.

Each Project owns at most one source Card for a remote object. Existing delivery PRs resolve to their owning Card rather than generating duplicate automatic tasks. Different Projects can own separate Cards for the same source. Closing a source or merging a plain related PR does not advance the local workflow or Archive. A confirmed delivery merge records a delivery receipt; Archive remains separate.

## One Add Card form

Add Card opens immediately, defaulting to Local. Local, GitHub Issue and GitHub PR are choices within that form. Search the connected repository by number or title; selecting a result previews it, and Add creates the Card in Backlog. Closed sources remain manually addable. Existing source/delivery Cards open instead of duplicating; removed source Cards restore their retained identity/history.

Local title/body are editable. GitHub title/body are read-only remote projections, and Additional instructions stay local. Form drafts survive ordinary navigation; switching kind does not overwrite local prose with remote content. Nothing is silently published to GitHub.

## Automatic intake

Issue rules support off, all Open Issues or collaborator-authored Open Issues. PR rules separately support off, all Open PRs, collaborator-authored, assigned to me, authored by me or Review requested from me. Author/Label lists narrow new intake; Draft inclusion is explicit. New matches enter Backlog without launching an Agent or creating a worktree. Manual addition is independent of automatic eligibility.

Turning intake off keeps tracked objects refreshed; changing rules never removes existing tasks. Labels do not select Agents or bypass human gates. The Project pause control stops its source worker while separate GitHub browsing remains available. See the [workspace contract](../github-workspace.md) for Board filters and synchronization events.

## Local removal and restoration

Removal is local only and retains an exclusion so automatic sync cannot recreate the task. Explicit restoration keeps the Card ID/number, local instructions, execution and delivery history. Removal cancels queued work and is refused while a live or uncertain execution still needs handling. Archive is a separate lifecycle operation.

## Persistence and remote content

Persist local workflow ownership and GitHub bindings/rules. Remote title/body/state/labels, notifications, comments, reviews, checks and diffs belong to a disposable memory/file cache, not business tables or permanent Timeline copies. Cached content remains usable until a refresh event replaces it or capacity/user cleanup evicts it. There is no short freshness TTL.

The unified discussion UI can display remote activity, but importing it never calls interactive local comment/mention operations. New execution inputs retain references and local instructions, not an automatic permanent remote-body snapshot. Agents can read current source context through AppServer; local authored comments/reports and actual delivery receipts remain durable.

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

## Ownership and dependencies

`canban-github` owns GitHub protocol models, API/gh transport, App authorization/credentials and disposable response caching. It has no Card workflow, SQLite or native UI policy. Typed references are shared DTOs.

`canban-kanban::github` owns Project bindings, intake decisions, source uniqueness, local exclusions/restoration and delivery policy. Storage owns transactions and constraints in the business database. AppServer owns service lifetime, background refresh and composition with existing workspace operations. Workflow Engine and Scheduler continue to own ordinary work admission and human gates; GitHub synchronization does not create another scheduler.

UI calls AppServer and consumes safe projections. Production components are shared with native Gallery scenes. Generic tokens/controls remain in UI Kit; GitHub-specific forms and views remain in product UI.

## Decision history

The 2026-11-09 design introduced Issue source Cards, unified creation, collaborator intake, local removal, App Device Flow and PR delivery. The 2026-11-10 discussion extends source ownership to PRs, moves account controls from Settings into a dedicated GitHub page, adds paginated GitHub-style browsing/review, and replaces permanent remote-content imports with event-refreshed disposable caches. The provider/Kanban ownership split and local workflow/human-decision boundaries remain.

Broad Issue editing/closing, GitHub Projects boards, Enterprise hosts, a general provider/plugin framework and a visual workflow-template editor remain outside this change.
