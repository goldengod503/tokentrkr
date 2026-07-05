# 2026-07-05 — CAS-lite guard against cross-process credentials clobber

## Summary

`ClaudeProvider::write_credentials` was a read-modify-write over
`~/.claude/.credentials.json` whose read→rename window spanned the
entire OAuth refresh network round-trip. Claude Code and `claude login`
are active concurrent writers of that file; if one rotated the
(single-use) `refreshToken` inside the window, our rename reverted the
file to an already-consumed token and both apps wedged — TokenTrkr in a
429 loop (the API returns 429, not 401, for stale tokens), Claude Code
failing auth, recovery manual `claude login`. The write now aborts
instead of clobbering when it detects an external rotation, and
recovers by re-reading disk. Fixes R1 (High) plus fold-in B2 from the
2026-06-10 core architecture review.

## Scope

### Files Created

- docs/correctness/releases/2026-07-05_001_credentials-toctou-guard.md (this doc)
- docs/release-ledger.md (ledger bootstrapped; prior release docs predate it)

### Files Modified

- src/claude.rs —
  - `write_credentials` gains a `consumed_refresh_token: &str` parameter
    (the token this refresh exchanged). Immediately before the atomic
    `fs::rename` it re-reads the live file's
    `claudeAiOauth.refreshToken`; on mismatch (or unreadable/unparseable
    file — "can't confirm safe" is treated as rotation) it removes the
    temp file and bails with a new **private** sentinel
    `ExternalCredentialRotation`.
  - B2 fold-in: the `claudeAiOauth` patch block gains an `else` that
    bails, so a missing section is an error instead of a silent
    unchanged-write followed by a misleading success log.
  - New `persist_refreshed_credentials(new_creds, consumed_refresh_token)`
    helper: on `Ok` persists and logs success; on rotation it re-reads
    disk and returns the external writer's pair — **never** a second
    `refresh_token` call. `refresh_token` now delegates to it.
- docs/ARCHITECTURE.md — new Known-deferred entry
  "Credentials-file write race — cross-process" (guard shipped,
  residual microsecond window accepted, coordination protocol deferred
  to system-architect; reopen trigger: a clobber surviving the guard).

## Behavioral Impact

| Scenario | Before | After |
|---|---|---|
| External writer rotates credentials during our refresh round-trip | Our rename clobbers the fresh pair with an orphaned one; both apps wedge on 429/auth failure until manual `claude login` | Write aborts; we adopt the external pair from disk and continue; one warn log |
| No concurrent writer (normal case) | Refresh persisted atomically | Unchanged — guard read matches, rename proceeds (byte-identical behavior) |
| Credentials file unreadable/unparseable at guard time | Rename proceeds over unknown state | Treated as rotation: abort, recovery re-read surfaces the real error |
| `claudeAiOauth` section missing during write | File rewritten unchanged, `Ok(())`, "Token refreshed successfully" logged | `Err`, no write, no success log (B2) |
| Rotation lands in the residual guard-read→rename microsecond window | Clobber | Still a clobber — window shrunk, not eliminated (documented, accepted) |

## Test Plan

Four new unit tests in `src/claude.rs::tests` (46 passing total, was 42):

- `write_credentials_with_externally_rotated_token_aborts_without_clobbering` —
  disk holds a rotated pair; asserts `Err` downcasting to
  `ExternalCredentialRotation`, file byte-identical to the external
  writer's content, and no `.json.tmp` left behind.
- `write_credentials_missing_oauth_section_errors_without_writing` (B2).
- `persist_after_external_rotation_returns_disk_creds_without_refreshing` —
  recovery returns the disk pair and leaves disk untouched; no network
  path exists in the helper, structurally guaranteeing "re-read, never
  re-refresh".
- `persist_with_unrotated_disk_writes_and_returns_our_creds` — happy
  path through the new helper.

Existing `write_credentials_results_in_0600_mode` and
`write_credentials_preserves_unrelated_fields` pass with the new
parameter (happy path preserved).

Verification:

```
cargo test                                   →  46 passed; 0 failed
cargo test --no-default-features             →  46 passed; 0 failed
cargo build --release                        →  cosmic target, no new warnings
cargo build --release --no-default-features  →  SNI target compiles
```

Warning counts are unchanged from before this change on both targets
(cosmic: the five tracked ones — A4 pair, `retrying_in`,
`poll_interval`, `format_summary`; SNI: those plus the pre-existing
feature-gated dead code). Nothing new introduced.

## Docs Updated

- docs/ARCHITECTURE.md — Known-deferred entry added (see Scope).
  Verified-against SHA **not** bumped: module boundaries, ownership,
  the `Provider` trait, and the retry/state machine are unchanged; the
  sentinel is private to `claude.rs` and never crosses the Provider
  boundary. `/robot:refresh-architecture` was not available in the
  implementing session — run it if a full re-verification is wanted.
- docs/release-ledger.md — created.

## Rollback Plan

```bash
git revert <this commit>
```

Single-commit change. No config, wire-format, or persistence-format
surface to undo; the credentials file format is untouched.

## Open Questions / Decisions

### Decision: rotation recovery adopts the on-disk pair and discards ours

- Status: Accepted
- Date: 2026-07-05
- Context: On the expiry path, when rotation is detected our
  just-received server-side tokens are valid but unpersisted, while
  disk holds Claude Code's pair. The spec left "confirm against
  observed OAuth provider behavior or take the conservative route"
  open.
- Decision: Conservative route — disk wins. Our pair may already be
  superseded server-side, and persisting it is exactly the clobber the
  guard exists to prevent. A second refresh is never attempted (it
  would consume the external writer's single-use token pointlessly).
- Consequences: One refresh round-trip's result is occasionally
  discarded (rare, bounded cost). The external writer's session is
  never damaged by ours.
- References: src/claude.rs `persist_refreshed_credentials`,
  `write_credentials` guard block.

### Decision: `ExternalCredentialRotation` is private to `claude.rs`, not a `provider.rs` sentinel

- Status: Accepted
- Date: 2026-07-05
- Context: Spec left placement open — `provider.rs` only if the error
  crosses the `Provider` boundary.
- Decision: Recovery is fully internal to `claude.rs`
  (`persist_refreshed_credentials` handles it before any caller sees
  an error), so the sentinel stays private. The ARCHITECTURE.md
  Intentional decision "Sentinel error types live in src/provider.rs"
  is untouched — that rule governs boundary-crossing sentinels.
- Consequences: `usage::service` and the shells are unaware of
  rotation events; they see either success or an ordinary error.
- References: src/claude.rs `ExternalCredentialRotation`.

## References

- Spec: `homelab2-docs/specs/tokentrkr/2026-06-10-credentials-toctou-guard.md`
  (R1 ← findings B5/C1; B2 fold-in), index
  `2026-06-10-core-review-INDEX.md`.
- Companion shipped earlier: `CredentialsOutcome` 429-refresh gating
  (`f47860a`) — prevents self-inflicted double refresh; this change
  addresses the orthogonal external-writer race.
- Source citations: src/claude.rs `write_credentials` (guard before
  rename), `persist_refreshed_credentials`, tests listed above.
