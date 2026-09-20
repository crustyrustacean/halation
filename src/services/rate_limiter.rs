// src/services/rate_limiter.rs

// dependencies
use flux_limiter::{FluxLimiter, FluxLimiterConfig, SystemClock};

/// GCRA rate limiter shared by the login and registration forms, keyed by
/// `"<form>:<ip>:<identifier>"`. Wraps flux-limiter's lock-free limiter;
/// one instance lives in app data for the whole server.
pub struct RateLimiter {
    limiter: FluxLimiter<String>,
}

impl RateLimiter {
    /// `rate_per_second` = sustained allowance; `burst_capacity` = how many
    /// may land back-to-back before denials begin. For login: 5 attempts
    /// spread across 15 minutes, burst 5.
    pub fn new(rate_per_second: f64, burst_capacity: f64) -> Result<Self, anyhow::Error> {
        let config = FluxLimiterConfig::new(rate_per_second, burst_capacity);
        let limiter = FluxLimiter::with_config(config, SystemClock)?;
        Ok(Self { limiter })
    }

    pub fn check(&self, client_key: &str) -> RateDecision {
        // Fail closed on limiter errors: a denial is safer than a pass.
        match self.limiter.check_request(client_key.to_string()) {
            Ok(decision) => RateDecision {
                allowed: decision.allowed,
                retry_after_seconds: decision
                    .retry_after_seconds
                    .map(|s| s.ceil().max(1.0) as u64)
                    .unwrap_or(1),
            },
            Err(_) => RateDecision {
                allowed: false,
                retry_after_seconds: 60,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RateDecision {
    pub allowed: bool,
    pub retry_after_seconds: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter() -> RateLimiter {
        // 1 request per second, burst 3 — small numbers for a fast test
        RateLimiter::new(1.0 / 60.0, 3.0).expect("Failed to build rate limiter")
    }

    #[tokio::test]
    async fn allows_burst_plus_one_then_denies() {
        // Arrange — GCRA semantics: burst capacity is allowance *beyond the
        // first* request, so burst 3 admits 4 back-to-back, denies the 5th.
        let limiter = limiter();

        // Act
        let outcomes: Vec<RateDecision> = (0..6)
            .map(|_| limiter.check("login:10.0.0.1:alice"))
            .collect();

        // Assert
        for (i, decision) in outcomes.iter().take(4).enumerate() {
            assert!(decision.allowed, "request {} should pass the burst", i + 1);
        }
        for (i, decision) in outcomes.iter().enumerate().skip(4) {
            assert!(!decision.allowed, "request {} is past the burst", i + 1);
            assert!(
                decision.retry_after_seconds >= 1,
                "denials carry a Retry-After value"
            );
        }
    }

    #[tokio::test]
    async fn keys_are_independent() {
        // Arrange
        let limiter = limiter();

        // Act — exhaust alice's burst
        for _ in 0..3 {
            limiter.check("login:10.0.0.1:alice");
        }
        let bob = limiter.check("login:10.0.0.2:bob");

        // Assert — bob's budget is untouched
        assert!(bob.allowed, "different keys never share a budget");
    }
}
