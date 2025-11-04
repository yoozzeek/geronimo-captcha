use crate::error::{CaptchaError, Result};
use crate::image::{NoiseOptions, encode_image, watermark_with_noise};
use crate::sprite::{SpriteFormat, SpriteTarget, create_sprite};
use crate::utils::get_timestamp;

use base64::{Engine as _, prelude::BASE64_STANDARD};
use hmac::{Hmac, Mac};
use image::{DynamicImage, Limits};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

pub struct CaptchaChallenge<T> {
    pub sprite: T,
    #[cfg(any(test, feature = "test-utils"))]
    pub sprite_dbg: DynamicImage,
    pub challenge_id: String,
    pub timestamp: u64,
    #[cfg(any(test, feature = "test-utils"))]
    pub correct_number: u8,
}

#[derive(Clone)]
pub struct GenerationOptions {
    pub cell_size: u32,
    pub sprite_format: SpriteFormat,
    pub limits: Option<Limits>,
}

impl Default for GenerationOptions {
    fn default() -> Self {
        Self {
            cell_size: 150,
            sprite_format: SpriteFormat::default(),
            limits: None,
        }
    }
}

pub fn generate<T: SpriteTarget>(
    base_buf: &[u8],
    secret: &[u8],
    opts: &GenerationOptions,
    noise: NoiseOptions,
) -> Result<CaptchaChallenge<T>> {
    let (mut sprite, correct_number) = create_sprite(base_buf, opts)?;
    watermark_with_noise(&mut sprite, noise);

    let rgb = sprite.to_rgb8();
    let dyn_rgb = DynamicImage::ImageRgb8(rgb);

    let (sprite_buf, mime) =
        encode_image(&dyn_rgb, &opts.sprite_format).map_err(CaptchaError::Encode)?;

    let sprite = T::from_bytes(sprite_buf, mime);

    let (challenge_id, timestamp) = build_challenge_id(correct_number, secret)?;

    #[cfg(any(test, feature = "test-utils"))]
    let challenge = CaptchaChallenge {
        sprite,
        sprite_dbg: dyn_rgb,
        challenge_id,
        timestamp,
        correct_number,
    };
    #[cfg(not(any(test, feature = "test-utils")))]
    let challenge = CaptchaChallenge {
        sprite,
        challenge_id,
        timestamp,
    };

    Ok(challenge)
}

fn build_challenge_id(correct_number: u8, secret: &[u8]) -> Result<(String, u64)> {
    let timestamp = get_timestamp();
    let nonce = Uuid::new_v4().to_string();

    let mut mac = HmacSha256::new_from_slice(secret)
        .map_err(|e| CaptchaError::Internal(format!("create HMAC: {e}")))?;
    mac.update(nonce.as_bytes());
    mac.update(&[correct_number]);
    mac.update(&timestamp.to_be_bytes());

    let code = BASE64_STANDARD.encode(mac.finalize().into_bytes());

    Ok((format!("{nonce}:{timestamp}:{code}"), timestamp))
}

pub fn verify(secret: &[u8], challenge_id: &str, selected_index: u8, ttl: u64) -> bool {
    let parts: Vec<&str> = challenge_id.split(':').collect();
    if parts.len() != 3 {
        return false;
    }

    let nonce = parts[0];
    let timestamp: u64 = match parts[1].parse() {
        Ok(t) => t,
        Err(_) => return false,
    };
    let expected_code_b64 = parts[2];

    let now = get_timestamp();
    if now > timestamp.saturating_add(ttl) {
        return false;
    }

    let mut mac = match HmacSha256::new_from_slice(secret) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(nonce.as_bytes());
    mac.update(&[selected_index]);
    mac.update(&timestamp.to_be_bytes());

    let computed = mac.finalize().into_bytes();

    let expected = match BASE64_STANDARD.decode(expected_code_b64) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };

    if expected.len() != computed.len() {
        return false;
    }

    computed[..].ct_eq(expected.as_slice()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SpriteUri;
    use base64::engine::general_purpose;
    use std::collections::HashSet;
    use std::thread::sleep;
    use std::time::Duration;

    const CHALLENGE_TTL: u64 = 60;
    const SECRET: &[u8] = b"secret-key";

    fn load_sample_image() -> Vec<u8> {
        include_bytes!("../assets/sample1.jpg").to_vec()
    }

    fn generate_challenge() -> CaptchaChallenge<SpriteUri> {
        let base = load_sample_image();
        let opts = GenerationOptions {
            cell_size: 150,
            sprite_format: SpriteFormat::Jpeg { quality: 70 },
            limits: None,
        };
        generate::<SpriteUri>(&base, SECRET, &opts, NoiseOptions::default())
            .expect("Failed to generate challenge")
    }

    #[test]
    fn test_generate_and_verify() {
        let challenge = generate_challenge();
        let result = verify(
            SECRET,
            &challenge.challenge_id,
            challenge.correct_number,
            CHALLENGE_TTL,
        );

        // challenge
        //     .sprite
        //     .save(Path::new("examples/exampleX.jpg"))
        //     .expect("Failed to save generated image");

        assert!(result, "Challenge verification failed for correct index");
    }

    #[test]
    fn test_verification_should_fail_for_wrong_guess() {
        let challenge = generate_challenge();

        let wrong = (challenge.correct_number + 1) % 9;
        let valid = verify(SECRET, &challenge.challenge_id, wrong, 60);

        assert!(!valid, "Verification should fail for wrong index");
    }

    #[test]
    fn test_challenge_correct_index_should_be_random() {
        let mut seen_indices = HashSet::new();

        for _ in 0..100 {
            let challenge = generate_challenge();
            seen_indices.insert(challenge.correct_number);
        }

        assert!(
            seen_indices.len() > 1,
            "Correct index never changes. Challenge randomization failed"
        );
    }

    #[test]
    fn test_challenge_should_expire_after_ttl() {
        let challenge = generate_challenge();

        sleep(Duration::from_secs(2));

        let expired = verify(SECRET, &challenge.challenge_id, challenge.correct_number, 1);

        assert!(!expired, "Expired challenge passed verification");
    }

    #[test]
    fn test_verification_should_not_leak_answer() {
        use std::time::Instant;

        let challenge = generate_challenge();

        let mut durations = vec![];
        for i in 0..9 {
            let start = Instant::now();
            let _ = verify(SECRET, &challenge.challenge_id, i, 60);
            durations.push(start.elapsed().as_nanos());
        }

        let min = *durations.iter().min().unwrap();
        let max = *durations.iter().max().unwrap();
        let delta = max - min;

        println!("Timing min={min}ns, max={max}ns, delta={delta}ns");

        // Allow a small margin (<50μs) due to CPU noise, but not large leak
        assert!(
            delta < 50_000,
            "Timing delta too large ({delta}ns), possible side channel",
        );
    }

    #[test]
    fn test_no_false_positives_over_many_challenges() {
        use std::time::Instant;

        let mut false_positives = 0;
        let mut durations = vec![];

        for _ in 0..60 {
            let start = Instant::now();
            let challenge = generate_challenge();
            durations.push(start.elapsed().as_nanos());

            for guess in 0..9 {
                if guess != challenge.correct_number
                    && verify(SECRET, &challenge.challenge_id, guess, 60)
                {
                    false_positives += 1;
                }
            }
        }

        let min = *durations.iter().min().unwrap();
        let max = *durations.iter().max().unwrap();
        let delta = max - min;

        println!("Timing min={min}ns, max={max}ns, delta={delta}ns");

        assert_eq!(
            false_positives, 0,
            "Detected {false_positives} false positives — verification failed securely",
        );
    }

    #[test]
    fn test_uniqueness_hmac() {
        let mut hmacs = HashSet::new();

        for _ in 0..60 {
            let challenge = generate_challenge();
            let suffix8 = challenge
                .challenge_id
                .rsplit(':')
                .next()
                .unwrap_or("")
                .chars()
                .rev()
                .take(8)
                .collect::<String>();

            hmacs.insert(suffix8);

            sleep(Duration::from_millis(10));
        }

        assert_eq!(
            hmacs.len(),
            60,
            "HMACs are not unique, potential rainbow table vulnerability"
        );
    }

    #[test]
    fn test_challenge_id_should_be_unforgeable() {
        let challenge = generate_challenge();

        let parts: Vec<&str> = challenge.challenge_id.split(':').collect();
        let forged_index = (challenge.correct_number + 1) % 9;

        // Recompute a forged HMAC for the wrong index
        let mut mac = hmac::Hmac::<Sha256>::new_from_slice(b"BAD_SECRET").unwrap();
        mac.update(parts[0].as_bytes());
        mac.update(&[forged_index]);
        mac.update(&parts[1].parse::<u64>().unwrap().to_be_bytes());
        let forged_code = general_purpose::STANDARD.encode(mac.finalize().into_bytes());

        let forged_challenge = format!("{}:{}:{}", parts[0], parts[1], forged_code);
        let valid = verify(SECRET, &forged_challenge, forged_index, CHALLENGE_TTL);
        assert!(
            !valid,
            "Forged challenge ID was accepted. HMAC security failure"
        )
    }
}
