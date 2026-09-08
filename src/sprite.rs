use crate::image::warp_tile;
use crate::{CaptchaError, GenerationOptions};

use ab_glyph::{FontArc, InvalidFont, PxScale};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use image::{GenericImage, ImageBuffer, ImageReader, Limits, Rgba, RgbaImage, imageops};
use imageproc::drawing::draw_text_mut;
use rand::prelude::SliceRandom;
use rand::rngs::ThreadRng;
use rand::{RngExt, rng};
#[cfg(feature = "parallel")]
use rayon::prelude::*;
use std::io::Cursor;
use std::sync::LazyLock;

static FONT: LazyLock<Result<FontArc, InvalidFont>> =
    LazyLock::new(|| FontArc::try_from_slice(include_bytes!("../assets/Roboto-Bold.ttf")));

pub(crate) const SPRITE_BG: Rgba<u8> = Rgba([255, 255, 255, 255]);

const COLS: u32 = 3;
const ROWS: u32 = 3;
const SPACING: u32 = 4;
const TILES: usize = (COLS * ROWS) as usize;

const UPRIGHT_JITTER_DEG: f32 = 5.0;
const MIN_TILE_SCALE: f32 = 0.5;
const MAX_TILE_SCALE: f32 = 0.8;

const INCORRECT_ANGLES: [f32; 11] = [
    38.0, 88.0, 114.0, 138.0, 176.0, 200.0, 229.0, 255.0, 278.0, 314.0, 320.0,
];

const LABELS: [&str; TILES] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// Destination for the encoded sprite bytes.
pub trait SpriteTarget: Sized {
    fn from_bytes(bytes: Vec<u8>, mime: &'static str) -> Self;
}

/// Sprite as a `data:` URI, ready for `<img src>`.
pub struct SpriteUri(pub String);

impl SpriteTarget for SpriteUri {
    fn from_bytes(bytes: Vec<u8>, mime: &'static str) -> Self {
        SpriteUri(sprite_to_base64(&bytes, mime))
    }
}

/// Sprite as raw encoded bytes plus its MIME type.
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

impl SpriteFormat {
    pub(crate) fn quality(&self) -> u8 {
        match *self {
            SpriteFormat::Jpeg { quality } => quality,
            SpriteFormat::Webp { quality, .. } => quality,
        }
    }
}

#[derive(Clone, Copy)]
struct Tile {
    angle: f32,
    scale: f32,
    flip: bool,
    correct: bool,
}

pub fn decode_base(
    base_buf: &[u8],
    format: image::ImageFormat,
    opts: &GenerationOptions,
) -> crate::Result<RgbaImage> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(opts.limits.max_width);
    limits.max_image_height = Some(opts.limits.max_height);
    limits.max_alloc = Some(opts.limits.max_alloc);

    let mut reader = ImageReader::with_format(Cursor::new(base_buf), format);
    reader.limits(limits);

    Ok(reader
        .decode()
        .map_err(CaptchaError::Decode)?
        .resize_exact(
            opts.cell_size,
            opts.cell_size,
            imageops::FilterType::Nearest,
        )
        .into_rgba8())
}

pub fn create_sprite(base: &RgbaImage, opts: &GenerationOptions) -> crate::Result<(RgbaImage, u8)> {
    let font = font()?;
    let mut rng = rng();

    let mut wrong = INCORRECT_ANGLES;
    wrong.shuffle(&mut rng);

    let mut tiles = [Tile {
        angle: 0.0,
        scale: 0.0,
        flip: false,
        correct: false,
    }; TILES];

    for (i, tile) in tiles.iter_mut().enumerate() {
        tile.angle = match i {
            0 => rng.random_range(-UPRIGHT_JITTER_DEG..=UPRIGHT_JITTER_DEG),
            _ => wrong[i - 1],
        };
        tile.scale = rng.random_range(MIN_TILE_SCALE..=MAX_TILE_SCALE);
        tile.flip = rng.random_bool(0.5);
        tile.correct = i == 0;
    }

    tiles.shuffle(&mut rng);

    let sprite_width = COLS * opts.cell_size + (COLS - 1) * SPACING;
    let sprite_height = ROWS * opts.cell_size + (ROWS - 1) * SPACING;

    let mut sprite = ImageBuffer::from_pixel(sprite_width, sprite_height, SPRITE_BG);
    let mut correct_number = 0;

    #[cfg(feature = "parallel")]
    let rendered = tiles
        .par_iter()
        .map(|tile| {
            let mut out = RgbaImage::new(0, 0);
            render_tile(base, *tile, opts.cell_size, &mut out).map(|()| out)
        })
        .collect::<crate::Result<Vec<_>>>()?;

    #[cfg(not(feature = "parallel"))]
    let mut scratch = RgbaImage::new(0, 0);

    for (i, tile) in tiles.iter().enumerate() {
        #[cfg(feature = "parallel")]
        let rendered = rendered
            .get(i)
            .ok_or_else(|| CaptchaError::Internal(format!("tile {i} was not rendered")))?;

        #[cfg(not(feature = "parallel"))]
        let rendered = {
            render_tile(base, *tile, opts.cell_size, &mut scratch)?;
            &scratch
        };

        place_tile(&mut sprite, rendered, i, opts.cell_size, &mut rng, font)?;

        if tile.correct {
            correct_number = (i + 1) as u8;
        }
    }

    Ok((sprite, correct_number))
}

fn render_tile(
    base: &RgbaImage,
    tile: Tile,
    cell_size: u32,
    out: &mut RgbaImage,
) -> crate::Result<()> {
    let size = (cell_size as f32 * tile.scale) as u32;

    let mut raw = std::mem::replace(out, RgbaImage::new(0, 0)).into_raw();
    raw.resize(size as usize * size as usize * 4, 0);

    *out = RgbaImage::from_raw(size, size, raw)
        .ok_or_else(|| CaptchaError::Internal("tile buffer does not fit its dimensions".into()))?;

    warp_tile(base, tile.angle, tile.flip, SPRITE_BG, out);

    Ok(())
}

fn place_tile(
    sprite: &mut RgbaImage,
    tile: &RgbaImage,
    index: usize,
    cell_size: u32,
    rng: &mut ThreadRng,
    font: &FontArc,
) -> crate::Result<()> {
    let size = tile.width();

    let col = index as u32 % COLS;
    let row = index as u32 / COLS;

    let base_x = col * (cell_size + SPACING);
    let base_y = row * (cell_size + SPACING);

    let offset = (cell_size - size) / 2;
    let jitter_limit = offset as i32;

    let draw_x =
        (base_x + offset).saturating_add_signed(rng.random_range(-jitter_limit..=jitter_limit));
    let draw_y =
        (base_y + offset).saturating_add_signed(rng.random_range(-jitter_limit..=jitter_limit));

    sprite
        .copy_from(tile, draw_x, draw_y)
        .map_err(|e| CaptchaError::Internal(format!("copy tile into sprite buffer: {e}")))?;

    let label_x = draw_x.saturating_add(size).saturating_sub(16);
    let label_y = draw_y.saturating_add(size).saturating_sub(16);

    let scale = PxScale::from(cell_size as f32 * rng.random_range(0.13..=0.17));
    let color = Rgba([
        rng.random_range(0..100),
        rng.random_range(0..100),
        rng.random_range(0..100),
        255,
    ]);

    draw_text_mut(
        sprite,
        color,
        (label_x + rng.random_range(0..=3)) as i32,
        (label_y + rng.random_range(0..=3)) as i32,
        scale,
        font,
        LABELS[index],
    );

    Ok(())
}

fn sprite_to_base64(buf: &[u8], mime: &str) -> String {
    const PREFIX: &str = "data:";
    const INFIX: &str = ";base64,";

    let mut uri =
        String::with_capacity(PREFIX.len() + mime.len() + INFIX.len() + buf.len().div_ceil(3) * 4);

    uri.push_str(PREFIX);
    uri.push_str(mime);
    uri.push_str(INFIX);

    BASE64_STANDARD.encode_string(buf, &mut uri);

    uri
}

fn font() -> crate::Result<&'static FontArc> {
    FONT.as_ref()
        .map_err(|e| CaptchaError::Internal(format!("load embedded font: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::challenge::{DecodeLimits, generate};
    use crate::image::NoiseOptions;

    const SECRET: &[u8] = b"unit-test-secret-key-32-bytes-min!!";

    fn opts(cell_size: u32, sprite_format: SpriteFormat) -> GenerationOptions {
        GenerationOptions {
            cell_size,
            sprite_format,
            rounds: 1,
            limits: DecodeLimits::default(),
        }
    }

    fn load_base(opts: &GenerationOptions) -> RgbaImage {
        decode_base(
            include_bytes!("../assets/sample1.jpg"),
            image::ImageFormat::Jpeg,
            opts,
        )
        .expect("decode sample")
    }

    fn decode(bytes: &[u8]) {
        ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .expect("guess format")
            .decode()
            .expect("decode sprite");
    }

    fn payload(uri: &str) -> Vec<u8> {
        let b64 = uri.split_once(',').map(|x| x.1).expect("data uri payload");
        BASE64_STANDARD.decode(b64).expect("base64 decode")
    }

    #[test]
    fn sprite_prefix_jpeg_and_decode() {
        let o = opts(120, SpriteFormat::Jpeg { quality: 60 });
        let base = load_base(&o);

        let ch = generate::<SpriteUri>(
            std::slice::from_ref(&base),
            SECRET,
            &o,
            NoiseOptions::default(),
        )
        .expect("jpeg generation failed");

        assert_eq!(ch.sprites.len(), 1);
        assert!(ch.sprites[0].0.starts_with("data:image/jpeg;base64,"));

        decode(&payload(&ch.sprites[0].0));
    }

    #[test]
    fn sprite_prefix_webp_and_decode() {
        let o = opts(
            120,
            SpriteFormat::Webp {
                quality: 75,
                lossless: false,
            },
        );

        let base = load_base(&o);
        let ch = generate::<SpriteUri>(
            std::slice::from_ref(&base),
            SECRET,
            &o,
            NoiseOptions::default(),
        )
        .expect("webp generation failed");

        assert!(ch.sprites[0].0.starts_with("data:image/webp;base64,"));

        decode(&payload(&ch.sprites[0].0));
    }

    #[test]
    fn sprite_binary_jpeg_and_decode() {
        let o = opts(150, SpriteFormat::Jpeg { quality: 70 });
        let base = load_base(&o);

        let ch = generate::<SpriteBinary>(
            std::slice::from_ref(&base),
            SECRET,
            &o,
            NoiseOptions::default(),
        )
        .expect("jpeg binary generation failed");

        assert_eq!(ch.sprites[0].mime, "image/jpeg");
        assert!(!ch.sprites[0].bytes.is_empty());

        decode(&ch.sprites[0].bytes);
    }

    #[test]
    fn sprite_binary_webp_and_decode() {
        let o = opts(
            150,
            SpriteFormat::Webp {
                quality: 70,
                lossless: false,
            },
        );

        let base = load_base(&o);
        let ch = generate::<SpriteBinary>(
            std::slice::from_ref(&base),
            SECRET,
            &o,
            NoiseOptions::default(),
        )
        .expect("webp binary generation failed");

        assert_eq!(ch.sprites[0].mime, "image/webp");
        assert!(!ch.sprites[0].bytes.is_empty());

        decode(&ch.sprites[0].bytes);
    }

    #[test]
    fn tile_silhouette_is_angle_independent() {
        let base_size = 64;
        let fill = Rgba([10, 20, 30, 255]);
        let base = RgbaImage::from_pixel(base_size, base_size, fill);

        let size = 41;
        let corners = [
            (0, 0),
            (size - 1, 0),
            (0, size - 1),
            (size - 1, size - 1),
            (size / 2, 0),
        ];

        let mut areas = Vec::new();
        for angle in [0.0, 3.0, -4.9, 38.0, 88.0, 138.0, 176.0, 278.0, 320.0] {
            for flip in [false, true] {
                let mut tile = RgbaImage::new(size, size);
                warp_tile(&base, angle, flip, SPRITE_BG, &mut tile);

                for (x, y) in corners {
                    assert_eq!(
                        *tile.get_pixel(x, y),
                        SPRITE_BG,
                        "angle {angle} flip {flip} leaves tile content at ({x}, {y})"
                    );
                }

                assert_eq!(*tile.get_pixel(size / 2, size / 2), fill);

                areas.push(tile.pixels().filter(|p| **p != SPRITE_BG).count());
            }
        }

        assert!(
            areas.windows(2).all(|w| w[0] == w[1]),
            "tile area varies with angle or flip, silhouette leaks the answer: {areas:?}"
        );
    }

    #[test]
    fn render_tile_reuses_scratch_capacity() {
        let o = opts(80, SpriteFormat::Jpeg { quality: 70 });
        let base = load_base(&o);

        let mut scratch = RgbaImage::new(0, 0);
        let mut capacities = Vec::new();

        for scale in [MAX_TILE_SCALE, MIN_TILE_SCALE, 0.65, MAX_TILE_SCALE] {
            let tile = Tile {
                angle: 38.0,
                scale,
                flip: false,
                correct: false,
            };

            render_tile(&base, tile, o.cell_size, &mut scratch).expect("render");

            let expected = (o.cell_size as f32 * scale) as u32;
            assert_eq!(scratch.dimensions(), (expected, expected));

            capacities.push(scratch.as_raw().capacity());
        }

        assert!(
            capacities.windows(2).all(|w| w[0] == w[1]),
            "scratch reallocated between tiles: {capacities:?}"
        );
    }

    #[test]
    fn noise_blends_without_writing_alpha() {
        let o = opts(80, SpriteFormat::Jpeg { quality: 70 });
        let base = load_base(&o);

        let (sprite, _) = create_sprite(&base, &o).expect("sprite");

        let mut noised = sprite.clone();
        crate::image::watermark_with_noise(&mut noised, NoiseOptions::default());

        let alpha_before: Vec<u8> = sprite.pixels().map(|p| p.0[3]).collect();
        let alpha_after: Vec<u8> = noised.pixels().map(|p| p.0[3]).collect();

        assert_eq!(
            alpha_before, alpha_after,
            "noise must blend, never write into the alpha channel"
        );
        assert_ne!(
            sprite.as_raw(),
            noised.as_raw(),
            "noise must actually modify the sprite"
        );
    }

    #[test]
    fn correct_number_is_within_grid() {
        let o = opts(80, SpriteFormat::Jpeg { quality: 70 });
        let base = load_base(&o);

        for _ in 0..16 {
            let (_, correct) = create_sprite(&base, &o).expect("sprite");
            assert!(
                (1..=9).contains(&correct),
                "correct number {correct} out of grid"
            );
        }
    }
}
