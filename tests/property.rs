use geronimo_captcha::{
    CaptchaError, CaptchaManager, GenerationOptions, NoiseOptions, SpriteFormat, SpriteUri,
};
use proptest::prelude::*;
use std::thread::sleep;
use std::time::Duration;

fn build_mgr(ttl: u64, cell_size: u32, jpeg_quality: u8) -> CaptchaManager {
    CaptchaManager::new(
        "s".into(),
        ttl,
        NoiseOptions::default(),
        None,
        GenerationOptions {
            cell_size,
            sprite_format: SpriteFormat::Jpeg {
                quality: jpeg_quality,
            },
            limits: None,
        },
    )
}

fn prop_malformed_is_rejected(id: &str) -> bool {
    let mgr = build_mgr(60, 100, 20);
    match mgr.verify_challenge(id, 5) {
        Ok(v) => !v,
        Err(CaptchaError::InvalidInput(_)) => true,
        Err(_) => false,
    }
}

fn prop_ttl_zero_expires(idx: u8) -> bool {
    let mgr = build_mgr(0, 100, 20);
    let ch = mgr.generate_challenge::<SpriteUri>().unwrap();
    let first = mgr.verify_challenge(&ch.challenge_id, idx).unwrap_or(false);

    if first {
        sleep(Duration::from_secs(1));
        !mgr.verify_challenge(&ch.challenge_id, idx).unwrap_or(true)
    } else {
        true
    }
}

fn prop_oob_rejected(idx: u8) -> bool {
    let mgr = build_mgr(60, 100, 20);
    let ch = mgr.generate_challenge::<SpriteUri>().unwrap();

    matches!(
        mgr.verify_challenge(&ch.challenge_id, idx),
        Err(CaptchaError::InvalidInput(_))
    )
}

#[cfg(feature = "test-utils")]
fn prop_correct_index_verifies(cell: u32, q: u8, ttl: u64) -> bool {
    let mgr = build_mgr(ttl.max(1), cell, q);
    let ch = mgr.generate_challenge::<SpriteUri>().unwrap();
    mgr.verify_challenge(&ch.challenge_id, ch.correct_number)
        .unwrap_or(false)
}

#[cfg(feature = "test-utils")]
fn prop_wrong_index_fails(cell: u32, q: u8, ttl: u64) -> bool {
    let mgr = build_mgr(ttl.max(1), cell, q);
    let ch = mgr.generate_challenge::<SpriteUri>().unwrap();

    // pick deterministic wrong index different from correct
    let wrong = if ch.correct_number == 9 {
        1
    } else {
        ch.correct_number + 1
    };

    !mgr.verify_challenge(&ch.challenge_id, wrong)
        .unwrap_or(true)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        .. ProptestConfig::default()
    })]

    #[test]
    fn malformed_challenge_id_is_rejected(s in ".{0,64}") {
        prop_assume!(!s.contains(':'));
        prop_assert!(prop_malformed_is_rejected(&s));
    }

    #[test]
    fn ttl_zero_always_expires(idx in 1u8..=9) {
        prop_assert!(prop_ttl_zero_expires(idx));
    }

    #[test]
    fn oob_index_rejected(idx in prop_oneof![Just(0u8), 10u8..=u8::MAX]) {
        prop_assert!(prop_oob_rejected(idx));
    }

    #[cfg(feature = "test-utils")]
    #[test]
    fn correct_index_verifies(
        cell in 80u32..=200,
        q in prop_oneof![Just(20u8), Just(60u8), Just(90u8)],
        ttl in 1u64..=120,
    ) {
        prop_assert!(prop_correct_index_verifies(cell, q, ttl));
    }

    #[cfg(feature = "test-utils")]
    #[test]
    fn wrong_index_fails(
        cell in 80u32..=200,
        q in prop_oneof![Just(20u8), Just(60u8), Just(90u8)],
        ttl in 1u64..=120,
    ) {
        prop_assert!(prop_wrong_index_fails(cell, q, ttl));
    }
}
