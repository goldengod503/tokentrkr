use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub rate_limit_delays: &'static [Duration],
    pub fetch_timeout: Duration,
    pub dormant_interval: Duration,
}

const DEFAULT_RATE_LIMIT_DELAYS: &[Duration] = &[
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(60),
];

impl RetryPolicy {
    pub fn default_for_tokentrkr() -> Self {
        Self {
            rate_limit_delays: DEFAULT_RATE_LIMIT_DELAYS,
            fetch_timeout: Duration::from_secs(120),
            dormant_interval: Duration::from_secs(15 * 60),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_has_three_step_ladder() {
        let p = RetryPolicy::default_for_tokentrkr();
        assert_eq!(p.rate_limit_delays.len(), 3);
        assert_eq!(p.rate_limit_delays[0], Duration::from_secs(10));
        assert_eq!(p.rate_limit_delays[1], Duration::from_secs(30));
        assert_eq!(p.rate_limit_delays[2], Duration::from_secs(60));
    }

    #[test]
    fn default_policy_has_120s_fetch_timeout() {
        assert_eq!(
            RetryPolicy::default_for_tokentrkr().fetch_timeout,
            Duration::from_secs(120)
        );
    }

    #[test]
    fn default_policy_has_15min_dormant_interval() {
        assert_eq!(
            RetryPolicy::default_for_tokentrkr().dormant_interval,
            Duration::from_secs(900)
        );
    }
}
