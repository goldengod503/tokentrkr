# 2026-05-17 — Extract usage::Service orchestration core

## Summary

Implements recommendation A1 from the 2026-05-17 architectural analysis.
Extracts the fetch + retry + error-classification loop from polling.rs
and the inline iced Subscription closure in cosmic_app.rs into a single
src/usage/ module with a typed UsageEvent stream that both shells
consume. Fixes R4 (no stop on Unauthorized), R2 (COSMIC backpressure
stall), B3 (spinner generation race) and resolves the S10 retry
asymmetry between shells.

## Scope

### Files Created

- src/usage/mod.rs
- src/usage/event.rs
- src/usage/retry.rs
- src/usage/service.rs (includes MockProvider in `#[cfg(test)]`)
- docs/correctness/releases/2026-05-17_002_usage-service-extraction.md (this doc)

### Files Modified

- src/main.rs — `mod usage;`, restructured `run_sni`, `cosmic_app::run` call site
- src/tray.rs — added `apply_event`, replaced `cmd_tx` with `refresh_tx`, plus 4 unit tests
- src/cosmic_app.rs — added `Message::Usage(UsageEvent)` + `handle_event`,
  replaced subscription closure with forwarder, swapped PROVIDER/POLL_SECS
  OnceLocks for USAGE_HANDLE + BOOTSTRAP, removed dead `provider` field
- Cargo.toml — added `tokio = { features = ["test-util"] }` to dev-dependencies
  (required for `#[tokio::test(start_paused = true)]`)

### Files Deleted

- src/polling.rs

## Behavioral Impact

| Scenario | Before | After |
|---|---|---|
| Normal poll cadence | 5 min (configurable) | Same |
| 429 rate-limit | SNI: 10/30/60s ladder; COSMIC: no retry | Same on SNI; COSMIC now matches |
| Manual Refresh | Immediate fetch | Same; now drains during retry-ladder so no extra fire |
| Unauthorized | Both paths: poll every 5 min forever | Service enters Dormant state; retries every 15 min; manual Refresh preempts |
| Spinner during manual refresh | Could apply older snapshot (B3) | `fetch_id` discriminates; stale snapshots dropped |
| UI backpressure | COSMIC could deadlock the poll loop (R2) | `try_send` + `Stalled` event; loop never blocks on send |

## Test Plan

12 new unit tests in `src/usage/`:

- event: 1 (variant construction across all 5 variants)
- retry: 3 (default policy values)
- service: 8 (full state machine via MockProvider + tokio::time::pause)
  - mock_returns_scripted_results_in_order
  - normal_happy_path_emits_fetch_started_then_snapshot
  - retry_ladder_emits_three_transient_then_snapshot
  - retry_ladder_exhausted_emits_final_transient_with_none
  - unauthorized_emits_permanent_error_then_no_normal_poll
  - dormant_returns_to_normal_after_15min_success
  - manual_refresh_preempts_dormant_wait
  - fetch_id_increments_monotonically_across_cycles

4 new unit tests in `src/tray.rs` for `apply_event`.

Total: 39 tests pass (23 pre-existing + 12 in `usage::` + 4 in `tray::tests`).

Manual verification of the COSMIC-spawned applet path deferred to the
next normal use of the desktop — `cosmic-panel` was not running during
the verification window. The 39 unit tests cover the state machine
exhaustively, and `cargo build --release` succeeds for both features.

## Docs Updated

- docs/correctness/releases/2026-05-17_002_usage-service-extraction.md (this doc)

Spec and plan in `~/homelab2-docs/`:
- specs/tokentrkr/2026-05-17-usage-service-design.md
- plans/tokentrkr/2026-05-17-usage-service-implementation.md

## Rollback Plan

Revert the merge commit:

```bash
git revert -m 1 <merge-sha>
```

Or, for partial rollback (keep tests but revert one commit at a time),
each commit on `feat/usage-service` is incrementally consistent EXCEPT
the deliberate "broken build" between Tasks 14 and 15 (commits `87c887d`
and `ade5fb9`) and between Tasks 15 and 17 (build is also broken with
cosmic feature on between those commits). A merge revert is the safe
rollback path.

## Open Questions / Decisions

### Decision: 15-min Dormant retry interval

- Status: Accepted
- Date: 2026-05-17
- Context: When Unauthorized fires, we need a policy for retry cadence.
  Options were: stop entirely (manual Refresh only), 1 hour, or 15 min.
- Decision: 15 min, with manual Refresh as a preemption.
- Consequences: Catches the common "user re-ran `claude login`" recovery
  workflow without requiring them to click Refresh. ~96 wasted 401s/day
  if creds stay revoked vs ~288 at 5 min; tradeoff accepted.
- References: src/usage/retry.rs, src/usage/service.rs

### Decision: try_send + Stalled on event channel backpressure

- Status: Accepted
- Date: 2026-05-17
- Context: The COSMIC subscription's prior pattern of
  `channel.send(...).await` outside the select! could deadlock the loop
  under UI rendering load (R2).
- Decision: Service never `.await`s on event-channel send. Uses
  `try_send`; on Full, attempts to emit `UsageEvent::Stalled` (also
  `try_send`, may itself be dropped). Loop keeps running.
- Consequences: Some events may be dropped under sustained UI overload.
  The shell can ignore Stalled or surface it as a transient warning.
- References: src/usage/service.rs (`emit` function)

### Open: live `SetInterval` reconfiguration

- Status: Open
- Date: 2026-05-17
- Question: The old PollCommand had `SetInterval(Duration)` but it
  was never wired to any UI. Dropped from the new design. If a future
  config-reload feature wants to change the poll interval without
  restarting the app, the service will need a control channel for
  this.
- Resolution: TBD — YAGNI until requested.

### Open: HTTP-layer testability

- Status: Open (carried from A2's release doc)
- Date: 2026-05-17
- Question: `ClaudeProvider` holds a concrete `reqwest::Client` with no
  injection point. The new `MockProvider` covers the state-machine layer
  but not the HTTP-layer retry behavior of `ClaudeProvider::fetch_usage`
  itself.
- Resolution: TBD — defer until an HTTP-layer bug warrants the
  injection refactor.

## References

- Architectural analysis report — 2026-05-17 (recommendation A1)
- Spec: ~/homelab2-docs/specs/tokentrkr/2026-05-17-usage-service-design.md
- Plan: ~/homelab2-docs/plans/tokentrkr/2026-05-17-usage-service-implementation.md
- A2 release doc: docs/correctness/releases/2026-05-17_001_token-refresh-429-gating.md
