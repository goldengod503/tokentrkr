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

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    pub fn spawn(self) -> UsageHandle {
        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let (refresh_tx, refresh_rx) = mpsc::channel(REFRESH_CHANNEL_CAPACITY);

        tokio::spawn(run_loop(self, event_tx, refresh_rx));

        UsageHandle {
            events: event_rx,
            refresh: refresh_tx,
        }
    }
}

async fn run_loop(
    service: UsageService,
    events: mpsc::Sender<UsageEvent>,
    mut refresh_rx: mpsc::Receiver<()>,
) {
    let mut fetch_id: u64 = 0;

    loop {
        while let Ok(()) = refresh_rx.try_recv() {}

        emit(&events, UsageEvent::FetchStarted { fetch_id });
        do_one_fetch(&service, fetch_id, &events).await;
        fetch_id += 1;

        tokio::select! {
            _ = tokio::time::sleep(service.poll_interval) => {}
            _ = refresh_rx.recv() => {}
        }
    }
}

/// Execute one fetch cycle including the 429 retry ladder.
/// Emits TransientError/Snapshot as appropriate.
async fn do_one_fetch(
    service: &UsageService,
    fetch_id: u64,
    events: &mpsc::Sender<UsageEvent>,
) {
    let delays = service.retry.rate_limit_delays;
    // attempts = initial + each ladder step
    for attempt in 0..=delays.len() {
        match service.provider.fetch_usage().await {
            Ok(snapshot) => {
                emit(events, UsageEvent::Snapshot { fetch_id, snapshot });
                return;
            }
            Err(e) if e.downcast_ref::<crate::RateLimited>().is_some() => {
                let retrying_in = delays.get(attempt).copied();
                emit(
                    events,
                    UsageEvent::TransientError {
                        fetch_id,
                        message: format!("{:#}", e),
                        retrying_in,
                    },
                );
                if let Some(d) = retrying_in {
                    tokio::time::sleep(d).await;
                } else {
                    // Ladder exhausted; fall through to normal interval wait.
                    return;
                }
            }
            Err(e) => {
                emit(
                    events,
                    UsageEvent::TransientError {
                        fetch_id,
                        message: format!("{:#}", e),
                        retrying_in: None,
                    },
                );
                return;
            }
        }
    }
}

fn emit(tx: &mpsc::Sender<UsageEvent>, event: UsageEvent) {
    // try_send is intentional: never block the loop on UI backpressure.
    // On Full, surface a Stalled marker (also try_send — may itself be
    // dropped). This is the R2 fix.
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        let _ = tx.try_send(UsageEvent::Stalled);
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

    #[tokio::test(start_paused = true)]
    async fn retry_ladder_emits_three_transient_then_snapshot() {
        let mock = MockProvider::new(vec![
            MockOutcome::RateLimited,
            MockOutcome::RateLimited,
            MockOutcome::RateLimited,
            MockOutcome::Ok,
        ]);
        let service = UsageService::new(mock, Duration::from_secs(300));
        let mut handle = service.spawn();

        // FetchStarted
        assert!(matches!(
            handle.events.recv().await.unwrap(),
            UsageEvent::FetchStarted { fetch_id: 0 }
        ));

        for expected_delay in [Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(60)] {
            match handle.events.recv().await.unwrap() {
                UsageEvent::TransientError { fetch_id: 0, retrying_in: Some(d), .. } => {
                    assert_eq!(d, expected_delay);
                }
                other => panic!("expected TransientError(retrying_in=Some({:?})), got {:?}", expected_delay, other),
            }
            // Advance past the retry delay so the loop's sleep returns.
            tokio::time::advance(expected_delay + Duration::from_millis(10)).await;
        }

        match handle.events.recv().await.unwrap() {
            UsageEvent::Snapshot { fetch_id: 0, .. } => {}
            other => panic!("expected Snapshot, got {:?}", other),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn normal_happy_path_emits_fetch_started_then_snapshot() {
        let mock = MockProvider::new(vec![MockOutcome::Ok]);
        let service = UsageService::new(mock, Duration::from_secs(300));
        let mut handle = service.spawn();

        let e1 = handle.events.recv().await.expect("FetchStarted");
        match e1 {
            UsageEvent::FetchStarted { fetch_id } => assert_eq!(fetch_id, 0),
            other => panic!("expected FetchStarted, got {:?}", other),
        }

        let e2 = handle.events.recv().await.expect("Snapshot");
        match e2 {
            UsageEvent::Snapshot { fetch_id, .. } => assert_eq!(fetch_id, 0),
            other => panic!("expected Snapshot, got {:?}", other),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn retry_ladder_exhausted_emits_final_transient_with_none() {
        let mock = MockProvider::new(vec![
            MockOutcome::RateLimited,
            MockOutcome::RateLimited,
            MockOutcome::RateLimited,
            MockOutcome::RateLimited,
        ]);
        let service = UsageService::new(mock, Duration::from_secs(300));
        let mut handle = service.spawn();

        // FetchStarted
        let _ = handle.events.recv().await.unwrap();

        for expected_delay in [Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(60)] {
            match handle.events.recv().await.unwrap() {
                UsageEvent::TransientError { retrying_in: Some(d), .. } => {
                    assert_eq!(d, expected_delay);
                }
                other => panic!("expected TransientError Some, got {:?}", other),
            }
            tokio::time::advance(expected_delay + Duration::from_millis(10)).await;
        }

        match handle.events.recv().await.unwrap() {
            UsageEvent::TransientError { retrying_in: None, .. } => {}
            other => panic!("expected TransientError(None), got {:?}", other),
        }
    }
}
