use crate::error::{CaptchaError, Result};
use crate::image::{NoiseOptions, blur_rgb, encode_image, watermark_with_noise};
use crate::sprite::{SpriteFormat, SpriteTarget, create_sprite};
use crate::utils::get_timestamp;

use base64::{Engine as _, prelude::BASE64_STANDARD};
use hmac::{Hmac, KeyInit, Mac};
use image::{DynamicImage, RgbaImage};
use rand::{RngExt, rng};
use sha2::Sha256;
use std::fmt::Write as _;
use subtle::ConstantTimeEq;
use uuid::Uuid;

pub(crate) const MAX_ROUNDS: u8 = 5;

// NONCE_LEN + ':' + u64::MAX digits + ':' + MAC_B64_LEN
pub(crate) const MAX_CHALLENGE_ID_LEN: usize = NONCE_LEN + 1 + 20 + 1 + MAC_B64_LEN;

const MIN_CELL_SIZE: u32 = 16;
const MAX_CELL_SIZE: u32 = 1024;

const MAX_DECODE_DIM: u32 = 16_384;
const MAX_DECODE_ALLOC: u64 = 1 << 30;

const NONCE_LEN: usize = 36;
const MAC_LEN: usize = 32;
const MAC_B64_LEN: usize = 44;

type HmacSha256 = Hmac<Sha256>;

pub struct CaptchaChallenge<T> {
    pub sprites: Vec<T>,
    pub challenge_id: String,
    pub timestamp: u64,
    #[cfg(test)]
    pub correct_numbers: Vec<u8>,
}

/// Ceilings applied to a sample image before it is decoded.
#[derive(Clone, Copy, Debug)]
pub struct DecodeLimits {
    /// Must be within 1..=16384.
    pub max_width: u32,

    /// Must be within 1..=16384.
    pub max_height: u32,

    /// Decoder allocation ceiling in bytes.
    /// Must be within 1..=1073741824.
    pub max_alloc: u64,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_width: 4096,
            max_height: 4096,
            max_alloc: 128 * 1024 * 1024,
        }
    }
}

/// Sprite geometry and encoding parameters.
#[derive(Clone, Copy, Debug)]
pub struct GenerationOptions {
    /// Edge length of one grid cell, in pixels.
    /// Must be within 16..=1024; the sprite is
    /// three cells wide plus 4px gutters.
    pub cell_size: u32,

    pub sprite_format: SpriteFormat,

    /// Sprites the client must solve in one submission.
    /// Must be within 1..=5. Each round draws its own sample image.
    pub rounds: u8,

    pub limits: DecodeLimits,
}

impl Default for GenerationOptions {
    fn default() -> Self {
        Self {
            cell_size: 150,
            sprite_format: SpriteFormat::default(),
            rounds: 1,
            limits: DecodeLimits::default(),
        }
    }
}

impl GenerationOptions {
    /// # Errors
    ///
    /// Returns [`CaptchaError::InvalidInput`] if `cell_size`,
    /// the format quality, `rounds` or any [`DecodeLimits`]
    /// field is outside its documented range.
    pub fn validate(&self) -> Result<()> {
        if !(MIN_CELL_SIZE..=MAX_CELL_SIZE).contains(&self.cell_size) {
            return Err(CaptchaError::InvalidInput(format!(
                "cell_size {} outside {MIN_CELL_SIZE}..={MAX_CELL_SIZE}",
                self.cell_size
            )));
        }

        let quality = self.sprite_format.quality();
        if !(1..=100).contains(&quality) {
            return Err(CaptchaError::InvalidInput(format!(
                "sprite quality {quality} outside 1..=100"
            )));
        }

        if !(1..=MAX_ROUNDS).contains(&self.rounds) {
            return Err(CaptchaError::InvalidInput(format!(
                "rounds {} outside 1..={MAX_ROUNDS}",
                self.rounds
            )));
        }

        for (name, dim) in [
            ("max_width", self.limits.max_width),
            ("max_height", self.limits.max_height),
        ] {
            if !(1..=MAX_DECODE_DIM).contains(&dim) {
                return Err(CaptchaError::InvalidInput(format!(
                    "decode {name} {dim} outside 1..={MAX_DECODE_DIM}"
                )));
            }
        }

        if !(1..=MAX_DECODE_ALLOC).contains(&self.limits.max_alloc) {
            return Err(CaptchaError::InvalidInput(format!(
                "decode max_alloc {} outside 1..={MAX_DECODE_ALLOC}",
                self.limits.max_alloc
            )));
        }

        Ok(())
    }
}

pub fn generate<T: SpriteTarget>(
    bases: &[RgbaImage],
    secret: &[u8],
    opts: &GenerationOptions,
    noise: NoiseOptions,
) -> Result<CaptchaChallenge<T>> {
    if bases.is_empty() {
        return Err(CaptchaError::Internal("no sample images available".into()));
    }

    let rounds = opts.rounds as usize;
    let mut picks = [0usize; MAX_ROUNDS as usize];

    select_bases(bases.len(), &mut picks[..rounds]);

    let mut sprites = Vec::with_capacity(rounds);
    let mut correct_numbers = Vec::with_capacity(rounds);

    for &pick in &picks[..rounds] {
        let (mut sprite, correct_number) = create_sprite(&bases[pick], opts)?;
        watermark_with_noise(&mut sprite, noise);

        let rgb = blur_rgb(
            DynamicImage::ImageRgba8(sprite).into_rgb8(),
            noise.blur_sigma,
        );

        let (sprite_buf, mime) = encode_image(&rgb, &opts.sprite_format)?;

        sprites.push(T::from_bytes(sprite_buf, mime));
        correct_numbers.push(correct_number);
    }

    let (challenge_id, timestamp) = build_challenge_id(&correct_numbers, secret)?;

    Ok(CaptchaChallenge {
        sprites,
        challenge_id,
        timestamp,
        #[cfg(test)]
        correct_numbers,
    })
}

fn select_bases(count: usize, picks: &mut [usize]) {
    let mut rng = rng();
    let distinct = count >= picks.len();

    for i in 0..picks.len() {
        loop {
            let pick = rng.random_range(0..count);
            if !distinct || !picks[..i].contains(&pick) {
                picks[i] = pick;
                break;
            }
        }
    }
}

fn build_challenge_id(correct_numbers: &[u8], secret: &[u8]) -> Result<(String, u64)> {
    let timestamp = get_timestamp()?;

    let mut nonce_buf = Uuid::encode_buffer();
    let nonce = Uuid::new_v4().hyphenated().encode_lower(&mut nonce_buf);

    let mut mac = HmacSha256::new_from_slice(secret)
        .map_err(|e| CaptchaError::Internal(format!("create HMAC: {e}")))?;
    mac.update(nonce.as_bytes());
    mac.update(&[correct_numbers.len() as u8]);
    mac.update(correct_numbers);
    mac.update(&timestamp.to_be_bytes());

    let mut id = String::with_capacity(MAX_CHALLENGE_ID_LEN);
    id.push_str(nonce);
    id.push(':');
    write!(id, "{timestamp}").map_err(|e| CaptchaError::Internal(format!("format id: {e}")))?;
    id.push(':');

    BASE64_STANDARD.encode_string(mac.finalize().into_bytes(), &mut id);

    Ok((id, timestamp))
}

pub fn verify(secret: &[u8], challenge_id: &str, selected: &[u8], ttl: u64) -> bool {
    if challenge_id.len() > MAX_CHALLENGE_ID_LEN {
        return false;
    }

    if selected.is_empty() || selected.len() > MAX_ROUNDS as usize {
        return false;
    }

    let mut parts = challenge_id.split(':');
    let (Some(nonce), Some(ts), Some(code), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };

    if nonce.len() != NONCE_LEN || code.len() != MAC_B64_LEN || !is_canonical_decimal(ts) {
        return false;
    }

    let Ok(timestamp) = ts.parse::<u64>() else {
        return false;
    };

    let Ok(now) = get_timestamp() else {
        return false;
    };

    if now > timestamp.saturating_add(ttl) {
        return false;
    }

    let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
        return false;
    };

    mac.update(nonce.as_bytes());
    mac.update(&[selected.len() as u8]);
    mac.update(selected);
    mac.update(&timestamp.to_be_bytes());

    let mut expected = [0u8; MAC_B64_LEN];
    let Ok(written) = BASE64_STANDARD.decode_slice(code, &mut expected) else {
        return false;
    };

    if written != MAC_LEN {
        return false;
    }

    mac.finalize().into_bytes()[..]
        .ct_eq(&expected[..MAC_LEN])
        .into()
}

fn is_canonical_decimal(s: &str) -> bool {
    match s.as_bytes() {
        [] => false,
        [b'0'] => true,
        [b'0', ..] => false,
        digits => digits.iter().all(u8::is_ascii_digit),
    }
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
    const SECRET: &[u8] = b"unit-test-secret-key-32-bytes-min!!";

    fn opts(rounds: u8) -> GenerationOptions {
        GenerationOptions {
            cell_size: 150,
            sprite_format: SpriteFormat::Jpeg { quality: 70 },
            rounds,
            limits: DecodeLimits::default(),
        }
    }

    fn generate_rounds(rounds: u8) -> CaptchaChallenge<SpriteUri> {
        let opts = opts(rounds);
        let base = crate::sprite::decode_base(
            include_bytes!("../assets/sample1.jpg"),
            image::ImageFormat::Jpeg,
            &opts,
        )
        .expect("decode sample");

        generate::<SpriteUri>(
            std::slice::from_ref(&base),
            SECRET,
            &opts,
            NoiseOptions::default(),
        )
        .expect("Failed to generate challenge")
    }

    fn generate_challenge() -> CaptchaChallenge<SpriteUri> {
        generate_rounds(1)
    }

    fn wrong_index(correct: u8) -> u8 {
        (correct % 9) + 1
    }

    #[test]
    fn generate_and_verify() {
        let challenge = generate_challenge();

        assert!(
            verify(
                SECRET,
                &challenge.challenge_id,
                &challenge.correct_numbers,
                CHALLENGE_TTL,
            ),
            "Challenge verification failed for correct index"
        );
    }

    #[test]
    fn verification_should_fail_for_wrong_guess() {
        let challenge = generate_challenge();
        let wrong = [wrong_index(challenge.correct_numbers[0])];

        assert!(
            !verify(SECRET, &challenge.challenge_id, &wrong, CHALLENGE_TTL),
            "Verification should fail for wrong index"
        );
    }

    #[test]
    fn challenge_correct_index_should_be_random() {
        let mut seen_indices = HashSet::new();
        for _ in 0..100 {
            seen_indices.insert(generate_challenge().correct_numbers[0]);
        }

        assert!(
            seen_indices.len() > 1,
            "Correct index never changes. Challenge randomization failed"
        );
    }

    #[test]
    fn challenge_should_expire_after_ttl() {
        let challenge = generate_challenge();

        sleep(Duration::from_secs(2));

        assert!(
            !verify(
                SECRET,
                &challenge.challenge_id,
                &challenge.correct_numbers,
                1
            ),
            "Expired challenge passed verification"
        );
    }

    #[test]
    fn exactly_one_index_verifies() {
        for _ in 0..60 {
            let challenge = generate_challenge();

            let accepted = (1u8..=9)
                .filter(|&guess| verify(SECRET, &challenge.challenge_id, &[guess], CHALLENGE_TTL))
                .count();

            assert_eq!(
                accepted, 1,
                "expected exactly one accepted index, got {accepted}"
            );
        }
    }

    #[test]
    fn rounds_draw_distinct_samples() {
        for rounds in 1..=MAX_ROUNDS as usize {
            let mut picks = [0usize; MAX_ROUNDS as usize];
            for _ in 0..64 {
                select_bases(8, &mut picks[..rounds]);

                let distinct: HashSet<usize> = picks[..rounds].iter().copied().collect();

                assert_eq!(distinct.len(), rounds, "two rounds share a sample image");
                assert!(picks[..rounds].iter().all(|&p| p < 8));
            }
        }
    }

    #[test]
    fn rounds_repeat_samples_when_corpus_is_smaller() {
        let mut picks = [7usize; MAX_ROUNDS as usize];
        select_bases(1, &mut picks[..3]);

        assert_eq!(&picks[..3], &[0, 0, 0]);
    }

    #[test]
    fn exactly_one_combination_verifies_across_rounds() {
        for rounds in 2u8..=3 {
            let challenge = generate_rounds(rounds);

            assert_eq!(challenge.sprites.len(), rounds as usize);
            assert_eq!(challenge.correct_numbers.len(), rounds as usize);

            let mut guess = vec![1u8; rounds as usize];
            let mut accepted = 0;

            loop {
                if verify(SECRET, &challenge.challenge_id, &guess, CHALLENGE_TTL) {
                    accepted += 1;
                }

                let mut i = 0;
                while i < guess.len() {
                    guess[i] += 1;

                    if guess[i] <= 9 {
                        break;
                    }

                    guess[i] = 1;
                    i += 1;
                }

                if i == guess.len() {
                    break;
                }
            }

            assert_eq!(
                accepted, 1,
                "rounds={rounds}: expected exactly one accepted combination of 9^{rounds}"
            );
        }
    }

    #[test]
    fn round_count_is_bound_into_the_mac() {
        let challenge = generate_rounds(2);
        let correct = &challenge.correct_numbers;

        assert!(!verify(
            SECRET,
            &challenge.challenge_id,
            &correct[..1],
            CHALLENGE_TTL
        ));

        let mut padded = correct.clone();
        padded.push(1);

        assert!(!verify(
            SECRET,
            &challenge.challenge_id,
            &padded,
            CHALLENGE_TTL
        ));

        assert!(!verify(SECRET, &challenge.challenge_id, &[], CHALLENGE_TTL));
    }

    #[test]
    fn malformed_challenge_ids_are_rejected() {
        let challenge = generate_challenge();
        let parts: Vec<&str> = challenge.challenge_id.split(':').collect();
        let (nonce, ts, code) = (parts[0], parts[1], parts[2]);

        let malformed = [
            String::new(),
            nonce.to_string(),
            format!("{nonce}:{ts}"),
            format!("{nonce}:{ts}:{code}:extra"),
            format!("{nonce}x:{ts}:{code}"),
            format!("{nonce}:{ts}:{code}x"),
            format!("{nonce}:not-a-number:{code}"),
            format!("{nonce}:+{ts}:{code}"),
            format!("{nonce}:0{ts}:{code}"),
            format!(":{ts}:{code}"),
        ];

        for id in malformed {
            assert!(
                !verify(SECRET, &id, &challenge.correct_numbers, CHALLENGE_TTL),
                "malformed challenge id accepted: {id:?}"
            );
        }
    }

    #[test]
    fn uniqueness_hmac() {
        let mut hmacs = HashSet::new();
        for _ in 0..60 {
            let challenge = generate_challenge();
            let code = challenge
                .challenge_id
                .rsplit(':')
                .next()
                .unwrap_or("")
                .to_string();

            hmacs.insert(code);
        }

        assert_eq!(
            hmacs.len(),
            60,
            "HMACs are not unique, potential rainbow table vulnerability"
        );
    }

    #[test]
    fn challenge_id_should_be_unforgeable() {
        let challenge = generate_challenge();

        let parts: Vec<&str> = challenge.challenge_id.split(':').collect();
        let forged = [wrong_index(challenge.correct_numbers[0])];

        let mut mac = HmacSha256::new_from_slice(b"BAD_SECRET").unwrap();
        mac.update(parts[0].as_bytes());
        mac.update(&[forged.len() as u8]);
        mac.update(&forged);
        mac.update(&parts[1].parse::<u64>().unwrap().to_be_bytes());

        let forged_code = general_purpose::STANDARD.encode(mac.finalize().into_bytes());

        let forged_challenge = format!("{}:{}:{}", parts[0], parts[1], forged_code);

        assert!(
            !verify(SECRET, &forged_challenge, &forged, CHALLENGE_TTL),
            "Forged challenge ID was accepted. HMAC security failure"
        );
    }

    #[test]
    fn options_validation_rejects_out_of_range() {
        for cell_size in [0, 1, MIN_CELL_SIZE - 1, MAX_CELL_SIZE + 1, u32::MAX] {
            let opts = GenerationOptions {
                cell_size,
                ..GenerationOptions::default()
            };

            assert!(
                opts.validate().is_err(),
                "cell_size {cell_size} should be rejected"
            );
        }

        for quality in [0, 101, u8::MAX] {
            let opts = GenerationOptions {
                sprite_format: SpriteFormat::Jpeg { quality },
                ..GenerationOptions::default()
            };

            assert!(
                opts.validate().is_err(),
                "quality {quality} should be rejected"
            );
        }

        for rounds in [0, MAX_ROUNDS + 1, u8::MAX] {
            let opts = GenerationOptions {
                rounds,
                ..GenerationOptions::default()
            };

            assert!(
                opts.validate().is_err(),
                "rounds {rounds} should be rejected"
            );
        }

        let bad_limits = [
            DecodeLimits {
                max_width: 0,
                ..DecodeLimits::default()
            },
            DecodeLimits {
                max_width: MAX_DECODE_DIM + 1,
                ..DecodeLimits::default()
            },
            DecodeLimits {
                max_height: 0,
                ..DecodeLimits::default()
            },
            DecodeLimits {
                max_height: MAX_DECODE_DIM + 1,
                ..DecodeLimits::default()
            },
            DecodeLimits {
                max_alloc: 0,
                ..DecodeLimits::default()
            },
            DecodeLimits {
                max_alloc: MAX_DECODE_ALLOC + 1,
                ..DecodeLimits::default()
            },
        ];

        for limits in bad_limits {
            let opts = GenerationOptions {
                limits,
                ..GenerationOptions::default()
            };

            assert!(
                opts.validate().is_err(),
                "limits {limits:?} should be rejected"
            );
        }

        assert!(GenerationOptions::default().validate().is_ok());
    }

    #[test]
    fn timestamp_encoding_is_canonical() {
        assert!(is_canonical_decimal("0"));
        assert!(is_canonical_decimal("1730534400"));
        assert!(is_canonical_decimal("18446744073709551615"));

        for s in ["", "+1", "-1", "01", "00", "1 ", " 1", "1_0", "1e3", "٣"] {
            assert!(!is_canonical_decimal(s), "{s:?} accepted as canonical");
        }
    }
}
