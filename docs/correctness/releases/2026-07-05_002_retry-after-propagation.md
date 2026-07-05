# 2026-07-05 — Carry server `retry-after` through the `RateLimited` sentinel

## Summary

On a 429, `ClaudeProvider` parsed the `retry-after` header, logged it,
and threw it away; the retry ladder slept its fixed 10/30/60s steps
regardless of what the server asked. If the server requested a 120s
backoff, all three ladder steps fired within ~100s against a
still-rate-limited server, exhausted, and the service waited the full
poll interval — up to ~8 minutes to recovery instead of the requested 2,
during exactly the heavy-usage sessions when the user is watching quota.
The sentinel now carries the hint and the ladder waits
`max(ladder_step, hint)`. Fixes R3 (Medium) from the 2026-06-10 core
architecture review.

## Scope

### Files Created

- docs/correctness/releases/2026-07-05_002_retry-after-propagation.md (this doc)

### Files Modified

- src/provider.rs — `RateLimited` is now
  `pub struct RateLimited { pub retry_after: Option<Duration> }`.
  Display string unchanged.
- src/claude.rs — the 429 arm constructs the sentinel via a new
  `parse_retry_after_secs` helper: RFC 9110 delta-seconds only
  (HTTP-date and garbage → `None`), capped at 900s (the dormant
  interval) so a malformed/hostile header can't park the loop.
- src/usage/service.rs — the `RateLimited` ladder arm computes
  `retrying_in` as `max(ladder_step, hint)` on in-ladder attempts and
  `None` on exhaustion (hint dropped — see Decisions). The emitted
  `TransientError.retrying_in` carries the effective delay.
  `MockProvider` gains a `RateLimitedWithHint(Duration)` script variant.
- docs/ARCHITECTURE.md — new Intentional decision recording the payload,
  the parse/cap choices, the max() floor semantics, and that the
  shared-retry-policy decision is preserved.
- docs/release-ledger.md — R3 marked Done.

## Behavioral Impact

| Scenario | Before | After |
|---|---|---|
| 429 with `retry-after: 120` | Slept 10s, 30s, 60s; exhausted in ~100s; ~5 min poll wait on top | First wait is 120s; server-requested backoff honored (floor: ladder step) |
| 429 with `retry-after` below the ladder step (e.g. 2s) | 10s ladder step | Unchanged — 10s; the ladder is a floor, hints never shorten it |
| 429 with no header | 10/30/60s ladder | Byte-identical — existing ladder tests pass unmodified |
| 429 with garbage/HTTP-date header | 10/30/60s ladder | Unchanged — parses as `None`, ladder applies |
| 429 with `retry-after: 86400` | 10/30/60s ladder | Clamped to 900s at parse time |
| Ladder exhaustion (4th consecutive 429) | `TransientError { retrying_in: None }` | Unchanged, even if the 4th 429 carried a hint (see Decisions) |

## Test Plan

Five new unit tests (51 passing total, was 46), both build targets:

- `server_retry_after_hint_above_ladder_step_stretches_first_wait` —
  paused-clock; asserts `retrying_in == 120s`, that no retry fires at
  the 10s ladder mark (call_count still 1 at t=11s), and the snapshot
  lands after the full hint elapses.
- `server_retry_after_hint_below_ladder_step_keeps_ladder_wait` —
  `max(10s, 2s)` → 10s.
- `parse_retry_after_accepts_delta_seconds` (incl. whitespace trim).
- `parse_retry_after_treats_garbage_and_http_dates_as_none` (incl.
  negative values, absent header).
- `parse_retry_after_clamps_to_dormant_interval_cap` (86400 → 900).

Existing ladder tests (`retry_ladder_emits_three_transient_then_snapshot`,
`retry_ladder_exhausted_emits_final_transient_with_none`) pass
unmodified — the no-header path is byte-identical (acceptance
criterion 2). The only pre-existing code change in tests is the
`MockOutcome::RateLimited` construction site inside the mock itself.

```
cargo test                        →  51 passed; 0 failed
cargo test --no-default-features  →  51 passed; 0 failed
```

## Docs Updated

- docs/ARCHITECTURE.md — Intentional decision added (see Scope).
  Verified-against SHA not bumped: the `Provider` contract gained a
  payload field but no boundary, ownership, or state-machine shape
  changed; the new decision entry documents the contract change in
  place.
- docs/release-ledger.md — updated.

## Rollback Plan

```bash
git revert <this commit>
```

Single-commit change. The sentinel struct literal appears in
`claude.rs` (construct), `service.rs` (mock), and the field is read in
one place — a revert is clean.

## Open Questions / Decisions

### Decision: drop the hint on ladder exhaustion instead of honoring one post-ladder wait

- Status: Accepted
- Date: 2026-07-05
- Context: The spec sketched `(None, Some(s)) => Some(s)` — one extra
  honored wait after ladder exhaustion — but explicitly allowed
  simplifying to `(None, _) => None` "if the extra arm proves awkward;
  the core requirement is only the max(ladder, hint) behavior on
  in-ladder steps."
- Decision: Simplify. In `do_one_fetch`, sleeping after the final
  attempt cannot be followed by another fetch (the loop ends), so the
  emitted `retrying_in: Some(hint)` would misstate the real wait as
  `hint` when it is actually `hint + poll_interval`. Dropping the hint
  keeps exhaustion semantics byte-identical to today and keeps the
  event honest.
- Consequences: A server hint on the 4th consecutive 429 is not
  honored; the next attempt comes at `poll_interval` (300s), which
  exceeds the 900s-capped hint in most realistic cases anyway. Reopen
  if hints beyond the ladder are observed to matter in practice.
- References: src/usage/service.rs `do_one_fetch` RateLimited arm;
  spec `2026-06-10-retry-after-propagation.md` §3.

### Decision: delta-seconds only, capped at 900s

- Status: Accepted
- Date: 2026-07-05
- Context: `retry-after` per RFC 9110 is delta-seconds or HTTP-date.
- Decision: Parse delta-seconds only (the realistic API behavior);
  HTTP-date and garbage → `None`. Cap at 900s, matching
  `dormant_interval`, per the risk assessment's "sleeping implausibly
  long on a bad header" hazard.
- Consequences: An HTTP-date hint falls back to the ladder — acceptable
  degradation, never worse than pre-change behavior.
- References: src/claude.rs `parse_retry_after_secs` and its three
  unit tests.

## References

- Spec: `homelab2-docs/specs/tokentrkr/2026-06-10-retry-after-propagation.md`
  (R3 ← finding B1).
- Preserved Intentional decision: "COSMIC and SNI share one retry
  policy" — backoff stays entirely in `usage::`, consumed by both
  shells; no per-shell logic introduced.
- Source citations: src/provider.rs `RateLimited`, src/claude.rs 429
  arm + `parse_retry_after_secs`, src/usage/service.rs `do_one_fetch`
  RateLimited arm.
