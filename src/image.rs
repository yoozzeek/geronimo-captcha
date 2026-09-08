use crate::SpriteFormat;
use crate::error::{CaptchaError, Result};

use image::{ExtendedColorType, RgbImage, Rgba, RgbaImage, imageops};
use imageproc::geometric_transformations::{Border, Interpolation, Projection, warp_into};
use rand::RngExt;
use webp::{Encoder as WebPEncoder, WebPConfig};

const MAX_NOISE_COUNT: u32 = 1_000_000;
const MAX_NOISE_SIZE: u32 = 64;
const MAX_NOISE_WORK: u64 = 4_000_000;
const MAX_BLUR_SIGMA: f32 = 32.0;

// libwebp effort level, valid range 0..=6.
const WEBP_METHOD: i32 = 1;

// JPEG at the qualities this crate encodes lands near 0.2 bytes per pixel.
const JPEG_BYTES_PER_PIXEL_SHIFT: usize = 4;

#[derive(Clone, Copy, Debug, Default)]
pub enum NoisePattern {
    Dots,
    Lines,
    #[default]
    Grid,
}

/// Parameters for the noise watermark drawn over
/// the composed sprite.
#[derive(Clone, Copy, Debug)]
pub struct NoiseOptions {
    /// Number of noise marks. Capped at 1_000_000.
    /// `count` multiplied by the pixels one mark
    /// covers (1, `size`, or `size` squared, by
    /// pattern) must not exceed 4_000_000.
    pub count: u32,

    /// Edge length of one mark, in pixels,
    /// for [`NoisePattern::Lines`]
    /// and [`NoisePattern::Grid`].
    /// Capped at 64.
    pub size: u32,

    /// Gaussian blur applied after the noise.
    /// Zero disables it. Capped at 32.0.
    pub blur_sigma: f32,

    /// Opacity of each mark, 0 (invisible) to 255 (opaque).
    /// Marks are alpha-blended onto the sprite.
    pub alpha: u8,

    /// Inclusive channel range each mark's colour
    /// is drawn from. `.0` must not exceed `.1`.
    pub color_range: (u8, u8),
    pub shape: NoisePattern,
    pub red: bool,
    pub green: bool,
    pub blue: bool,
}

impl Default for NoiseOptions {
    fn default() -> Self {
        NoiseOptions {
            count: 300 * 9,
            size: 2,
            alpha: 100,
            color_range: (0, 255),
            shape: NoisePattern::default(),
            red: true,
            green: true,
            blue: true,
            blur_sigma: 0.7,
        }
    }
}

impl NoiseOptions {
    /// # Errors
    ///
    /// Returns [`CaptchaError::InvalidInput`] if
    /// any field is outside its documented range.
    pub fn validate(&self) -> Result<()> {
        if self.color_range.0 > self.color_range.1 {
            return Err(CaptchaError::InvalidInput(format!(
                "noise color_range is inverted: {} > {}",
                self.color_range.0, self.color_range.1
            )));
        }

        if self.count > MAX_NOISE_COUNT {
            return Err(CaptchaError::InvalidInput(format!(
                "noise count {} exceeds maximum {MAX_NOISE_COUNT}",
                self.count
            )));
        }

        if self.size == 0 || self.size > MAX_NOISE_SIZE {
            return Err(CaptchaError::InvalidInput(format!(
                "noise size {} outside 1..={MAX_NOISE_SIZE}",
                self.size
            )));
        }

        if !self.blur_sigma.is_finite() || self.blur_sigma < 0.0 || self.blur_sigma > MAX_BLUR_SIGMA
        {
            return Err(CaptchaError::InvalidInput(format!(
                "noise blur_sigma {} outside 0.0..={MAX_BLUR_SIGMA}",
                self.blur_sigma
            )));
        }

        let work = u64::from(self.count) * self.pixels_per_mark();
        if work > MAX_NOISE_WORK {
            return Err(CaptchaError::InvalidInput(format!(
                "noise count {} at size {} blends {work} pixels, maximum is {MAX_NOISE_WORK}",
                self.count, self.size
            )));
        }

        Ok(())
    }

    fn pixels_per_mark(&self) -> u64 {
        match self.shape {
            NoisePattern::Dots => 1,
            NoisePattern::Lines => u64::from(self.size),
            NoisePattern::Grid => u64::from(self.size) * u64::from(self.size),
        }
    }
}

/// Caller must compose the result onto `bg`.
pub fn warp_tile(base: &RgbaImage, angle_deg: f32, flip: bool, bg: Rgba<u8>, out: &mut RgbaImage) {
    let (in_w, in_h) = base.dimensions();
    let (out_w, out_h) = out.dimensions();

    let scale = out_w as f32 / in_w as f32;
    let scale_x = match flip {
        true => -scale,
        false => scale,
    };

    let projection = Projection::translate(out_w as f32 / 2.0, out_h as f32 / 2.0)
        * Projection::scale(scale_x, scale)
        * Projection::rotate(angle_deg.to_radians())
        * Projection::translate(-(in_w as f32) / 2.0, -(in_h as f32) / 2.0);

    warp_into(
        base,
        projection,
        Interpolation::Bilinear,
        Border::Constant(bg),
        out,
    );

    mask_outside_circle(out, bg);
}

pub fn watermark_with_noise(img: &mut RgbaImage, opts: NoiseOptions) {
    let mut rng = rand::rng();
    let (width, height) = img.dimensions();

    for _ in 0..opts.count {
        let x = rng.random_range(0..width);
        let y = rng.random_range(0..height);

        let channel = |enabled: bool, rng: &mut _| match enabled {
            true => RngExt::random_range(rng, opts.color_range.0..=opts.color_range.1),
            false => 0,
        };

        let color = Rgba([
            channel(opts.red, &mut rng),
            channel(opts.green, &mut rng),
            channel(opts.blue, &mut rng),
            opts.alpha,
        ]);

        match opts.shape {
            NoisePattern::Dots => blend_pixel(img, x, y, color),
            NoisePattern::Lines => {
                for i in 0..opts.size.min(width - x) {
                    blend_pixel(img, x + i, y, color);
                }
            }
            NoisePattern::Grid => {
                for dx in 0..opts.size.min(width - x) {
                    for dy in 0..opts.size.min(height - y) {
                        blend_pixel(img, x + dx, y + dy, color);
                    }
                }
            }
        }
    }
}

pub fn blur_rgb(img: RgbImage, sigma: f32) -> RgbImage {
    match sigma > 0.0 {
        true => imageops::fast_blur(&img, sigma),
        false => img,
    }
}

pub fn encode_image(img: &RgbImage, fmt: &SpriteFormat) -> Result<(Vec<u8>, &'static str)> {
    match *fmt {
        SpriteFormat::Jpeg { quality } => {
            let mut buf = Vec::with_capacity(img.as_raw().len() >> JPEG_BYTES_PER_PIXEL_SHIFT);
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);

            enc.encode(
                img.as_raw(),
                img.width(),
                img.height(),
                ExtendedColorType::Rgb8,
            )
            .map_err(CaptchaError::Encode)?;

            Ok((buf, "image/jpeg"))
        }
        SpriteFormat::Webp { quality, lossless } => {
            let enc = WebPEncoder::from_rgb(img.as_raw(), img.width(), img.height());

            let webp = match lossless {
                true => enc.encode_lossless(),
                false => {
                    let mut config = WebPConfig::new()
                        .map_err(|()| CaptchaError::Internal("build webp config".into()))?;
                    config.quality = quality as f32;
                    config.method = WEBP_METHOD;

                    enc.encode_advanced(&config)
                        .map_err(|e| CaptchaError::Internal(format!("webp encode: {e:?}")))?
                }
            };

            Ok((webp.to_vec(), "image/webp"))
        }
    }
}

fn mask_outside_circle(img: &mut RgbaImage, fill: Rgba<u8>) {
    let (width, height) = img.dimensions();

    let radius = width.min(height) as f32 / 2.0 - 1.0;
    if radius <= 0.0 {
        return;
    }

    let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
    let r2 = radius * radius;

    for y in 0..height {
        let dy = y as f32 + 0.5 - cy;
        let span = r2 - dy * dy;

        let (lo, hi) = match span > 0.0 {
            true => {
                let dx = span.sqrt();
                (
                    (cx - dx).ceil().clamp(0.0, width as f32) as u32,
                    (cx + dx).floor().clamp(0.0, width as f32) as u32,
                )
            }
            false => (width, width),
        };

        for x in (0..lo).chain(hi..width) {
            img.put_pixel(x, y, fill);
        }
    }
}

fn blend_pixel(img: &mut RgbaImage, x: u32, y: u32, src: Rgba<u8>) {
    let dst = img.get_pixel_mut(x, y);
    let a = u32::from(src.0[3]);
    let inv = 255 - a;

    for c in 0..3 {
        dst.0[c] = ((u32::from(src.0[c]) * a + u32::from(dst.0[c]) * inv) / 255) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_validation_rejects_out_of_range() {
        let cases = [
            (
                "inverted color_range",
                NoiseOptions {
                    color_range: (200, 100),
                    ..NoiseOptions::default()
                },
            ),
            (
                "count over cap",
                NoiseOptions {
                    count: MAX_NOISE_COUNT + 1,
                    shape: NoisePattern::Dots,
                    ..NoiseOptions::default()
                },
            ),
            (
                "size zero",
                NoiseOptions {
                    size: 0,
                    ..NoiseOptions::default()
                },
            ),
            (
                "size over cap",
                NoiseOptions {
                    size: MAX_NOISE_SIZE + 1,
                    ..NoiseOptions::default()
                },
            ),
            (
                "negative blur",
                NoiseOptions {
                    blur_sigma: -0.1,
                    ..NoiseOptions::default()
                },
            ),
            (
                "blur over cap",
                NoiseOptions {
                    blur_sigma: MAX_BLUR_SIGMA + 0.1,
                    ..NoiseOptions::default()
                },
            ),
            (
                "blur nan",
                NoiseOptions {
                    blur_sigma: f32::NAN,
                    ..NoiseOptions::default()
                },
            ),
            (
                "blur infinite",
                NoiseOptions {
                    blur_sigma: f32::INFINITY,
                    ..NoiseOptions::default()
                },
            ),
            (
                "grid work over cap",
                NoiseOptions {
                    count: MAX_NOISE_COUNT,
                    size: MAX_NOISE_SIZE,
                    shape: NoisePattern::Grid,
                    ..NoiseOptions::default()
                },
            ),
            (
                "lines work over cap",
                NoiseOptions {
                    count: MAX_NOISE_COUNT,
                    size: MAX_NOISE_SIZE,
                    shape: NoisePattern::Lines,
                    ..NoiseOptions::default()
                },
            ),
        ];

        for (name, noise) in cases {
            assert!(
                matches!(noise.validate(), Err(CaptchaError::InvalidInput(_))),
                "{name} should be rejected"
            );
        }
    }

    #[test]
    fn noise_validation_accepts_bounds() {
        let accepted = [
            NoiseOptions::default(),
            NoiseOptions {
                count: 0,
                ..NoiseOptions::default()
            },
            NoiseOptions {
                count: MAX_NOISE_COUNT,
                shape: NoisePattern::Dots,
                ..NoiseOptions::default()
            },
            NoiseOptions {
                size: MAX_NOISE_SIZE,
                count: 1,
                ..NoiseOptions::default()
            },
            NoiseOptions {
                blur_sigma: 0.0,
                ..NoiseOptions::default()
            },
            NoiseOptions {
                blur_sigma: MAX_BLUR_SIGMA,
                ..NoiseOptions::default()
            },
            NoiseOptions {
                color_range: (7, 7),
                ..NoiseOptions::default()
            },
        ];

        for noise in accepted {
            assert!(noise.validate().is_ok(), "{noise:?} should be accepted");
        }
    }
}
