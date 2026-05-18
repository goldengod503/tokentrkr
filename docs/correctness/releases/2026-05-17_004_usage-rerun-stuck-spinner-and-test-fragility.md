# 2026-05-17 — Usage rerun: stuck-spinner recovery, deterministic exit test, race documentation

## Summary

Implements three recommendations (A1, A2, A3) from the SECOND
architectural-analysis run on `src/usage/` (the rerun executed against
the post-`2026-05-17_003` codebase). Closes the libcosmic
subscription-restart stuck-spinner path (R1 — Medium), re-arms the A1
leak regression guard with a deterministic termination assertion
(R2 — Medium), and amends `docs/ARCHITECTURE.md` to acknowledge the
bounded Full→Closed `emit()` race surfaced by the rerun's C2 finding
(R3 — Low, doc-only).

Also includes a small inline comment at the `RateLimited` retry-ladder
arm in `do_one_fetch` to record why it uses raw `emit()` rather than
the `emit_or_abort` helper (rerun S2 — deferred to comment-only per
the architect).

## Scope

### Files Created

- docs/correctness/releases/2026-05-17_004_usage-rerun-stuck-spinner-and-test-fragility.md (this doc)

### Files Modified

- src/cosmic_app.rs — adds `Message::UsageStreamUnavailable` variant
  (line 116-126 block), emits it from the subscription closure's
  `take()-returned-None` branch before the idle loop (lines 572-578),
  handles it in `update()` by clearing `refreshing` / `fetch_done` /
  `pending_snapshot` and setting a "restart applet" error.
- src/usage/service.rs — adds `#[cfg(test)] pub(crate) task:
  tokio::task::JoinHandle<()>` field on `UsageHandle`; `spawn()`
  captures the handle and threads it into the struct via a cfg-gated
  field; the `run_loop_exits_when_event_receiver_is_dropped` test now
  uses `tokio::time::timeout(1s, handle.task)` for deterministic
  termination + `assert_eq!(mock.call_count(), 1)`; one inline comment
  added at the `RateLimited` arm of `do_one_fetch`.
- src/main.rs — destructure pattern updated from
  `let UsageHandle { events, refresh } = ...` to
  `let UsageHandle { events, refresh, .. } = ...` to ignore the new
  test-only field.
- docs/ARCHITECTURE.md — amends `try_send + Stalled` Intentional
  decision with a Race paragraph documenting the Full→Closed TOCTOU
  bound; adds two Known-deferred entries (`retrying_in` dead field,
  `FetchOutcome::Aborted` conflation) per the rerun's deferral list;
  adds a `Recent architectural changes` entry for this shipment;
  updates line counts in `Current structure`.

## Behavioral Impact

| Scenario | Before | After |
|---|---|---|
| libcosmic subscription restarts after `USAGE_HANDLE.take()` returns `Some(handle)` while a fetch was in flight (`refreshing == true`) | `loop { sleep(86400) }` idle branch never emits anything; `refreshing` stays true; `SpinTick` subscription fires at 50ms (20Hz) for process lifetime; UI shows permanent spinner; user has no recovery path | Subscription closure emits `Message::UsageStreamUnavailable` once before idling; `update()` clears spinner state, drops any pending snapshot, displays "Usage stream unavailable (restart applet)" error. `SpinTick` stops. User sees a visible error and knows to restart the applet. |
| Receiver-drop regression test (`run_loop_exits_when_event_receiver_is_dropped`) | Asserted `mock.call_count() <= 2` after `yield_now() × 2` — empirical bound with no derivation. Silently passes if a refactor adds an `.await` point between sleep-wake and the `FetchStarted` emit. | Awaits the spawned task's `JoinHandle` with a 1s simulated-time timeout; asserts `mock.call_count() == 1` exactly. Fails immediately if the loop fails to terminate, regardless of yield geometry. |
| Normal operation (poll, retry ladder, 401 → Dormant, manual refresh, EmptyResponse → Dormant) | Existing behavior | Unchanged — all 13 pre-rerun usage tests still pass. |
| `emit()` Full→Closed race | Documentation said "returns immediately on Closed" — strictly true only outside the Full→Closed TOCTOU window. | Documentation now acknowledges the bounded one-iteration overrun (one extra `fetch_usage()` call in the narrow race window). Code unchanged — risk-analyst R3 verdict was "leave alone." |
| `UsageHandle` public API | Two `pub` fields: `events` and `refresh`. | Same plus one `#[cfg(test)] pub(crate)` `task` field. Production callers `main.rs` and `cosmic_app.rs::init` are unaffected (`init` stores by value, `main.rs` destructures with `..`). |

## Test Plan

The receiver-drop test was rewritten in place (not added). One existing
test's assertion shape changed:

- `run_loop_exits_when_event_receiver_is_dropped` — now uses
  `tokio::time::timeout(Duration::from_secs(1), handle.task).await`
  followed by `assert_eq!(mock.call_count(), 1)`. The previous
  `yield_now()` × 2 with `<= 2` bound is gone.

Verification:

```
cargo test --no-default-features  →  41 passed; 0 failed
cargo test                        →  41 passed; 0 failed
cargo build                       →  cosmic feature compiles cleanly
```

Test count is unchanged at 41 (the rewrite is in-place).

Manual verification of the new `UsageStreamUnavailable` UI path
deferred to the next libcosmic subscription restart in real use —
`cosmic-panel` was not running during the verification window, and
forcing a subscription restart programmatically is not exposed by
libcosmic at rev `17291536`. The change is straightforward enough that
inspection-grade review covers it: one new Message variant, one
`channel.send` before the existing idle loop, one `update` arm that
sets four fields.

## Docs Updated

- docs/correctness/releases/2026-05-17_004_usage-rerun-stuck-spinner-and-test-fragility.md (this doc)
- docs/ARCHITECTURE.md:
  - `try_send + Stalled` Intentional decision amended with the
    Full→Closed Race paragraph (A3).
  - Two new Known-deferred entries: `retrying_in` dead field (S1) and
    `FetchOutcome::Aborted` conflation (S3).
  - New `Recent architectural changes` entry for this shipment.
  - Line counts in `Current structure` updated for `cosmic_app.rs`
    (858 → 878) and `service.rs` (592 → 611).
- CLAUDE.md — verified-against SHA will be bumped in a follow-up
  doc-refresh commit on main, matching the pattern from release
  `2026-05-17_003`.

## Rollback Plan

Revert the merge commit:

```bash
git revert -m 1 <merge-sha>
```

The three sub-changes (A1, A2, A3) are coupled through the same PR
because A1's UI behavior is hard to verify without A2's tightened
test, and A3 documents the race the rerun surfaced. Splitting the
rollback would require landing them as three separate commits on the
branch (not done — the codebase favors small bundled PRs over
fine-grained commit history for the same logical change).

## Open Questions / Decisions

### Decision: `Message::UsageStreamUnavailable` is a one-shot signal, not a recoverable event

- Status: Accepted
- Date: 2026-05-17
- Context: The libcosmic `OnceLock<UsageHandle>` pattern is take-once
  by construction. When the second subscription invocation finds the
  handle already consumed, it cannot re-acquire it from inside the
  subscription closure — the source of truth is `BOOTSTRAP`'s spawn
  call inside `init()`, and `init()` runs exactly once per applet
  lifetime. Recovery requires restarting the applet process.
- Decision: The variant is emitted once before the closure enters the
  86400s idle loop. The `update()` handler clears in-flight UI state
  and surfaces an error string instructing the user to restart. No
  auto-recovery attempted.
- Consequences: User sees a clear "Usage stream unavailable (restart
  applet)" message instead of a stuck spinner with no explanation.
  The 20Hz `SpinTick` timer-burn also stops because `refreshing` is
  cleared. Manual restart is the only recovery path — accepted as
  the existing libcosmic constraint.
- References: src/cosmic_app.rs:572-580 (subscription emission),
  src/cosmic_app.rs Message::UsageStreamUnavailable arm in update().

### Decision: Receiver-drop test asserts `call_count == 1`, not `<= 2`

- Status: Accepted
- Date: 2026-05-17
- Context: After the receiver is dropped and the post-fetch sleep
  fires, `run_loop` runs: drain `refresh_rx` (no items, no await),
  call `emit(FetchStarted)` which observes `ChannelClosed`, return.
  No `.await` between sleep-wake and return. `do_one_fetch` is never
  called on this path.
- Decision: `assert_eq!(mock.call_count(), 1)` — exactly one fetch
  (the pre-drop cycle). The architect's sketch suggested `== 2` but
  trace analysis confirms the post-wake path observes Closed *before*
  calling `do_one_fetch`.
- Consequences: Test is strictly tighter than the architect's
  suggestion. Future refactor that inserts an `.await` between
  sleep-wake and the FetchStarted emit will cause the timeout to
  fire (loop fails to terminate within 1s) or the assertion to fail
  (call_count > 1 because do_one_fetch ran first). Both produce a
  clear failure signal.
- References: src/usage/service.rs `run_loop_exits_when_event_receiver_is_dropped`.

### Decision: `Full→Closed` TOCTOU is documented, not fixed

- Status: Accepted
- Date: 2026-05-17
- Context: The rerun's C2 finding identified a one-iteration overrun
  when `emit()`'s primary `try_send` returns `Full` and the second
  `try_send(Stalled)` finds the channel newly `Closed`. The function
  returns `DroppedFull`, the loop continues one iteration, and the
  next emit observes `Closed`. ARCHITECTURE.md previously said the
  loop "returns immediately on Closed" — strictly contradicted in
  this race.
- Decision: Document the bounded overrun. Do not change the code.
- Consequences: One extra authenticated `fetch_usage()` HTTP call per
  subscription restart that happens to hit the Full→Closed window.
  Risk-analyst rated this Low; action risk (re-touching the `emit()`
  contract that guards the A1 leak fix) exceeds inaction risk. Doc
  amendment lets future analysis runs classify this as "documented
  accepted tradeoff" rather than re-prosecuting it.
- References: src/usage/service.rs `emit()` at lines 219-226; doc
  amendment in the `try_send + Stalled` Intentional decision section.

### Open: A4 (RetryPolicy public surface) still deferred

- Status: Open (carried from `2026-05-17_003`)
- Date: 2026-05-17
- Question: `with_retry()` has zero external callers; compiler warns.
  Recorded as Known-deferred A4 in ARCHITECTURE.md.
- Resolution: TBD — reopen trigger ("any caller outside `usage/`
  constructs a `RetryPolicy` or calls `with_retry()`") has not fired.

## References

- Architectural-analysis re-run report — 2026-05-17 (recommendations A1, A2, A3 from the rerun)
- Precursor release docs:
  - `docs/correctness/releases/2026-05-17_003_usage-leak-and-empty-classification.md`
  - `docs/correctness/releases/2026-05-17_002_usage-service-extraction.md`
- ARCHITECTURE.md: `try_send + Stalled` Intentional decision, the new Known-deferred entries for S1 (`retrying_in` dead field) and S3 (`FetchOutcome::Aborted` conflation), and the `Recent architectural changes` entry for this shipment.
