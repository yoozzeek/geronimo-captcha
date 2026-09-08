use geronimo_captcha::{
    CaptchaError, CaptchaManager, DecodeLimits, GenerationOptions, NoiseOptions, SampleSet,
    SpriteFormat, SpriteUri,
};
use proptest::prelude::*;

const SECRET: &str = "property-test-secret-key-32-bytes!!";

fn build_mgr(ttl: u64, cell_size: u32, jpeg_quality: u8, rounds: u8) -> CaptchaManager {
    CaptchaManager::new(
        SECRET.to_string(),
        ttl,
        NoiseOptions::default(),
        None,
        GenerationOptions {
            cell_size,
            sprite_format: SpriteFormat::Jpeg {
                quality: jpeg_quality,
            },
            rounds,
            limits: DecodeLimits::default(),
        },
        &SampleSet::demo_insecure(),
    )
    .expect("valid manager configuration")
}

fn prop_malformed_is_rejected(id: &str) -> bool {
    let mgr = build_mgr(60, 100, 20, 1);

    match mgr.verify_challenge(id, &[5]) {
        Ok(v) => !v,
        Err(CaptchaError::InvalidInput(_)) => true,
        Err(_) => false,
    }
}

fn prop_oob_rejected(idx: u8) -> bool {
    let mgr = build_mgr(60, 100, 20, 1);
    let ch = mgr.generate_challenge::<SpriteUri>().unwrap();

    matches!(
        mgr.verify_challenge(&ch.challenge_id, &[idx]),
        Err(CaptchaError::InvalidInput(_))
    )
}

fn prop_exactly_one_index_verifies(cell: u32, q: u8, ttl: u64) -> bool {
    let mgr = build_mgr(ttl, cell, q, 1);
    let ch = mgr.generate_challenge::<SpriteUri>().unwrap();

    let accepted = (1u8..=9)
        .filter(|&idx| {
            mgr.verify_challenge(&ch.challenge_id, &[idx])
                .unwrap_or(false)
        })
        .count();

    accepted == 1
}

fn prop_wrong_round_count_rejected(rounds: u8, submitted: usize) -> bool {
    let mgr = build_mgr(60, 100, 20, rounds);
    let ch = mgr.generate_challenge::<SpriteUri>().unwrap();

    if ch.sprites.len() != rounds as usize {
        return false;
    }

    matches!(
        mgr.verify_challenge(&ch.challenge_id, &vec![1u8; submitted]),
        Err(CaptchaError::InvalidInput(_))
    )
}

fn prop_short_secret_rejected(secret: &str) -> bool {
    matches!(
        CaptchaManager::new(
            secret.to_string(),
            60,
            NoiseOptions::default(),
            None,
            GenerationOptions::default(),
            &SampleSet::demo_insecure(),
        ),
        Err(CaptchaError::InvalidInput(_))
    )
}

fn prop_out_of_range_ttl_rejected(ttl: u64) -> bool {
    matches!(
        CaptchaManager::new(
            SECRET.to_string(),
            ttl,
            NoiseOptions::default(),
            None,
            GenerationOptions::default(),
            &SampleSet::demo_insecure(),
        ),
        Err(CaptchaError::InvalidInput(_))
    )
}

fn prop_out_of_range_cell_size_rejected(cell_size: u32) -> bool {
    matches!(
        CaptchaManager::new(
            SECRET.to_string(),
            60,
            NoiseOptions::default(),
            None,
            GenerationOptions {
                cell_size,
                sprite_format: SpriteFormat::default(),
                rounds: 1,
                limits: DecodeLimits::default(),
            },
            &SampleSet::demo_insecure(),
        ),
        Err(CaptchaError::InvalidInput(_))
    )
}

fn prop_inverted_color_range_rejected(lo: u8, hi: u8) -> bool {
    let noise = NoiseOptions {
        color_range: (lo, hi),
        ..NoiseOptions::default()
    };

    matches!(
        CaptchaManager::new(
            SECRET.to_string(),
            60,
            noise,
            None,
            GenerationOptions::default(),
            &SampleSet::demo_insecure(),
        ),
        Err(CaptchaError::InvalidInput(_))
    )
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
    fn oob_index_rejected(idx in prop_oneof![Just(0u8), 10u8..=u8::MAX]) {
        prop_assert!(prop_oob_rejected(idx));
    }

    #[test]
    fn exactly_one_index_verifies(
        cell in 80u32..=200,
        q in prop_oneof![Just(20u8), Just(60u8), Just(90u8)],
        ttl in 30u64..=120,
    ) {
        prop_assert!(prop_exactly_one_index_verifies(cell, q, ttl));
    }

    #[test]
    fn wrong_round_count_rejected(
        rounds in 1u8..=3,
        submitted in 0usize..=6,
    ) {
        prop_assume!(submitted != rounds as usize);
        prop_assert!(prop_wrong_round_count_rejected(rounds, submitted));
    }

    #[test]
    fn short_secret_rejected(s in "[a-z]{0,31}") {
        prop_assert!(prop_short_secret_rejected(&s));
    }

    #[test]
    fn out_of_range_ttl_rejected(
        ttl in prop_oneof![Just(0u64), 86_401u64..=u64::MAX],
    ) {
        prop_assert!(prop_out_of_range_ttl_rejected(ttl));
    }

    #[test]
    fn out_of_range_cell_size_rejected(
        cell in prop_oneof![0u32..16, 1025u32..=u32::MAX],
    ) {
        prop_assert!(prop_out_of_range_cell_size_rejected(cell));
    }

    #[test]
    fn inverted_color_range_rejected(lo in 1u8..=255) {
        let hi = lo - 1;
        prop_assert!(prop_inverted_color_range_rejected(lo, hi));
    }
}
