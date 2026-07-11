# TokenTrkr

Rust system-tray applet that tracks Claude usage quotas. Two build targets: COSMIC native applet (default) and SNI tray (no-default-features).

**Last verified against commit:** ccc6278 (2026-07-11)

See `docs/ARCHITECTURE.md` for the full structural map, intentional decisions, known-deferred issues, and recent architectural changes. That doc is the authoritative architectural reference; this file is the per-session context loaded into every Claude Code conversation.

## Build & test

```bash
cargo build --release                 # default features (cosmic)
cargo build --release --no-default-features  # SNI only
cargo test
```

## Key paths

- `src/usage/` — orchestration core (fetch + retry + state machine)
- `src/claude.rs` — OAuth refresh + usage fetch
- `src/cosmic_app.rs` — libcosmic applet
- `src/tray.rs` — ksni SNI tray
- `docs/correctness/releases/` — behavioral change logs
- `docs/portability/releases/` — packaging / build change logs

## When the SHA above changes

If you make architectural changes (new module boundaries, ownership shifts, retry/state-machine reshuffles), invoke `/robot:refresh-architecture` to update both `docs/ARCHITECTURE.md` and the SHA line above in the same commit. The robot analytical skills compare these two to detect drift.
