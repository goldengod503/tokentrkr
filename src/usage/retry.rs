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
            // reqwest's 30s client timeout (claude.rs) is the primary
            // authority for production fetches; this outer timeout is a
            // backstop for a Provider that lacks its own (MockProvider in
            // tests, or a future impl that forgets a client timeout). Keep
            // it above the reqwest ceiling or it silently takes over.
            fetch_timeout: Duration::from_secs(45),
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
    fn default_fetch_timeout_is_a_backstop_above_the_reqwest_ceiling() {
        // The 30s reqwest client timeout (claude.rs) must fire first in
        // production; if this assertion forces you to lower fetch_timeout
        // toward 30s (or you're raising the reqwest side past 45s), you're
        // about to invert which layer owns timeouts — see ARCHITECTURE.md
        // "Single timeout authority".
        let p = RetryPolicy::default_for_tokentrkr();
        assert_eq!(p.fetch_timeout, Duration::from_secs(45));
        assert!(p.fetch_timeout > Duration::from_secs(30));
    }

    #[test]
    fn default_policy_has_15min_dormant_interval() {
        assert_eq!(
            RetryPolicy::default_for_tokentrkr().dormant_interval,
            Duration::from_secs(900)
        );
    }
}
