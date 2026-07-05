# 2026-07-05 — State & UI completeness one-liners (R5, R6, R8)

## Summary

Three independent one-line fixes from the 2026-06-10 core review, each
delegating a behaviorally significant case to a catch-all/omission that
did the wrong thing: (R5) a 429 on the dormant recovery poll kept the
service Dormant for a second full 15-minute wait (~30 min to recover);
(R6) the COSMIC Refresh button became a permanent silent no-op after a
terminal stream failure; (R8) the SNI tooltip showed a stale error as
if terminal for up to ~8 minutes while the service was actively
retrying. Each fix is its own commit, independently revertible.

## Scope

### Files Created

- docs/correctness/releases/2026-07-05_003_state-ui-completeness.md (this doc)

### Files Modified

- src/usage/service.rs — `(State::Dormant, FetchOutcome::Transient) =>
  State::Normal` added before the catch-all, with rationale comment
  (server reachable contradicts the Dormant precondition). `Aborted`
  still exits via the early return before the state table — the
  Known-deferred Aborted-conflation note is untouched.
- src/cosmic_app.rs — `UsageStreamUnavailable` handler now also sets
  `self.refresh_tx = None`, making `RefreshNow`'s existing `Some`-guard
  honest. No OnceLock/subscription machinery touched.
- src/tray.rs — `FetchStarted` clears `error`; `Stalled` remains a
  no-op. No per-error-class styling added (explicitly rejected).
- docs/ARCHITECTURE.md — exit-on-Transient note added under the
  "15-minute Dormant retry" Intentional decision (R5). R6/R8 are below
  architectural altitude.
- docs/release-ledger.md — R5/R6/R8 marked Done.

## Behavioral Impact

| Scenario | Before | After |
|---|---|---|
| `[EmptyResponse → Dormant, 429 on 15-min recovery poll, then Ok]` | Catch-all kept Dormant; user waited another 15 min (~30 min total) | Transient exits Dormant; next poll at normal cadence (~15 min total) |
| Unauthorized → Dormant transition and 15-min cadence | — | Unchanged (Intentional decision preserved; only the exit-on-Transient added) |
| Refresh button after `UsageStreamUnavailable` | Visually live, `try_send` into dead channel forever | Guard sees `None`; button is an honest no-op; "restart applet" error remains the recovery instruction |
| SNI tooltip during 429 ladder + poll wait | Stale error shown up to ~8 min, indistinguishable from "re-login required" | Cleared when the retry fetch starts |
| SNI tooltip on `Stalled` | Unchanged | Unchanged (no-op, explicit test) |

## Test Plan

Two new + one restructured test (53 passing total, was 51), both targets:

- `transient_error_during_dormant_recovery_returns_to_normal_cadence`
  (service.rs, paused clock) — scripts
  `[EmptyResponse, 429×4, Ok]`; asserts the post-Transient poll fires
  at 300s, not 900s, with a bounded timeout so paused-clock
  auto-advance can't mask a still-Dormant loop. **Proven
  discriminating: run red with the arm commented out, green with it.**
- `fetch_started_after_transient_error_clears_stale_error` (tray.rs).
- `fetch_started_and_stalled_are_no_ops` split: the Stalled half
  survives as `stalled_is_a_no_op` (now seeded with an error to prove
  retention); the FetchStarted half is superseded by the new test.
- R6 (cosmic_app.rs) has no unit test — UI-shell handler with no
  existing harness; verified by review against the `RefreshNow` guard.
  No visual claim made.

```
cargo test                        →  53 passed; 0 failed
cargo test --no-default-features  →  53 passed; 0 failed
```

## Docs Updated

- docs/ARCHITECTURE.md — Dormant Intentional decision amended (R5).
  Verified-against SHA not bumped: one state-table arm added within the
  documented state machine; the decision entry records it in place.
- docs/release-ledger.md — updated.

## Rollback Plan

Each fix is its own commit — revert independently:

```bash
git revert <R5 commit>   # fix(usage): exit Dormant on Transient outcome
git revert <R6 commit>   # fix(cosmic): drop refresh_tx on UsageStreamUnavailable
git revert <R8 commit>   # fix(tray): clear stale error when a new fetch starts
```

## Open Questions / Decisions

None — all three fixes follow the spec as written; the spec's
constraints (no Aborted change, no OnceLock refactor, no per-error-class
styling) were honored.

## References

- Spec: `homelab2-docs/specs/tokentrkr/2026-06-10-state-ui-completeness.md`
  (R5 ← B4, R6 ← C4, R8 ← B6).
- Source citations: src/usage/service.rs state table + new test;
  src/cosmic_app.rs `UsageStreamUnavailable` / `RefreshNow` handlers;
  src/tray.rs `apply_event` + tests.
