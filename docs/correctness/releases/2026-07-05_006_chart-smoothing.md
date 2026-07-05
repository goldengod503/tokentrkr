# 2026-07-05 — Smooth & theme-aware usage chart (Option C)

## Summary

The usage-history chart in the COSMIC applet popup drew its two series
(Session = blue `#3C88FC`, Weekly = amber `#F59E0B`) as straight
`<polyline>` segments between raw samples, and hardcoded all chrome
colors as white-alpha. Two problems: sampling is event-driven (one point
per usage poll), so idle-then-jump stretches rendered as visible kinks;
and on a light COSMIC theme the grid/labels were nearly invisible.

This change (spec `homelab2-docs/specs/tokentrkr/2026-07-05-chart-smoothing-design.md`,
"Option C" from a rendered four-option comparison) replaces the polylines
with **monotone-cubic smoothed paths**, adds **soft gradient area fills**
under each series, marks the current value with an **endpoint dot**, and
makes the chrome **theme-aware** (light/dark). It is a rendering-only
change confined to `build_chart_svg`; no data model, sampling, retention,
dependency, or SNI-tray changes. The chart exists only in the COSMIC
popup — `tray.rs`/`icon.rs` have no chart.

The smoothing uses **Fritsch–Carlson monotone cubic** interpolation
specifically because it is guaranteed never to overshoot the input
samples — the curve cannot fabricate a peak or dip below 0% / above 100%
between two readings, which matters for an honest quota gauge.

## Scope

### Files Created

- docs/correctness/releases/2026-07-05_006_chart-smoothing.md (this doc)

### Files Modified

- src/cosmic_app.rs
  - New pure helper `smooth_path(pts: &[(f64, f64)]) -> String` —
    Fritsch–Carlson monotone cubic → SVG path `d` string (empty slice →
    `""`, single point → `"M x y"`).
  - `build_chart_svg` signature gains `is_dark: bool`; grid / tick /
    label / empty-state / endpoint-ring colors switch on it, series
    colors stay theme-constant.
  - Series drawing rewritten: two `<polyline>` → two smoothed `<path>`
    plus two gradient area fills (`<defs>` linear gradients, weekly
    behind session) plus two endpoint `<circle>` dots (Session drawn
    last/on top).
  - Tick loop's local `label` renamed `tick_label` to stop it shadowing
    the new theme-color `label`.
  - Call site computes `is_dark = cosmic::theme::active().cosmic().is_dark`.
- docs/release-ledger.md — new "Feature work" row.

## Behavioral Impact

| Scenario | Before | After |
|---|---|---|
| Series lines | Straight segments between samples (visible kinks) | Monotone-cubic smoothed curves |
| Curve vs. data | Faithful (linear) | Faithful — monotone spline never overshoots sample range |
| Series separation | Two thin lines, hard to tell apart where they cross | Soft gradient fill under each + endpoint dot per series |
| Current value | Implicit (end of line) | Explicit ringed dot on the latest sample |
| Light COSMIC theme | Grid/labels white-alpha → near-invisible | Slate-alpha chrome, legible |
| Dark COSMIC theme | Correct | Unchanged (same white-alpha values) |
| <2 data points | "No history data yet" (white-alpha) | Same text, theme-correct color |
| SNI tray | No chart | No chart (untouched) |

## Test Plan

Eight new tests in `src/cosmic_app.rs` (64 passing total, both targets):

- `smoothing_a_spiky_series_never_overshoots_input_range` — the headline
  correctness property: feeds an adversarial alternating series and
  asserts every emitted control-point Y stays within the input's
  `[min, max]` (a path-parsing helper extracts the actual control-point
  Ys, so the assertion constrains the curve, not just the anchors).
- `smoothing_multiple_points_emits_cubic_path_with_no_nan`
- `smoothing_a_single_point_emits_only_a_moveto`
- `dark_and_light_themes_produce_different_grid_colors`
- `too_few_points_renders_empty_state_with_theme_label_color`
- `a_populated_chart_draws_smoothed_paths_fills_and_endpoint_dots`
  (asserts `<path>` + `C` segments, `linearGradient`, exactly two
  `<circle>` dots, and no residual `<polyline>`)
- `endpoint_dot_ring_follows_the_theme_surface`

```
cargo test                        →  64 passed; 0 failed
cargo test --no-default-features  →  57 passed; 0 failed
```

Release build: `cargo build --release --no-default-features` then
`cargo build --release` (COSMIC last) both succeed; the applet binary is
~28 MB. No new warnings — the `smooth_path` dead-code warning present
mid-branch is resolved once the function has a production caller (cosmic
build: 3 pre-existing warnings, unchanged).

**Visual verification: PENDING (Peter).** Per standing project rule,
build + `cargo test` is not sufficient to claim the chart matches intent
— screenshots of the popup on a dark AND a light COSMIC theme are
required before this is considered visually confirmed. Not yet performed.

## Docs Updated

- docs/release-ledger.md — new row.
- No ARCHITECTURE.md change: no module boundary, ownership, or
  state-machine behavior moved — this is rendering breadth inside the
  existing `build_chart_svg` in `cosmic_app.rs`.

## Rollback Plan

Three commits on `feat/chart-smoothing`
(`43e9ccb` helper, `03c81df` theme-aware chrome, `59dc3d8` smoothed
lines/fills/dots). `git revert` of the three, or dropping the branch,
restores the straight-polyline dark-only chart.

## Open Questions / Decisions

- **Monotone, not Catmull-Rom:** chose Fritsch–Carlson specifically for
  the no-overshoot guarantee. A plain Catmull-Rom/cardinal spline is
  visually smoother but can dip below 0% or above 100% between samples,
  which would misrepresent a quota.
- **Series colors stay constant across themes:** blue/amber read
  acceptably on both light and dark grounds, so only the chrome
  (grid/tick/label/ring) switches. If amber-on-white proves too light in
  practice, darken only the light-theme amber later.
- **Rest of the popup is still dark-only:** the progress bars and other
  popup chrome hardcode white-alpha independently of theme. Only the
  chart was made theme-aware here; a broader popup theming pass is out of
  scope.
- **Edge-case tests deferred (Minor):** `smooth_path`'s empty-slice and
  zero-dx branches are implemented and correct but not unit-tested; no
  test asserts the tick-`<text>` fill (the value guarded by the
  `tick_label` rename). Logged for a future hardening pass.

## References

- Spec: `homelab2-docs/specs/tokentrkr/2026-07-05-chart-smoothing-design.md`
- Plan: `homelab2-docs/plans/tokentrkr/2026-07-05-chart-smoothing.md`
- libcosmic theme API used: `cosmic::theme::active().cosmic().is_dark`
  (pinned rev `1729153`). Branched from main at `137a2e1`.
