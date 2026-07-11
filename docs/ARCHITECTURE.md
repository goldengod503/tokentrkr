# TokenTrkr Architecture

Living reference for the codebase's structure, the deliberate choices that
shape it, and the issues we know about and consciously deferred. The
point of this doc is to give future architectural reviews a baseline to
compare against — findings that contradict a stated decision here must
argue against the doc rather than against a vacuum.

**Last verified against commit:** ccc6278 (2026-07-11, applet hardening merged)
**Inputs:** `.code-review/REVIEW.md` (2026-05-16) + architectural-analysis
recommendations A1–A7 (2026-05-17, output not retained on disk; A1 and A2
shipped per `docs/correctness/releases/`) + architectural-analysis on
`src/cosmic_app.rs` (2026-07-11, A1–A3 shipped per release doc
`2026-07-11_001`; A4–A6 deferred, see ledger).

## Current structure

```
src/
  main.rs            90  composition root — builds Config, ClaudeProvider, UsageService, then the shell
  config.rs         224  on-disk config + tilde-path expansion
  models.rs         199  wire types for the usage API and the in-memory snapshot (incl. model-scoped limits)
  claude.rs         816  ClaudeProvider — OAuth refresh + usage fetch + atomic creds write + CAS-lite rotation guard
  provider.rs        58  Provider trait + RateLimited / Unauthorized / EmptyResponse sentinel error types
  history.rs        228  30-day percent history; ordered serialization + off-thread atomic JSON writes
  icon.rs           153  SVG icon rendering for SNI tray (vendored DejaVu Sans Bold)
  tray.rs           407  ksni-based SNI tray + apply_event(&UsageEvent)
  cosmic_app.rs    1223  libcosmic applet — popup + iced Subscription forwarder; UsageStreamUnavailable handler; chart SVG helpers (smooth_path, build_chart_svg) + the file's first unit tests
  usage/                 orchestration core (extracted in A1)
    mod.rs            6    re-exports
    event.rs         66    UsageEvent enum (5 variants)
    retry.rs         63    RetryPolicy (10/30/60s ladder)
    service.rs      741    UsageService::spawn() — fetch + retry + state machine + MockProvider; EmitResult signals ChannelClosed; test-only JoinHandle on UsageHandle
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
loop returns — that exit path closes the COSMIC subscription-restart
leak documented in release doc `2026-05-17_003`.

**Race:** a Full→Closed channel transition between the primary
`try_send` and the `Stalled` `try_send` produces a `DroppedFull` return
for what has become a `Closed` channel. Bounded to at most one
additional `fetch_usage()` call before the next `emit` observes `Closed`
and returns. Accepted as cheaper than the locking or retry that would
close the race; see release doc `2026-05-17_004` for the decision.

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

Exit-on-Transient (added 2026-07-05, R5): a `Transient` outcome on the
dormant recovery poll returns the service to `Normal` — the server
answering (even with a 429) contradicts the auth/empty condition that
justified Dormant. The Unauthorized→Dormant transition and its 15-min
cadence are unchanged; without the exit arm a throttled recovery poll
re-applied the full dormant wait (~30 min total to recover). `Aborted`
still exits the loop before the state table. See release doc
`2026-07-05_003`.

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

### `RateLimited` carries `retry_after`; the ladder takes `max(ladder, hint)`

The 429 sentinel is a payload struct: `RateLimited { retry_after:
Option<Duration> }`. `ClaudeProvider` parses the `retry-after` header in
RFC 9110 delta-seconds form only (HTTP-date and garbage → `None`) and
caps it at the 15-minute dormant interval so a malformed or hostile
header cannot park the fetch loop. The retry ladder in `do_one_fetch`
waits `max(ladder_step, hint)` on in-ladder attempts — the ladder is a
floor, never shortened by a small hint. On ladder exhaustion the hint is
dropped (`retrying_in: None`, ordinary Transient): honoring it there
would emit a "retrying in X" that misstates the real wait
(X + poll_interval). Backoff logic stays entirely in `usage::` — the
"COSMIC and SNI share one retry policy" decision above is preserved by
construction. See release doc `2026-07-05_002`.

### Single timeout authority: reqwest primary, 45s outer backstop

Two layers time out one fetch attempt. reqwest's client timeout (30s
request / 10s connect, `claude.rs`) is the **primary** authority for
production fetches. `RetryPolicy.fetch_timeout` (45s) is a deliberate
**backstop** just above the reqwest ceiling — it exists to bound a
`Provider` with no internal timeout (`MockProvider` in tests, or a
future impl that forgets a client timeout), not to fire in production.
Chosen over deleting the outer timeout entirely (Option B) to keep the
test/mock bound; decision by Peter 2026-07-05. If either value moves,
preserve `fetch_timeout > reqwest timeout` — inverting the order
silently changes which layer owns timeouts (the R9 drift trap this
decision exists to prevent). The `default_fetch_timeout_is_a_backstop_
above_the_reqwest_ceiling` test enforces the margin. See release doc
`2026-07-05_004`.

### History persistence: serialize on the update thread, write on a blocking thread, last-rename-wins

`apply_usage_result` serializes via `UsageHistory::serialize_pruned()` on
the winit thread — so byte payloads are produced strictly in `record()`
order — then hands `(path, bytes)` to `tokio::task::spawn_blocking` for
the `atomic_write` + fsync (2026-07-11 A2; previously the fsync ran
synchronously in `update()`, freezing the UI for the fsync's duration).
Consequence: writes may now run concurrently. `atomic_write` uses a
per-call tmp filename so concurrent writes cannot truncate each other's
in-flight tmp (a shared tmp could corrupt `history.json`, which `load()`
answers by wiping history); rename order is not guaranteed, so **last
rename wins** — always a complete valid JSON, at most one stale data
point until the next fetch rewrites the file. Accepted over a dedicated
single-writer channel as the cheapest scheme whose failure mode is
self-healing staleness rather than corruption. Also intentional: the
history path is captured once at `load()`; a `Default`-constructed
history has no path and persistence is a no-op — this is what keeps
`handle_event` unit tests off the real history file. A synchronous-write
fallback covers `Handle::try_current()` failing (not expected inside
`update()`; preserves pre-A2 behavior instead of panicking if a libcosmic
bump changes executor semantics). Do not flag the rename race or the
no-path no-op as findings without arguing against this rationale. See
release doc `2026-07-11_001`.

### `FetchStarted` flushes the parked min-spin result, not guarded by `fetch_id`

The min-spin gate parks a completed result in `pending_snapshot`; the
`FetchStarted` arm applies it via `apply_usage_result` *before* resetting
spin state. The flush is deliberately **not** compared against the
incoming event's `fetch_id`: the parked result belongs to the previous
fetch by construction (parking already required `fetch_id ==
latest_fetch_id` at `Snapshot` time), so guarding it against the *new*
fetch's id would re-drop it — exactly the silent history data loss
(2026-07-11 analysis B1/R1) the flush exists to fix. See release doc
`2026-07-11_001`.

## Known-deferred issues

Items we have evidence for but consciously chose not to address now.
Each has a reopen trigger.

### A5 — Split `cosmic_app.rs` (1223 lines as of 2026-07-11) into submodules
- **Why deferred:** The file is the largest in the codebase but it's
  organized into clear sections (popup, subscription, message handling,
  chart helpers). No friction is being paid today — every change since
  the A1 extraction has been local to one section (verified against the
  full commit range at the 2026-07-11 analysis; the 2026-07-11 hardening
  merge bundled three independent single-section fixes, not one change
  spanning three sections).
- **Known severable island (2026-07-11 S1):** the chart block
  (`smooth_path` + `build_chart_svg` + their tests, ~350 lines, zero
  `cosmic::` references) is extractable to `src/chart.rs` at near-zero
  risk. Deferred under the same trigger — no friction paid today.
- **Reopen when:** A change requires editing three or more sections of
  `cosmic_app.rs` in the same PR, OR a second long-lived
  `iced::Subscription` is added, OR a single PR must edit the chart
  helpers and message handling together.
- **Source:** Architectural-analysis A5 (2026-05-17); re-examined
  2026-07-11 (S1/S2).

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

### Credentials-file write race — cross-process
- **Status:** CAS-lite guard shipped 2026-07-05. `write_credentials`
  re-reads the on-disk `refreshToken` immediately before its atomic
  rename and aborts (private `ExternalCredentialRotation` sentinel in
  `claude.rs`) if it no longer matches the token the refresh consumed.
  Recovery treats disk as authoritative: re-read only, never a second
  refresh (the external pair is fresher; ours is orphaned).
- **Why deferred (residual):** The guard shrinks the clobber window
  from a full OAuth network round-trip to the microseconds between the
  guard read and the rename — it does not eliminate it. Elimination
  requires a cross-process coordination protocol with Claude Code,
  which we do not own (system-architect scope, escalated in the
  2026-06-10 core-review index).
- **Reopen when:** A recorded clobber incident survives the guard.
- **Source:** 2026-06-10 core review R1 (findings B5/C1); spec
  `homelab2-docs/specs/tokentrkr/2026-06-10-credentials-toctou-guard.md`.

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

### "Updated X min ago" formatter duplication
- **Why deferred:** Byte-identical `ago < 60 → "Updated just now" /
  else "Updated {} min ago"` logic implemented independently in both
  shells (`src/cosmic_app.rs:973`, `src/tray.rs:248`). Third instance of
  the WARN-005/SUGG-002 pattern. Extraction would cross the cosmic/SNI
  feature gate for a two-line cosmetic helper — failed YAGNI test.
- **Reopen when:** The two copies drift, OR a third caller appears, OR
  a shared shell-formatting module is created for another reason.
- **Source:** 2026-07-11 architectural-analysis S6.

### A4 — `RetryPolicy` public surface (zero external callers)
- **Partially taken 2026-07-05 (R10):** the confirmed-dead subset —
  `with_retry()` and the `pub use retry::RetryPolicy` re-export — is
  deleted; their two dead-code warnings are gone.
- **Still deferred:** demoting `RetryPolicy` fields to `pub(crate)`.
  The original trigger has not fired.
- **Reopen when:** Any caller outside `usage/` constructs a
  `RetryPolicy` (the `with_retry` half of the old trigger is moot —
  it no longer exists).
- **Source:** 2026-05-17 architectural-analysis A4; release doc
  `2026-07-05_004`.

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

### `UsageEvent::TransientError.retrying_in` is populated but unread
- **Why deferred:** The retry ladder at `src/usage/service.rs:153-169`
  faithfully populates `retrying_in: Option<Duration>` on every
  `TransientError` emit, but both shells discard it via `..` patterns
  (`src/cosmic_app.rs:443`, `src/tray.rs:31`). The compiler emits a
  dead-code warning. The field exists as infrastructure for a future
  "retrying in Xs" UI element that has not been built. Removing it now
  is reversible churn — when the UI is added the field returns. Note:
  the timeout arm at `src/usage/service.rs:130` emits `None` even
  though the service will retry on the next poll cycle; that is a
  semantic mismatch with the ladder-exhausted `None` which means
  "no more retries" — clarify when the consumer is built.
- **Reopen when:** A shell adds a "retrying in Xs" UI element, OR a
  third release ships with the warning unconsumed.
- **Source:** 2026-05-17 re-run architectural-analysis S1.

### `FetchOutcome::Aborted` conflates network classification with loop-exit signal
- **Why deferred:** `FetchOutcome` at `src/usage/service.rs:58-66` has
  four variants: `Success`/`Transient`/`Permanent` classify the network
  outcome; `Aborted` is a control-flow signal that the receiver was
  dropped. The state-machine `match` at lines 99-103 silently swallows
  `Aborted` via `(s, _) => s`, guarded only by the early `return` at
  lines 95-97 firing first. A future exhaustive match over
  `FetchOutcome` must remember this. Refactoring to
  `Result<FetchOutcome, ()>` would touch ~6 return sites in
  `do_one_fetch` for a type-level distinction that does not change
  behavior — YAGNI at current scale.
- **Reopen when:** A third use of `FetchOutcome::Aborted` is added, OR
  the state machine grows past five variants, OR a second
  control-flow-exit variant is needed.
- **Source:** 2026-05-17 re-run architectural-analysis S3.

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

### 2026-05-17: re-run architectural-analysis followups (A1 stuck-spinner recovery, A2 deterministic exit test, A3 try_send race note)

Driven by the SECOND architectural-analysis run on `src/usage/`,
focused on the post-A1/A2/A3 code introduced above. One merged branch:

- `fix/usage-rerun-A1-A2-A3` — **A1** (R1, Medium): new
  `Message::UsageStreamUnavailable` variant emitted once by the COSMIC
  subscription closure's `take()-returned-None` branch before idling.
  `update()` clears `refreshing`, `fetch_done`, `pending_snapshot` and
  surfaces a "restart applet" error. Closes the permanent stuck-spinner
  + 20Hz timer-burn path identified by the rerun's B4/B5 findings.
  **A2** (R2, Medium): `UsageHandle` gains a `#[cfg(test)] task: JoinHandle<()>`
  field; `run_loop_exits_when_event_receiver_is_dropped` now asserts
  termination structurally via `tokio::time::timeout(1s, handle.task)`
  + `assert_eq!(call_count, 1)` instead of yield-counting. The A1 leak
  regression guard is no longer brittle to refactors that add `.await`
  points in the loop. **A3** (R3, Low): ARCHITECTURE.md "try_send +
  Stalled" Intentional decision amended with a Race paragraph noting
  the Full→Closed TOCTOU and its one-iteration bound (this doc).
  Release doc `2026-05-17_004`.

  Also: inline comment at `src/usage/service.rs:152` explaining why
  the `RateLimited` arm uses raw `emit()` instead of `emit_or_abort`
  (S2 from the rerun, deferred to comment-only per architect).

  41 unit tests pass on both build targets; net structural delta is
  ~14 lines in `cosmic_app.rs` (variant + handler) and ~20 lines in
  `service.rs` (test rewrite + cfg(test) field).

### 2026-05-23 → 2026-07-05: core-review remediation + feature growth (no module moves)

Seven release docs, no structural boundary changes — the Intentional
decisions above were amended in place (`retry-after` propagation, exit
Dormant on Transient, 45s backstop, credentials CAS-lite guard; release
docs `2026-07-05_001`–`_004`). Feature growth: local-timezone reset
rendering (`2026-05-23_001`), model-scoped limits (Fable) parsing grew
`claude.rs`/`models.rs` (`2026-07-05_005`), and the chart-smoothing pass
(`2026-07-05_006`) grew `cosmic_app.rs` by ~350 lines of
framework-independent SVG helpers (`smooth_path`, `build_chart_svg`)
plus the file's first `#[cfg(test)]` module. Also added: `justfile`,
`docs/release-ledger.md`.

### 2026-07-11: applet hardening (min-spin flush, history persistence split, themed bars)

Driven by the architectural-analysis run focused on `src/cosmic_app.rs`.
One merged branch (`fix/applet-hardening`, commit `9ecd0fb`, merge
`ccc6278`):

- **A1** (B1/R1, High): `FetchStarted` applies a `pending_snapshot`
  parked by the min-spin gate *before* resetting spin state, closing the
  silent history data-loss path (manual refresh during the ~3s spin
  window dropped the fetched reading). First `handle_event` unit tests
  (flush Ok/Err, post-min-spin direct apply, stale `fetch_id` rejection).
- **A2** (C1/R2, Medium): history persistence ownership shift —
  `UsageHistory::save()` replaced by `serialize_pruned()` (update
  thread, ordered with `record()`) + `write_bytes()` on
  `spawn_blocking`; path captured at `load()`; per-call tmp names in
  `atomic_write`. See the new Intentional decision above.
- **A3** (B5/R3, Medium): `progress_bar_bg` honors the active theme
  (white-alpha track on dark, black-alpha on light) — closes the
  progress-bar half of the chart-smoothing release doc's
  "rest of popup is dark-only" open item.

Release doc `2026-07-11_001`. 69 tests pass (cosmic), 58 (SNI); both
targets build. A4–A6 from the same analysis deferred (ledger,
2026-07-11 section).

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
