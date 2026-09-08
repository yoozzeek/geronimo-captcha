use geronimo_captcha::{
    CaptchaError, CaptchaManager, ChallengeInMemoryRegistry, ChallengeRegistry, GenerationOptions,
    NoiseOptions, RegistryRejection, SampleSet, SpriteFormat, SpriteUri,
};
use std::sync::Arc;
use std::thread;

const SECRET: &str = "registry-integration-secret-key!!!!";
const TTL: u64 = 60;

fn opts() -> GenerationOptions {
    GenerationOptions {
        cell_size: 80,
        sprite_format: SpriteFormat::Jpeg { quality: 40 },
        ..GenerationOptions::default()
    }
}

fn manager(registry: Option<Arc<dyn ChallengeRegistry>>) -> CaptchaManager {
    CaptchaManager::new(
        SECRET.to_string(),
        TTL,
        NoiseOptions::default(),
        registry,
        opts(),
        &SampleSet::demo_insecure(),
    )
    .expect("valid manager configuration")
}

fn registry(max_attempts: u16, max_entries: usize) -> Arc<dyn ChallengeRegistry> {
    Arc::new(
        ChallengeInMemoryRegistry::new(TTL, max_attempts, max_entries).expect("valid registry"),
    )
}

fn solve(open: &CaptchaManager, challenge_id: &str) -> u8 {
    (1u8..=9)
        .find(|&idx| open.verify_challenge(challenge_id, &[idx]).unwrap_or(false))
        .expect("exactly one index verifies")
}

#[test]
fn attempts_are_exhausted_then_refused() {
    let open = manager(None);
    let guarded = manager(Some(registry(3, 1_000)));

    let challenge = guarded
        .generate_challenge::<SpriteUri>()
        .expect("generate challenge");

    let wrong = [solve(&open, &challenge.challenge_id) % 9 + 1];

    for attempt in 1..=3 {
        assert_eq!(
            guarded
                .verify_challenge(&challenge.challenge_id, &wrong)
                .ok(),
            Some(false),
            "attempt {attempt} should be counted, not refused"
        );
    }

    assert!(matches!(
        guarded.verify_challenge(&challenge.challenge_id, &wrong),
        Err(CaptchaError::Registry(
            RegistryRejection::MaxAttemptsLimitExceeded
        ))
    ));
}

#[test]
fn solved_challenge_cannot_be_replayed() {
    let open = manager(None);
    let guarded = manager(Some(registry(3, 1_000)));

    let challenge = guarded
        .generate_challenge::<SpriteUri>()
        .expect("generate challenge");

    let correct = [solve(&open, &challenge.challenge_id)];

    assert_eq!(
        guarded
            .verify_challenge(&challenge.challenge_id, &correct)
            .ok(),
        Some(true)
    );

    assert!(matches!(
        guarded.verify_challenge(&challenge.challenge_id, &correct),
        Err(CaptchaError::Registry(RegistryRejection::AlreadyVerified))
    ));
}

#[test]
fn unknown_id_is_refused_without_burning_attempt() {
    let open = manager(None);
    let guarded = manager(Some(registry(3, 1_000)));

    let challenge = guarded
        .generate_challenge::<SpriteUri>()
        .expect("generate challenge");

    let wrong = [solve(&open, &challenge.challenge_id) % 9 + 1];

    let mut forged = challenge.challenge_id.clone();
    let flipped = match forged.starts_with('0') {
        true => "1",
        false => "0",
    };

    forged.replace_range(0..1, flipped);

    for _ in 0..8 {
        assert!(matches!(
            guarded.verify_challenge(&forged, &[1]),
            Err(CaptchaError::Registry(RegistryRejection::NotRegistered))
        ));
    }

    for attempt in 1..=3 {
        assert_eq!(
            guarded
                .verify_challenge(&challenge.challenge_id, &wrong)
                .ok(),
            Some(false),
            "attempt {attempt} on the real id was refused"
        );
    }
}

#[test]
fn concurrent_correct_guesses_admit_exactly_one() {
    let open = manager(None);
    let guarded = Arc::new(manager(Some(registry(16, 1_000))));

    let challenge = guarded
        .generate_challenge::<SpriteUri>()
        .expect("generate challenge");

    let correct = [solve(&open, &challenge.challenge_id)];
    let id = Arc::new(challenge.challenge_id);

    let handles: Vec<_> = (0..16)
        .map(|_| {
            let guarded = Arc::clone(&guarded);
            let id = Arc::clone(&id);
            thread::spawn(move || guarded.verify_challenge(&id, &correct))
        })
        .collect();

    let (mut accepted, mut replayed) = (0, 0);

    for handle in handles {
        match handle.join().expect("thread panicked") {
            Ok(true) => accepted += 1,
            Err(CaptchaError::Registry(RegistryRejection::AlreadyVerified)) => replayed += 1,
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    assert_eq!(accepted, 1, "solved challenge accepted more than once");
    assert_eq!(replayed, 15);
}

#[test]
fn concurrent_wrong_guesses_never_exceed_attempt_cap() {
    let open = manager(None);
    let guarded = Arc::new(manager(Some(registry(3, 1_000))));

    let challenge = guarded
        .generate_challenge::<SpriteUri>()
        .expect("generate challenge");

    let correct = solve(&open, &challenge.challenge_id);
    let id = Arc::new(challenge.challenge_id);

    let handles: Vec<_> = (1u8..=9)
        .filter(|&guess| guess != correct)
        .map(|guess| {
            let guarded = Arc::clone(&guarded);
            let id = Arc::clone(&id);
            thread::spawn(move || guarded.verify_challenge(&id, &[guess]))
        })
        .collect();

    let (mut evaluated, mut refused) = (0, 0);

    for handle in handles {
        match handle.join().expect("thread panicked") {
            Ok(false) => evaluated += 1,
            Err(CaptchaError::Registry(RegistryRejection::MaxAttemptsLimitExceeded)) => {
                refused += 1;
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    assert_eq!(evaluated, 3, "attempt cap bypassed by parallel guesses");
    assert_eq!(refused, 5);
}

#[test]
fn full_registry_refuses_to_issue_dead_challenge() {
    let guarded = manager(Some(registry(3, 1)));

    guarded
        .generate_challenge::<SpriteUri>()
        .expect("first challenge fits");

    assert!(matches!(
        guarded.generate_challenge::<SpriteUri>(),
        Err(CaptchaError::Registry(RegistryRejection::AtCapacity))
    ));
}

#[test]
fn oversized_challenge_id_is_rejected_before_registry() {
    let guarded = manager(Some(registry(3, 1_000)));

    assert!(matches!(
        guarded.verify_challenge(&"x".repeat(1_000_000), &[1]),
        Err(CaptchaError::InvalidInput(_))
    ));
}
