use crate::challenge::GenerationOptions;
use crate::error::{CaptchaError, Result};
use crate::sprite::decode_base;

use image::{ImageFormat, RgbaImage};
use tracing::warn;

const MAX_SAMPLES: usize = 4096;
const MAX_CORPUS_BYTES: usize = 512 * 1024 * 1024;
const RECOMMENDED_MIN_SAMPLES: usize = 32;

const DEMO_SAMPLES: &[&[u8]] = &[
    include_bytes!("../assets/sample1.jpg"),
    include_bytes!("../assets/sample2.jpg"),
    include_bytes!("../assets/sample3.jpg"),
    include_bytes!("../assets/sample4.jpg"),
    include_bytes!("../assets/sample5.jpg"),
    include_bytes!("../assets/sample6.jpg"),
    include_bytes!("../assets/sample7.jpg"),
    include_bytes!("../assets/sample8.jpg"),
];

/// Encoded format of every image in a [`SampleSet`].
/// The decoder is pinned to this
/// rather than guessing from content.
#[derive(Clone, Copy, Debug)]
pub enum SampleFormat {
    Jpeg,
    Webp,
}

impl SampleFormat {
    fn to_image_format(self) -> ImageFormat {
        match self {
            SampleFormat::Jpeg => ImageFormat::Jpeg,
            SampleFormat::Webp => ImageFormat::WebP,
        }
    }
}

/// Source images a challenge draws its nine tiles from.
///
/// An attacker who holds the same images can render every
/// rotation offline and match the upright tile without
/// any model. Measured at 99.5% top-1 accuracy against
/// a known corpus. Treat the corpus as the security boundary:
/// supply your own, keep it large, and rotate it.
///
/// A corpus of `n` images is fully harvested in roughly
/// `n * ln(n)` requests, after which its size no longer helps.
pub struct SampleSet {
    images: Vec<Vec<u8>>,
    format: SampleFormat,
}

impl SampleSet {
    /// # Errors
    ///
    /// Returns [`CaptchaError::InvalidInput`]
    /// if `images` is empty, longer than 4096,
    /// or contains an empty entry.
    pub fn new(format: SampleFormat, images: Vec<Vec<u8>>) -> Result<Self> {
        if images.is_empty() {
            return Err(CaptchaError::InvalidInput(
                "sample set must contain at least one image".into(),
            ));
        }

        if images.len() > MAX_SAMPLES {
            return Err(CaptchaError::InvalidInput(format!(
                "sample set holds {} images, maximum is {MAX_SAMPLES}",
                images.len()
            )));
        }

        if let Some(i) = images.iter().position(Vec::is_empty) {
            return Err(CaptchaError::InvalidInput(format!(
                "sample image at index {i} is empty"
            )));
        }

        if images.len() < RECOMMENDED_MIN_SAMPLES {
            warn!(
                samples = images.len(),
                recommended = RECOMMENDED_MIN_SAMPLES,
                "small sample corpus, a harvested corpus defeats the challenge"
            );
        }

        Ok(Self { images, format })
    }

    /// The eight images published inside this crate.
    ///
    /// Anyone can download them from crates.io and
    /// precompute the answer to every challenge they
    /// produce. Tests, examples and benchmarks only.
    pub fn demo_insecure() -> Self {
        warn!("using the published demo sample set, challenges are trivially solvable");

        Self {
            images: DEMO_SAMPLES.iter().map(|b| b.to_vec()).collect(),
            format: SampleFormat::Jpeg,
        }
    }

    pub(crate) fn decode_all(&self, opts: &GenerationOptions) -> Result<Vec<RgbaImage>> {
        let per_image = (opts.cell_size as usize)
            .checked_mul(opts.cell_size as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| CaptchaError::InvalidInput("cell_size overflows".into()))?;

        let total = per_image
            .checked_mul(self.images.len())
            .ok_or_else(|| CaptchaError::InvalidInput("sample corpus overflows".into()))?;

        if total > MAX_CORPUS_BYTES {
            return Err(CaptchaError::InvalidInput(format!(
                "decoded corpus needs {total} bytes, maximum is {MAX_CORPUS_BYTES}"
            )));
        }

        let format = self.format.to_image_format();

        self.images
            .iter()
            .map(|buf| decode_base(buf, format, opts))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DecodeLimits, SpriteFormat};

    fn opts(cell_size: u32) -> GenerationOptions {
        GenerationOptions {
            cell_size,
            sprite_format: SpriteFormat::default(),
            rounds: 1,
            limits: DecodeLimits::default(),
        }
    }

    #[test]
    fn new_rejects_degenerate_corpora() {
        assert!(SampleSet::new(SampleFormat::Jpeg, Vec::new()).is_err());

        let too_many = vec![vec![1u8]; MAX_SAMPLES + 1];
        assert!(SampleSet::new(SampleFormat::Jpeg, too_many).is_err());

        let with_empty = vec![vec![1u8], Vec::new(), vec![1u8]];
        assert!(SampleSet::new(SampleFormat::Jpeg, with_empty).is_err());

        assert!(SampleSet::new(SampleFormat::Jpeg, vec![vec![1u8]]).is_ok());
    }

    #[test]
    fn demo_set_decodes() {
        let set = SampleSet::demo_insecure();

        let bases = set.decode_all(&opts(64)).expect("decode demo set");
        assert_eq!(bases.len(), DEMO_SAMPLES.len());
        assert!(bases.iter().all(|b| b.width() == 64 && b.height() == 64));
    }

    #[test]
    fn decode_all_rejects_oversized_corpus() {
        let images = vec![vec![1u8]; 200];
        let set = SampleSet::new(SampleFormat::Jpeg, images).expect("valid set");

        let err = set.decode_all(&opts(1024)).expect_err("corpus cap");
        assert!(matches!(err, CaptchaError::InvalidInput(_)));
    }

    #[test]
    fn format_is_pinned_not_guessed() {
        let jpeg = include_bytes!("../assets/sample1.jpg").to_vec();
        let set = SampleSet::new(SampleFormat::Webp, vec![jpeg]).expect("valid set");

        let err = set
            .decode_all(&opts(64))
            .expect_err("jpeg must not decode as webp");
        assert!(matches!(err, CaptchaError::Decode(_)));
    }

    #[test]
    fn webp_corpus_decodes() {
        let jpeg = SampleSet::demo_insecure()
            .decode_all(&opts(64))
            .expect("decode demo")
            .remove(0);

        let rgb = image::DynamicImage::ImageRgba8(jpeg).into_rgb8();
        let (bytes, _) = crate::image::encode_image(
            &rgb,
            &SpriteFormat::Webp {
                quality: 80,
                lossless: false,
            },
        )
        .expect("encode webp");

        let set = SampleSet::new(SampleFormat::Webp, vec![bytes]).expect("valid set");
        let bases = set.decode_all(&opts(64)).expect("decode webp corpus");

        assert_eq!(bases.len(), 1);
        assert_eq!(bases[0].width(), 64);
    }
}
