# 2026-05-17 — Usage service: channel-closed propagation, EmptyResponse classification, sentinel relocation

## Summary

Implements three recommendations (A1, A2, A3) from the 2026-05-17
architectural analysis of `src/usage/`. Closes the COSMIC subscription
restart task leak (R1 — High), brings `EmptyResponse` under the Dormant
retry ceiling (R4 — Medium), and relocates the three sentinel error
types from `main.rs` to `provider.rs` next to the trait they describe
(R3 — Medium).

## Scope

### Files Created

- docs/correctness/releases/2026-05-17_003_usage-leak-and-empty-classification.md (this doc)

### Files Modified

- src/usage/service.rs — `EmitResult` enum, `FetchOutcome::Aborted`,
  `emit_or_abort` helper, `run_loop` early-return on `ChannelClosed`,
  `do_one_fetch` propagation of `ChannelClosed` → `Aborted`, explicit
  `EmptyResponse` downcast arm mapping to `FetchOutcome::Permanent`,
  `MockOutcome::EmptyResponse` test variant, two new unit tests.
- src/provider.rs — `RateLimited`, `Unauthorized`, `EmptyResponse`
  structs (with `Display` and `Error` impls) moved in next to the
  `Provider` trait.
- src/main.rs — sentinel struct definitions removed; `use std::fmt`
  dropped (no longer used after the relocation).
- src/claude.rs — six call sites updated from `crate::<Sentinel>` to
  `crate::provider::<Sentinel>` (production code + two tests).

## Behavioral Impact

| Scenario | Before | After |
|---|---|---|
| COSMIC subscription restart (libcosmic re-fires the subscription) | UI frozen with stale snapshot + leaked tokio task continues authenticated HTTP fetches at ~12/hour forever | `run_loop` detects `TrySendError::Closed` on the event channel and returns. Task exits cleanly; no further HTTP traffic. |
| SNI path: tray shutdown closes the receiver | `emit()` swallowed `Closed`; loop kept polling against a closed channel | Same exit path — loop terminates on the next emit attempt. |
| Persistently empty usage API response | `EmptyResponse` fell into the catch-all `Err(e)` arm → `Transient` → polled every 5 min indefinitely (~288 retries/day) | Explicit `EmptyResponse` arm → `Permanent` → state machine drops to Dormant → 15-min retry cadence (~96 retries/day). Manual Refresh preempts. |
| `RateLimited` / `Unauthorized` / `EmptyResponse` produce + downcast paths | Symbols at `crate::*` (defined in `main.rs`) | Symbols at `crate::provider::*` (defined alongside the `Provider` trait). No runtime change. |
| Normal poll, 429 ladder, 401 → Dormant, manual refresh, dormant→normal recovery | Existing behavior | Unchanged — all 13 pre-existing usage tests still pass without modification. |

The `try_send + Stalled` backpressure decision documented in
`docs/ARCHITECTURE.md` is preserved — `EmitResult::DroppedFull` still
fires a `Stalled` sentinel and continues the loop. Only the
`ChannelClosed` case is new behavior.

## Test Plan

Two new unit tests added in `src/usage/service.rs`:

- `run_loop_exits_when_event_receiver_is_dropped` — drains one fetch
  cycle, drops the `Receiver<UsageEvent>`, advances 50 minutes of
  simulated time, asserts `mock.call_count() <= 2`. Without the fix
  the loop would have made ~10 additional `fetch_usage` calls into the
  closed channel.
- `empty_response_emits_permanent_error_then_no_normal_poll` — mirrors
  the existing `unauthorized_emits_permanent_error_then_no_normal_poll`
  test. Asserts the service emits `PermanentError`, enters Dormant, and
  does not poll again within 10 minutes (proving the 15-min interval
  applies, not the 5-min `poll_interval`).

Verification:

```
cargo test --no-default-features  →  41 passed; 0 failed
cargo test                        →  41 passed; 0 failed
cargo build                       →  cosmic feature compiles
```

Test count moves from 39 to 41.

## Docs Updated

- docs/correctness/releases/2026-05-17_003_usage-leak-and-empty-classification.md (this doc)
- docs/ARCHITECTURE.md — `Recent architectural changes` section gains a
  2026-05-17 entry for A1/A2/A3; the `Intentional decisions` section
  is amended with a brief note on `EmitResult::ChannelClosed` and the
  sentinel location.
- CLAUDE.md — verified-against SHA bumped to the merge commit of this
  branch.

## Rollback Plan

Revert the merge commit:

```bash
git revert -m 1 <merge-sha>
```

Each of A1, A2, A3 is a self-contained patch. A1 and A2 land together
in `src/usage/service.rs` (they share the same `do_one_fetch` rewrite
and a new test mock variant), so partial rollback would require
splitting that file's commit. A3 is purely a relocation + import
rewrite and could be reverted independently if needed.

## Open Questions / Decisions

### Decision: `EmptyResponse` is `Permanent`, not a third outcome class

- Status: Accepted
- Date: 2026-05-17
- Context: The architectural analysis flagged that `EmptyResponse`
  bypasses the 15-min Dormant ceiling. Options: (a) introduce a
  fourth `FetchOutcome` variant (e.g., `EmptyData`) with its own
  cadence; (b) classify as `Permanent` and reuse the Dormant gate.
- Decision: (b). The semantics already match — the API is responsive
  but useless, identical from the user's perspective to a 401 that
  needs manual recovery. Reusing the Dormant path keeps the state
  machine at two states and produces a `PermanentError` event the
  shells already render correctly.
- Consequences: A user whose API consistently returns empty bodies
  sees the same "ERR" indicator they would for a 401, with retries
  every 15 minutes instead of every 5. Manual Refresh preempts.
- References: src/usage/service.rs (the `EmptyResponse` arm in
  `do_one_fetch`), src/claude.rs (the `EmptyResponse` `bail!` site).

### Decision: `FetchOutcome::Aborted` is internal — no `UsageEvent` is emitted

- Status: Accepted
- Date: 2026-05-17
- Context: When the event receiver drops, the service has nothing it
  can usefully tell the (now-gone) consumer. We could emit a
  `PermanentError` first (which would also fail to send), or simply
  exit silently.
- Decision: Exit silently. `run_loop` returns immediately on
  `ChannelClosed`. No log line — the most common cause is process
  teardown where logging is noise. If the cause is a COSMIC
  subscription restart, the issue is upstream (libcosmic) and a log
  here would not help diagnose it.
- Consequences: No observability of the exit-from-leak path. If this
  turns out to matter in practice (e.g., to distinguish "shut down
  cleanly" from "subscription restarted"), add a `tracing::info!` at
  the `return` site.
- References: src/usage/service.rs (`run_loop`'s two early `return`
  statements).

### Decision: Sentinels live in `src/provider.rs`, not a new `src/errors.rs`

- Status: Accepted
- Date: 2026-05-17
- Context: A3 needed a home for the three sentinel structs other than
  `main.rs`. Options: (a) a new `src/errors.rs` module; (b) collapse
  into `src/provider.rs` next to the trait that defines the contract
  they belong to.
- Decision: (b). The sentinels are part of the `Provider` contract —
  every implementation must produce them and every consumer downcasts
  to them. Co-locating them with the trait keeps cohesion high and
  avoids a third tiny module.
- Consequences: `src/provider.rs` grows from 11 to ~50 lines. If a
  future error type is unrelated to `Provider` (e.g., a config-load
  sentinel), a new file may be warranted at that time.
- References: src/provider.rs (the new struct definitions).

### Open: HTTP-layer testability for `ClaudeProvider`

- Status: Open (carried from release doc `2026-05-17_002`)
- Date: 2026-05-17
- Question: `ClaudeProvider::fetch_usage` itself is still not exercised
  by tests. The state-machine layer is now exhaustively covered.
- Resolution: TBD — defer until a `ClaudeProvider` bug ships that the
  existing unit tests don't catch.

## References

- Architectural analysis report — 2026-05-17 (recommendations A1, A2, A3)
- A1 release doc (precursor extraction): `docs/correctness/releases/2026-05-17_002_usage-service-extraction.md`
- A2 release doc (precursor token-refresh gating): `docs/correctness/releases/2026-05-17_001_token-refresh-429-gating.md`
