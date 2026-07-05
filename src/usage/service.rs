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
    /// Test-only: lets the receiver-drop regression test await actual
    /// loop termination via `JoinHandle` instead of yield-counting.
    #[cfg(test)]
    pub(crate) task: tokio::task::JoinHandle<()>,
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

        let _task = tokio::spawn(run_loop(self, event_tx, refresh_rx));

        UsageHandle {
            events: event_rx,
            refresh: refresh_tx,
            #[cfg(test)]
            task: _task,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum State {
    Normal,
    Dormant,
}

#[derive(Debug)]
enum FetchOutcome {
    Success,
    Transient,
    Permanent,
    /// The event receiver was dropped mid-fetch. The caller should stop
    /// the run loop rather than continue making network calls into a
    /// closed channel.
    Aborted,
}

#[derive(Debug, Clone, Copy)]
enum EmitResult {
    Delivered,
    DroppedFull,
    ChannelClosed,
}

async fn run_loop(
    service: UsageService,
    events: mpsc::Sender<UsageEvent>,
    mut refresh_rx: mpsc::Receiver<()>,
) {
    let mut fetch_id: u64 = 0;
    let mut state = State::Normal;

    loop {
        while let Ok(()) = refresh_rx.try_recv() {}

        if matches!(
            emit(&events, UsageEvent::FetchStarted { fetch_id }),
            EmitResult::ChannelClosed
        ) {
            return;
        }
        let outcome = do_one_fetch(&service, fetch_id, &events).await;
        fetch_id += 1;

        if matches!(outcome, FetchOutcome::Aborted) {
            return;
        }

        state = match (state, outcome) {
            (_, FetchOutcome::Permanent) => State::Dormant,
            (State::Dormant, FetchOutcome::Success) => State::Normal,
            (s, _) => s,
        };

        let wait = match state {
            State::Normal => service.poll_interval,
            State::Dormant => service.retry.dormant_interval,
        };

        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = refresh_rx.recv() => {}
        }
    }
}

async fn do_one_fetch(
    service: &UsageService,
    fetch_id: u64,
    events: &mpsc::Sender<UsageEvent>,
) -> FetchOutcome {
    let delays = service.retry.rate_limit_delays;
    for attempt in 0..=delays.len() {
        let fetch_result = match tokio::time::timeout(
            service.retry.fetch_timeout,
            service.provider.fetch_usage(),
        ).await {
            Ok(r) => r,
            Err(_elapsed) => {
                return emit_or_abort(
                    events,
                    UsageEvent::TransientError {
                        fetch_id,
                        message: format!(
                            "Fetch timed out after {}s",
                            service.retry.fetch_timeout.as_secs()
                        ),
                        retrying_in: None,
                    },
                    FetchOutcome::Transient,
                );
            }
        };
        match fetch_result {
            Ok(snapshot) => {
                return emit_or_abort(
                    events,
                    UsageEvent::Snapshot { fetch_id, snapshot },
                    FetchOutcome::Success,
                );
            }
            Err(e) if e.downcast_ref::<crate::provider::RateLimited>().is_some() => {
                let server_hint = e
                    .downcast_ref::<crate::provider::RateLimited>()
                    .and_then(|r| r.retry_after);
                let ladder_step = delays.get(attempt).copied();
                // Honor a server retry-after hint by waiting at least that
                // long, but never less than the ladder step. Once the ladder
                // is exhausted the hint is dropped too: emitting Some(hint)
                // here would misstate the real wait (hint + poll_interval).
                let retrying_in = match (ladder_step, server_hint) {
                    (Some(l), Some(s)) => Some(l.max(s)),
                    (Some(l), None) => Some(l),
                    (None, _) => None,
                };
                // This arm uses raw `emit()` instead of `emit_or_abort` because
                // it must conditionally sleep-and-loop or return Transient
                // *after* the emit — `emit_or_abort` collapses both into a
                // single FetchOutcome and cannot express that.
                if matches!(
                    emit(
                        events,
                        UsageEvent::TransientError {
                            fetch_id,
                            message: format!("{:#}", e),
                            retrying_in,
                        },
                    ),
                    EmitResult::ChannelClosed,
                ) {
                    return FetchOutcome::Aborted;
                }
                if let Some(d) = retrying_in {
                    tokio::time::sleep(d).await;
                } else {
                    return FetchOutcome::Transient;
                }
            }
            Err(e) if e.downcast_ref::<crate::provider::Unauthorized>().is_some() => {
                return emit_or_abort(
                    events,
                    UsageEvent::PermanentError {
                        fetch_id,
                        message: format!("{:#}", e),
                    },
                    FetchOutcome::Permanent,
                );
            }
            Err(e) if e.downcast_ref::<crate::provider::EmptyResponse>().is_some() => {
                // EmptyResponse means the API is structurally responsive but
                // returning no usable data. Treat as Permanent so the state
                // machine drops to the 15-min Dormant retry instead of
                // churning at poll_interval forever.
                return emit_or_abort(
                    events,
                    UsageEvent::PermanentError {
                        fetch_id,
                        message: format!("{:#}", e),
                    },
                    FetchOutcome::Permanent,
                );
            }
            Err(e) => {
                return emit_or_abort(
                    events,
                    UsageEvent::TransientError {
                        fetch_id,
                        message: format!("{:#}", e),
                        retrying_in: None,
                    },
                    FetchOutcome::Transient,
                );
            }
        }
    }
    FetchOutcome::Transient
}

fn emit(tx: &mpsc::Sender<UsageEvent>, event: UsageEvent) -> EmitResult {
    // try_send is intentional: never block the loop on UI backpressure.
    // On Full, surface a Stalled marker (also try_send — may itself be
    // dropped). This is the R2 fix.
    // On Closed, signal the caller so run_loop can stop instead of
    // continuing to make network calls into a dropped receiver.
    match tx.try_send(event) {
        Ok(()) => EmitResult::Delivered,
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            let _ = tx.try_send(UsageEvent::Stalled);
            EmitResult::DroppedFull
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => EmitResult::ChannelClosed,
    }
}

fn emit_or_abort(
    tx: &mpsc::Sender<UsageEvent>,
    event: UsageEvent,
    fallback: FetchOutcome,
) -> FetchOutcome {
    match emit(tx, event) {
        EmitResult::ChannelClosed => FetchOutcome::Aborted,
        _ => fallback,
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
        RateLimitedWithHint(Duration),
        Unauthorized,
        EmptyResponse,
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
                MockOutcome::RateLimited => {
                    bail!(crate::provider::RateLimited { retry_after: None })
                }
                MockOutcome::RateLimitedWithHint(d) => {
                    bail!(crate::provider::RateLimited { retry_after: Some(d) })
                }
                MockOutcome::Unauthorized => bail!(crate::provider::Unauthorized),
                MockOutcome::EmptyResponse => bail!(crate::provider::EmptyResponse),
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
        assert!(e1.downcast_ref::<crate::provider::RateLimited>().is_some());
        let e2 = mock.fetch_usage().await.unwrap_err();
        assert!(e2.downcast_ref::<crate::provider::Unauthorized>().is_some());
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

    #[tokio::test(start_paused = true)]
    async fn server_retry_after_hint_above_ladder_step_stretches_first_wait() {
        let mock = MockProvider::new(vec![
            MockOutcome::RateLimitedWithHint(Duration::from_secs(120)),
            MockOutcome::Ok,
        ]);
        let service = UsageService::new(mock.clone(), Duration::from_secs(300));
        let mut handle = service.spawn();

        // FetchStarted
        let _ = handle.events.recv().await.unwrap();
        // The hint (120s) beats the first ladder step (10s).
        match handle.events.recv().await.unwrap() {
            UsageEvent::TransientError { retrying_in: Some(d), .. } => {
                assert_eq!(d, Duration::from_secs(120));
            }
            other => panic!("expected TransientError(Some(120s)), got {:?}", other),
        }

        // The ladder step alone would have elapsed here — no retry yet.
        tokio::time::advance(Duration::from_secs(11)).await;
        assert_eq!(mock.call_count(), 1, "retried before the server-requested wait");

        tokio::time::advance(Duration::from_secs(110)).await;
        match handle.events.recv().await.unwrap() {
            UsageEvent::Snapshot { fetch_id: 0, .. } => {}
            other => panic!("expected Snapshot after honoring hint, got {:?}", other),
        }
        assert_eq!(mock.call_count(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn server_retry_after_hint_below_ladder_step_keeps_ladder_wait() {
        let mock = MockProvider::new(vec![
            MockOutcome::RateLimitedWithHint(Duration::from_secs(2)),
            MockOutcome::Ok,
        ]);
        let service = UsageService::new(mock, Duration::from_secs(300));
        let mut handle = service.spawn();

        let _ = handle.events.recv().await.unwrap();

        // max(ladder 10s, hint 2s) — the ladder floor wins.
        match handle.events.recv().await.unwrap() {
            UsageEvent::TransientError { retrying_in: Some(d), .. } => {
                assert_eq!(d, Duration::from_secs(10));
            }
            other => panic!("expected TransientError(Some(10s)), got {:?}", other),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn unauthorized_emits_permanent_error_then_no_normal_poll() {
        let mock = MockProvider::new(vec![
            MockOutcome::Unauthorized,
            MockOutcome::Ok, // would be served if loop kept polling normally
        ]);
        let service = UsageService::new(mock.clone(), Duration::from_secs(300));
        let mut handle = service.spawn();

        // FetchStarted (fetch_id 0)
        let _ = handle.events.recv().await.unwrap();
        // PermanentError
        match handle.events.recv().await.unwrap() {
            UsageEvent::PermanentError { fetch_id: 0, .. } => {}
            other => panic!("expected PermanentError, got {:?}", other),
        }

        // Advance 10 minutes — well past the 5min normal poll, far short of 15min dormant.
        tokio::time::advance(Duration::from_secs(600)).await;

        // No new event should be available — service is dormant.
        let next = tokio::time::timeout(Duration::from_millis(50), handle.events.recv()).await;
        assert!(next.is_err(), "expected no event in dormant state, got {:?}", next);

        // Mock should have been called exactly once.
        assert_eq!(mock.call_count(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn dormant_returns_to_normal_after_15min_success() {
        let mock = MockProvider::new(vec![MockOutcome::Unauthorized, MockOutcome::Ok]);
        let service = UsageService::new(mock, Duration::from_secs(300));
        let mut handle = service.spawn();

        // First cycle: FetchStarted, PermanentError
        let _ = handle.events.recv().await.unwrap();
        let _ = handle.events.recv().await.unwrap();

        // Advance the 15-min dormant interval.
        tokio::time::advance(Duration::from_secs(15 * 60 + 1)).await;

        // Second cycle: FetchStarted (id=1), Snapshot (id=1)
        match handle.events.recv().await.unwrap() {
            UsageEvent::FetchStarted { fetch_id: 1 } => {}
            other => panic!("expected FetchStarted(1), got {:?}", other),
        }
        match handle.events.recv().await.unwrap() {
            UsageEvent::Snapshot { fetch_id: 1, .. } => {}
            other => panic!("expected Snapshot(1), got {:?}", other),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn manual_refresh_preempts_dormant_wait() {
        let mock = MockProvider::new(vec![MockOutcome::Unauthorized, MockOutcome::Ok]);
        let service = UsageService::new(mock, Duration::from_secs(300));
        let mut handle = service.spawn();

        // First cycle: drain FetchStarted + PermanentError
        let _ = handle.events.recv().await.unwrap();
        let _ = handle.events.recv().await.unwrap();

        // Advance only 30 seconds — far less than the 15min dormant interval.
        tokio::time::advance(Duration::from_secs(30)).await;
        handle.refresh.send(()).await.expect("refresh send");

        // Should fire a new fetch immediately.
        match handle.events.recv().await.unwrap() {
            UsageEvent::FetchStarted { fetch_id: 1 } => {}
            other => panic!("expected FetchStarted(1), got {:?}", other),
        }
        match handle.events.recv().await.unwrap() {
            UsageEvent::Snapshot { fetch_id: 1, .. } => {}
            other => panic!("expected Snapshot(1), got {:?}", other),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fetch_id_increments_monotonically_across_cycles() {
        let mock = MockProvider::new(vec![
            MockOutcome::Ok,
            MockOutcome::Ok,
            MockOutcome::Ok,
        ]);
        let service = UsageService::new(mock, Duration::from_secs(300));
        let mut handle = service.spawn();

        for expected_id in 0u64..3 {
            match handle.events.recv().await.unwrap() {
                UsageEvent::FetchStarted { fetch_id } => assert_eq!(fetch_id, expected_id),
                other => panic!("expected FetchStarted({}), got {:?}", expected_id, other),
            }
            match handle.events.recv().await.unwrap() {
                UsageEvent::Snapshot { fetch_id, .. } => assert_eq!(fetch_id, expected_id),
                other => panic!("expected Snapshot({}), got {:?}", expected_id, other),
            }
            tokio::time::advance(Duration::from_secs(301)).await;
        }
    }

    /// Regression: dropping the event receiver must stop the service loop.
    /// Before the A1 fix in release `2026-05-17_003`, `emit()` swallowed
    /// `TrySendError::Closed` and `run_loop` had no exit condition — after
    /// a COSMIC subscription restart the spawned task continued calling
    /// `fetch_usage` forever against a closed channel.
    ///
    /// This test asserts termination structurally via the spawned task's
    /// `JoinHandle` rather than counting `yield_now()` calls. Any refactor
    /// that adds an `.await` between sleep-wake and the next emit will
    /// still cause this test to fail if the loop fails to terminate within
    /// one second of simulated time.
    #[tokio::test(start_paused = true)]
    async fn run_loop_exits_when_event_receiver_is_dropped() {
        let mock = MockProvider::new(vec![
            MockOutcome::Ok,
            MockOutcome::Ok,
            MockOutcome::Ok,
        ]);
        let service = UsageService::new(mock.clone(), Duration::from_secs(300));
        let mut handle = service.spawn();

        // Drain one fetch cycle so the task is sleeping on the post-fetch
        // select! when we drop the receiver.
        let _ = handle.events.recv().await.expect("FetchStarted");
        let _ = handle.events.recv().await.expect("Snapshot");
        assert_eq!(mock.call_count(), 1);

        drop(handle.events);

        // Advance past the post-fetch sleep so the loop wakes and tries
        // the next FetchStarted emit, which must observe ChannelClosed
        // and return.
        tokio::time::advance(Duration::from_secs(50 * 60)).await;

        tokio::time::timeout(Duration::from_secs(1), handle.task)
            .await
            .expect("run_loop did not exit within 1s of receiver drop")
            .expect("run_loop panicked");

        // Exactly one fetch happened (the pre-drop cycle). The post-wake
        // path observes ChannelClosed on the FetchStarted emit *before*
        // calling `do_one_fetch`, so no second fetch is made.
        assert_eq!(
            mock.call_count(),
            1,
            "expected exactly 1 mock call (pre-drop), got {}",
            mock.call_count()
        );
    }

    /// EmptyResponse must drop the service to Dormant so it stops polling
    /// at the normal cadence. The state machine has no other path to limit
    /// the retry rate for a structurally-responsive-but-empty upstream.
    #[tokio::test(start_paused = true)]
    async fn empty_response_emits_permanent_error_then_no_normal_poll() {
        let mock = MockProvider::new(vec![
            MockOutcome::EmptyResponse,
            MockOutcome::Ok, // would be served if loop kept polling normally
        ]);
        let service = UsageService::new(mock.clone(), Duration::from_secs(300));
        let mut handle = service.spawn();

        // FetchStarted (fetch_id 0)
        let _ = handle.events.recv().await.unwrap();
        // PermanentError
        match handle.events.recv().await.unwrap() {
            UsageEvent::PermanentError { fetch_id: 0, .. } => {}
            other => panic!("expected PermanentError, got {:?}", other),
        }

        // Advance 10 minutes — well past the 5min normal poll, far short of
        // the 15min dormant interval.
        tokio::time::advance(Duration::from_secs(600)).await;

        let next = tokio::time::timeout(Duration::from_millis(50), handle.events.recv()).await;
        assert!(next.is_err(), "expected no event in dormant state, got {:?}", next);

        assert_eq!(mock.call_count(), 1);
    }
}
