use crate::SpriteFormat;

use image::{DynamicImage, GenericImageView, Rgba};
use imageproc::geometric_transformations::{Interpolation, rotate_about_center};
use rand::Rng;
use webp::Encoder as WebPEncoder;

#[derive(Clone, Copy, Default)]
pub enum NoisePattern {
    Dots,
    Lines,
    #[default]
    Grid,
}

#[derive(Clone, Copy)]
pub struct NoiseOptions {
    pub count: u32,
    pub size: u32,
    pub blur_sigma: f32,
    pub alpha: u8,
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

/// Rotate image by arbitrary angle
/// using imageproc (nearest-neighbor).
pub fn rotate_image(img: &DynamicImage, angle_deg: f32) -> DynamicImage {
    if angle_deg == 0.0 {
        return img.clone();
    }

    let rgba = img.to_rgba8();
    let bg = Rgba([255, 255, 255, 255]);
    let rotated = rotate_about_center(&rgba, angle_deg.to_radians(), Interpolation::Nearest, bg);

    DynamicImage::ImageRgba8(rotated)
}

pub fn watermark_with_noise(img: &mut DynamicImage, opts: NoiseOptions) {
    let mut rng = rand::rng();
    let (width, height) = img.dimensions();
    let mut img_buf = img.to_rgba8();

    for _ in 0..opts.count {
        let x = rng.random_range(0..width);
        let y = rng.random_range(0..height);

        let r = if opts.red {
            rng.random_range(opts.color_range.0..=opts.color_range.1)
        } else {
            0
        };
        let g = if opts.green {
            rng.random_range(opts.color_range.0..=opts.color_range.1)
        } else {
            0
        };
        let b = if opts.blue {
            rng.random_range(opts.color_range.0..=opts.color_range.1)
        } else {
            0
        };

        let color = Rgba([r, g, b, opts.alpha]);

        match opts.shape {
            NoisePattern::Dots => {
                img_buf.put_pixel(x, y, color);
            }
            NoisePattern::Lines => {
                for i in 0..opts.size {
                    if x + i < width {
                        img_buf.put_pixel(x + i, y, color);
                    }
                }
            }
            NoisePattern::Grid => {
                for dx in 0..opts.size {
                    for dy in 0..opts.size {
                        if x + dx < width && y + dy < height {
                            img_buf.put_pixel(x + dx, y + dy, color);
                        }
                    }
                }
            }
        }
    }

    *img = DynamicImage::ImageRgba8(img_buf);

    if opts.blur_sigma > 0.0 {
        *img = img.fast_blur(opts.blur_sigma);
    }
}

pub fn encode_image(img: &DynamicImage, fmt: &SpriteFormat) -> (Vec<u8>, &'static str) {
    match *fmt {
        SpriteFormat::Jpeg { quality } => {
            let mut buf = Vec::new();
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);

            let rgb = img.to_rgb8();
            let dyn_rgb = image::DynamicImage::ImageRgb8(rgb);

            if let Err(_e) = enc.encode_image(&dyn_rgb) {
                return (Vec::new(), "image/jpeg");
            }

            (buf, "image/jpeg")
        }
        SpriteFormat::Webp { quality, lossless } => {
            let rgba = img.to_rgba8();
            let enc = WebPEncoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height());

            let webp = if lossless {
                enc.encode_lossless()
            } else {
                enc.encode(quality as f32)
            };

            (webp.to_vec(), "image/webp")
        }
    }
}
