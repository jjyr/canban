# MVP progress and recommended next steps

Assessment date: 2026-11-07. Status: recommendations for discussion, not additional accepted scope or release requirements. Current implementation and verification are recorded in [implementation](../implementation.md).

## Current position

Canban is a working local macOS MVP ready for focused daily use and feedback. Its central automation and conversation path has real Codex acceptance:

```text
Notes + Card -> Backlog -> Develop -> explicit completion -> Review report
                               |                              |
                     live terminal / @reply              human disposition
                               |                              |
                        exact-ID Resume                    Archive
```

Projects/Boards, local Cards, workflow JSON, profile routing, Timeline, captured context, resources, independent Developer/Reviewer histories, interactive terminals, explicit reports, archive/restore and unrestricted cross-Board scheduling are implemented. Workspace tests, deterministic real-Herdr acceptance and real Codex acceptance passed. Native interaction checks cover selected production flows; they do not establish exhaustive UI or recovery coverage.

The current implementation does not yet provide an integrated accept/request-changes/merge experience. Stage movement and Archive are available; Archive preserves history and handles the worktree, but does not merge its changes into the source branch. Claude's interactive TUI starts, but actual model/report/Stop/resume acceptance is blocked by local authentication returning Invalid API key. Full Chinese IME and sustained recovery/output-pressure matrices remain open.

## Recommended order

1. Use Canban for a few real tasks across at least two Projects. Exercise a normal task, a human-input blockage, a rejected review/rework cycle and an interrupted task. Record friction in Cards before expanding scope. A useful acceptance target is completing work without manually repairing SQLite, editing runtime metadata or searching for lost working directories.
2. Make human disposition and code delivery explicit. First agree on a small Review surface: read the report, inspect a basic diff, send changes back to Develop with instructions, or accept the result. Define the target branch and a deliberate commit/merge action separately from Archive. A basic diff is sufficient; this does not require importing the full Shear editor. Merge policy and conflict behavior still need a product decision before implementation.
3. Stabilize the existing native interaction and recovery paths. Prioritize Chinese IME, resize/focus, static redraw/scrolling, long output, unavailable core, interrupted delivery, missing provider history and host restart during live work. Keep the existing simple scheduling model; these checks do not justify concurrency caps or a new orchestration framework.
4. After valid Claude authentication is available, run the same live reply/completion/normal-exit/exact-resume acceptance used for Codex. Do not infer provider acceptance from successful startup or adapter tests.

Do not expand into GitHub backends, visual template design, advanced timers, daemon extraction or full Shear integration before the daily-use loop identifies a concrete need. Those remain deferred capabilities, not prerequisites for trying this MVP.

## Next milestone

The recommended next milestone is a small daily-use build in which a person can understand task state, steer an Agent, review and request changes, deliberately bring accepted code back to the intended branch, and archive the Card while preserving its history. Track the remaining gaps explicitly rather than expressing progress as an unsupported percentage.
