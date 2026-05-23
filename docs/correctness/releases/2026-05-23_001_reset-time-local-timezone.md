# 2026-05-23 — Render absolute reset time in local timezone

## Summary

`RateWindow::format_reset_time` was formatting the parsed
`DateTime<Utc>` directly, so weekly reset windows rendered with UTC
month/day/hour components — visibly mismatching the Claude web UI,
which displays the reset in the user's local timezone. The function
now converts to `chrono::Local` before formatting. User-visible impact:
the absolute date/time string shown for the weekly bar (only used when
the reset is more than 24h away) now matches the web UI for any
non-UTC user.

## Scope

### Files Created

- docs/correctness/releases/2026-05-23_001_reset-time-local-timezone.md (this doc)

### Files Modified

- src/models.rs — splits `format_reset_time(&self)` into a thin public
  wrapper that calls a new private `format_reset_time_in<Tz>(&self,
  now: DateTime<Utc>, tz: &Tz)`. The wrapper passes `Utc::now()` and
  `chrono::Local`. The `> 24h` branch now does
  `reset.with_timezone(tz).format(...)` instead of formatting the UTC
  value directly. The `Resets in Xh Ym` / `Resets in Xm` / `Resetting
  soon...` branches are unchanged — they are duration-based and
  timezone-independent.

## Behavioral Impact

| Scenario | Before | After |
|---|---|---|
| Weekly reset > 24h away, user in non-UTC timezone | "Resets" string used UTC components (e.g. UTC-4 user saw "Resets May 30, 2:00 AM" for a 02:00 UTC reset) | "Resets" string uses local components (same example renders "Resets May 29, 10:00 PM"), matching the Claude web UI |
| Weekly reset ≤ 24h away | "Resets in Xh Ym" / "Resets in Xm" | Unchanged — pure duration, never depended on timezone |
| Reset ≤ now | "Resetting soon..." | Unchanged |
| `resets_at` absent | Falls back to `reset_description` | Unchanged |
| User in UTC | No visible change (UTC == local) | No visible change |

## Test Plan

One new unit test added in `src/models.rs::tests`:

- `absolute_reset_time_is_rendered_in_supplied_timezone_not_utc` —
  constructs a `RateWindow` with `resets_at = 2026-05-30 02:00 UTC`,
  `now = 2026-05-23 00:00 UTC` (so the `> 24h` branch is taken), and
  passes a `FixedOffset::west(4h)` (US Eastern) as the timezone. The
  test injects an explicit timezone via the new
  `format_reset_time_in` helper so the assertion does not depend on
  the test runner's local timezone (CI runners are typically UTC,
  which would render the assertion vacuous on the old code path).
  Asserts the rendered string is exactly `"Resets May 29, 10:00 PM"` —
  the calendar boundary crossed by the TZ shift makes a UTC-vs-local
  bug visible in the assertion message.

Verification:

```
cargo test                                   →  42 passed; 0 failed (was 41)
cargo build --release                        →  cosmic feature compiles
cargo build --release --no-default-features  →  SNI feature compiles
```

## Docs Updated

- docs/correctness/releases/2026-05-23_001_reset-time-local-timezone.md (this doc)
- docs/ARCHITECTURE.md — not updated. This is a behavioral fix
  contained inside an existing helper; module boundaries, ownership,
  retry/state-machine shape, and trigger surface are unchanged. No
  structural drift to record.
- CLAUDE.md — verified-against SHA not bumped. Same reasoning as
  above; the architecture doc still describes the current shape
  faithfully.

## Rollback Plan

```bash
git revert d5980ca
```

Single-commit change, no migration / config / format surface to undo.

## Open Questions / Decisions

### Decision: Inject timezone via a private helper rather than calling `chrono::Local` inline

- Status: Accepted
- Date: 2026-05-23
- Context: The bug is one line — replace `reset.format(...)` with
  `reset.with_timezone(&chrono::Local).format(...)`. A regression test
  that calls the public `format_reset_time()` would assert against
  the runner's local timezone, which is non-deterministic across dev
  machines and CI (CI is usually UTC, where the test would silently
  pass even on the broken code).
- Decision: Extract a private `format_reset_time_in<Tz>(&self, now:
  DateTime<Utc>, tz: &Tz)` that takes both `now` and the timezone as
  parameters. Public `format_reset_time(&self)` delegates with
  `Utc::now()` and `chrono::Local`. Test calls the private helper
  with a `FixedOffset` so the assertion is deterministic.
- Consequences: One small private helper added; public API surface
  unchanged. Future tests for the duration branches can use the same
  helper for deterministic `now` injection.
- References: src/models.rs `format_reset_time` and
  `format_reset_time_in`.

## References

- Bug report: user feedback that the app's weekly reset time did not
  match the Claude web UI.
- Source citations: src/models.rs `format_reset_time_in` (the `> 24h`
  branch), src/models.rs `tests::absolute_reset_time_is_rendered_in_supplied_timezone_not_utc`.
