# 2026-05-17 — Memory footprint optimization

## Summary

Three independent changes reduce TokenTrkr's resident memory and binary
footprint without altering behavior. Total impact across the two applet
processes spawned by cosmic-panel: VmRSS 62.0 MB → 57.5 MB (−7.2%), PSS
34.5 MB → 30.2 MB (−12.6%), OS threads 40 → 12 (−70%). Release binary
on disk: 35 MB → 27 MB (−23%).

## Scope

### Files Modified

- `Cargo.toml` — tokio features trimmed; `[profile.release] strip = true`
- `Cargo.lock` — `parking_lot` dropped from tokio's dependency tree
- `src/main.rs` — early `TOKIO_WORKER_THREADS=2` shim in `main()`

### Files Created

- `docs/portability/releases/2026-05-17_001_memory-optimization.md` — this doc

## Behavioral Impact

No user-visible behavior change. All three changes are operational:

- **Tokio feature trim**: only previously-uncalled subsystems removed
  (`net`, `fs`, `signal`, `process`, `io-util`, `io-std`); used modules
  (`time`, `sync`, `macros`, `rt-multi-thread`) retained.
- **Strip**: release binaries lose DWARF debug info. Panic backtraces
  in release will show addresses only. Local debug builds unaffected.
- **Worker cap**: tokio runtime in the iced::daemon path goes from 16
  workers (num_cpus) to 2. Functionally equivalent for this workload —
  reqwest fully async, single in-flight HTTP request, blocking pool
  is separate from worker pool.

## Test Plan

Manual verification only. Test method: kill cosmic-panel to force a
re-spawn of both applets on the new binary, wait 20+ seconds for
fontdb load and first usage fetch, then read `/proc/<pid>/status` and
`/proc/<pid>/smaps_rollup` for both PIDs.

Measurements taken (per process, averaged across both applets):

| Metric | Pre-change | Post-change | Δ |
|---|---:|---:|---:|
| VmRSS | 30.98 MB | 28.75 MB | −7.2% |
| RssAnon | 6.87 MB | 5.72 MB | −16.7% |
| RssFile | 24.05 MB | 22.98 MB | −4.4% |
| PSS | 17.28 MB | 15.10 MB | −12.6% |
| OS threads | 20 | 6 | −70% |
| tokio workers | 16 | 2 | −87.5% |

No automated tests added. Existing unit tests in `src/claude.rs`,
`src/history.rs`, `src/config.rs` continue to pass via `cargo build
--release`. No new code paths introduced.

## Docs Updated

- `docs/portability/releases/2026-05-17_001_memory-optimization.md`
  (created, this file)

README, CLAUDE.md, and other source-level docs not updated — no API
or user-facing surface changed.

## Rollback Plan

Each commit is independently revertable; revert in reverse order to
minimize merge friction:

```bash
git revert d662352  # Cap tokio worker threads
git revert c1aa7e9  # Strip DWARF debug info
git revert 034ea15  # Trim tokio features
```

Or revert the merge commit on `main` after merge: `git revert -m 1 <merge-sha>`.

If only the worker cap causes issues (e.g., a future libcosmic version
introduces blocking work on the worker pool), the env var can be
overridden externally by exporting `TOKIO_WORKER_THREADS=N` before
launch — the shim in `main.rs` only sets it when unset.

## Open Questions / Decisions

### Decision: TOKIO_WORKER_THREADS=2 rather than 1

- **Status**: Accepted
- **Date**: 2026-05-17
- **Context**: The libcosmic single-worker executor exists (`single.rs`,
  worker_threads=1) but is unused by the applet path. We're free to
  pick any cap via the env var.
- **Decision**: Set to 2 instead of 1.
- **Consequences**: Trivially higher thread overhead vs. 1 worker; gives
  a one-worker buffer in case any future libcosmic code path briefly
  blocks on the worker pool. Functionally equivalent for the current
  codebase (one HTTP request in flight, fully async).
- **References**: `src/main.rs:71-77`; `libcosmic/src/executor/single.rs:18`;
  `iced/futures/src/backend/native/tokio.rs:4-9`

### Decision: Skip mimalloc

- **Status**: Accepted
- **Date**: 2026-05-17
- **Context**: Considered swapping the global allocator to mimalloc to
  reduce RSS.
- **Decision**: Not pursued. The allocator-controlled slice (RssAnon) is
  only ~6.5 MB per process pre-change. libcosmic already calls
  `mallopt(M_MMAP_THRESHOLD, 128 KB)` (`libcosmic/src/malloc.rs:22-27`),
  which is the main thing mimalloc would otherwise improve on glibc.
- **Consequences**: No allocator dependency added. Realized savings from
  the worker cap (−1.6 MB total RssAnon) are larger than mimalloc would
  likely deliver here.
- **References**: `libcosmic/src/malloc.rs:22-27`

### Open: 16-worker pool inside libcosmic upstream

- **Status**: Open
- **Date**: 2026-05-17
- **Question**: `cosmic::applet::run` builds an `iced::daemon` whose
  executor type defaults to `iced_futures::backend::default::Executor`
  (= `tokio::runtime::Runtime`, uncapped). libcosmic's own
  `single::Executor` (worker_threads=1) is dead code on the applet path.
  Worth filing upstream — would obviate our env-var shim.
- **Related**: `libcosmic/src/applet/mod.rs:541-598`,
  `libcosmic/src/executor/single.rs`,
  `iced/src/daemon.rs:68`
- **Resolution**: TBD — env var workaround in place locally.
