# TokenTrkr Architecture

Living reference for the codebase's structure, the deliberate choices that
shape it, and the issues we know about and consciously deferred. The
point of this doc is to give future architectural reviews a baseline to
compare against — findings that contradict a stated decision here must
argue against the doc rather than against a vacuum.

**Last verified against commit:** 56a4055 (2026-05-17, A1/A2/A3 followups merged)
**Inputs:** `.code-review/REVIEW.md` (2026-05-16) + architectural-analysis
recommendations A1–A7 (2026-05-17, output not retained on disk; A1 and A2
shipped per `docs/correctness/releases/`).

## Current structure

```
src/
  main.rs            90  composition root — builds Config, ClaudeProvider, UsageService, then the shell
  config.rs         224  on-disk config + tilde-path expansion
  models.rs         137  wire types for the usage API and the in-memory snapshot
  claude.rs         516  ClaudeProvider — OAuth refresh + usage fetch + atomic creds write
  provider.rs        52  Provider trait + RateLimited / Unauthorized / EmptyResponse sentinel error types
  history.rs        194  30-day percent history with atomic JSON writes
  icon.rs           153  SVG icon rendering for SNI tray (vendored DejaVu Sans Bold)
  tray.rs           385  ksni-based SNI tray + apply_event(&UsageEvent)
  cosmic_app.rs     858  libcosmic applet — popup + iced Subscription forwarder
  usage/                 orchestration core (extracted in A1)
    mod.rs            7    re-exports
    event.rs         66    UsageEvent enum (5 variants)
    retry.rs         54    RetryPolicy (10/30/60s ladder)
    service.rs      592    UsageService::spawn() — fetch + retry + state machine + MockProvider; EmitResult signals ChannelClosed
```

**Build targets.** Two: `cargo build` with default features pulls libcosmic
and the COSMIC applet; `cargo build --no-default-features` builds the SNI
tray path only. Both must compile on every commit.

**Dependency direction (intentional).**
- Shells (`tray.rs`, `cosmic_app.rs`) depend on `usage::`.
- `usage::` depends on `provider::Provider` (the trait), never on `claude.rs`.
- `claude.rs` implements `provider::Provider` and is the only module that
  touches the network or OAuth state.
- Sentinel errors (`RateLimited`, `Unauthorized`, `EmptyResponse`) live
  in `provider.rs` next to the trait. `usage::` no longer reaches into
  the crate root for its error vocabulary.
- `models.rs`, `config.rs`, `history.rs` are leaves — no other module
  imports the shells back.

## Intentional decisions

The choices below look "wrong" to an adversarial reviewer but are
deliberate. Future analysis runs that flag any of these as findings must
argue against the rationale stated here, not against the code in isolation.

### `Provider` trait has one implementation

`src/provider.rs` defines a single-method trait used by exactly one
production type (`ClaudeProvider`). The code-review's "what's good"
section flagged this as load-bearing despite the single impl, because the
shells hold `Arc<dyn Provider>` and never import `claude.rs`. The seam
exists to keep `usage::Service` testable via `MockProvider` (in
`src/usage/service.rs` under `#[cfg(test)]`) — which is the second
implementation, gated to the test build. **Do not collapse this to a
concrete type** under "one impl, YAGNI" — the test impl is the one that
justifies it.

### `usage::Service` uses `try_send` + `Stalled` for backpressure

`UsageService` never `.await`s on event-channel sends. `emit()` returns
an `EmitResult` of `Delivered` / `DroppedFull` / `ChannelClosed`. On
`Full` it attempts a `Stalled` event (also `try_send`, may itself drop)
and the loop keeps running. On `Closed` (receiver has been dropped) the
loop returns immediately — that exit path closes the COSMIC
subscription-restart leak documented in release doc `2026-05-17_003`.
**Tradeoff:** under sustained UI render pressure, events can be dropped
silently. Accepted because the alternative — blocking the loop on UI
backpressure — is how the prior COSMIC subscription deadlocked (R2).
See release doc `2026-05-17_002` for the original decision record.

### 15-minute Dormant retry after `Unauthorized`

On 401 the service transitions to `Dormant` and only retries every 15
minutes; manual `Refresh` preempts the wait. Considered alternatives:
stop entirely (manual only) and 1-hour retry. 15 min catches the common
"user re-ran `claude login`" recovery path with ~96 wasted 401s/day in
the worst case (vs ~288 at 5 min). Accepted.

### COSMIC and SNI share one retry policy

After A1, both shells consume the same `UsageEvent` stream and inherit
the 10/30/60s 429 ladder from `usage::RetryPolicy`. The prior asymmetry
(SNI had a ladder, COSMIC did not) is gone and **must not return** —
divergent retry policies caused S10 in the original review.

### `OnceLock<UsageHandle>` in the COSMIC shell

`cosmic_app.rs` stashes the service handle in a `OnceLock` so the
`Subscription` closure (which is `fn`-typed and cannot capture state)
can reach it. This is a workaround for a libcosmic API constraint — see
`memory/libcosmic.md` and `memory/cosmic-subscriptions-static-state.md`.
**Do not refactor away the `OnceLock`** without confirming the
subscription API has changed; the rev is pinned at `17291536`.

### Cargo `open` crate kept in manifest, `Command::new("xdg-open")` used at call sites

SUGG-001 from the code-review flagged the `open` crate as declared but
unused. **Intentionally kept** as documentation of intent — the
migration to `open::that` is a follow-up but the manifest entry signals
the chosen direction. If the migration lands or stalls beyond one more
release, drop the dep.

### `EmptyResponse` is classified as `Permanent`, not a third outcome class

A structurally valid but semantically empty usage response is
indistinguishable from an `Unauthorized` from the user's perspective —
the API is responsive but useless and manual recovery is the resolution.
The `do_one_fetch` arm for `EmptyResponse` maps to `FetchOutcome::Permanent`,
reusing the 15-minute Dormant retry gate that already exists for 401.
The alternative — adding a fourth `FetchOutcome` variant with its own
cadence — was rejected as YAGNI: the cadence and event shape both match
`Permanent` exactly. See release doc `2026-05-17_003`.

### Sentinel error types live in `src/provider.rs`

`RateLimited`, `Unauthorized`, and `EmptyResponse` are part of the
`Provider` contract — every implementation produces them and every
consumer downcasts to them. They live in `src/provider.rs` next to the
trait, not at the crate root and not in a separate `errors` module.
A new file is warranted only if a future error type is unrelated to
`Provider` (e.g., a config-load sentinel). See release doc `2026-05-17_003`.

## Known-deferred issues

Items we have evidence for but consciously chose not to address now.
Each has a reopen trigger.

### A5 — Split `cosmic_app.rs` (858 lines) into submodules
- **Why deferred:** The file is the largest in the codebase but it's
  organized into clear sections (popup, subscription, message handling).
  No friction is being paid today — every change since the A1 extraction
  has been local to one section.
- **Reopen when:** A change requires editing three or more sections of
  `cosmic_app.rs` in the same PR, OR a second long-lived
  `iced::Subscription` is added.
- **Source:** Architectural-analysis A5 (2026-05-17).

### A7 — Move wire types out of `models.rs` into `claude::wire`
- **Why deferred:** `models.rs` mixes the wire DTOs with the in-memory
  snapshot type. Separation would be cleaner, but `models.rs` is 137
  lines and only `claude.rs` touches the wire fields. YAGNI gate fails —
  no third caller, no measured friction.
- **Reopen when:** A second `Provider` impl lands, OR `models.rs`
  exceeds ~250 lines.
- **Source:** Architectural-analysis A7 (2026-05-17).

### HTTP-layer testability — `ClaudeProvider` holds a concrete `reqwest::Client`
- **Why deferred:** The state-machine layer is now exhaustively tested
  via `MockProvider`. End-to-end coverage of `fetch_usage`'s 429 retry
  and refresh sequencing would require either `mockito` or an HTTP-client
  abstraction. Cost outweighs current bug rate in `claude.rs`.
- **Reopen when:** A bug in `ClaudeProvider::fetch_usage` ships to a
  release that the existing unit tests don't catch.
- **Source:** Release docs `2026-05-17_001` and `_002` Open Questions.

### Live `SetInterval` reconfiguration
- **Why deferred:** The old `PollCommand::SetInterval(Duration)` was
  defined but never wired to any UI. Dropped in A1. A future
  config-reload feature would need a control channel into
  `usage::Service`.
- **Reopen when:** Config hot-reload is specced.
- **Source:** Release doc `2026-05-17_002` Open Questions.

### CRIT-002 — Font portability
- **Status:** Already addressed by commit `5f338c3` (vendored the font
  into `assets/`). Listed here only so reviewers don't re-flag it as
  open from the May-16 review table.

### WARN-005 — `format_plan_name` duplication
- **Why deferred:** Still duplicated between `cosmic_app.rs` and
  `tray.rs` with the same drift (`"Pro"` vs `"Pro Plan"`). Visible to
  the user but cosmetic.
- **Reopen when:** A third caller appears, OR the strings drift further.

### SUGG-002 — `bucket_color` duplication
- **Why deferred:** Two byte-identical copies. Cosmetic; no compile-time
  catch on drift. Reopen if thresholds change.

### A4 — `RetryPolicy` public surface (zero external callers)
- **Why deferred:** The 2026-05-17 architectural-analysis on `src/usage/`
  flagged `with_retry()` as a builder with no callers and `RetryPolicy`
  as having `pub` fields with no invariant guard. The fix (delete
  `with_retry`, demote fields to `pub(crate)`) is a deletion, but
  bundling it with the A1/A2/A3 PR would have grown blast radius for
  no current friction. The single dead-code warning on `with_retry`
  is accepted.
- **Reopen when:** Any caller outside `usage/` constructs a
  `RetryPolicy` or calls `with_retry()`.
- **Source:** 2026-05-17 architectural-analysis A4.

### `Stalled` does not carry a `fetch_id`
- **Why deferred:** The 2026-05-17 architectural-analysis identified a
  residual spinner-stuck path (B3 in that report): if the event channel
  saturates at the moment a `Snapshot` would fire, the result is
  dropped and the replacement `Stalled` is consumed as a no-op by
  `cosmic_app::handle_event`. `refreshing` stays `true` until the next
  `FetchStarted` (≤ one `poll_interval`). Adding `fetch_id` to
  `Stalled` would force every shell to add a fourth identified-event
  match arm for a ≤5-minute cosmetic glitch — failed YAGNI test.
- **Reopen when:** A user-visible incident of a stuck spinner is
  recorded, OR `Stalled` gains additional uses where shells need to
  reason about it per-fetch.
- **Source:** 2026-05-17 architectural-analysis B3.

## Recent architectural changes

### 2026-05-16 → 2026-05-17: hardening pass
Driven by `.code-review/REVIEW.md` (Mode C audit). Six Critical + two
Security + multiple Warnings landed across three merged branches:

- `fix/cosmic-build` — bumped libcosmic, fixed the broken `cosmic` feature build (CRIT-001).
- `fix/credentials-security` — atomic creds write at `0600`, NaN guard on
  utilization, 401-vs-429 split, clean shutdown via `PollCommand::Quit`,
  empty-response detection (SEC-001, CRIT-003/004/005/006, SEC-002,
  WARN-001, WARN-007).
- `fix/portability` — vendored font, tilde-path expansion fix, atomic
  history writes, dead config-field hardening, tokio worker thread cap,
  DWARF strip, trimmed tokio features (CRIT-002, WARN-002/003/004/006).

Closed all Critical findings; remaining open items moved into the
"Known-deferred" section above.

### 2026-05-17: orchestration core extraction (A1) + token refresh gating (A2)
Driven by the architectural-analysis run. Two merged branches:

- `fix/token-refresh-race` (A2 / commit `f47860a`) — `CredentialsOutcome`
  enum gates the 429-branch re-refresh on whether
  `ensure_fresh_credentials` already refreshed. Prevents burning
  single-use refresh tokens on `expiry-refresh + 429`. Release doc
  `2026-05-17_001`.
- `feat/usage-service` (A1 / merge `4f4a56a`) — extracted the fetch +
  retry + classification loop from `polling.rs` and the inline
  `cosmic_app.rs` subscription into `src/usage/`. Closes R2 (COSMIC
  backpressure deadlock), R4 (no stop on Unauthorized), B3 (spinner
  generation race), S10 (retry asymmetry between shells). Release doc
  `2026-05-17_002`.

Net structural delta: deleted `src/polling.rs`; added `src/usage/`
(4 files, ~586 lines); 39 unit tests now pass (was 2 before the
hardening pass).

### 2026-05-17: architectural-analysis followups (A1 channel-closed, A2 EmptyResponse, A3 sentinel relocation)

Driven by the architectural-analysis run on `src/usage/`. One merged branch:

- `fix/usage-leak-and-empty-response` (commit `0eb8dc5`, merge `56a4055`) —
  **A1** (R1, High): `emit()` returns `EmitResult` with `Delivered` /
  `DroppedFull` / `ChannelClosed`. `run_loop` and `do_one_fetch` propagate
  `ChannelClosed` via the new private `FetchOutcome::Aborted` variant and
  exit cleanly. Closes the COSMIC subscription-restart task leak
  (B7+B8+C2 cluster). **A2** (R4, Medium): explicit `EmptyResponse`
  downcast arm in `do_one_fetch` maps to `FetchOutcome::Permanent`,
  routing through the Dormant gate (~96 retries/day vs ~288 before).
  **A3** (R3, Medium): `RateLimited`, `Unauthorized`, `EmptyResponse`
  structs relocated from `main.rs` into `provider.rs` next to the
  `Provider` trait. Release doc `2026-05-17_003`. 41 unit tests pass
  on both build targets (was 39).

## Out of scope for this doc

- File-level code style, naming, formatting — those live in clippy /
  rustfmt configs and the shared `CLAUDE.md`.
- Per-file API documentation — covered by doc-comments in source.
- Release-by-release behavioral changes — those live in
  `docs/correctness/releases/` and `docs/portability/releases/`.
- Open / shipped feature work — tracked in `~/homelab2-docs/specs/tokentrkr/`
  and `~/homelab2-docs/plans/tokentrkr/`.

## How to use this doc in future reviews

When running `han:architectural-analysis` or `han:code-review` on this
repo, feed this file to the analysis as context. For each finding the
analysis produces, classify it as:

1. **Contradicts an Intentional decision** — finding must argue against
   the rationale here, not just the code. If the argument holds, update
   the decision (and explain what changed). Otherwise drop the finding.
2. **Restates a Known-deferred issue** — drop it; already tracked. If
   the reopen trigger has fired, promote the deferred item.
3. **Genuinely new** — actionable. Apply YAGNI gates before recommending
   structural change.

Only category (3) findings should rise into the actionable section of
the report. This is the project-specific filter that keeps adversarial
analysis from re-generating the same recommendation list run after run.
