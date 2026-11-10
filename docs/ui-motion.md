# Native UI motion

Status: implemented first batch, 2026-11-10. Motion communicates an existing
state change; it never delays navigation or an application operation.

## Shared policy and ownership

`canban-ui-kit::motion` owns the durations, reduced-motion policy and retained
presence primitive. NSH owns native interpolation, layout transitions, clipping,
hit testing and modal presentation. Product UI owns routing and document identity.
There is no application-wide layout animation: unrelated Agent/event refreshes
must not start movement throughout the window.

| Surface | Transition | Lifetime and interaction |
| --- | --- | --- |
| Board → Card pane | 220ms ease-out reveal and fade | Board stays mounted; text is laid out at the final pane width |
| Card pane → Board | 180ms ease-out collapse and fade | Outgoing Card remains mounted until native animation completion; pointer input and submit shortcuts stop immediately |
| Focused/unfocused comment editor | 160ms scoped layout transition | Same editor, selection and undo owner; focused controls expand and blur compacts them |
| Quick Open / Project chooser / utility popup / Terminal rename | NSH native 200ms fade | Native modal dismissal removes interaction immediately and animates its outgoing image |

The native modal fade is 200ms rather than introducing a custom shorter modal
engine. This preserves NSH's dismissal and keyboard ownership. The macOS Reduce
Motion preference skips Canban transitions. Gallery can additionally opt into
reduced motion through a scoped `ReducedMotion` context without changing OS
settings.

## Card workspace

```text
one mounted workspace for a Project
  Board                         retained Card region
  +-------------------------+   +------------------------+
  | columns / scroll state  |   | scoped Card ID          |
  |                         |<->| header / reader / editor|
  +-------------------------+   +------------------------+
                   native resizable boundary

Open A   -> mount A, reveal region
Select B -> replace A with B; keep the region open
Close    -> change route now; retain A/B only for the exit animation
Reopen   -> retarget the native animation from its current value
```

Closing and reopening increments a generation; an interrupted animation's answer
cannot unmount the new destination. Unmount uses actual native completion, not a
timeout. A scoped Card identity prevents the outgoing view from reading the
cleared global route or targeting a different Card. Drafts remain in the existing
document store. Project changes replace the workspace; other pages retain their
own routing behavior. Narrow windows show the full Card pane and restore the
Board on close. The covered Board and outgoing Card are excluded from pointer
targeting and accessibility. Resizing the split remains a direct native interaction.

The common reader follows the bottom through composer resizing only when it
was already following; reading older messages retains its position. Overlay
dismissal restores a previously focused Markdown editor only while its document
is still the current destination. This explicitly handles NSH's editor sink,
which can be skipped by native modal focus restoration because it rejects
pointer targeting. A modal selection that navigates away cannot focus an outgoing
editor.

## Regression scenes

Run `CANBAN_GALLERY_SCENE=motion mise run gallery` for production Board/Card
components with controls for two Cards, close, reversal after 80ms, reduced motion,
Quick Open and narrow/wide composition. The ordinary Card conversation scene
still covers long Markdown histories and background replies.

`CANBAN_MOTION_TRACE=1` optionally records workspace/Card mounts and a bounded
sample of native panel opacity. Sampling is disabled in normal runs. These logs
help distinguish a real fade/reversal from simply reaching the final state;
they are not FPS or frame-latency measurements.

Overview inspectors, Terminal session sidebars and secondary disclosures are
outside this first batch. Board drag/placeholder animations retain their existing
implementation.

## Verification (2026-11-10)

- Packaged App and standalone Gallery built with `mise run build` and
  `mise run gallery`; 150 workspace tests and strict all-target Clippy passed.
- Native Gallery exercised two-Card switching, draft retention, boundary resize
  (700 → 594 points), close/reopen, 80ms reversal, reduced motion and narrow/wide
  composition. Opt-in samples captured a reversed fade continuing at 0.465 and
  progressing to 1.000, with one workspace mount across these Card operations.
- The packaged App was copied into the previously approved native acceptance
  bundle and run against `/private/tmp/cb-card-editor-8ckkfqds/home`, using its
  existing long/short Cards and isolated Core configuration. Actual clicks checked
  panel dismissal/reopening, composer expansion/contraction and draft retention;
  Command+Enter persisted the retained comment. Quick Open cancellation restored
  typing into the same editor; selecting another Card left focus at the new
  destination. Shortcuts outside-click dismissal and Theme open/close also passed.
- `mise run acceptance` passed Develop/Review, mention/blocked reporting,
  Archive/Restore, WAL, restart and three-Board concurrency over real Herdr PTYs
  at `/private/tmp/cb-1gh8ef5n/home`. Its first run exposed a fixture bootstrap race:
  default and configured Core sessions competed for the scratch socket. The
  harness now stops bootstrap observation/Core before starting the configured
  session; the full harness passed after that repair.

These are functional native checks and deterministic PTY acceptance, not a new
real-model evaluation or measured frame-rate benchmark. Chinese IME was excluded
as requested. Terminal rename inherits the same native modal adapter but its
click-level flow was not independently repeated in this motion pass.
