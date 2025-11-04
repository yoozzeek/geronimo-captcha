use crate::image::rotate_image;
use crate::{CaptchaError, GenerationOptions};

use ab_glyph::{FontArc, PxScale};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use image::{DynamicImage, GenericImage, ImageBuffer, ImageReader, Limits, Rgba, imageops};
use imageproc::drawing::draw_text_mut;
use once_cell::sync::Lazy;
use rand::prelude::SliceRandom;
use rand::{Rng, rng};
#[cfg(feature = "parallel")]
use rayon::prelude::*;
use std::io::Cursor;

static FONT: Lazy<FontArc> = Lazy::new(|| {
    FontArc::try_from_slice(include_bytes!("../assets/Roboto-Bold.ttf"))
        .expect("embedded font should be valid")
});

pub trait SpriteTarget: Sized {
    fn from_bytes(bytes: Vec<u8>, mime: &'static str) -> Self;
}

pub struct SpriteUri(pub String);

impl SpriteTarget for SpriteUri {
    fn from_bytes(bytes: Vec<u8>, mime: &'static str) -> Self {
        SpriteUri(sprite_to_base64(&bytes, mime))
    }
}

pub struct SpriteBinary {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
}

impl SpriteTarget for SpriteBinary {
    fn from_bytes(bytes: Vec<u8>, mime: &'static str) -> Self {
        SpriteBinary { bytes, mime }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SpriteFormat {
    Jpeg { quality: u8 },
    Webp { quality: u8, lossless: bool },
}

impl Default for SpriteFormat {
    fn default() -> Self {
        SpriteFormat::Jpeg { quality: 70 }
    }
}

pub fn create_sprite(
    base_buf: &[u8],
    opts: &GenerationOptions,
) -> crate::Result<(DynamicImage, u8)> {
    let mut reader = ImageReader::with_format(Cursor::new(base_buf), image::ImageFormat::Jpeg);
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

fn sprite_to_base64(buf: &[u8], mime: &str) -> String {
    format!("data:{};base64,{}", mime, BASE64_STANDARD.encode(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::challenge::generate;
    use crate::image::NoiseOptions;
    use std::fs;
    use std::path::Path;

    const SECRET: &[u8] = b"secret-key";

    fn load_sample_image() -> Vec<u8> {
        include_bytes!("../assets/sample1.jpg").to_vec()
    }

    #[test]
    fn test_sprite_prefix_jpeg_and_decode() {
        let base = load_sample_image();
        let opts = GenerationOptions {
            cell_size: 120,
            sprite_format: SpriteFormat::Jpeg { quality: 60 },
            limits: None,
        };

        let ch = generate::<SpriteUri>(&base, SECRET, &opts, NoiseOptions::default())
            .expect("jpeg generation failed");
        assert!(ch.sprite.0.starts_with("data:image/jpeg;base64,"));

        let data_b64 = ch
            .sprite
            .0
            .split_once(',')
            .map(|x| x.1)
            .expect("missing data uri payload");
        let bytes = BASE64_STANDARD
            .decode(data_b64)
            .expect("base64 decode jpeg");

        let _img = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .expect("guess jpeg format")
            .decode()
            .expect("decode jpeg");
    }

    #[test]
    fn test_sprite_prefix_webp_and_decode() {
        let base = load_sample_image();
        let opts = GenerationOptions {
            cell_size: 120,
            sprite_format: SpriteFormat::Webp {
                quality: 75,
                lossless: false,
            },
            limits: None,
        };

        let ch = generate::<SpriteUri>(&base, SECRET, &opts, NoiseOptions::default())
            .expect("webp generation failed");
        assert!(ch.sprite.0.starts_with("data:image/webp;base64,"));

        let data_b64 = ch
            .sprite
            .0
            .split_once(',')
            .map(|x| x.1)
            .expect("missing data uri payload");
        let bytes = BASE64_STANDARD
            .decode(data_b64)
            .expect("base64 decode webp");

        let _img = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .expect("guess webp format")
            .decode()
            .expect("decode webp");
    }

    #[test]
    fn test_sprite_binary_jpeg_and_decode() {
        let base = load_sample_image();
        let opts = GenerationOptions {
            cell_size: 150,
            sprite_format: SpriteFormat::Jpeg { quality: 70 },
            limits: None,
        };
        let ch = generate::<SpriteBinary>(&base, SECRET, &opts, NoiseOptions::default())
            .expect("jpeg binary generation failed");

        assert_eq!(ch.sprite.mime, "image/jpeg");
        assert!(!ch.sprite.bytes.is_empty());

        fs::write(Path::new("examples/exampleX.jpeg"), &ch.sprite.bytes)
            .expect("Failed to save generated image");

        let _img = ImageReader::new(Cursor::new(&ch.sprite.bytes))
            .with_guessed_format()
            .expect("guess jpeg format")
            .decode()
            .expect("decode jpeg binary");
    }

    #[test]
    fn test_sprite_binary_webp_and_decode() {
        let base = load_sample_image();
        let opts = GenerationOptions {
            cell_size: 150,
            sprite_format: SpriteFormat::Webp {
                quality: 70,
                lossless: false,
            },
            limits: None,
        };
        let ch = generate::<SpriteBinary>(&base, SECRET, &opts, NoiseOptions::default())
            .expect("webp binary generation failed");

        assert_eq!(ch.sprite.mime, "image/webp");
        assert!(!ch.sprite.bytes.is_empty());

        fs::write(Path::new("examples/exampleX.webp"), &ch.sprite.bytes)
            .expect("Failed to save generated image");

        let _img = ImageReader::new(Cursor::new(&ch.sprite.bytes))
            .with_guessed_format()
            .expect("guess webp format")
            .decode()
            .expect("decode webp binary");
    }
}
