# GitHub integration implementation and acceptance

Status: implemented on 2026-11-09, with the acceptance boundaries below. Product contracts are in [GitHub Cards](product/github-cards.md) and [authentication](product/github-authentication.md).

## Registered application

- Owner: `jjyr`.
- GitHub App: <https://github.com/apps/canban-app>.
- App ID: `5671006`.
- Public Client ID: `Iv23li1ImpryrkMbQjKF`.
- Installation availability: any account, explicitly selected by the user.
- Device Flow enabled; expiring user tokens retained; no webhooks.
- Public App API verification confirmed Metadata/Issues/Checks/Statuses read and Contents/Pull requests write permissions. No App private key or Client Secret is distributed.

The user completed installation and Device Flow authorization for the private acceptance repository `jjyr/canban-github-acceptance-20261109` (installation `180044896`). Live collaborator lookup and the full Issue → developer → reviewer → PR publication → GitHub merge → Card Archive path passed. [PR #3](https://github.com/jjyr/canban-github-acceptance-20261109/pull/3) is the merged delivery. Authorization is scoped to the selected repository; other accounts install the public App separately.

## Crates and operation flow

```text
UI / CLI --> AppServer
                |
                +--> canban-kanban
                |       +-- common transactional Card creation
                |       +-- github: source intake / reconciliation
                |                  exclusions / event mapping / delivery policy
                |
                +--> canban-github: HTTP, Device Flow, local credentials, API
                +--> canban-storage: schema, constraints, transactions
                +--> canban-workspaces: commit / push / clean-head validation
                +--> existing Workflow Engine / Scheduler / Herdr
```

There is no `canban-integrations` crate. The GitHub provider has no dependency on Card, database, UI or runtime crates. Source-specific product rules live under `crates/kanban/src/github`. AppServer exposes operations, owns service lifetime and composes Git/network effects.

## Data and compatibility

- Schema version 7 adds `github_issues` and a unique source identity index on Cards within each Project.
- `Card.content` is `local` or `github_issue { source }`. A source records repository ID, Issue ID, repository name, number and URL independently of the local Card ID/number.
- Existing `title` and `body` fields remain compatible with current UI/CLI/execution consumers. For GitHub Cards, title is a read-only last-known projection of the source title, while body is editable local Additional instructions. The remote description lives in the source snapshot, not in that editable body.
- Agent context manifests include the selected remote Issue snapshot; prompts include its description and local instructions. Later synchronization does not rewrite historical execution inputs.
- Removal is reversible: mark the Card removed, retain its source exclusion and history, cancel queued work, and exclude it from normal snapshots/listing/admission. Refuse removal while a live or uncertain execution still needs handling. Explicit re-add restores the original identity/history; it does not modify GitHub.
- Sources and account authorization have generation fences. Manual Add carries the search result's source generation; late results cannot silently bind a different repository's same-numbered Issue.

## Source synchronization

Automatic intake offers all Open Issues or Open Issues authored by current repository collaborators. Full paginated Open-Issue listings run on a dedicated worker approximately every 120 seconds and after source configuration. Tracked Issues absent from the Open listing are fetched individually, so closure updates source state without deleting the local task. Explicit Sync now is available.

Collaborator-only mode validates access before saving. Failed or incomplete listings do not become an empty membership set or partial successful synchronization. Membership compares stable author IDs. The current version refreshes the collaborator list per sync rather than persisting a separate membership cache.

Remote Issue comments, PR conversation comments and submitted reviews become source Timeline events with external revision deduplication. They never invoke interactive `comment.add` and cannot dispatch an Agent through @mentions. GitHub Labels remain separate from routing tags. The existing stage/human-gate/admission rules own execution.

## Authentication

`canban-github::auth::Service` owns one account per AppServer home, Device Flow polling and serialized refresh. At the user's explicit request, credentials are stored locally in `<CANBAN_HOME>/credentials/github.json` (normally `~/.canban/credentials/github.json`), separately from SQLite, configuration, snapshots and Agent context. On Unix the directory is `0700` and file `0600`; refresh replaces the file atomically and syncs it before publishing rotated credentials. This is a plaintext local credential store protected by filesystem permissions, without Keychain access prompts. The path is ignored by Git and outside Card archives. Disconnect removes the local file. Access/refresh tokens are not returned to UI/CLI. Expiry metadata is honored and rotated credentials are saved before use.

`CANBAN_GITHUB_CLIENT_ID` and matching `CANBAN_GITHUB_INSTALL_URL` can select a separately registered app at service startup. The default identity is Canban's registered app. Sign-in and installation/repository access are separate; Settings links to the installation page and source validation reports missing permissions.

Runtime authentication no longer reads or writes Keychain. Credentials from the initial Keychain-backed test are not silently migrated: reconnect once to populate the local store. The native app is accepted on macOS; Windows filesystem ACL behavior is not accepted. API authentication does not configure Git push credentials on execution machines.

## Public operations

All are available through `canban call METHOD JSON` and the same UI adapter. Account/source/delivery operations reject an injected Agent execution context.

| Method | Purpose |
| --- | --- |
| `github.auth.status` | UI-safe account/device state |
| `github.auth.begin` / `github.auth.cancel` | Start/cancel Device Flow |
| `github.auth.refresh` | Refresh an existing expiring authorization without exposing tokens |
| `github.auth.disconnect` | Forget local credentials, preserving Cards/history |
| `github.installations` | Inspect accessible installations |
| `project.github` | Validate/save repository, intake mode and default delivery mode |
| `github.source.disconnect` | Stop the Project source without deleting Cards |
| `github.issue.search` | Search current repository by number or title; include existing/removed identity and source generation |
| `github.issue.add` | Add/restore selected source, optional local instructions and Note context |
| `github.sync` | Explicit source synchronization |
| `card.remove` | Reversible local removal; no remote mutation |
| `github.pr.publish` | Accept a current diff, commit/push and create/reuse its branch PR |
| `github.pr.refresh` | Reconcile PR state/head/checks/reviews and remote delivery |
| `github.pr.merge` | Explicit GitHub merge guarded by the displayed head and clean local checkout |

Source configuration uses `intake: all | collaborators` and `delivery_mode: local | github_pr`. Manual Add requires `project_id`, `number` and `source_generation` from search. Card mutations retain the existing revision checks. Publication/merge require a durable `request_key`; duplicate keys return receipts instead of replaying effects.

## Native interactions and delivery

Add Card opens the existing-route form immediately. Local is the default; switching to GitHub within the form enables debounced title/number search, source selection/preview and local instructions before final Add. Drafts survive switching. Existing Issue results open their Card; locally removed results permit explicit restoration.

The shared Card tile shows a small theme-aware GitHub mark and separate local/Issue numbers, with a full source tooltip. Remote source content is inside the Card's main scroll region. Source title/body are read-only, with local instructions edited through the shared editor. Settings provides account authorization and Project repository/intake/delivery configuration. Gallery contains the production GitHub Card scene.

Review selects Local merge or GitHub PR. Publication verifies the reviewed checkout, validates that the push repository is the target or its fork, saves an intent, commits/pushes without updating the local target, then creates/reuses the matching PR. Commit hooks changing the reviewed tree require another review. Existing delivery repository/workspace bindings stay pinned.

PR state, head, check runs, combined commit statuses, conversation comments and reviews refresh on demand and on the background worker. GitHub merge uses the displayed expected SHA; unpublished local changes or a different local head block it. A merged PR records delivery without a second local merge; Archive remains a separate action. An unmerged closed PR is not delivery. Uncertain writes retain receipts and require remote reconciliation before replay.

## Validation and reproducibility

Completed so far:

- All 104 Rust workspace tests, including legacy schema upgrades, shared local creation, source uniqueness, Timeline deduplication, stale-source/older-snapshot rejection, source closure preservation and removal/re-add. Log: `/private/tmp/cb-github-k3z1r0ci/rust-tests.log`.
- HTTP fixtures verify a failed later collaborator page returns no partial list and remote errors do not expose response bodies/credentials.
- Real Git commit/push against an isolated bare remote verifies publication leaves the local target unchanged and rejects stale previews/unpublished modifications.
- Strict workspace Clippy with warnings denied, native build/package and Gallery build.
- Native Add Card entry, Local/GitHub switching, retained title/body drafts, local creation into the existing Card/Timeline and account/source Settings. Follow-up fixes reconcile external Project settings without replacing dirty inputs and give search/instructions distinct native control identities.
- Native Gallery inspection of the shared GitHub source Card and theme-aware official mark.
- Live GitHub public App metadata verification, including the registered identity and requested permissions.
- Existing isolated real-Herdr acceptance passed Develop/Review, mentions/blocking, Archive/Restore, restart and three-Board concurrency. Its Agent programs are fixtures, not live model acceptance. Evidence home: `/private/tmp/cb-4ywrg9o_/home`.

Live integration harness: `scripts/acceptance-github.py`. It requires a running isolated AppServer, completed Device Flow and an explicitly authorized disposable repository. Its `--allow-remote-writes` flag opts into fixture comments and PR delivery; `--real-agent` exercises developer/reviewer execution, publishing, merge and Archive. GitHub CLI only sets up fixtures; the application path uses its own GitHub App tokens. The script never reads or prints tokens.

Real acceptance ran against `/private/tmp/cb-github-k3z1r0ci` and the private repository `jjyr/canban-github-acceptance-20261109`, with [Issue #1](https://github.com/jjyr/canban-github-acceptance-20261109/issues/1). Evidence: `/private/tmp/cb-github-k3z1r0ci/github-evidence.json`.

The live harness passed Device Flow refresh, Open-Issue Backlog-only intake, removal suppression and identity-preserving re-add, collaborator mode, and remote comments as Timeline context without Agent dispatch. Two real Codex invocations consumed the source context as separate developer/reviewer conversations; both completed, bound their provider IDs, and exited successfully. Publication produced merged PR #3 from head `b70f321900f4182f5e8fba91b8b718c210666e7c`, without locally merging the target checkout. Receipt replay did not duplicate publication. GitHub merge synchronized delivery, and explicit Archive completed the Card lifecycle.

A closed Issue #2 was manually searched by title and added through the native unified form as local Card #3 without scheduling an Agent. The fixture PR has no required CI checks or mandatory reviews: protected-branch/check failures have logic/fixture coverage, not live protected-repository acceptance. One collaborator request encountered a transient connect timeout; an explicit retry succeeded. HTTP connections now reuse the provider Agent; write operations are never automatically replayed.

The initial complete workflow used Keychain-backed credentials. After the user selected local storage, a fresh Device Flow sign-in saved the local file with directory/file permissions `0700`/`0600`. Real refresh succeeded, AppServer was stopped and restarted, the account restored without Keychain, and a second real refresh atomically replaced the file while preserving permissions. The installation API, collaborator-mode synchronization and archived/merged delivery still worked. UI snapshots contained no API tokens. Evidence: `/private/tmp/cb-github-k3z1r0ci/local-credential-evidence.json`.

Native regression checks confirmed that externally changing intake/delivery updates a mounted Settings page, while an unsaved repository draft and its focus survive another external update. Searching for closed Issue #4 by title, selecting it, and adding local instructions leaves the instructions blank before editing and stores the exact final string, including trailing punctuation. Card #4 remained in Backlog with no Agent request. Evidence: `/private/tmp/cb-github-k3z1r0ci/native-input-evidence.json`. The earlier missing period occurred when automated typing and submission were issued in one UI call; it did not reproduce with an observed edit before submission. Do not treat that earlier observation as an established general data-loss defect.

The user's live Canban/Core was not used for acceptance. Task-owned native windows, isolated AppServer and isolated Core were stopped after verification; test data and authorization remain in the isolated home for reproduction.

Deferred: remote Issue editing/closing/comment publication, broad bidirectional synchronization, a repository picker beyond `owner/repo` input, Enterprise hosts and general provider/plugin abstraction.
