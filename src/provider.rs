use anyhow::Result;
use async_trait::async_trait;

use crate::models::UsageSnapshot;

#[async_trait]
#[allow(dead_code)]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    async fn fetch_usage(&self) -> Result<UsageSnapshot>;
}
