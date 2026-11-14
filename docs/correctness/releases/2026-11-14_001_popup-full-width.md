# 2026-11-14 — Popup chart and range buttons fill the popup width

## Summary

In the COSMIC applet popup, the usage chart and the 1h/6h/1d/7d/30d range
buttons stopped well short of the popup's right edge, while the percentage
labels above them were right-aligned to it. Both now span the full content
width. Layout-only change in `src/cosmic_app.rs`; no data, polling, or
SNI-tray change.

The cause was two hardcoded sizes that predate the popup's current width.
The popup's size limits are 280–360px, and because its rows contain
`Length::Fill` children it always lays out at the 360px maximum — 336px of
content after the column's 12px padding. The chart was a fixed 280px SVG
and the range buttons were shrink-to-fit.

## Scope

### Files Created

- docs/correctness/releases/2026-11-14_001_popup-full-width.md (this doc)
- assets/TokenTrkr_Popup.png — new README screenshot

### Files Modified

- src/cosmic_app.rs
  - New constants `POPUP_MAX_WIDTH` (360), `POPUP_PADDING` (12) and
    `CHART_WIDTH` (derived: 336). The popup size limit and the column
    padding now read from them instead of repeating the literals.
  - `build_chart_svg` draws at `CHART_WIDTH` instead of 280; the `Svg`
    widget is `Length::Fill` wide (height stays fixed at 130).
  - Range picker: row is `Length::Fill`; each button is
    `button::custom` with a centered label, `Length::Fill` wide, at the
    text-button height (`space_l`) and text size (14), with the same
    `Standard` / `Suggested` classes as before.
- docs/release-ledger.md — new row.
- README.md — the two applet screenshots replaced by one current capture.

### Files Deleted

- assets/TokenTrkr_SessionWeeklyView.png
- assets/TokenTrkr_FrostedGlassPopup.png

## Behavioral Impact

| Element | Before | After |
|---|---|---|
| Usage chart | 280px wide, left-aligned, 56px gap on the right | Full content width (336px) |
| Range buttons | Shrink-to-fit, left-aligned | Five equal-width buttons spanning the row, labels centered |
| Chart plot area | 242px between axis and right edge | 298px — same data, wider x scale |
| Progress bars | Fixed 240px | Unchanged (not part of this change) |
| Refresh / Dashboard row | Spans the width, mode icon pinned right | Unchanged |
| SNI tray | No popup | Untouched |

Why `button::custom`: libcosmic's text-button builder puts its label in a
row with no horizontal alignment, so a stretched `button::standard`
left-aligns its text. A custom button with a centered container is the
smallest way to get a centered label at fill width.

## Test Plan

No tests added — this is widget layout, which the unit tests cannot
observe. Existing suites unchanged:

```
cargo test                        →  69 passed; 0 failed
cargo test --no-default-features  →  58 passed; 0 failed
```

**Visual verification: confirmed by Peter 2026-11-14** on the installed
release build (`just restart`), dark theme, 1d range — the capture now in
the README (`assets/TokenTrkr_Popup.png`). Light theme not separately
checked; the change touches no colors.

## Docs Updated

- docs/release-ledger.md — new row.
- README.md — screenshot.
- No ARCHITECTURE.md change: no module boundary, ownership, or
  state-machine behavior moved.

## Rollback Plan

`git revert 33b9221` restores the 280px chart and shrink-to-fit buttons.
The screenshot swap is a separate commit on the same branch
(`fix/popup-full-width`) and can stay or be reverted independently.

## Open Questions / Decisions

- **Progress bars left at 240px.** They are now the only popup element
  that stops short of the right edge. Not changed here because the
  request was the chart and buttons; raised with Peter as a follow-up.
- **Chart width is a constant, not measured.** `CHART_WIDTH` assumes the
  popup lays out at its max width, which holds as long as any row has a
  `Length::Fill` child. If the popup ever shrinks, the SVG scales down
  (aspect-preserving) rather than clipping.
