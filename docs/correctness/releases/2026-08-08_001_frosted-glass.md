# 2026-08-08 — Frosted glass: translucent applet popup via libcosmic bump

## Summary

This is a dependency bump, not a rendering change. The `libcosmic` pin
moved from `17291536a10124a053b40c49bb459d7b5085331b` to
`8a017a15ee7753241c0a631b4294a68edf79b13b` to pick up upstream's
compositor-side background blur for the applet popup. TokenTrkr added no
blur rendering code of its own; the popup now participates in a path that
already exists in `libcosmic`/`cosmic-theme`: `Core::frosted()` maps
`AppType::Applet` to `theme.frosted_applets`, which — when true — drives
`iced::window::enable_blur`, which requests blur from the compositor via
`org_kde_kwin_blur_manager`. Whether the popup is actually translucent at
runtime is controlled entirely by the user's COSMIC Settings → Appearance
frosted-glass toggle, not by anything TokenTrkr does.

The only API break surfaced by the bump was `surface::action::app_popup`
gaining a leading `live_settings` parameter. TokenTrkr passes
`LiveSettings::default()` (all fields `None`), so the popup inherits the
surface-type defaults rather than carrying a TokenTrkr-local override.

## Scope

### Files Created

- docs/correctness/releases/2026-08-08_001_frosted-glass.md (this doc)

### Files Modified

- Cargo.toml — `libcosmic` `rev` pin updated (1 line).
- Cargo.lock — full regeneration. The locked `tokio 1.50.0` and
  `zbus 5.14.0` no longer satisfied the new libcosmic's transitive
  requirements (`cosmic-config` wants tokio `^1.52`,
  `cosmic-settings-daemon` wants zbus `^5.15`); `cargo update -p tokio`
  alone still conflicted on zbus, so the lockfile was deleted and
  regenerated from scratch rather than incrementally updated.
- src/cosmic_app.rs — 5 lines: the `LiveSettings` import added to the
  existing `cosmic::surface::action::{app_popup, destroy_popup}` use, and
  a `|_state: &TokenTrkrApplet| LiveSettings::default()` argument (plus
  comment) added to the `app_popup::<TokenTrkrApplet>(...)` call.

No API breaks beyond `app_popup` gaining the `live_settings` parameter
turned up. Verified unchanged across the bump: the `Application` trait,
`applet::run`, `applet::style`,
`Context::{autosize_window, get_popup_settings, popup_container}`,
`Subscription::run_with` (still an `fn` pointer, so the `OnceLock`
static-state pattern was retained), and `iced::stream::channel`.

## Behavioral Impact

| Scenario | Before | After |
|---|---|---|
| Popup, system frosting **off** | Opaque popup background | Unchanged — opaque popup background |
| Popup, system frosting **on** | Opaque popup background (no blur path existed) | Translucent, compositor-blurred popup background |
| Tray button | Transparent, inherits `frosted_panel` | Unchanged — already transparent, inherits `frosted_panel` |
| SNI target (`--no-default-features`) | Does not link libcosmic | Unchanged — does not link libcosmic |

## Test Plan

```
cargo test                        →  69 passed; 0 failed
cargo test --no-default-features  →  58 passed; 0 failed
```

Both counts match the pre-bump baseline exactly (measured in Task 2 on
this same commit `c31e3d6`; not re-run for this doc).

Release builds, SNI-first then COSMIC-last (per project convention, since
both targets write `target/release/tokentrkr`):

- `cargo build --release --no-default-features` — clean, 13 pre-existing
  dead-code warnings, binary 9,418,976 bytes (≈9.0 MB).
- `cargo build --release` — clean, 3 pre-existing dead-code warnings,
  final binary **28,935,128 bytes** (≈28.9 MB) at the path the applet
  symlink points at.

**Blur-path evidence.** The plan's original check
(`strings -a target/release/tokentrkr | grep -c "enable_blur"`, expecting
`> 0`) is unsound and was not used as evidence: `Cargo.toml:39` sets
`[profile.release] strip = true`, so Rust function identifiers do not
survive into the stripped binary's strings, and the command in fact
returned `0` on the built binary — a result that proves nothing either
way, since it would return `0` whether or not the blur path is compiled
in. It was replaced with a three-part evidence chain, all measured against
the actual `c31e3d6` build:

1. **Compile-time gate is active.** `src/app/cosmic.rs:943` guards the
   blur-toggling block with `#[cfg(wayland_platform)]`. libcosmic's build
   script emits that cfg for this build
   (`target/release/build/libcosmic-*/output` contains
   `cargo:rustc-cfg=wayland_platform`), so the block compiles in.
2. **Source differential.** The new pinned rev (`8a017a1`) has 10
   `enable_blur` call sites in `src/app/cosmic.rs` (lines 948, 1047, 1175,
   1315, 1465, 1523, 1581, 1596, 1614, 1629). The old pinned rev
   (`1729153`) has zero occurrences of `enable_blur` anywhere in `src/`.
3. **Binary differential.** The stripped binary's surviving strings (serde
   field names) show the new `cosmic-theme` struct is what got linked:
   `frosted_applets`, `frosted_panel`, `frosted_windows`,
   `frosted_system_interface`, and `frosted_maximized_apps` are present,
   and there are zero occurrences of the old theme's dead `is_frosted`
   field.

Full detail: `.superpowers/sdd/2026-08-08-frosted-glass/task-3-report.md`.

**Visual verification: PENDING (Peter).** This doc asserts only that the
correct binary was built and that the blur code path is compiled into it.
Frosted glass is currently OFF in COSMIC Settings on this machine, no
agent restarted the applet or `cosmic-panel` or swapped the running
binary, and no screenshot has been taken. The popup's actual translucent
appearance — over a busy wallpaper, in both light and dark themes — is not
claimed here and remains Peter's gate before merge.

## Docs Updated

- docs/release-ledger.md — new section for this work stream.

`CLAUDE.md`'s "Last verified against commit" line and `docs/ARCHITECTURE.md`
are intentionally not touched: this change shifts no module boundaries,
ownership, or state machines, so it does not meet the
`/robot:refresh-architecture` trigger.

## Rollback Plan

Single code commit `c31e3d6` on `feat/frosted-glass`.
`git revert c31e3d6` restores: the libcosmic pin at
`17291536a10124a053b40c49bb459d7b5085331b`, the prior `Cargo.lock`, and
the 2-argument `app_popup` call (no `LiveSettings`).

## Open Questions / Decisions

- **No TokenTrkr blur override added.** The `app_popup` call passes
  `LiveSettings::default()` (all fields `None`) rather than a
  TokenTrkr-local override, so the popup's frosting follows whatever the
  user has set in COSMIC Settings → Appearance. Adding a TokenTrkr-level
  frosting config setting was explicitly out of scope for this work.
- **`feat/popup-redesign-hud` remains frozen and out of scope.** No work
  from that branch or its stashes was revived, rebased, or cherry-picked
  as part of this bump.

## References

- Spec: `homelab2-docs/specs/tokentrkr/2026-08-08-frosted-glass-design.md`
- Prior libcosmic bump: `565748f` ("Fix COSMIC popup crash by upgrading
  libcosmic and migrating to new APIs").
- Branched from main at `e9688f1`.
