use anyhow::Result;
use async_trait::async_trait;
use std::fmt;
use std::time::Duration;

use crate::models::UsageSnapshot;

#[async_trait]
#[allow(dead_code)]
pub trait Provider: Send + Sync {
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

/// Sentinel error for 401 responses. Distinct from `RateLimited` so the
/// polling loop does not retry — credentials are revoked or invalid and
/// the user must re-authenticate.
#[derive(Debug)]
pub struct Unauthorized;

impl fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Authentication failed — re-login in Claude Code")
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
