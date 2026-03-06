use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RateWindow {
    pub label: String,
    pub used_percent: f64,
    pub window_minutes: Option<u32>,
    pub resets_at: Option<DateTime<Utc>>,
    pub reset_description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExtraUsage {
    pub is_enabled: bool,
    pub monthly_limit: f64,
    pub used_credits: f64,
    #[allow(dead_code)]
    pub currency: String,
}

#[derive(Debug, Clone)]
pub struct UsageSnapshot {
    pub primary: Option<RateWindow>,
    pub secondary: Option<RateWindow>,
    pub tertiary: Option<RateWindow>,
    pub extra_usage: Option<ExtraUsage>,
    pub updated_at: DateTime<Utc>,
    pub identity: Option<AccountIdentity>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AccountIdentity {
    pub email: Option<String>,
    pub organization: Option<String>,
    pub plan: Option<String>,
}

// API response types

#[derive(Debug, Deserialize)]
pub struct UsageApiResponse {
    pub five_hour: Option<WindowResponse>,
    pub seven_day: Option<WindowResponse>,
    pub seven_day_sonnet: Option<WindowResponse>,
    pub extra_usage: Option<ExtraUsageResponse>,
}

#[derive(Debug, Deserialize)]
pub struct WindowResponse {
    pub utilization: f64,
    pub resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExtraUsageResponse {
    pub is_enabled: bool,
    pub monthly_limit: Option<f64>,
    pub used_credits: Option<f64>,
    pub currency: Option<String>,
}

// Credential types

#[derive(Debug, Deserialize)]
pub struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    pub claude_ai_oauth: Option<OAuthCredentials>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OAuthCredentials {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
    pub scopes: Option<Vec<String>>,
    #[serde(rename = "subscriptionType")]
    pub subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier")]
    pub rate_limit_tier: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TokenRefreshResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
}

impl RateWindow {
    pub fn format_reset_time(&self) -> String {
        match self.resets_at {
            Some(reset) => {
                let now = Utc::now();
                let duration = reset.signed_duration_since(now);
                if duration.num_seconds() <= 0 {
                    return "Resetting soon...".to_string();
                }
                let hours = duration.num_hours();
                let minutes = duration.num_minutes() % 60;
                if hours > 24 {
                    reset.format("Resets %b %-d, %-I:%M %p").to_string()
                } else if hours > 0 {
                    format!("Resets in {}h {}m", hours, minutes)
                } else {
                    format!("Resets in {}m", minutes)
                }
            }
            None => self
                .reset_description
                .clone()
                .unwrap_or_default(),
        }
    }

    pub fn format_bar(&self, width: usize) -> String {
        let filled = ((self.used_percent / 100.0) * width as f64).round() as usize;
        let filled = filled.min(width);
        let empty = width - filled;
        let bar: String = "▓".repeat(filled) + &"░".repeat(empty);
        bar
    }

    pub fn format_summary(&self) -> String {
        format!("{:<20} {:.0}% used", self.label, self.used_percent)
    }

}
