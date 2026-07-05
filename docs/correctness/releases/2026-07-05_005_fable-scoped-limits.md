# 2026-07-05 — Parse model-scoped limits (Fable) from the usage API `limits` array

## Summary

The usage API moved per-model quotas out of the flat `seven_day_*` keys
(now returned as `null`) and into a `limits` array; model-scoped entries
carry `kind: "weekly_scoped"` and a `scope.model.display_name` (currently
"Fable"). TokenTrkr was fetching this data and silently dropping it, so
the per-model section of both UIs had gone empty. This change parses the
array, maps model-scoped entries into the existing `model_windows`
rendering path (no UI changes needed), and keeps the flat keys as a
fallback for older response shapes. It also corrects the `seven_day`
window's label from "Opus (7d)" to "Weekly (7d)" — that key is the
all-models weekly, and the old label would read as a contradiction next
to a genuine per-model "Fable (7d)" row.

## Scope

### Files Created

- docs/correctness/releases/2026-07-05_005_fable-scoped-limits.md (this doc)

### Files Modified

- src/models.rs — `UsageApiResponse` gains `limits:
  Option<Vec<LimitResponse>>`; new `LimitResponse` /
  `LimitScopeResponse` / `ModelScopeResponse` deserialization types
  (only the fields we consume; serde ignores the rest — `severity`,
  `is_active`, `group` are available if a future change wants them).
- src/claude.rs — new `build_model_windows()` helper replaces the
  inline legacy-key loop in `fetch_usage`; `seven_day` label fixed;
  four new tests.
- docs/release-ledger.md — new "Feature work" section with this row.

## Behavioral Impact

| Scenario | Before | After |
|---|---|---|
| Current API shape (flat model keys null, `limits` populated) | Per-model section empty in tray menu and popup | "Fable (7d)" row with percent and reset time |
| Older API shape (flat `seven_day_sonnet` etc. populated, no scoped limits) | Rendered from flat keys | Unchanged — flat keys are the fallback |
| Both shapes populated at once | n/a (flat loop only) | Scoped entries win; flat keys ignored (prevents an Opus quota rendering twice) |
| `weekly_scoped` entry with no model name (null model / surface scope) | n/a | Skipped — no renderable label |
| `seven_day` all-models weekly | Labeled "Opus (7d)" (wrong — key is not Opus-specific) | Labeled "Weekly (7d)" |
| `EmptyResponse` detection | Considers `model_windows` | Unchanged — a limits-only response now counts as non-empty, which is correct |

## Test Plan

Four new tests in `src/claude.rs` (57 passing total, both targets):

- `model_scoped_limit_entry_becomes_a_model_window` — deserializes the
  real 2026-07-05 response shape (verbatim structure from a live fetch)
  and asserts one "Fable (7d)" window at 33% with a reset time.
- `scoped_limits_take_precedence_over_legacy_model_keys` — both shapes
  populated → only the scoped entry renders.
- `legacy_model_keys_still_render_when_limits_has_no_scoped_entries` —
  fallback path intact.
- `scoped_limit_without_model_name_is_ignored` — null-model and
  null-scope entries produce nothing.

```
cargo test                        →  57 passed; 0 failed
cargo test --no-default-features  →  57 passed; 0 failed
```

Warning counts unchanged from before this change on both targets
(3 cosmic / 13 SNI).

Live verification: the change was motivated by a live fetch of
`https://api.anthropic.com/api/oauth/usage` on 2026-07-05 showing the
Fable scoped limit at 33%; the primary test fixture mirrors that
response structurally.

## Docs Updated

- docs/release-ledger.md — new row.
- No ARCHITECTURE.md change: no module boundaries, ownership, or
  state-machine behavior moved; this is response-parsing breadth inside
  `claude.rs`/`models.rs`, both already owned by the provider layer.

## Rollback Plan

Single commit — `git revert` restores the flat-key-only parsing (and
the old "Opus (7d)" label).

## Open Questions / Decisions

- **Precedence, not merge:** when `limits` has any model-scoped entry,
  the flat keys are ignored entirely rather than merged, because the
  API populates one shape or the other and merging risks duplicate rows
  for the same quota. If Anthropic ever splits quotas across both
  shapes simultaneously, revisit.
- **`session` / `weekly_all` limit kinds not consumed:** `five_hour`
  and `seven_day` flat keys still populate primary/secondary and are
  still non-null in the live response. Migrating those to the `limits`
  array is deliberately out of scope until the flat keys actually
  disappear.
- **`severity` / `is_active` unread:** available in the parsed types'
  source JSON but not declared as fields; add them when a UI wants
  warning states driven by the server rather than local thresholds.

## References

- Live API response captured 2026-07-05 (structure reproduced in the
  primary test fixture).
- Follows the 2026-06-10 review remediation stack
  (`2026-07-05_001`–`_004`); merged main was `f6c7c4c` at branch time.
