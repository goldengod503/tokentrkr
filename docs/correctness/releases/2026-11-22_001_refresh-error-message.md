# 2026-11-22 — Readable "signed out" error when token refresh is rejected

## Summary

When the OAuth token-refresh endpoint rejected a request, the popup showed
the raw response verbatim — e.g.
`Error: Token refresh failed (400 Bad Request): {"type":"error","error":{"type":"invalid_request_error","message":"Invalid request format"},"request_id":"req_…"}`
— wrapped across most of the popup. Running `/login` in Claude Code fixes
it, but nothing on screen said so.

A 4xx (other than 429) from the refresh endpoint now surfaces as the
existing `Unauthorized` sentinel, whose message is reworded to say what to
do. The raw body goes to the log at `warn`.

## Scope

### Files Created

- docs/correctness/releases/2026-11-22_001_refresh-error-message.md (this doc)

### Files Modified

- src/claude.rs
  - New `refresh_failure(status, body)` maps a refresh failure to an error:
    429 → `RateLimited { retry_after: None }`; other 4xx → `Unauthorized`;
    5xx/other → the previous `Token refresh failed (…): …` message.
    4xx bodies are logged with `warn!`.
  - `refresh_token` uses it instead of `bail!` with the body.
  - Tests: 400 → `Unauthorized` with no JSON in the message; 429 →
    `RateLimited`; 502 → neither sentinel. Existing sentinel-message test
    updated.
- src/provider.rs — `Unauthorized` message and doc comment.
- docs/release-ledger.md — new row.

## Behavioral Impact

| Case | Before | After |
|---|---|---|
| Refresh 400/401/403 | `Error: Token refresh failed (400 Bad Request): {…json…}`; Transient → retried every poll interval | `Error: Signed out of Claude — run /login in Claude Code, then click Refresh`; Permanent → Dormant (15-min retry) |
| Refresh 429 | Same raw-JSON message, Transient | `Rate limited (429)`, goes through the 429 retry ladder |
| Refresh 5xx | Raw message, Transient | Unchanged |
| Usage API 401 | `Authentication failed — re-login in Claude Code`, Permanent | New wording, Permanent (unchanged classification) |

Dormant after a rejected refresh is deliberate: the refresh token is dead
until the user logs in again, so retrying every poll interval only hammers
the endpoint. Clicking Refresh after `/login` re-reads the credentials file
and recovers immediately.

## Test Plan

```
cargo test                        →  72 passed; 0 failed
cargo test --no-default-features  →  61 passed; 0 failed
```

**Visual verification: pending Peter.** The wording in the popup has not
been checked on screen; this can't easily be reproduced on purpose, because
it needs the refresh endpoint to reject a request.

## Docs Updated

- docs/release-ledger.md — new row.
- No ARCHITECTURE.md change: no module boundary or state-machine change.
  The existing Unauthorized → Permanent path now has one more source.

## Rollback Plan

`git revert` the fix commit on `fix/refresh-error-message`.

## Open Questions / Decisions

- **Root cause of the 400 is unconfirmed.** It cleared after `/login`, and
  the form-encoded refresh has otherwise worked, so it is treated as a
  one-off sign-in problem. Claude Code 2.1.330 refreshes with a JSON body
  (`Content-Type: application/json`, plus `scope`) while TokenTrkr sends
  form-encoded; if this message starts appearing on every token expiry,
  switch the request format.
- Partially addresses SEC-002 in `.code-review/REVIEW.md` (raw bodies in
  user-facing strings) for 4xx refresh failures only; 5xx and usage-API
  non-401 errors still include the body.
