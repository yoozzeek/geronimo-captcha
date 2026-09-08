use crate::challenge::{self, MAX_CHALLENGE_ID_LEN};
use crate::error::{CaptchaError, Result};
use crate::image::NoiseOptions;
use crate::registry::ChallengeRegistry;
use crate::samples::SampleSet;
use crate::sprite::{SpriteFormat, SpriteTarget};
use crate::utils::MAX_TTL_SECS;

use image::RgbaImage;
use std::sync::Arc;
use tracing::{debug, info, warn};
use zeroize::Zeroizing;

const MIN_SECRET_LEN: usize = 32;

pub struct CaptchaManager {
    bases: Vec<RgbaImage>,
    registry: Option<Arc<dyn ChallengeRegistry>>,
    challenge_ttl: u64,
    noise: NoiseOptions,
    secret: Zeroizing<Vec<u8>>,
    gen_opts: challenge::GenerationOptions,
}

impl CaptchaManager {
    /// `secret` must be at least 32 bytes. It is zeroized on drop.
    ///
    /// Without a `registry` every challenge accepts nine guesses.
    ///
    /// `samples` is decoded once here.
    ///
    /// # Errors
    ///
    /// Returns [`CaptchaError::InvalidInput`] if `secret` is shorter
    /// than 32 bytes, if `challenge_ttl` is outside 1..=86400, or
    /// if `noise`, `gen_opts` or `samples` are out of range.
    /// Returns [`CaptchaError::Decode`] if a sample fails to decode.
    pub fn new(
        secret: String,
        challenge_ttl: u64,
        noise: NoiseOptions,
        registry: Option<Arc<dyn ChallengeRegistry>>,
        gen_opts: challenge::GenerationOptions,
        samples: &SampleSet,
    ) -> Result<Self> {
        let secret = Zeroizing::new(secret.into_bytes());

        if secret.len() < MIN_SECRET_LEN {
            return Err(CaptchaError::InvalidInput(format!(
                "secret must be at least {MIN_SECRET_LEN} bytes, got {}",
                secret.len()
            )));
        }

        if challenge_ttl == 0 || challenge_ttl > MAX_TTL_SECS {
            return Err(CaptchaError::InvalidInput(format!(
                "challenge_ttl {challenge_ttl} outside 1..={MAX_TTL_SECS}"
            )));
        }

        noise.validate()?;
        gen_opts.validate()?;

        let bases = samples.decode_all(&gen_opts)?;

        if registry.is_none() {
            warn!("captcha manager built without a registry, brute-force is unrestricted");
        }

        info!(
            rounds = gen_opts.rounds,
            blind_guess_pct = 100.0 / 9f64.powi(i32::from(gen_opts.rounds)),
            "captcha manager ready"
        );

        Ok(Self {
            bases,
            registry,
            challenge_ttl,
            noise,
            secret,
            gen_opts,
        })
    }

    /// # Errors
    ///
    /// Returns [`CaptchaError::Decode`] or [`CaptchaError::Encode`]
    /// when the sprite cannot be built, and [`CaptchaError::Clock`]
    /// when the system clock predates the unix epoch.
    pub fn generate_challenge<T: SpriteTarget>(&self) -> Result<challenge::CaptchaChallenge<T>> {
        let challenge = challenge::generate::<T>(
            &self.bases,
            self.secret.as_slice(),
            &self.gen_opts,
            self.noise,
        )?;

        if let Some(reg) = &self.registry {
            reg.register(&challenge.challenge_id)?;
        }

        let (format, quality, lossless) = match self.gen_opts.sprite_format {
            SpriteFormat::Jpeg { quality } => ("jpeg", quality, false),
            SpriteFormat::Webp { quality, lossless } => (
                if lossless { "webp-lossless" } else { "webp" },
                quality,
                lossless,
            ),
        };

        debug!(
            cell_size = self.gen_opts.cell_size,
            format = format,
            quality = quality,
            lossless = lossless,
            "captcha generated"
        );

        Ok(challenge)
    }

    /// `selected` holds one index per round, in the order
    /// the sprites were issued. Returns `Ok(false)` for
    /// a well-formed guess that does not match.
    ///
    /// # Errors
    ///
    /// Returns [`CaptchaError::InvalidInput`] for a
    /// `challenge_id` that is empty or longer than an
    /// issued id, a `selected` length other than
    /// `GenerationOptions::rounds`, or any index
    /// outside 1..=9. Returns [`CaptchaError::Registry`]
    /// when the registry refuses the id, including a
    /// correct guess that lost a race to an earlier one.
    pub fn verify_challenge(&self, challenge_id: &str, selected: &[u8]) -> Result<bool> {
        if challenge_id.is_empty() || challenge_id.len() > MAX_CHALLENGE_ID_LEN {
            return Err(CaptchaError::InvalidInput(format!(
                "challenge id length {} outside 1..={MAX_CHALLENGE_ID_LEN}",
                challenge_id.len()
            )));
        }

        if selected.len() != self.gen_opts.rounds as usize {
            return Err(CaptchaError::InvalidInput(format!(
                "expected {} indices, got {}",
                self.gen_opts.rounds,
                selected.len()
            )));
        }

        if selected.iter().any(|&i| i == 0 || i > 9) {
            return Err(CaptchaError::InvalidInput(
                "Selected index out of bounds".into(),
            ));
        }

        if let Some(registry) = &self.registry {
            registry
                .begin_attempt(challenge_id)
                .inspect_err(|e| warn!("challenge refused by registry: {e}"))?;
        }

        let valid = challenge::verify(
            self.secret.as_slice(),
            challenge_id,
            selected,
            self.challenge_ttl,
        );

        match (valid, &self.registry) {
            (true, Some(registry)) => {
                registry
                    .mark_verified(challenge_id)
                    .inspect_err(|e| warn!("correct guess refused by registry: {e}"))?;
                debug!("captcha verified successfully");
            }
            (true, None) => debug!("captcha verified successfully"),
            (false, _) => debug!("captcha verification failed"),
        }

        Ok(valid)
    }
}
