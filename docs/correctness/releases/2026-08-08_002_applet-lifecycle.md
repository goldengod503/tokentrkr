# 2026-08-08 — Applet lifecycle: desktop-entry field code and SNI exit paths

## Summary

Two hygiene fixes to how the applet process is launched and how it exits.
Neither changes COSMIC-target behavior, and neither is the cause of the
display blanking that prompted the investigation (see **Context** below) —
that was environmental, not a TokenTrkr defect.

The desktop entry declared `Exec=tokentrkr %F`. `cosmic-panel` passes the
`Exec` line through without expanding XDG field codes, so the applet
really did run as `tokentrkr %F` (observed in `ps`). TokenTrkr parses no
argv anywhere (`grep -n "env::args\|args_os\|clap" src/*.rs` returns
nothing), so the stray argument was inert — this is a correctness-of-
declaration fix, not a bug fix for observed misbehavior.

The SNI target called `std::process::exit(0)` in two places. Under a
panel-hosted applet a process that vanishes mid-callback makes
`cosmic-panel` tear down and rebuild every applet surface, which costs the
compositor a modeset. Both call sites now unwind normally instead.

## Context — why this was investigated

Both monitors were blanking and restoring at seemingly random intervals.
Root cause was environmental: the COSMIC desktop stack was upgraded by
`apt` at 11:45:56 on 2026-08-08, 92 seconds after the session started, and
the session was never restarted. The running `cosmic-comp` was therefore
the pre-upgrade binary (`/proc/<pid>/exe -> /usr/bin/cosmic-comp
(deleted)`, 74 deleted library mappings) while TokenTrkr, rebuilt during
the frosted-glass work, linked the *post*-upgrade `libcosmic`
(`8a017a15`). The new libcosmic requests a blurred layer surface via
`ext_background_effect_v1`; the old compositor could not serve it, the
panel rebuilt its surfaces, and `cosmic-comp` logged `Failed to destroy
old mode property blob` — a modeset, which is what blanked the monitors.

Evidence that this is environmental and not a TokenTrkr regression:

- The first hour of the session (11:44 → 12:44), after the `apt` upgrade
  but before any frosted-glass build existed, was completely clean.
- Every blanking event falls inside the frosted-glass work window, which
  began at 12:44:55; the code commit `2d0f88d` landed at 12:46:08.
- Boot `-1` ran seven days with TokenTrkr in the panel and produced zero
  runtime panel reloads — that was the pre-bump TokenTrkr.

The fix for the blanking is a reboot, which puts a compositor that speaks
`ext_background_effect_v1` underneath the applet that requests it. Nothing
in this release doc addresses it.

The `exited without error` line the panel logged at 13:34:48 came from
`cosmic::applet::run()` returning `Ok(())` when its event loop ended, then
`main` returning — **not** from either `process::exit(0)` call. Under
COSMIC, `src/main.rs:54` returns into `cosmic_app::run()`, so `run_sni`
and the whole `TrkrTray` path are never reached. The exit-path changes
below are latent-bug fixes, not a fix for the observed exit.

## Scope

### Files Created

- docs/correctness/releases/2026-08-08_002_applet-lifecycle.md (this doc)

### Files Modified

- resources/com.github.goldengod503.TokenTrkr.desktop — `Exec=tokentrkr %F`
  → `Exec=tokentrkr` (1 line). The installed copy at
  `~/.local/share/applications/` was edited to match; it is not tracked by
  this repo.
- src/tray.rs — `TrkrTray` gains a `shutdown_tx: mpsc::Sender<()>` field;
  `TrkrTray::new` gains the matching parameter; the "Quit" menu item
  signals that channel instead of calling `std::process::exit(0)`. Test
  helper `make_tray()` updated for the new signature.
- src/main.rs — `run_sni` creates the shutdown channel and passes it to
  both `TrkrTray::new` and `sni_event_loop`; `sni_event_loop` now
  `select!`s over the usage-event stream and the shutdown signal, breaking
  out of the loop on either, and returns rather than calling
  `std::process::exit(0)`.

## Behavioral Impact

| Scenario | Before | After |
|---|---|---|
| COSMIC target (default features) | Runs `cosmic_app::run()`; `TrkrTray` never constructed | **Unchanged** — no code on this path was touched |
| Applet launch argv | `tokentrkr %F` (argument ignored, nothing parses argv) | `tokentrkr` — no observable difference |
| SNI target, "Quit" menu item | `process::exit(0)` mid-callback; destructors skipped, `tray_handle.shutdown()` never runs | Signals shutdown channel; loop breaks, `tray_handle.shutdown().await` runs, runtime unwinds |
| SNI target, usage service dropped | `tray_handle.shutdown().await` then `process::exit(0)` | `tray_handle.shutdown().await` then normal return; `main` returns `Ok(())` |

The SNI path is reachable under COSMIC: `src/main.rs:56-59` falls through
to the SNI tray when COSMIC is detected but the `cosmic` feature is not
compiled, which is the documented `--no-default-features` build. That
configuration is the reason these two call sites are worth fixing even
though they are dead code in the default build.

Exit status is unchanged in both SNI cases — `main` returns `Ok(())`,
which is also status 0.

## Test Plan

```
cargo test                        →  69 passed; 0 failed
cargo test --no-default-features  →  58 passed; 0 failed
```

Both counts match the v0.9.0 baseline exactly (69 / 58, recorded in
[2026-08-08_001](2026-08-08_001_frosted-glass.md)), confirming the
`TrkrTray::new` signature change and the `select!` rewrite broke nothing.

Release builds, SNI-first then COSMIC-last (per project convention, since
both targets write `target/release/tokentrkr`):

- `cargo build --release --no-default-features` — clean, 13 pre-existing
  dead-code warnings.
- `cargo build --release` — clean, 3 pre-existing dead-code warnings.

No new tests were added. The Quit path needs a live SNI tray and a D-Bus
StatusNotifierItem host to exercise, and `sni_event_loop`'s `select!` has
no seam to drive without spawning the real ksni handle — both are
integration-level concerns that the current suite has no harness for. This
is a known gap, not an oversight; see Open Questions.

## Docs Updated

- docs/release-ledger.md — entry appended under the 2026-08-08 section.

`CLAUDE.md`'s "Last verified against commit" line and `docs/ARCHITECTURE.md`
are intentionally not touched: adding a channel field to an existing
struct shifts no module boundary, ownership, or state machine, so this
does not meet the `/robot:refresh-architecture` trigger.

## Rollback Plan

Single code commit on `fix/applet-lifecycle`, merged to `main` via
`--no-ff`. `git revert` the merge commit restores `Exec=tokentrkr %F`, the
2-field `TrkrTray`, and both `std::process::exit(0)` calls. The change
touches no dependency pins, so no relock is involved in a revert.

## Open Questions / Decisions

- **No test coverage for either exit path.** Both require integration
  scaffolding (live D-Bus StatusNotifierItem host for Quit; a driveable
  seam around `sni_event_loop` for the shutdown signal) that this project
  does not currently have. Deferred rather than silently skipped.
- **`Cargo.toml` version is `0.1.0` while releases are tagged `v0.9.x`.**
  Pre-existing inconsistency, unrelated to this change and deliberately
  not touched here. Releases are cut from git tags, so the crate version
  is currently decorative. Flagged for a future decision.
- **The installed desktop entry is edited in place, not installed from
  `resources/`.** `~/.local/share/applications/com.github.goldengod503.TokenTrkr.desktop`
  was corrected by hand to match. There is no install step wiring the two
  together, so they can drift again. Not addressed here.

## References

- Prior release: [2026-08-08_001](2026-08-08_001_frosted-glass.md) — the
  libcosmic bump whose rebuild surfaced the compositor mismatch.
- Branched from `main` at `ddef3a5` (tag `v0.9.0`).
