# GitHub integration implementation and acceptance

The current implementation contract is [GitHub workspace and disposable cache](github-workspace.md). It covers the GitHub rail page, repository configuration, Local/Issue/PR Cards, pagination, explicit remote actions and file-backed event-refreshed caches. The dated acceptance below records the earlier App integration; it is not fresh acceptance of the new workspace.

## Provider and product boundaries

`canban-github` owns API/gh transport, protocol models, App Device Flow, credentials and disposable responses. `canban-kanban::github` owns source bindings, intake, source uniqueness, exclusions/restoration and delivery policy. AppServer wires their lifetimes and operations to storage, workspaces and the existing scheduler. UI consumes AppServer data and never owns API credentials or polling.

The registered `canban-app` identity remains App ID `5671006`, Client ID `Iv23li1ImpryrkMbQjKF`, with Device Flow, any-account installation and no webhook. App authorization and Git push credentials remain independent. GitHub CLI supplies notifications and repository operations when no App account is connected. The App's original Issues permission is read-only; publishing Issue comments with that authorization requires a permission upgrade. No new permission grant was performed for this branch.

## Current operations

Existing `github.auth.*`, `project.github`, `github.source.disconnect`, Issue search/add, `github.sync` and delivery publish/refresh/merge remain. New operations are:

| Method | Purpose |
| --- | --- |
| `github.cli.status` | Verify the gh account without extracting credentials |
| `github.browse` | One remote notification/Issue/PR page |
| `github.detail` | One detail/discussion/file/Review/check page |
| `github.item.add` | Add or restore an Issue/PR source Card in a connected Project |
| `github.reference.add` / `.remove` | Edit local typed GitHub references |
| `github.context` | Read the source context of a bound Card |
| `github.remote.comment` / `.reply` / `.review` | Explicit receipt-backed publication |
| `github.notification.read` / `.done` | Explicit notification handling |
| `github.cache.status` / `.clear` | Inspect or clear disposable responses |

Schema 9 retains local work and bindings; source bodies/comments live in cache. See the current contract for migration, query bounds, authority and acceptance commands. The integrated desktop API is level 8 (including durable editor drafts and updates); the current schema is 10.

## Historical validation and reproducibility: 2026-11-09

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

Deferred at the time of this acceptance: remote Issue editing/closing/comment publication, broad bidirectional synchronization, repository picking, Enterprise hosts and general provider/plugin abstraction. The new workspace supports explicit comment publication and PR Review; see its current contract.
