# Contextual Board/Git navigation acceptance

Date: 2026-11-17. Card #108, based on `312e0628fe1384da4252b299ac8d4b67f0400426`.

## Implemented scope

Board/Git entry from a Card or Terminal resolves that object's associated
Project at click time. Other entry points retain their previous behavior.
There is no additional global working-Project state, Terminal-selection watcher
or Git history schema. Back/Forward retains its recorded Project. Git discards
foreign branches, ranges, checkouts and results before reading another Project.

## Native exercise

Built with `RUSTUP_AUTO_INSTALL=0 bu exec -- mise run build`. The production
`.local/Canban.app` was copied and re-signed under the unique application ID
`com.canban.navigation.cb-project-nav-zw39gznt`. Its Mach-O build UUID matches the
packaged production executable. Re-signing changes the file hash.

The scratch home is `/private/tmp/cb-project-nav-zw39gznt/home`. Herdr used its own
`project-navigation` session, socket and XDG directories. Two disposable Git
repositories contain distinct `only-a` / `only-b` branches and distinct commits.
Each Project has a Card and a real shell PTY in its Card worktree. No model was
launched and no installed application home or user Core was modified.

| Exercise | Observed result |
| --- | --- |
| Board B -> A Terminal -> Settings -> Board | Board B; selecting a Terminal did not establish a new default |
| A Terminal -> Board | Board A |
| Back from Board A -> A Terminal -> Settings -> Board | Board B; history retained its original Project |
| A Terminal -> Git | Project A's main checkout, `only-a` history and Branch A diff |
| Set Compare to `only-a`, then select Project B in Git | B's `only-b` history and Branch B diff; A's selection/results cleared |
| Back from Git B | Git A with A's refs and diff |
| Quick Open Card A -> Git rail entry | Project A's main checkout |
| Card A branch action | Explicit `a-6761/1` Card worktree, preserving the existing precise entry |

The task-owned UI, AppServer and isolated Core were stopped after verification.
The fixture metadata and observations are retained in the execution run directory
as `native-project-navigation-evidence.json`.

## Automated checks and limits

`mise run acceptance`, wrapped with `bu exec`, passed against an isolated real
Herdr Core using deterministic fixture providers. The new UI regressions cover
click-time targeting, unchanged Terminal selection/default behavior, history
Project restoration, unknown destinations, same-Project range retention and
cross-Project selection/result invalidation.

The complete workspace run
`RUSTUP_AUTO_INSTALL=0 bu exec -- mise run test -- --no-fail-fast --test-threads 1`
passed: **585 passed, 1 skipped**. The suite's existing background-delivery test
has a five-second push-start deadline: it timed out in parallel runs on this host,
then passed in isolation and in the complete serial run. No unrelated production
behavior or test deadlines were changed. Changed-file formatting and
`git diff --check` also passed.

This is local macOS navigation acceptance, not new remote-host, IME or paid-model
acceptance. No UI component styling or renderer changed.
