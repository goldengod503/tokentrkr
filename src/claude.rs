use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::fs;
use std::path::PathBuf;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::models::*;
use crate::provider::Provider;

const OAUTH_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const USAGE_API_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

pub struct ClaudeProvider {
    credentials_path: PathBuf,
    client: reqwest::Client,
}

/// Private sentinel: an external writer (Claude Code, `claude login`) rotated
/// the credentials file between our read and our write-back. Never crosses the
/// `Provider` boundary — recovery is fully internal to this module (re-read
/// disk, discard our now-orphaned token pair).
#[derive(Debug)]
struct ExternalCredentialRotation;

impl std::fmt::Display for ExternalCredentialRotation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "credentials rotated externally during refresh")
    }
}

impl std::error::Error for ExternalCredentialRotation {}

/// Outcome of ensuring we have valid credentials.
///
/// The distinction matters for 429 handling: a 429 *after* we just refreshed
/// is a genuine rate-limit, so re-refreshing would burn another (potentially
/// single-use) refresh token for no gain. A 429 on a *cached* token may be
/// Claude Code rotating credentials under us — one targeted refresh is the
/// right recovery there.
enum CredentialsOutcome {
    Cached(OAuthCredentials),
    Refreshed(OAuthCredentials),
}

impl CredentialsOutcome {
    fn creds(&self) -> &OAuthCredentials {
        match self {
            Self::Cached(c) | Self::Refreshed(c) => c,
        }
    }
    fn was_refreshed(&self) -> bool {
        matches!(self, Self::Refreshed(_))
    }
}

impl ClaudeProvider {
    pub fn new(config: &Config) -> Result<Self> {
        let credentials_path = config.credentials_path();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self {
            credentials_path,
            client,
        })
    }

    fn read_credentials(&self) -> Result<OAuthCredentials> {
        let contents = fs::read_to_string(&self.credentials_path).with_context(|| {
            format!(
                "Failed to read credentials from {}",
                self.credentials_path.display()
            )
        })?;

        let creds_file: CredentialsFile =
            serde_json::from_str(&contents).context("Failed to parse credentials JSON")?;

        creds_file
            .claude_ai_oauth
            .context("No claudeAiOauth section in credentials file")
    }

    /// `consumed_refresh_token` is the refresh token this refresh exchanged
    /// (and thereby invalidated — refresh tokens are single-use). If disk no
    /// longer holds that token, an external writer rotated credentials during
    /// our network round-trip and writing would clobber their fresher pair.
    fn write_credentials(&self, creds: &OAuthCredentials, consumed_refresh_token: &str) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let contents = fs::read_to_string(&self.credentials_path)?;
        let mut raw: serde_json::Value = serde_json::from_str(&contents)?;

        if let Some(oauth) = raw.get_mut("claudeAiOauth") {
            oauth["accessToken"] = serde_json::Value::String(creds.access_token.clone());
            oauth["refreshToken"] = serde_json::Value::String(creds.refresh_token.clone());
            oauth["expiresAt"] = serde_json::Value::Number(creds.expires_at.into());
        } else {
            bail!("credentials file missing claudeAiOauth section during write");
        }

        let updated = serde_json::to_string_pretty(&raw)?;

        let tmp_path = self.credentials_path.with_extension("json.tmp");
        {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)
                .with_context(|| {
                    format!("Failed to open temp credentials at {}", tmp_path.display())
                })?;
            f.write_all(updated.as_bytes())
                .context("Failed to write credentials to temp file")?;
            f.sync_all()
                .context("Failed to sync credentials temp file")?;
        }

        // CAS-lite guard: re-read the live file immediately before the rename.
        // If its refreshToken is no longer the one this refresh consumed, an
        // external writer landed during our read→rename window — abort rather
        // than clobber. This shrinks the race window from a network round-trip
        // to the microseconds between this read and the rename; it does not
        // eliminate it. An unreadable/unparseable file is treated as rotation:
        // when we can't confirm it's safe to clobber, we don't.
        let on_disk_refresh_token = fs::read_to_string(&self.credentials_path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("claudeAiOauth")
                    .and_then(|o| o.get("refreshToken"))
                    .and_then(|t| t.as_str())
                    .map(str::to_owned)
            });
        if on_disk_refresh_token.as_deref() != Some(consumed_refresh_token) {
            let _ = fs::remove_file(&tmp_path);
            bail!(ExternalCredentialRotation);
        }

        fs::rename(&tmp_path, &self.credentials_path).with_context(|| {
            format!(
                "Failed to atomically replace credentials at {}",
                self.credentials_path.display()
            )
        })?;

        // Defends against an unusual umask (e.g. 0o700) that could mask away
        // the mode bits passed to open(2).
        fs::set_permissions(
            &self.credentials_path,
            std::fs::Permissions::from_mode(0o600),
        )
        .context("Failed to set 0600 mode on credentials")?;

        Ok(())
    }

    fn is_expired(creds: &OAuthCredentials, now_ms: u64) -> bool {
        // Defends against well-known bad values on disk:
        //   - 0 (CLI never initialized expiresAt)
        //   - seconds-as-ms (a value below 2001-09-09 in ms is implausible
        //     and almost certainly seconds stored where ms was expected)
        // In both cases we force a refresh rather than trust the value,
        // because the alternative ("never expired") leaves the user stuck
        // on a stale access token with no signal.
        const PLAUSIBLE_MS_MIN: u64 = 1_000_000_000_000;
        if creds.expires_at < PLAUSIBLE_MS_MIN {
            warn!(
                "Implausible expiresAt={} on disk; forcing token refresh",
                creds.expires_at
            );
            return true;
        }
        // Refresh 60 seconds early
        creds.expires_at < now_ms + 60_000
    }

    async fn refresh_token(&self, creds: &OAuthCredentials) -> Result<OAuthCredentials> {
        info!("Refreshing OAuth token");

        let resp = self
            .client
            .post(OAUTH_TOKEN_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!(
                "grant_type=refresh_token&refresh_token={}&client_id={}",
                creds.refresh_token, CLIENT_ID
            ))
            .send()
            .await
            .context("Failed to send token refresh request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Token refresh failed ({}): {}", status, body);
        }

        let token_resp: TokenRefreshResponse = resp
            .json()
            .await
            .context("Failed to parse token refresh response")?;

        let now_ms = Utc::now().timestamp_millis() as u64;
        let new_creds = OAuthCredentials {
            access_token: token_resp.access_token,
            refresh_token: token_resp.refresh_token,
            expires_at: now_ms + (token_resp.expires_in * 1000),
            scopes: creds.scopes.clone(),
            subscription_type: creds.subscription_type.clone(),
            rate_limit_tier: creds.rate_limit_tier.clone(),
        };

        self.persist_refreshed_credentials(new_creds, &creds.refresh_token)
    }

    /// Persist just-refreshed credentials, treating an external rotation as a
    /// re-read trigger — never a refresh trigger. Disk is authoritative: the
    /// external writer's pair is fresher, and our just-received pair may
    /// already be superseded server-side, so we discard ours rather than burn
    /// another single-use refresh token chasing the race.
    fn persist_refreshed_credentials(
        &self,
        new_creds: OAuthCredentials,
        consumed_refresh_token: &str,
    ) -> Result<OAuthCredentials> {
        match self.write_credentials(&new_creds, consumed_refresh_token) {
            Ok(()) => {
                info!("Token refreshed successfully");
                Ok(new_creds)
            }
            Err(e) if e.downcast_ref::<ExternalCredentialRotation>().is_some() => {
                warn!(
                    "Credentials rotated externally during refresh; \
                     discarding our tokens and re-reading disk"
                );
                self.read_credentials()
            }
            Err(e) => Err(e),
        }
    }

    async fn ensure_fresh_credentials(&self) -> Result<CredentialsOutcome> {
        let creds = self.read_credentials()?;
        let now_ms = Utc::now().timestamp_millis().max(0) as u64;

        if Self::is_expired(&creds, now_ms) {
            debug!("Token expired, refreshing");
            let fresh = self.refresh_token(&creds).await?;
            Ok(CredentialsOutcome::Refreshed(fresh))
        } else {
            Ok(CredentialsOutcome::Cached(creds))
        }
    }

    async fn fetch_usage_api(&self, access_token: &str) -> Result<UsageApiResponse> {
        let resp = self
            .client
            .get(USAGE_API_URL)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("anthropic-beta", "oauth-2025-04-20")
            .send()
            .await
            .context("Failed to send usage API request")?;

        let status = resp.status();
        if !status.is_success() {
            let retry_after = resp.headers().get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let body = resp.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                warn!("429 response — retry-after: {:?}, body: {}", retry_after, body);
                bail!(crate::provider::RateLimited {
                    retry_after: parse_retry_after_secs(retry_after.as_deref()),
                });
            }
            if status == reqwest::StatusCode::UNAUTHORIZED {
                warn!("401 response — body: {}", body);
                bail!(crate::provider::Unauthorized);
            }
            bail!("Usage API request failed ({}): {}", status, body);
        }

        resp.json()
            .await
            .context("Failed to parse usage API response")
    }
}

fn parse_reset_time(s: &str) -> Option<DateTime<Utc>> {
    s.parse::<DateTime<Utc>>().ok()
}

/// Parse a `retry-after` header as RFC 9110 delta-seconds. The HTTP-date
/// form and garbage both parse as `None` — the ladder applies unchanged.
/// Capped at the dormant interval (15 min) so a malformed or hostile header
/// can't park the fetch loop for an implausible wait.
fn parse_retry_after_secs(value: Option<&str>) -> Option<std::time::Duration> {
    const CAP_SECS: u64 = 15 * 60;
    let secs = value?.trim().parse::<u64>().ok()?;
    Some(std::time::Duration::from_secs(secs.min(CAP_SECS)))
}

fn sanitize_utilization(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, 100.0)
    } else {
        warn!("Non-finite utilization {} from API; coercing to 0.0", v);
        0.0
    }
}

fn window_from_response(resp: &WindowResponse, label: &str, window_minutes: Option<u32>) -> RateWindow {
    RateWindow {
        label: label.to_string(),
        used_percent: sanitize_utilization(resp.utilization),
        window_minutes,
        resets_at: resp.resets_at.as_deref().and_then(parse_reset_time),
        reset_description: None,
    }
}

/// Model-scoped windows come from the `limits` array when it carries any
/// (the current API shape); the flat `seven_day_*` keys are a fallback for
/// older responses. Never both — an Opus entry present in each would
/// otherwise render twice.
fn build_model_windows(api_resp: &UsageApiResponse) -> Vec<RateWindow> {
    let scoped: Vec<RateWindow> = api_resp
        .limits
        .iter()
        .flatten()
        .filter(|l| l.kind == "weekly_scoped")
        .filter_map(|l| {
            let name = l
                .scope
                .as_ref()?
                .model
                .as_ref()?
                .display_name
                .as_deref()?;
            Some(RateWindow {
                label: format!("{} (7d)", name),
                used_percent: sanitize_utilization(l.percent),
                window_minutes: Some(10080),
                resets_at: l.resets_at.as_deref().and_then(parse_reset_time),
                reset_description: None,
            })
        })
        .collect();
    if !scoped.is_empty() {
        return scoped;
    }

    let legacy_fields: &[(&Option<WindowResponse>, &str)] = &[
        (&api_resp.seven_day_sonnet, "Sonnet (7d)"),
        (&api_resp.seven_day_opus, "Opus Extra (7d)"),
        (&api_resp.seven_day_cowork, "Cowork (7d)"),
        (&api_resp.seven_day_oauth_apps, "OAuth Apps (7d)"),
    ];
    legacy_fields
        .iter()
        .filter_map(|(field, label)| {
            field
                .as_ref()
                .map(|w| window_from_response(w, label, Some(10080)))
        })
        .collect()
}

#[async_trait]
impl Provider for ClaudeProvider {
    fn name(&self) -> &str {
        "Claude"
    }

    async fn fetch_usage(&self) -> Result<UsageSnapshot> {
        let outcome = self.ensure_fresh_credentials().await?;
        let creds = outcome.creds();
        let api_resp = match self.fetch_usage_api(&creds.access_token).await {
            Ok(resp) => resp,
            Err(e) if e.downcast_ref::<crate::provider::RateLimited>().is_some() => {
                if outcome.was_refreshed() {
                    // Token was just refreshed this call — 429 is a genuine
                    // rate-limit, not a stale-token symptom. Burning another
                    // refresh chasing it risks invalidating a single-use RT.
                    return Err(e);
                }
                // Cached token may be stale: Claude Code can rotate
                // credentials on disk under us between our read and the API
                // call. Re-read and force one refresh before propagating.
                info!("Got 429 on cached token, re-reading and refreshing");
                let disk_creds = self.read_credentials()?;
                let fresh = self.refresh_token(&disk_creds).await?;
                self.fetch_usage_api(&fresh.access_token).await?
            }
            Err(e) => return Err(e),
        };

        debug!("Usage API response received");

        let primary = api_resp
            .five_hour
            .as_ref()
            .map(|w| window_from_response(w, "Session (5h)", Some(300)));

        // `seven_day` is the all-models weekly window, not Opus-specific —
        // model-scoped weeklies land in `model_windows` below.
        let secondary = api_resp
            .seven_day
            .as_ref()
            .map(|w| window_from_response(w, "Weekly (7d)", Some(10080)));

        let tertiary = None;

        let model_windows = build_model_windows(&api_resp);

        let extra = api_resp.extra_usage.as_ref().map(|e| ExtraUsage {
            is_enabled: e.is_enabled,
            monthly_limit: e.monthly_limit.unwrap_or(0.0) / 100.0,
            used_credits: e.used_credits.unwrap_or(0.0) / 100.0,
            currency: e.currency.clone().unwrap_or_else(|| "USD".to_string()),
        });

        let identity = AccountIdentity {
            email: None, // usage endpoint doesn't return email
            organization: None,
            plan: creds.subscription_type.clone(),
        };

        if primary.is_none()
            && secondary.is_none()
            && tertiary.is_none()
            && model_windows.is_empty()
            && extra.is_none()
        {
            bail!(crate::provider::EmptyResponse);
        }

        if let Some(ref p) = primary {
            info!(
                "Usage fetched: session={:.0}%, weekly={:.0}%",
                p.used_percent,
                secondary.as_ref().map(|s| s.used_percent).unwrap_or(0.0)
            );
        }

        Ok(UsageSnapshot {
            primary,
            secondary,
            tertiary,
            model_windows,
            extra_usage: extra,
            updated_at: Utc::now(),
            identity: Some(identity),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn make_creds() -> OAuthCredentials {
        OAuthCredentials {
            access_token: "test_access".to_string(),
            refresh_token: "test_refresh".to_string(),
            expires_at: 1_700_000_000_000,
            scopes: None,
            subscription_type: None,
            rate_limit_tier: None,
        }
    }

    fn make_provider(path: &std::path::Path) -> ClaudeProvider {
        let mut config = Config::default();
        config.claude.credentials_path = Some(path.to_string_lossy().into_owned());
        ClaudeProvider::new(&config).expect("provider")
    }

    #[test]
    fn write_credentials_results_in_0600_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");

        fs::write(
            &path,
            r#"{"claudeAiOauth":{"accessToken":"old","refreshToken":"old","expiresAt":0}}"#,
        )
        .expect("seed");
        fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("seed mode");

        make_provider(&path)
            .write_credentials(&make_creds(), "old")
            .expect("write");

        let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {:o}", mode);
    }

    #[test]
    fn unauthorized_sentinel_renders_user_facing_message() {
        assert_eq!(
            crate::provider::Unauthorized.to_string(),
            "Authentication failed — re-login in Claude Code"
        );
    }

    #[test]
    fn empty_response_sentinel_renders_user_facing_message() {
        assert_eq!(
            crate::provider::EmptyResponse.to_string(),
            "Claude usage API returned no data"
        );
    }

    #[test]
    fn model_scoped_limit_entry_becomes_a_model_window() {
        // Real response shape as of 2026-07-05: flat seven_day_* keys null,
        // model-scoped quota only present in the limits array.
        let json = r#"{
            "five_hour": {"utilization": 30.0, "resets_at": null},
            "seven_day": {"utilization": 29.0, "resets_at": null},
            "seven_day_opus": null,
            "limits": [
                {"kind": "session", "group": "session", "percent": 30, "severity": "normal", "resets_at": null, "scope": null, "is_active": false},
                {"kind": "weekly_all", "group": "weekly", "percent": 29, "severity": "normal", "resets_at": null, "scope": null, "is_active": false},
                {"kind": "weekly_scoped", "group": "weekly", "percent": 33, "severity": "normal", "resets_at": "2026-07-07T01:00:00+00:00", "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}, "is_active": true}
            ]
        }"#;
        let resp: UsageApiResponse = serde_json::from_str(json).expect("parse");

        let windows = super::build_model_windows(&resp);

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Fable (7d)");
        assert_eq!(windows[0].used_percent, 33.0);
        assert!(windows[0].resets_at.is_some());
    }

    #[test]
    fn scoped_limits_take_precedence_over_legacy_model_keys() {
        let json = r#"{
            "seven_day_sonnet": {"utilization": 10.0, "resets_at": null},
            "limits": [
                {"kind": "weekly_scoped", "percent": 33, "resets_at": null, "scope": {"model": {"display_name": "Fable"}}}
            ]
        }"#;
        let resp: UsageApiResponse = serde_json::from_str(json).expect("parse");

        let windows = super::build_model_windows(&resp);

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Fable (7d)");
    }

    #[test]
    fn legacy_model_keys_still_render_when_limits_has_no_scoped_entries() {
        let json = r#"{
            "seven_day_sonnet": {"utilization": 10.0, "resets_at": null},
            "limits": [
                {"kind": "session", "percent": 30, "resets_at": null, "scope": null}
            ]
        }"#;
        let resp: UsageApiResponse = serde_json::from_str(json).expect("parse");

        let windows = super::build_model_windows(&resp);

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Sonnet (7d)");
    }

    #[test]
    fn scoped_limit_without_model_name_is_ignored() {
        // A weekly_scoped entry scoped to a surface (or with a null model)
        // has no renderable label — skip it rather than inventing one.
        let json = r#"{
            "limits": [
                {"kind": "weekly_scoped", "percent": 20, "resets_at": null, "scope": {"model": null}},
                {"kind": "weekly_scoped", "percent": 21, "resets_at": null, "scope": null}
            ]
        }"#;
        let resp: UsageApiResponse = serde_json::from_str(json).expect("parse");

        assert!(super::build_model_windows(&resp).is_empty());
    }

    #[test]
    fn parse_retry_after_accepts_delta_seconds() {
        assert_eq!(
            super::parse_retry_after_secs(Some("120")),
            Some(std::time::Duration::from_secs(120))
        );
        assert_eq!(
            super::parse_retry_after_secs(Some(" 30 ")),
            Some(std::time::Duration::from_secs(30))
        );
    }

    #[test]
    fn parse_retry_after_treats_garbage_and_http_dates_as_none() {
        assert_eq!(super::parse_retry_after_secs(Some("soon")), None);
        assert_eq!(
            super::parse_retry_after_secs(Some("Fri, 05 Jul 2026 12:00:00 GMT")),
            None
        );
        assert_eq!(super::parse_retry_after_secs(Some("-5")), None);
        assert_eq!(super::parse_retry_after_secs(None), None);
    }

    #[test]
    fn parse_retry_after_clamps_to_dormant_interval_cap() {
        assert_eq!(
            super::parse_retry_after_secs(Some("86400")),
            Some(std::time::Duration::from_secs(900))
        );
    }

    #[test]
    fn sanitize_utilization_clamps_finite_values() {
        assert_eq!(super::sanitize_utilization(-5.0), 0.0);
        assert_eq!(super::sanitize_utilization(0.0), 0.0);
        assert_eq!(super::sanitize_utilization(50.0), 50.0);
        assert_eq!(super::sanitize_utilization(100.0), 100.0);
        assert_eq!(super::sanitize_utilization(105.7), 100.0);
    }

    #[test]
    fn sanitize_utilization_coerces_nan_and_infinity() {
        assert_eq!(super::sanitize_utilization(f64::NAN), 0.0);
        assert_eq!(super::sanitize_utilization(f64::INFINITY), 0.0);
        assert_eq!(super::sanitize_utilization(f64::NEG_INFINITY), 0.0);
    }

    fn creds_expiring_at(expires_at: u64) -> OAuthCredentials {
        OAuthCredentials {
            access_token: "a".to_string(),
            refresh_token: "r".to_string(),
            expires_at,
            scopes: None,
            subscription_type: None,
            rate_limit_tier: None,
        }
    }

    #[test]
    fn is_expired_returns_true_for_zero_expires_at() {
        // Known CLI failure mode — uninitialized timestamp.
        assert!(ClaudeProvider::is_expired(
            &creds_expiring_at(0),
            1_700_000_000_000
        ));
    }

    #[test]
    fn is_expired_returns_true_for_seconds_stored_as_milliseconds() {
        // 1_700_000_000 seconds (2023-11) below the 2001 ms threshold.
        assert!(ClaudeProvider::is_expired(
            &creds_expiring_at(1_700_000_000),
            1_700_000_000_000
        ));
    }

    #[test]
    fn is_expired_returns_true_for_past_ms_value() {
        // Token expired one hour ago.
        let now = 1_700_000_000_000u64;
        assert!(ClaudeProvider::is_expired(&creds_expiring_at(now - 3_600_000), now));
    }

    #[test]
    fn is_expired_returns_true_for_value_inside_60s_skew_window() {
        // We refresh 60s early to avoid races; 30s in the future is "expired".
        let now = 1_700_000_000_000u64;
        assert!(ClaudeProvider::is_expired(&creds_expiring_at(now + 30_000), now));
    }

    #[test]
    fn is_expired_returns_false_for_value_outside_skew_window() {
        // 5 minutes in the future — not expired.
        let now = 1_700_000_000_000u64;
        assert!(!ClaudeProvider::is_expired(
            &creds_expiring_at(now + 300_000),
            now
        ));
    }

    #[test]
    fn credentials_outcome_distinguishes_cached_from_refreshed() {
        let cached = CredentialsOutcome::Cached(make_creds());
        let refreshed = CredentialsOutcome::Refreshed(make_creds());

        assert!(!cached.was_refreshed());
        assert!(refreshed.was_refreshed());
        assert_eq!(cached.creds().access_token, "test_access");
        assert_eq!(refreshed.creds().access_token, "test_access");
    }

    #[test]
    fn write_credentials_with_externally_rotated_token_aborts_without_clobbering() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        // Disk holds a pair from an external writer (Claude Code rotated
        // credentials during our refresh round-trip) — not the token our
        // refresh consumed ("old").
        let external = r#"{"claudeAiOauth":{"accessToken":"cc_access","refreshToken":"cc_rotated","expiresAt":1800000000000}}"#;
        fs::write(&path, external).expect("seed");

        let err = make_provider(&path)
            .write_credentials(&make_creds(), "old")
            .expect_err("must abort");

        assert!(err.downcast_ref::<ExternalCredentialRotation>().is_some());
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            external,
            "external writer's file must survive untouched"
        );
        assert!(
            !path.with_extension("json.tmp").exists(),
            "aborted write must not leave a temp file behind"
        );
    }

    #[test]
    fn write_credentials_missing_oauth_section_errors_without_writing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        let original = r#"{"otherKey":"keep me"}"#;
        fs::write(&path, original).expect("seed");

        let result = make_provider(&path).write_credentials(&make_creds(), "old");

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&path).expect("read"), original);
    }

    #[test]
    fn persist_after_external_rotation_returns_disk_creds_without_refreshing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(
            &path,
            r#"{"claudeAiOauth":{"accessToken":"cc_access","refreshToken":"cc_rotated","expiresAt":1800000000000}}"#,
        )
        .expect("seed");

        // Our refresh consumed "old", but disk was rotated to "cc_rotated"
        // mid-flight. Recovery must be a re-read — never another refresh —
        // and must discard our now-orphaned pair.
        let effective = make_provider(&path)
            .persist_refreshed_credentials(make_creds(), "old")
            .expect("recovery");

        assert_eq!(effective.access_token, "cc_access");
        assert_eq!(effective.refresh_token, "cc_rotated");
        let on_disk: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("parse");
        assert_eq!(on_disk["claudeAiOauth"]["refreshToken"], "cc_rotated");
    }

    #[test]
    fn persist_with_unrotated_disk_writes_and_returns_our_creds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(
            &path,
            r#"{"claudeAiOauth":{"accessToken":"stale","refreshToken":"old","expiresAt":0}}"#,
        )
        .expect("seed");

        let effective = make_provider(&path)
            .persist_refreshed_credentials(make_creds(), "old")
            .expect("persist");

        assert_eq!(effective.access_token, "test_access");
        let on_disk: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("parse");
        assert_eq!(on_disk["claudeAiOauth"]["refreshToken"], "test_refresh");
    }

    #[test]
    fn write_credentials_preserves_unrelated_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");

        fs::write(
            &path,
            r#"{"claudeAiOauth":{"accessToken":"old","refreshToken":"old","expiresAt":0,"scopes":["a"]},"otherKey":"keep me"}"#,
        )
        .expect("seed");

        make_provider(&path)
            .write_credentials(&make_creds(), "old")
            .expect("write");

        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("parse");
        assert_eq!(value["otherKey"], "keep me");
        assert_eq!(value["claudeAiOauth"]["accessToken"], "test_access");
        assert_eq!(value["claudeAiOauth"]["scopes"][0], "a");
        assert_eq!(value["claudeAiOauth"]["expiresAt"], 1_700_000_000_000u64);
    }
}
