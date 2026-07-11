# 2026-07-11 — Applet hardening: min-spin snapshot flush, off-thread history writes, themed progress bars

## Summary

Three fixes from the 2026-07-11 `/robot:architectural-analysis` run focused
on `src/cosmic_app.rs` (recommendations A1–A3, risks R1–R3). The headline
fix (A1, High risk): `FetchStarted` unconditionally cleared
`pending_snapshot`, so a manual refresh clicked during the ~3s minimum-spin
animation window permanently and silently dropped a successfully fetched
reading from `history.json` — `apply_usage_result` is the sole
`record()`/persist site and the parked result never reached it. A2 (Medium)
moves the fsync-bearing history write off the winit main thread, and A3
(Medium) makes the popup progress-bar track theme-aware (the same
invisible-on-light-theme failure the 2026-07-05 chart fix addressed).

## Scope

### Files Created

- docs/correctness/releases/2026-07-11_001_applet-hardening.md (this doc)

### Files Modified

- src/cosmic_app.rs
  - `FetchStarted` arm: `self.pending_snapshot.take()` is applied via
    `apply_usage_result` **before** the arm resets spin state (A1). The
    min-spin gate itself is unchanged; `apply_usage_result` remains the
    only record/persist site.
  - `apply_usage_result`: `history.save()` replaced by
    `serialize_pruned()` on the main thread (write ordering follows
    `record()` ordering) + `tokio::task::spawn_blocking(write_bytes)`;
    falls back to a synchronous write if no runtime context exists (A2).
  - `progress_bar_bg`: reads the `&Theme` it previously discarded —
    white 8%-alpha track on dark themes, black 8%-alpha on light. Fill
    colors (bucket colors) unchanged (A3).
  - First unit tests for `handle_event` (4 new tests, see Test Plan).
- src/history.rs
  - `UsageHistory` gains a `#[serde(skip)] path: Option<PathBuf>` field,
    captured once in `load()`. `Default`-constructed histories have no
    path, making persistence a no-op — this is what keeps the new
    `handle_event` tests off the real history file.
  - `save()` replaced by `serialize_pruned() -> Option<(PathBuf, Vec<u8>)>`
    and `write_bytes(&Path, &[u8])`.
  - `atomic_write` uses a per-call tmp filename (`AtomicU64` counter).
    With writes now on concurrent blocking threads, two writes sharing
    one `history.json.tmp` could truncate each other mid-write and leave
    a corrupt file, which `load()` answers by wiping history. Unique tmp
    names reduce the worst case to "last rename wins" — always a
    complete, valid JSON, at most one stale point until the next fetch.
- docs/release-ledger.md — new work-stream section.

## Behavioral Impact

| Scenario | Before | After |
|---|---|---|
| Manual refresh during min-spin window | Parked snapshot silently dropped; reading permanently missing from history.json | Parked result applied (recorded + persisted) before the new fetch's state reset |
| Parked error + new fetch starts | Error silently dropped | Error surfaces via `apply_usage_result` (then cleared by the next successful snapshot) |
| Successful fetch → history persist | Synchronous `atomic_write` + fsync on the winit main thread (UI freeze = fsync latency) | Serialize on main thread; write + fsync on a tokio blocking thread |
| History persist ordering | Strictly ordered (single-threaded) | Serialization still ordered with `record()`; concurrent writes cannot corrupt (unique tmp names), last rename wins |
| Progress bars on light COSMIC theme | White 8%-alpha track — near-invisible | Black 8%-alpha track |
| Progress bars on dark theme | White 8%-alpha track | Unchanged |
| SNI tray | No history use | Unchanged (compiles clean, no new warnings) |

## Test Plan

Four new `handle_event` tests in `src/cosmic_app.rs` (the file's first —
the module previously tested only chart helpers):

- `fetch_started_during_min_spin_window_flushes_the_parked_snapshot_into_history`
  — the headline regression test for A1: parks a snapshot behind the
  min-spin gate, delivers a second `FetchStarted`, asserts the reading
  landed in `history.data_points`.
- `fetch_started_during_min_spin_window_flushes_a_parked_error`
- `snapshot_after_min_spin_elapsed_applies_directly_and_stops_refreshing`
- `a_snapshot_from_a_superseded_fetch_is_ignored` (fetch_id staleness)

History tests updated/added in `src/history.rs`:

- `serialize_pruned_then_write_bytes_round_trips_through_the_injected_path`
  — replaces the old serde-only round-trip; the injected path removes the
  "can't redirect history_path() without a deeper refactor" limitation.
- `serialize_pruned_without_a_backing_path_returns_none`
- `atomic_write_leaves_no_tmp_artifact_on_success` — reworked to assert
  no leftover files at all (tmp names are no longer fixed).

```
cargo test                        →  69 passed; 0 failed
cargo test --no-default-features  →  58 passed; 0 failed
```

Release builds: `cargo check --no-default-features` clean (13
pre-existing SNI dead-code warnings, unchanged set), then
`cargo build --release` (COSMIC last) succeeds — 3 pre-existing warnings,
binary ~28 MB.

**Visual verification: PENDING (Peter).** A3 is build/test-verified only.
Per standing project rule, the light-theme progress-bar track needs a
popup screenshot on a light COSMIC theme (and a dark-theme sanity check)
before it is considered visually confirmed.

## Docs Updated

- docs/release-ledger.md — new section with three rows (A1/A2/A3).
- docs/ARCHITECTURE.md — updated via `/robot:refresh-architecture` in the
  same push (persistence ownership shift + `FetchStarted` state-machine
  change meet the refresh trigger in CLAUDE.md).

## Rollback Plan

Single code commit `9ecd0fb` on `fix/applet-hardening`.
`git revert 9ecd0fb` restores: synchronous `save()` on the UI thread,
fixed tmp name, unconditional `pending_snapshot` clear (reintroduces the
data-loss bug), and the dark-only progress track.

## Open Questions / Decisions

- **Flush is deliberately not fetch_id-guarded (A1):** the parked result
  belongs to the previous fetch by construction; guarding it against the
  *new* fetch's id would re-drop it. Architect guidance followed.
- **`record()` stays synchronous (A2):** only the disk edge moved.
  Serialization happens on the main thread precisely so byte payloads are
  produced in `record()` order; the unique-tmp scheme makes out-of-order
  *renames* harmless rather than impossible (worst case: one stale point
  until the next fetch rewrites the file).
- **Sync fallback when no runtime context (A2):** `Handle::try_current()`
  failing inside `update()` is not expected (init spawns tokio tasks from
  the same context), but the fallback preserves the old blocking behavior
  instead of panicking if a libcosmic bump changes executor semantics.
- **Config save still synchronous:** `CycleTrayMode`'s `config.save()` is
  a rare, tiny, non-fsync write; left on the UI thread (out of A2 scope).
- **Fill colors stay theme-constant (A3):** bucket colors read acceptably
  on both grounds (same call as the chart's series-color decision).

## References

- Analysis: 2026-07-11 `/robot:architectural-analysis` (focus
  `src/cosmic_app.rs`), findings B1/C1/B5 → recommendations A1/A2/A3,
  risks R1 (High) / R2 / R3 (Medium).
- Prior art for A3: docs/correctness/releases/2026-07-05_006_chart-smoothing.md
  (chart chrome theming; the "rest of popup dark-only" open item is now
  partially closed — progress bars done, other chrome unaudited).
- Branched from main at `5f0b963`.
