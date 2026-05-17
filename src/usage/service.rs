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
