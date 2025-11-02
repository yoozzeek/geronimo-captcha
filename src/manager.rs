use crate::error::{CaptchaError, Result};
use crate::image_ops::NoiseOptions;
use crate::registry::ChallengeRegistry;
use crate::{RegistryCheckResult, challenge};

use rand::prelude::IndexedRandom;
use rand::rng;
use std::sync::Arc;
use tracing::{info, warn};
use zeroize::Zeroizing;

pub const SAMPLE_IMAGES: &[&[u8]] = &[
    include_bytes!("../assets/sample1.jpg"),
    include_bytes!("../assets/sample2.jpg"),
    include_bytes!("../assets/sample3.jpg"),
    include_bytes!("../assets/sample4.jpg"),
    include_bytes!("../assets/sample5.jpg"),
    include_bytes!("../assets/sample6.jpg"),
    include_bytes!("../assets/sample7.jpg"),
];

pub struct CaptchaManager {
    registry: Option<Arc<dyn ChallengeRegistry>>,
    challenge_ttl: u64,
    noise: NoiseOptions,
    secret: Zeroizing<Vec<u8>>,
    gen_opts: challenge::GenerationOptions,
}

impl CaptchaManager {
    pub fn new(
        secret: String,
        challenge_ttl: u64,
        noise: NoiseOptions,
        registry: Option<Arc<dyn ChallengeRegistry>>,
        gen_opts: challenge::GenerationOptions,
    ) -> Self {
        Self {
            registry,
            challenge_ttl,
            noise,
            secret: Zeroizing::new(secret.into_bytes()),
            gen_opts,
        }
    }

    pub fn generate_challenge(&self) -> Result<challenge::CaptchaChallenge> {
        let sample_image = match SAMPLE_IMAGES.choose(&mut rng()) {
            Some(img) => *img,
            None => return Err(CaptchaError::Internal("no sample images available".into())),
        };

        let challenge = challenge::generate(
            sample_image,
            self.secret.as_slice(),
            &self.gen_opts,
            self.noise,
        )?;

        if let Some(reg) = &self.registry {
            reg.register(&challenge.challenge_id);
        }

        info!(
            cell_size = self.gen_opts.cell_size,
            jpeg_quality = self.gen_opts.jpeg_quality,
            "captcha generated"
        );

        Ok(challenge)
    }

    pub fn verify_challenge(&self, challenge_id: &str, selected_index: u8) -> Result<bool> {
        if challenge_id.is_empty() {
            return Err(CaptchaError::InvalidInput("Challenge ID cannot be empty"));
        }

        if selected_index == 0 || selected_index > 9 {
            return Err(CaptchaError::InvalidInput("Selected index out of bounds"));
        }

        if let Some(registry) = &self.registry {
            let result = registry.check(challenge_id);
            if result != RegistryCheckResult::Ok {
                warn!("challenge rejected by registry: {result}");
                return Err(CaptchaError::Registry(result));
            }
        }

        let valid = challenge::verify(
            self.secret.as_slice(),
            challenge_id,
            selected_index,
            self.challenge_ttl,
        );

        if valid {
            if let Some(registry) = &self.registry {
                registry.verify(challenge_id);
            }

            info!("captcha verified successfully");
        } else if let Some(registry) = &self.registry {
            registry.note_attempt(challenge_id, false);
            warn!("captcha verification failed");
        }

        Ok(valid)
    }
}
