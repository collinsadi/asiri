//! In-memory token buckets per device (architecture §9.8). Counters reset on restart, which is
//! acceptable: the limits exist to blunt abuse by a compromised device, not to bill anyone.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::ids::Id;

/// What is being limited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// `Send` frames.
    Send,
    /// `FetchKeyPackage` frames.
    KeyPackageFetch,
    /// `AuthorizeDevice` frames.
    Authorize,
    /// Blob uploads (chunks).
    BlobChunk,
    /// `CallToken` frames.
    CallToken,
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    updated: Instant,
}

/// A set of token buckets keyed by device and kind.
#[derive(Debug, Default)]
pub struct RateLimiter {
    rules: HashMap<Kind, (f64, Duration)>,
    buckets: Mutex<HashMap<(Id, Kind), Bucket>>,
}

impl RateLimiter {
    /// Creates a limiter with no rules (everything allowed).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allows `capacity` events per `period` for `kind`, refilling continuously.
    #[must_use]
    pub fn with_rule(mut self, kind: Kind, capacity: u32, period: Duration) -> Self {
        self.rules.insert(kind, (f64::from(capacity), period));
        self
    }

    /// Consumes one token; returns `false` when the device is over its limit.
    pub fn check(&self, device: &Id, kind: Kind) -> bool {
        let Some(&(capacity, period)) = self.rules.get(&kind) else {
            return true;
        };
        let now = Instant::now();
        let Ok(mut buckets) = self.buckets.lock() else {
            return false;
        };
        // Bound memory: forget idle buckets when the table grows.
        if buckets.len() > 10_000 {
            buckets.retain(|_, b| now.duration_since(b.updated) < period);
        }
        let bucket = buckets.entry((*device, kind)).or_insert(Bucket {
            tokens: capacity,
            updated: now,
        });
        let refill = now.duration_since(bucket.updated).as_secs_f64() / period.as_secs_f64();
        bucket.tokens = (bucket.tokens + refill * capacity).min(capacity);
        bucket.updated = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_per_device_and_kind() {
        let rl = RateLimiter::new().with_rule(Kind::Send, 3, Duration::from_secs(60));
        let a = [1u8; 16];
        let b = [2u8; 16];
        assert!((0..3).all(|_| rl.check(&a, Kind::Send)));
        assert!(!rl.check(&a, Kind::Send));
        assert!(rl.check(&b, Kind::Send), "other devices are unaffected");
        assert!(
            rl.check(&a, Kind::CallToken),
            "kinds without rules are unlimited"
        );
    }
}
