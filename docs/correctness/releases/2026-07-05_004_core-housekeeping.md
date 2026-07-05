# 2026-07-05 — Core housekeeping: timeout authority, dead-code deletions, allow scope (R9, R10, R11)

## Summary

Three hygiene items from the 2026-06-10 core review, one commit each.
The load-bearing one is R9: two independent timeout values governed one
fetch attempt invisibly (reqwest 30s vs a dormant, unreachable 120s
outer ceiling), and a future edit raising the reqwest side would have
silently resurrected the 120s value. Peter chose **Option A** (keep one
documented backstop) over Option B (delete the outer timeout):
`fetch_timeout` is now 45s, explicitly documented as a backstop just
above the reqwest ceiling. R10 deletes the confirmed-dead `with_retry`
builder and `RetryPolicy` re-export; R11 narrows a blanket
`#[allow(dead_code)]` from the whole `Provider` trait to the one
actually-dead method.

## Scope

### Files Created

- docs/correctness/releases/2026-07-05_004_core-housekeeping.md (this doc)

### Files Modified

- src/usage/retry.rs — `fetch_timeout` 120s → 45s with a layering
  comment (reqwest primary, this a backstop for providers with no
  internal timeout). Test renamed to
  `default_fetch_timeout_is_a_backstop_above_the_reqwest_ceiling` and
  now also asserts the `> 30s` margin.
- src/usage/service.rs — `with_retry` deleted (zero callers incl. tests).
- src/usage/mod.rs — `pub use retry::RetryPolicy;` deleted (served only
  external `with_retry` callers, of which there were none).
- src/provider.rs — `#[allow(dead_code)]` moved from the trait onto
  `fn name` only; verified it composes with `async_trait` on both
  targets.
- docs/ARCHITECTURE.md — new Intentional decision "Single timeout
  authority" (records Option A and the ordering invariant); A4
  Known-deferred amended (deletions taken, field demotion still
  deferred, trigger updated).
- docs/release-ledger.md — R9/R10/R11 marked Done.

## Behavioral Impact

| Scenario | Before | After |
|---|---|---|
| Production fetch timeout | reqwest 30s fires; outer 120s unreachable | Identical — reqwest 30s still fires first; outer backstop at 45s remains unreachable in production |
| Hung provider with no internal timeout (MockProvider, future impl) | Bounded at 120s | Bounded at 45s |
| Future edit raising reqwest past the outer value | 120s ceiling silently took over at 120s | Margin assertion + documented invariant make the inversion a conscious choice |
| `with_retry` / re-export | Two dead-code warnings every build | Gone (build warnings 5 → 3 on the cosmic target) |
| A dead method added to `Provider` later | Compiler silent (blanket allow) | Compiler warns |

No runtime behavior change on any production path.

## Test Plan

No new tests; one renamed/strengthened (53 passing total, unchanged):

- `default_fetch_timeout_is_a_backstop_above_the_reqwest_ceiling` —
  asserts 45s and the `> 30s` ordering margin, with a comment pointing
  at the ARCHITECTURE.md decision if the assertion ever forces a
  choice.

```
cargo test                        →  53 passed; 0 failed
cargo test --no-default-features  →  53 passed; 0 failed
```

Acceptance: zero warnings from the focus-area files
(`src/usage/`, `src/claude.rs`, `src/provider.rs`) except the tracked
`retrying_in` at `src/usage/event.rs:17` (Known-deferred, untouched).
Remaining repo warnings (`poll_interval`, `format_summary`) are outside
the focus area and pre-date this work.

## Docs Updated

- docs/ARCHITECTURE.md — Intentional decision added + A4 amended (see
  Scope). Verified-against SHA not bumped; `/robot:refresh-architecture`
  was not available in the implementing session — the spec recommends
  running it for the timeout-authority decision when available.
- docs/release-ledger.md — updated.

## Rollback Plan

Each item is its own commit — revert independently:

```bash
git revert <R9 commit>    # chore(usage): lower fetch_timeout to 45s
git revert <R10 commit>   # chore(usage): delete dead with_retry + re-export
git revert <R11 commit>   # chore(provider): narrow dead_code allow
```

## Open Questions / Decisions

### Decision: Option A — keep `fetch_timeout` as a 45s documented backstop

- Status: Accepted
- Date: 2026-07-05
- Context: R9 offered two resolutions — A: lower to 45s and document
  the layering; B: delete the outer timeout, reqwest sole authority.
  Architect leaned A (MockProvider has no internal timeout; the
  backstop bounds hung mocks in tests and guards future Provider
  impls). The spec required a user pick.
- Decision: Peter chose A (2026-07-05).
- Consequences: One outer timeout survives, documented as
  intentionally unreachable in production; the ordering invariant
  (`fetch_timeout > reqwest timeout`) is asserted in a test and
  recorded as an Intentional decision so R9 is never re-flagged.
- References: docs/ARCHITECTURE.md "Single timeout authority";
  src/usage/retry.rs.

## References

- Spec: `homelab2-docs/specs/tokentrkr/2026-06-10-core-housekeeping.md`
  (R9 ← S5, R10 ← S1/A4-subset, R11 ← S4).
- Coordination note from the spec: the retry-after packet
  (`2026-07-05_002`) landed first in the same stack; the provider.rs
  edits composed without conflict as predicted.
