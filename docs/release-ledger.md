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
| Retry-after propagation (R3) | Medium | Not Started | spec `2026-06-10-retry-after-propagation.md` |
| State & UI completeness (R5/R6/R8) | Low | Not Started | spec `2026-06-10-state-ui-completeness.md` |
| Core housekeeping (R9/R10/R11) | Low | Blocked | spec `2026-06-10-core-housekeeping.md` — awaiting user pick on timeout Option A/B (defaults to A) |
