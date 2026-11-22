use anyhow::Result;
use async_trait::async_trait;
use std::fmt;
use std::time::Duration;

use crate::models::UsageSnapshot;

#[async_trait]
pub trait Provider: Send + Sync {
    #[allow(dead_code)]
    fn name(&self) -> &str;
    async fn fetch_usage(&self) -> Result<UsageSnapshot>;
}

/// Sentinel error for 429 responses so the polling loop can retry with backoff.
/// Carries the server's `retry-after` hint when one was sent (delta-seconds
/// form only, capped at parse time); the retry ladder takes
/// `max(ladder_step, hint)` so a server asking for a longer wait is honored.
#[derive(Debug)]
pub struct RateLimited {
    pub retry_after: Option<Duration>,
}

impl fmt::Display for RateLimited {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Rate limited (429)")
    }
}

impl std::error::Error for RateLimited {}

/// Sentinel error for a rejected sign-in: a 401 from the usage API or a 4xx
/// (other than 429) from the token-refresh endpoint. Distinct from
/// `RateLimited` so the polling loop does not retry — the credentials are
/// unusable until the user re-authenticates in Claude Code.
#[derive(Debug)]
pub struct Unauthorized;

impl fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Signed out of Claude — run /login in Claude Code, then click Refresh")
    }
}

impl std::error::Error for Unauthorized {}

/// Sentinel error for a structurally valid but semantically empty usage
/// response (every field None / empty). Surfaced instead of silently
/// writing 0% to the history chart.
#[derive(Debug)]
pub struct EmptyResponse;

impl fmt::Display for EmptyResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Claude usage API returned no data")
    }
}

impl std::error::Error for EmptyResponse {}
