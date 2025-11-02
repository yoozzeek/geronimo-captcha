use crate::error::{CaptchaError, Result};
use crate::image_ops::{NoiseOptions, rotate_image, sprite_to_base64, watermark_with_noise};
use crate::utils::get_timestamp;

use ab_glyph::{FontArc, PxScale};
use base64::{Engine as _, prelude::BASE64_STANDARD};
use hmac::{Hmac, Mac};
use image::codecs::jpeg::JpegEncoder;
use image::{
    DynamicImage, GenericImage, ImageBuffer, ImageFormat, ImageReader, Limits, Rgba, imageops,
};
use imageproc::drawing::draw_text_mut;
use once_cell::sync::Lazy;
use rand::prelude::SliceRandom;
use rand::{Rng, rng};
#[cfg(feature = "parallel")]
use rayon::prelude::*;
use sha2::Sha256;
use std::io::Cursor;
use subtle::ConstantTimeEq;
use uuid::Uuid;

static FONT: Lazy<FontArc> = Lazy::new(|| {
    FontArc::try_from_slice(include_bytes!("../assets/Roboto-Bold.ttf"))
        .expect("embedded font should be valid")
});

type HmacSha256 = Hmac<Sha256>;

pub struct CaptchaChallenge {
    pub sprite_uri: String,
    #[cfg(any(test, feature = "test-utils"))]
    pub sprite: DynamicImage,
    pub challenge_id: String,
    pub timestamp: u64,
    #[cfg(any(test, feature = "test-utils"))]
    pub correct_number: u8,
}

#[derive(Clone)]
pub struct GenerationOptions {
    pub cell_size: u32,
    pub jpeg_quality: u8,
    pub limits: Option<Limits>,
}

impl Default for GenerationOptions {
    fn default() -> Self {
        Self {
            cell_size: 150,
            jpeg_quality: 20,
            limits: None,
        }
    }
}

pub fn generate(
    base_buf: &[u8],
    secret: &[u8],
    opts: &GenerationOptions,
    noise: NoiseOptions,
) -> Result<CaptchaChallenge> {
    let (mut sprite, correct_number) = create_sprite(base_buf, opts)?;
    watermark_with_noise(&mut sprite, noise);

    let rgb = sprite.to_rgb8();
    let dyn_rgb = DynamicImage::ImageRgb8(rgb);

    let mut sprite_buf = Vec::new();
    {
        let mut encoder = JpegEncoder::new_with_quality(&mut sprite_buf, opts.jpeg_quality);
        encoder
            .encode_image(&dyn_rgb)
            .map_err(|e| CaptchaError::EncodeError(format!("encode sprite as JPEG: {e}")))?;
    }
    let sprite_uri = sprite_to_base64(&sprite_buf, ImageFormat::Jpeg);

    let (challenge_id, timestamp) = build_challenge_id(correct_number, secret)?;

    #[cfg(any(test, feature = "test-utils"))]
    let challenge = CaptchaChallenge {
        sprite: dyn_rgb,
        sprite_uri,
        challenge_id,
        timestamp,
        correct_number,
    };
    #[cfg(not(any(test, feature = "test-utils")))]
    let challenge = CaptchaChallenge {
        sprite_uri,
        challenge_id,
        timestamp,
    };

    Ok(challenge)
}

fn create_sprite(base_buf: &[u8], opts: &GenerationOptions) -> Result<(DynamicImage, u8)> {
    let mut reader = ImageReader::with_format(Cursor::new(base_buf), ImageFormat::Jpeg);
    if let Some(limits) = opts.limits.clone() {
        reader.limits(limits);
    } else {
        let mut limits = Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(128 * 1024 * 1024);

        reader.limits(limits);
    }

    let base = reader
        .decode()
        .map_err(|e| CaptchaError::DecodeError(format!("load captcha sample image: {e}")))?
        .resize_exact(
            opts.cell_size,
            opts.cell_size,
            imageops::FilterType::Nearest,
        );
    let mut rng = rng();

    let correct_angle = 0.0;
    let incorrect_angles = [
        38.0, 88.0, 114.0, 138.0, 176.0, 200.0, 229.0, 255.0, 278.0, 314.0, 320.0,
    ];

    let mut angles = Vec::with_capacity(1 + incorrect_angles.len());
    angles.push(correct_angle);
    angles.extend_from_slice(&incorrect_angles);

    let precomputed: Vec<(f32, image::RgbaImage)> = {
        #[cfg(feature = "parallel")]
        {
            angles
                .par_iter()
                .map(|&a| (a, rotate_image(&base, a).to_rgba8()))
                .collect()
        }
        #[cfg(not(feature = "parallel"))]
        {
            angles
                .iter()
                .map(|&a| (a, rotate_image(&base, a).to_rgba8()))
                .collect()
        }
    };

    let mut tiles = vec![(true, correct_angle)];
    let mut others = incorrect_angles.to_vec();

    others.shuffle(&mut rng);

    for &angle in others.iter().take(8) {
        tiles.push((false, angle));
    }

    tiles.shuffle(&mut rng);

    let font = &*FONT;
    let cols = 3;
    let rows = 3;
    let spacing = 4;
    let sprite_width = cols * opts.cell_size + (cols - 1) * spacing;
    let sprite_height = rows * opts.cell_size + (rows - 1) * spacing;

    let mut sprite_buf =
        ImageBuffer::from_pixel(sprite_width, sprite_height, Rgba([255, 255, 255, 255]));

    let mut correct_number = 0;

    for (i, (is_correct, angle)) in tiles.iter().enumerate() {
        // Create and draw each tile
        let tile_scale = 0.5 + rng.random_range(0.0..0.3);
        let shrink_size = (opts.cell_size as f32 * tile_scale) as u32;
        let rotated = precomputed
            .iter()
            .find(|(a, _)| (*a - *angle).abs() < f32::EPSILON)
            .map(|(_, img)| img)
            .ok_or_else(|| CaptchaError::Internal("missing precomputed angle".into()))?;

        let mut tile = image::imageops::resize(
            rotated,
            shrink_size,
            shrink_size,
            imageops::FilterType::Lanczos3,
        );

        let should_flip = rng.random_bool(0.5);
        if should_flip {
            tile = imageops::flip_horizontal(&tile);
        }

        let col = i as u32 % cols;
        let row = i as u32 / cols;

        let base_x = col * (opts.cell_size + spacing);
        let base_y = row * (opts.cell_size + spacing);

        let offset_x = (opts.cell_size - shrink_size) / 2;
        let offset_y = (opts.cell_size - shrink_size) / 2;

        let jitter_limit_x = offset_x as i32;
        let jitter_limit_y = offset_y as i32;

        let jitter_x = rng.random_range(-jitter_limit_x..=jitter_limit_x);
        let jitter_y = rng.random_range(-jitter_limit_y..=jitter_limit_y);

        let draw_x = (base_x as i32 + offset_x as i32 + jitter_x) as u32;
        let draw_y = (base_y as i32 + offset_y as i32 + jitter_y) as u32;

        sprite_buf
            .copy_from(&tile, draw_x, draw_y)
            .map_err(|e| CaptchaError::Internal(format!("copy tile into sprite buffer: {e}")))?;

        // Draw the number label
        let label = format!("{}", i + 1);

        let label_x = draw_x.saturating_add(shrink_size).saturating_sub(16);
        let label_y = draw_y.saturating_add(shrink_size).saturating_sub(16);

        let scale_factor = rng.random_range(0.13..=0.17);
        let scale = PxScale::from(opts.cell_size as f32 * scale_factor);

        let color = Rgba([
            rng.random_range(0..100),
            rng.random_range(0..100),
            rng.random_range(0..100),
            255,
        ]);

        let offset_x = rng.random_range(0..=3);
        let offset_y = rng.random_range(0..=3);

        draw_text_mut(
            &mut sprite_buf,
            color,
            (label_x + offset_x) as i32,
            (label_y + offset_y) as i32,
            scale,
            &font,
            &label,
        );

        if *is_correct {
            correct_number = (i + 1) as u8;
        }
    }

    Ok((DynamicImage::ImageRgba8(sprite_buf), correct_number))
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
    use base64::engine::general_purpose;
    use std::collections::HashSet;
    use std::fs;
    use std::path::Path;
    use std::thread::sleep;
    use std::time::Duration;

    const CHALLENGE_TTL: u64 = 60;
    const SECRET: &[u8] = b"secret-key";

    fn load_sample_image() -> Vec<u8> {
        fs::read("assets/sample2.jpg").expect("Missing assets/sample1.jpg")
    }

    fn generate_challenge() -> CaptchaChallenge {
        let base = load_sample_image();
        let opts = GenerationOptions {
            cell_size: 150,
            jpeg_quality: 20,
            limits: None,
        };
        generate(&base, SECRET, &opts, NoiseOptions::default())
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

        challenge
            .sprite
            .save(Path::new("assets/example.jpg"))
            .expect("Failed to save generated image");

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

        for _ in 0..30 {
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
    fn test_verify_timing_should_not_leak_answer() {
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

        for _ in 0..100 {
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

        for _ in 0..100 {
            let challenge = generate_challenge();
            let prefix = challenge
                .challenge_id
                .chars()
                .rev()
                .take(8)
                .collect::<String>();
            hmacs.insert(prefix);
            sleep(Duration::from_millis(10));
        }

        assert_eq!(
            hmacs.len(),
            100,
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
