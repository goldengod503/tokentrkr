use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::provider::Provider;
use crate::usage::event::UsageEvent;
use crate::usage::retry::RetryPolicy;

const EVENT_CHANNEL_CAPACITY: usize = 8;
const REFRESH_CHANNEL_CAPACITY: usize = 4;

pub struct UsageHandle {
    pub events: mpsc::Receiver<UsageEvent>,
    pub refresh: mpsc::Sender<()>,
}

pub struct UsageService {
    provider: Arc<dyn Provider>,
    poll_interval: Duration,
    retry: RetryPolicy,
}

impl UsageService {
    pub fn new(provider: Arc<dyn Provider>, poll_interval: Duration) -> Self {
        Self {
            provider,
            poll_interval,
            retry: RetryPolicy::default_for_tokentrkr(),
        }
    }

    #[allow(dead_code)]
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use anyhow::{bail, Result};
    use async_trait::async_trait;
    use chrono::Utc;

    use crate::models::{AccountIdentity, UsageSnapshot};

    /// Scripted Provider: returns the next item from `script` on each call.
    /// Each entry is either `Ok(())` (becomes a synthetic Snapshot) or
    /// `Err(kind)` where kind names the sentinel to raise.
    pub(super) struct MockProvider {
        script: Mutex<Vec<MockOutcome>>,
        call_count: Mutex<usize>,
    }

    #[derive(Debug, Clone)]
    pub(super) enum MockOutcome {
        Ok,
        RateLimited,
        Unauthorized,
        Other(&'static str),
    }

    impl MockProvider {
        pub fn new(script: Vec<MockOutcome>) -> Arc<Self> {
            Arc::new(Self {
                script: Mutex::new(script),
                call_count: Mutex::new(0),
            })
        }

        pub fn call_count(&self) -> usize {
            *self.call_count.lock().unwrap()
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }

        async fn fetch_usage(&self) -> Result<UsageSnapshot> {
            *self.call_count.lock().unwrap() += 1;
            let mut script = self.script.lock().unwrap();
            let outcome = if script.is_empty() {
                MockOutcome::Other("script_exhausted")
            } else {
                script.remove(0)
            };
            drop(script);

            match outcome {
                MockOutcome::Ok => Ok(synthetic_snapshot()),
                MockOutcome::RateLimited => bail!(crate::RateLimited),
                MockOutcome::Unauthorized => bail!(crate::Unauthorized),
                MockOutcome::Other(msg) => bail!("{}", msg),
            }
        }
    }

    fn synthetic_snapshot() -> UsageSnapshot {
        UsageSnapshot {
            primary: None,
            secondary: None,
            tertiary: None,
            model_windows: vec![],
            extra_usage: None,
            updated_at: Utc::now(),
            identity: Some(AccountIdentity {
                email: None,
                organization: None,
                plan: None,
            }),
        }
    }

    #[tokio::test]
    async fn mock_returns_scripted_results_in_order() {
        let mock = MockProvider::new(vec![
            MockOutcome::Ok,
            MockOutcome::RateLimited,
            MockOutcome::Unauthorized,
        ]);

        assert!(mock.fetch_usage().await.is_ok());
        let e1 = mock.fetch_usage().await.unwrap_err();
        assert!(e1.downcast_ref::<crate::RateLimited>().is_some());
        let e2 = mock.fetch_usage().await.unwrap_err();
        assert!(e2.downcast_ref::<crate::Unauthorized>().is_some());
        assert_eq!(mock.call_count(), 3);
    }
}
