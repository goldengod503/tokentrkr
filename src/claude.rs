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

    fn write_credentials(&self, creds: &OAuthCredentials) -> Result<()> {
        let contents = fs::read_to_string(&self.credentials_path)?;
        let mut raw: serde_json::Value = serde_json::from_str(&contents)?;

        if let Some(oauth) = raw.get_mut("claudeAiOauth") {
            oauth["accessToken"] = serde_json::Value::String(creds.access_token.clone());
            oauth["refreshToken"] = serde_json::Value::String(creds.refresh_token.clone());
            oauth["expiresAt"] = serde_json::Value::Number(creds.expires_at.into());
        }

        let updated = serde_json::to_string_pretty(&raw)?;
        fs::write(&self.credentials_path, updated)?;

        Ok(())
    }

    fn is_expired(creds: &OAuthCredentials) -> bool {
        let now_ms = Utc::now().timestamp_millis() as u64;
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

        self.write_credentials(&new_creds)?;
        info!("Token refreshed successfully");

        Ok(new_creds)
    }

    async fn get_valid_credentials(&self) -> Result<OAuthCredentials> {
        let creds = self.read_credentials()?;

        if Self::is_expired(&creds) {
            debug!("Token expired, refreshing");
            self.refresh_token(&creds).await
        } else {
            Ok(creds)
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
                bail!(crate::RateLimited);
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

fn window_from_response(resp: &WindowResponse, label: &str, window_minutes: Option<u32>) -> RateWindow {
    RateWindow {
        label: label.to_string(),
        used_percent: resp.utilization,
        window_minutes,
        resets_at: resp.resets_at.as_deref().and_then(parse_reset_time),
        reset_description: None,
    }
}

#[async_trait]
impl Provider for ClaudeProvider {
    fn name(&self) -> &str {
        "Claude"
    }

    async fn fetch_usage(&self) -> Result<UsageSnapshot> {
        let creds = self.get_valid_credentials().await?;
        let api_resp = match self.fetch_usage_api(&creds.access_token).await {
            Ok(resp) => resp,
            Err(e) if e.downcast_ref::<crate::RateLimited>().is_some() => {
                // 429 often means a stale token (Claude Code may have refreshed
                // credentials on disk, invalidating our copy). Re-read from disk
                // and force a token refresh before propagating.
                info!("Got 429, re-reading credentials and refreshing token");
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

        let secondary = api_resp
            .seven_day
            .as_ref()
            .map(|w| window_from_response(w, "Weekly (7d)", Some(10080)));

        let tertiary = api_resp
            .seven_day_sonnet
            .as_ref()
            .map(|w| window_from_response(w, "Sonnet (7d)", Some(10080)));

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
            extra_usage: extra,
            updated_at: Utc::now(),
            identity: Some(identity),
        })
    }
}
