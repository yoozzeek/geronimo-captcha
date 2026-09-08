use crate::error::{CaptchaError, RegistryRejection, Result};
use crate::utils::{MAX_TTL_SECS, get_timestamp};

use dashmap::DashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use tracing::warn;

/// Implementations must make `begin_attempt`
/// and `mark_verified` atomic per id.
pub trait ChallengeRegistry: Send + Sync {
    /// # Errors
    ///
    /// Returns [`RegistryRejection::AtCapacity`] when the
    /// registry is full. The caller must discard the challenge.
    fn register(&self, id: &str) -> Result<()>;

    /// Every `Ok` counts toward the attempt limit,
    /// whether or not the guess is correct.
    fn begin_attempt(&self, id: &str) -> Result<()>;

    /// Exactly one caller succeeds per id.
    ///
    /// # Errors
    ///
    /// [`RegistryRejection::AlreadyVerified`] on a repeat,
    /// [`RegistryRejection::NotRegistered`] on an unknown id.
    fn mark_verified(&self, id: &str) -> Result<()>;
}

struct ChallengeStatus {
    verified: bool,
    attempts_count: u16,
    timestamp: u64,
}

// Invariant: `len == ttl + 1` and `pos == now % len`.
// `schedule_expiry` indexes buckets by wall clock.
struct Wheel {
    buckets: Vec<Vec<String>>,
    pos: usize,
    last_tick: u64,
    len: usize,
}

pub struct ChallengeInMemoryRegistry {
    cache: DashMap<String, ChallengeStatus>,
    max_attempts: u16,
    max_entries: usize,
    ttl: u64,
    wheel: Mutex<Wheel>,
}

impl ChallengeInMemoryRegistry {
    /// `ttl` is in seconds. At `max_entries`
    /// further ids are refused, not evicted.
    ///
    /// # Errors
    ///
    /// Returns [`CaptchaError::InvalidInput`] if `ttl` is outside
    /// 1..=86400, or if `max_attempts` or `max_entries` is zero.
    pub fn new(ttl: u64, max_attempts: u16, max_entries: usize) -> Result<Self> {
        Self::new_at(ttl, max_attempts, max_entries, get_timestamp()?)
    }

    fn new_at(ttl: u64, max_attempts: u16, max_entries: usize, now: u64) -> Result<Self> {
        if ttl == 0 || ttl > MAX_TTL_SECS {
            return Err(CaptchaError::InvalidInput(format!(
                "registry ttl {ttl} outside 1..={MAX_TTL_SECS}"
            )));
        }

        if max_attempts == 0 {
            return Err(CaptchaError::InvalidInput(
                "registry max_attempts must be non-zero".into(),
            ));
        }

        if max_entries == 0 {
            return Err(CaptchaError::InvalidInput(
                "registry max_entries must be non-zero".into(),
            ));
        }

        let len = ttl as usize + 1;

        Ok(Self {
            cache: DashMap::new(),
            max_attempts,
            max_entries,
            ttl,
            wheel: Mutex::new(Wheel {
                buckets: vec![Vec::new(); len],
                pos: (now % len as u64) as usize,
                last_tick: now,
                len,
            }),
        })
    }

    fn expiry_bucket(&self, registered_at: u64, len: usize) -> usize {
        registered_at
            .saturating_add(self.ttl)
            .saturating_add(1)
            .rem_euclid(len as u64) as usize
    }

    fn lock_wheel(&self) -> MutexGuard<'_, Wheel> {
        self.wheel.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn advance_wheel(&self, now: u64) {
        let mut w = self.lock_wheel();
        if now <= w.last_tick {
            return;
        }

        let steps = (now - w.last_tick).min(w.len as u64) as usize;
        for _ in 0..steps {
            w.pos = (w.pos + 1) % w.len;

            let pos = w.pos;
            for id in std::mem::take(&mut w.buckets[pos]) {
                let registered_at = self.cache.get(&id).map(|cs| cs.timestamp);

                match registered_at {
                    Some(ts) if now.saturating_sub(ts) > self.ttl => {
                        self.cache.remove(&id);
                    }
                    Some(ts) => {
                        let target = self.expiry_bucket(ts, w.len);
                        w.buckets[target].push(id);
                    }
                    None => {}
                }
            }
        }

        w.pos = (now % w.len as u64) as usize;
        w.last_tick = now;
    }

    fn schedule_expiry(&self, id: &str, now: u64) {
        let mut w = self.lock_wheel();

        let target = self.expiry_bucket(now, w.len);
        w.buckets[target].push(id.to_string());
    }

    fn register_at(&self, id: &str, now: u64) -> Result<()> {
        self.advance_wheel(now);

        if self.cache.len() >= self.max_entries && !self.cache.contains_key(id) {
            warn!(
                max_entries = self.max_entries,
                "challenge registry at capacity, refusing to register"
            );

            return Err(RegistryRejection::AtCapacity.into());
        }

        self.cache.insert(
            id.to_string(),
            ChallengeStatus {
                verified: false,
                attempts_count: 0,
                timestamp: now,
            },
        );

        self.schedule_expiry(id, now);

        Ok(())
    }

    fn begin_attempt_at(&self, id: &str, now: u64) -> Result<()> {
        self.advance_wheel(now);

        let Some(mut cs) = self.cache.get_mut(id) else {
            return Err(RegistryRejection::NotRegistered.into());
        };

        if now.saturating_sub(cs.timestamp) > self.ttl {
            return Err(RegistryRejection::NotRegistered.into());
        }

        if cs.verified {
            return Err(RegistryRejection::AlreadyVerified.into());
        }

        if cs.attempts_count >= self.max_attempts {
            return Err(RegistryRejection::MaxAttemptsLimitExceeded.into());
        }

        cs.attempts_count += 1;

        Ok(())
    }
}

impl ChallengeRegistry for ChallengeInMemoryRegistry {
    fn register(&self, id: &str) -> Result<()> {
        self.register_at(id, get_timestamp()?)
    }

    fn begin_attempt(&self, id: &str) -> Result<()> {
        self.begin_attempt_at(id, get_timestamp()?)
    }

    fn mark_verified(&self, id: &str) -> Result<()> {
        let Some(mut cs) = self.cache.get_mut(id) else {
            return Err(RegistryRejection::NotRegistered.into());
        };

        if cs.verified {
            return Err(RegistryRejection::AlreadyVerified.into());
        }

        cs.verified = true;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    const DEFAULT_TTL: u64 = 60;
    const MAX_ENTRIES: usize = 10_000;

    fn registry(ttl: u64, max_attempts: u16) -> ChallengeInMemoryRegistry {
        ChallengeInMemoryRegistry::new(ttl, max_attempts, MAX_ENTRIES).expect("valid registry")
    }

    fn rejection(result: Result<()>) -> Option<RegistryRejection> {
        match result {
            Ok(()) => None,
            Err(CaptchaError::Registry(r)) => Some(r),
            Err(other) => panic!("unexpected error: {other}"),
        }
    }

    fn count_ok(handles: Vec<thread::JoinHandle<Result<()>>>) -> usize {
        handles
            .into_iter()
            .map(|h| h.join().expect("thread panicked"))
            .filter(Result::is_ok)
            .count()
    }

    #[test]
    fn register_and_begin_attempt() {
        let registry = registry(DEFAULT_TTL, 1);

        assert_eq!(rejection(registry.register("challenge-123")), None);
        assert_eq!(rejection(registry.begin_attempt("challenge-123")), None);
    }

    #[test]
    fn unregistered_id_is_refused() {
        let registry = registry(DEFAULT_TTL, 1);

        assert_eq!(
            rejection(registry.begin_attempt("challenge-123")),
            Some(RegistryRejection::NotRegistered)
        );
        assert_eq!(
            rejection(registry.mark_verified("challenge-123")),
            Some(RegistryRejection::NotRegistered)
        );
    }

    #[test]
    fn verified_id_refuses_attempts_and_repeat_marks() {
        let registry = registry(DEFAULT_TTL, 3);
        registry.register("challenge-123").expect("register");

        assert_eq!(rejection(registry.mark_verified("challenge-123")), None);
        assert_eq!(
            rejection(registry.begin_attempt("challenge-123")),
            Some(RegistryRejection::AlreadyVerified)
        );
        assert_eq!(
            rejection(registry.mark_verified("challenge-123")),
            Some(RegistryRejection::AlreadyVerified)
        );
    }

    #[test]
    fn every_begin_attempt_counts() {
        let registry = registry(DEFAULT_TTL, 2);
        registry.register("challenge-123").expect("register");

        assert_eq!(rejection(registry.begin_attempt("challenge-123")), None);
        assert_eq!(rejection(registry.begin_attempt("challenge-123")), None);
        assert_eq!(
            rejection(registry.begin_attempt("challenge-123")),
            Some(RegistryRejection::MaxAttemptsLimitExceeded)
        );
    }

    #[test]
    fn new_rejects_invalid_arguments() {
        assert!(ChallengeInMemoryRegistry::new(0, 3, 100).is_err());
        assert!(ChallengeInMemoryRegistry::new(MAX_TTL_SECS + 1, 3, 100).is_err());
        assert!(ChallengeInMemoryRegistry::new(DEFAULT_TTL, 0, 100).is_err());
        assert!(ChallengeInMemoryRegistry::new(DEFAULT_TTL, 3, 0).is_err());
        assert!(ChallengeInMemoryRegistry::new(DEFAULT_TTL, 3, 100).is_ok());
    }

    #[test]
    fn capacity_bound_refuses_new_ids() {
        let reg = ChallengeInMemoryRegistry::new_at(DEFAULT_TTL, 3, 2, 1_000).expect("registry");

        assert_eq!(rejection(reg.register_at("a", 1_000)), None);
        assert_eq!(rejection(reg.register_at("b", 1_000)), None);
        assert_eq!(
            rejection(reg.register_at("c", 1_000)),
            Some(RegistryRejection::AtCapacity)
        );

        assert_eq!(reg.cache.len(), 2, "capacity bound not enforced");
        assert_eq!(
            rejection(reg.begin_attempt_at("c", 1_000)),
            Some(RegistryRejection::NotRegistered)
        );
    }

    #[test]
    fn entry_lives_until_ttl_inclusive() {
        let start = 1_000;
        let reg = ChallengeInMemoryRegistry::new_at(DEFAULT_TTL, 3, MAX_ENTRIES, start)
            .expect("registry");

        reg.register_at("a", start).expect("register");

        assert_eq!(
            rejection(reg.begin_attempt_at("a", start + DEFAULT_TTL)),
            None,
            "evicted while the mac is still inside its ttl"
        );
        assert_eq!(
            rejection(reg.begin_attempt_at("a", start + DEFAULT_TTL + 1)),
            Some(RegistryRejection::NotRegistered)
        );
    }

    #[test]
    fn wheel_position_tracks_wall_clock_after_long_gap() {
        let start = 1_000;
        let reg = ChallengeInMemoryRegistry::new_at(DEFAULT_TTL, 3, MAX_ENTRIES, start)
            .expect("registry");

        for gap in [1, 30, 59, 60, 61, 100, 3_600, 86_400] {
            let now = start + gap;
            reg.advance_wheel(now);

            assert_eq!(
                reg.lock_wheel().pos,
                (now % (DEFAULT_TTL + 1)) as usize,
                "wheel drifted from wall clock after a {gap}s gap"
            );
        }
    }

    #[test]
    fn expiry_survives_idle_gap_longer_than_ttl() {
        let start = 1_000;

        for gap in [61, 100, 119, 3_600, 86_400] {
            let reg = ChallengeInMemoryRegistry::new_at(DEFAULT_TTL, 3, MAX_ENTRIES, start)
                .expect("registry");

            reg.register_at("seed", start).expect("register");

            let mut now = start + gap;
            for i in 0..300 {
                reg.register_at(&format!("post-{i}"), now)
                    .expect("register");
                now += 1;
            }

            reg.advance_wheel(now);

            let leaked = reg
                .cache
                .iter()
                .filter(|cs| now.saturating_sub(cs.timestamp) > DEFAULT_TTL)
                .count();

            assert_eq!(
                leaked, 0,
                "{leaked} expired challenges leaked after a {gap}s idle gap"
            );
        }
    }

    #[test]
    fn concurrent_registration_safe() {
        let registry = Arc::new(registry(DEFAULT_TTL, 1));
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let reg = registry.clone();
                thread::spawn(move || reg.register(&format!("challenge-{i}")).expect("register"))
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        for i in 0..10 {
            assert_eq!(
                rejection(registry.begin_attempt(&format!("challenge-{i}"))),
                None
            );
        }
    }

    #[test]
    fn concurrent_attempts_never_exceed_cap() {
        let registry = Arc::new(registry(DEFAULT_TTL, 3));
        registry.register("shared").expect("register");

        let handles: Vec<_> = (0..16)
            .map(|_| {
                let reg = Arc::clone(&registry);
                thread::spawn(move || reg.begin_attempt("shared"))
            })
            .collect();

        assert_eq!(
            count_ok(handles),
            3,
            "attempt cap bypassed under concurrency"
        );
    }

    #[test]
    fn concurrent_mark_verified_admits_exactly_one() {
        let registry = Arc::new(registry(DEFAULT_TTL, 3));
        registry.register("shared").expect("register");

        let handles: Vec<_> = (0..16)
            .map(|_| {
                let reg = Arc::clone(&registry);
                thread::spawn(move || reg.mark_verified("shared"))
            })
            .collect();

        assert_eq!(
            count_ok(handles),
            1,
            "solved challenge marked verified more than once"
        );
    }
}
