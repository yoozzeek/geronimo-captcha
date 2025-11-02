use crate::utils::get_timestamp;

use dashmap::DashMap;
use std::fmt;
use std::sync::Mutex;

/// This file defines trait for the challenge registry implementation that
/// stores generated challenges in memory or database, checks how
/// many resolving attempts performed to prevent brute-force.
pub trait ChallengeRegistry: Send + Sync {
    fn register(&self, id: &str);
    fn check(&self, id: &str) -> RegistryCheckResult;
    fn verify(&self, id: &str);
    fn note_attempt(&self, id: &str, success: bool);
}

#[derive(PartialEq, Debug)]
pub enum RegistryCheckResult {
    Ok,
    AlreadyVerified,
    NotRegistered,
    MaxAttemptsLimitExceeded,
}

impl fmt::Display for RegistryCheckResult {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", format!("{self:?}").to_uppercase())
    }
}

struct ChallengeStatus {
    verified: bool,
    attempts_count: u16,
    timestamp: u64,
}

struct Wheel {
    buckets: Vec<Vec<String>>, // ids scheduled to expire at bucket index
    pos: usize,                // current bucket index (advances with time)
    last_tick: u64,            // last observed time (secs)
    len: usize,                // buckets length == ttl (secs)
}

pub struct ChallengeInMemoryRegistry {
    cache: DashMap<String, ChallengeStatus>,
    max_attempts: u16,
    ttl: u64,
    wheel: Mutex<Wheel>,
}

impl ChallengeInMemoryRegistry {
    pub fn new(ttl: u64, max_attempts: u16) -> Self {
        let now = get_timestamp();
        let len = ttl.max(1) as usize;
        let pos = (now as usize) % len;
        let wheel = Wheel {
            buckets: vec![Vec::new(); len],
            pos,
            last_tick: now,
            len,
        };

        Self {
            cache: DashMap::new(),
            max_attempts,
            ttl,
            wheel: Mutex::new(wheel),
        }
    }

    fn advance_wheel(&self, now: u64) {
        let mut w = self.wheel.lock().unwrap();
        if now <= w.last_tick {
            return;
        }

        let steps = ((now - w.last_tick) as usize).min(w.len);
        for _ in 0..steps {
            w.pos = (w.pos + 1) % w.len;

            let pos = w.pos;
            let expired_ids = std::mem::take(&mut w.buckets[pos]);

            for id in expired_ids {
                if let Some(cs_ref) = self.cache.get(&id) {
                    let expired = now.saturating_sub(cs_ref.timestamp) >= self.ttl;
                    drop(cs_ref);
                    if expired {
                        let _ = self.cache.remove(&id);
                    }
                } else {
                    // already removed
                }
            }
        }

        w.last_tick = now;
    }

    fn schedule_expiry(&self, id: &str, now: u64) {
        let mut w = self.wheel.lock().unwrap();

        // Schedule at (now + ttl) bucket
        let target = (now + self.ttl) as usize % w.len;
        w.buckets[target].push(id.to_string());
    }
}

impl ChallengeRegistry for ChallengeInMemoryRegistry {
    fn register(&self, id: &str) {
        let now = get_timestamp();
        self.advance_wheel(now);
        self.cache.insert(
            id.to_string(),
            ChallengeStatus {
                verified: false,
                attempts_count: 0,
                timestamp: now,
            },
        );
        self.schedule_expiry(id, now);
    }

    fn check(&self, id: &str) -> RegistryCheckResult {
        let now = get_timestamp();
        self.advance_wheel(now);

        if let Some(challenge_ref) = self.cache.get(id) {
            let cs = challenge_ref.value();
            if cs.verified {
                return RegistryCheckResult::AlreadyVerified;
            }

            if cs.attempts_count >= self.max_attempts {
                return RegistryCheckResult::MaxAttemptsLimitExceeded;
            }

            if now.saturating_sub(cs.timestamp) <= self.ttl {
                return RegistryCheckResult::Ok;
            }
        }

        RegistryCheckResult::NotRegistered
    }

    fn verify(&self, id: &str) {
        if let Some(mut challenge_ref) = self.cache.get_mut(id) {
            let cs = challenge_ref.value_mut();
            cs.verified = true;
        }
    }

    fn note_attempt(&self, id: &str, success: bool) {
        if let Some(mut challenge_ref) = self.cache.get_mut(id) {
            let cs = challenge_ref.value_mut();
            if !success {
                cs.attempts_count = cs.attempts_count.saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const DEFAULT_TTL: u64 = 60;

    #[test]
    fn test_register_and_check() {
        let registry = ChallengeInMemoryRegistry::new(DEFAULT_TTL, 1);
        let challenge_id = "challenge-123";

        registry.register(challenge_id);
        assert_eq!(registry.check(challenge_id), RegistryCheckResult::Ok);
    }

    #[test]
    fn test_check_unregistered() {
        let registry = ChallengeInMemoryRegistry::new(DEFAULT_TTL, 1);
        let challenge_id = "challenge-123";
        assert_eq!(
            registry.check(challenge_id),
            RegistryCheckResult::NotRegistered
        );
    }

    #[test]
    fn test_check_already_verified() {
        let registry = ChallengeInMemoryRegistry::new(DEFAULT_TTL, 1);
        let challenge_id = "challenge-123";
        registry.register(challenge_id);
        registry.verify(challenge_id);
        assert_eq!(
            registry.check(challenge_id),
            RegistryCheckResult::AlreadyVerified
        );
    }

    #[test]
    fn test_check_max_attempts_limit() {
        let registry = ChallengeInMemoryRegistry::new(DEFAULT_TTL, 2);
        let challenge_id = "challenge-123";
        registry.register(challenge_id);

        assert_eq!(registry.check(challenge_id), RegistryCheckResult::Ok);

        registry.note_attempt(challenge_id, false);
        assert_eq!(registry.check(challenge_id), RegistryCheckResult::Ok);

        registry.note_attempt(challenge_id, false);
        assert_eq!(
            registry.check(challenge_id),
            RegistryCheckResult::MaxAttemptsLimitExceeded
        );
    }

    #[test]
    fn test_concurrent_usage_safe() {
        use std::thread;

        let registry = Arc::new(ChallengeInMemoryRegistry::new(DEFAULT_TTL, 1));
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let reg = registry.clone();
                thread::spawn(move || {
                    let challenge_id = format!("challenge-{i}");
                    reg.register(&challenge_id);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        for i in 0..10 {
            let id = format!("challenge-{i}");
            assert_eq!(registry.check(&id), RegistryCheckResult::Ok);
        }
    }
}
