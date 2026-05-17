# 2026-05-17 — Gate 429 retry refresh on cached-vs-fresh token state

## Summary

Fixes a HIGH-risk correctness bug where `ClaudeProvider::fetch_usage`
could call `refresh_token` twice in a single call: once via
`get_valid_credentials` for an expiring token, then again
unconditionally in the 429 branch. If the OAuth server treats refresh
tokens as single-use, the second call consumed the just-rotated RT
and could leave the user in a permanent auth-broken state recoverable
only by `claude login`. The fix introduces a `CredentialsOutcome` enum
so the 429 branch can decide whether a re-refresh is warranted.

## Scope

### Files Modified

- `src/claude.rs` — new `CredentialsOutcome` enum + `was_refreshed()`
  predicate; `get_valid_credentials` renamed to
  `ensure_fresh_credentials`; `fetch_usage` 429 branch gated on the
  outcome.

### Files Created

- `docs/correctness/releases/2026-05-17_001_token-refresh-429-gating.md`
  — this doc

## Behavioral Impact

Behavior changes in exactly one path: **just-refreshed-token + 429**.

| Scenario | Pre-change | Post-change |
|---|---|---|
| Cache hit + 2xx response | Return snapshot | Same |
| Cache hit + 429 | Re-read disk, refresh, retry | Same (1 refresh) |
| Cache hit + non-429 error | Propagate error | Same |
| Expiry refresh + 2xx response | Return snapshot | Same (1 refresh) |
| **Expiry refresh + 429** | **Refresh again, retry (2 refreshes)** | **Propagate `RateLimited` (1 refresh)** |
| Expiry refresh + non-429 error | Propagate error | Same (1 refresh) |

User-visible impact of the changed scenario: instead of the app
attempting a second refresh and (in the worst case) breaking auth
silently, it surfaces `RateLimited` to the polling layer, which on
the SNI path triggers the existing 10/30/60s backoff. The COSMIC
path currently has no backoff (deferred to recommendation A1), so
the user sees the same "rate limited" symptom they would have seen
on the original retry-then-fail path, minus the burnt refresh token.

## Test Plan

Added one focused unit test:

- `credentials_outcome_distinguishes_cached_from_refreshed` — verifies
  the enum's `was_refreshed()` predicate and `creds()` accessor return
  the right thing for each variant.

End-to-end testing of the 429 retry path requires a mockable HTTP
client (not currently injected — see Open Questions). Manual
verification via `cargo test` shows all 23 existing tests still pass.

```
test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured;
0 filtered out; finished in 0.01s
```

## Docs Updated

- `docs/correctness/releases/2026-05-17_001_token-refresh-429-gating.md`
  (created, this file)

README / CLAUDE.md unchanged — no public-API surface change.

## Rollback Plan

Revert the single commit:

```bash
git revert f47860a
```

Or revert the merge commit on `main`: `git revert -m 1 <merge-sha>`.

There is no data migration and no on-disk format change, so revert is
safe at any time. The reverted code path is the prior (buggy)
behavior, which never failed catastrophically — it just risked burning
single-use refresh tokens.

## Open Questions / Decisions

### Decision: Propagate `RateLimited` on just-refreshed + 429

- **Status**: Accepted
- **Date**: 2026-05-17
- **Context**: When `ensure_fresh_credentials` reports `Refreshed` and
  the subsequent API call returns 429, the token cannot be stale —
  only a real rate-limit explains the 429. The choice is between
  (a) re-refreshing anyway and retrying, (b) propagating `RateLimited`
  to the caller's backoff layer, or (c) doing neither and returning
  `Ok` with a "rate-limited" sentinel snapshot.
- **Decision**: Propagate `RateLimited`. The polling layer
  (`polling.rs`) already has 10/30/60s backoff for `RateLimited`.
  Re-refreshing serves no purpose because the token is already as
  fresh as possible; the rate-limit is on the *account*, not the
  token.
- **Consequences**: COSMIC path users will see a brief
  "rate limited" error in the popup until the next 5-minute poll
  interval (no backoff on the COSMIC path — addressed by A1). SNI
  path users get the existing graceful backoff.
- **References**: `src/claude.rs:265-280` (the gating logic);
  `src/polling.rs:79-134` (the backoff loop that consumes the
  propagated error).

### Open: HTTP-layer testability

- **Status**: Open
- **Date**: 2026-05-17
- **Question**: `ClaudeProvider` holds a concrete `reqwest::Client`
  with no injection point. End-to-end tests of `fetch_usage` (429
  retry behavior, token refresh sequencing, error path coverage)
  require either mocking at the HTTP transport level (e.g.,
  `mockito`) or injecting an HTTP-client abstraction. Neither is
  in place today. The unit test added in this change covers the
  enum logic but not the gating logic in `fetch_usage`.
- **Related**: `src/claude.rs:22-34` (constructor); `src/claude.rs:262`
  (the gated branch).
- **Resolution**: TBD. Recommend addressing as part of recommendation
  A1 (orchestration core extraction) when the fetch loop gets its own
  testable seam.

## References

- Architectural analysis report — 2026-05-17 (recommendation A2,
  finding R3 / B1 / C1)
- `Cargo.toml` — no changes
