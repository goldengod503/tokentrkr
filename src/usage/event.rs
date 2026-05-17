use std::time::Duration;

use crate::models::UsageSnapshot;

#[derive(Debug, Clone)]
pub enum UsageEvent {
    FetchStarted {
        fetch_id: u64,
    },
    Snapshot {
        fetch_id: u64,
        snapshot: UsageSnapshot,
    },
    TransientError {
        fetch_id: u64,
        message: String,
        retrying_in: Option<Duration>,
    },
    PermanentError {
        fetch_id: u64,
        message: String,
    },
    Stalled,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AccountIdentity, UsageSnapshot};
    use chrono::Utc;

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

    #[test]
    fn variants_construct_with_expected_fields() {
        let _ = UsageEvent::FetchStarted { fetch_id: 0 };
        let _ = UsageEvent::Snapshot {
            fetch_id: 1,
            snapshot: synthetic_snapshot(),
        };
        let _ = UsageEvent::TransientError {
            fetch_id: 2,
            message: "x".into(),
            retrying_in: Some(Duration::from_secs(10)),
        };
        let _ = UsageEvent::PermanentError {
            fetch_id: 3,
            message: "y".into(),
        };
        let _ = UsageEvent::Stalled;
    }
}
