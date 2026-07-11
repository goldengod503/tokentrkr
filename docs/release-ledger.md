# Release Ledger

Single source of truth for work-stream status. Status values:
`Not Started` · `In Progress` · `Blocked` · `Done` · `Skipped`.
Every `Done` links to its release doc; details live there, not here.

Release docs dated before 2026-07-05 predate this ledger — see
`docs/correctness/releases/` and `docs/portability/releases/` directly.

## 2026-06-10 core architecture review remediation

Specs: `homelab2-docs/specs/tokentrkr/2026-06-10-core-review-INDEX.md`

| Item | Risk | Status | Release doc / notes |
|---|---|---|---|
| Credentials TOCTOU guard (R1 + B2) | High | Done | [2026-07-05_001](correctness/releases/2026-07-05_001_credentials-toctou-guard.md) |
| Retry-after propagation (R3) | Medium | Done | [2026-07-05_002](correctness/releases/2026-07-05_002_retry-after-propagation.md) |
| State & UI completeness (R5/R6/R8) | Low | Done | [2026-07-05_003](correctness/releases/2026-07-05_003_state-ui-completeness.md) |
| Core housekeeping (R9/R10/R11) | Low | Done | [2026-07-05_004](correctness/releases/2026-07-05_004_core-housekeeping.md) — Option A per Peter |

## 2026-07-11 architectural-analysis remediation (cosmic_app focus)

| Item | Risk | Status | Release doc / notes |
|---|---|---|---|
| A1: min-spin parked-snapshot flush (B1/R1) | High | Done | [2026-07-11_001](correctness/releases/2026-07-11_001_applet-hardening.md) |
| A2: history fsync off the UI thread (C1/R2) | Medium | Done | [2026-07-11_001](correctness/releases/2026-07-11_001_applet-hardening.md) |
| A3: theme-aware progress-bar track (B5/R3) | Medium | Done | [2026-07-11_001](correctness/releases/2026-07-11_001_applet-hardening.md) — visual verify pending Peter |
| A4–A6: clamp helper, warn! logs, doc sync | Low | Not Started | Deferred; A6 doc items partially covered by refresh-architecture pass |

## Feature work

| Item | Risk | Status | Release doc / notes |
|---|---|---|---|
| Model-scoped limits (Fable) from `limits` array | Low | Done | [2026-07-05_005](correctness/releases/2026-07-05_005_fable-scoped-limits.md) |
| Smooth & theme-aware usage chart (Option C) | Low | Done | [2026-07-05_006](correctness/releases/2026-07-05_006_chart-smoothing.md) — visual verify pending Peter |
