#![forbid(unsafe_code)]

//! A JavaScript-free CAPTCHA. A speed bump for bulk
//! automation, layered over server-side rate limiting.
//! Not a wall.
//!
//! A challenge is one to five rounds. Each round
//! renders its own source image at nine orientations
//! in a 3x3 grid, exactly one of them upright. The
//! challenge id is an HMAC-SHA256 tag binding the
//! length-prefixed correct tile numbers to a nonce
//! and a timestamp. Verification recomputes the tag
//! over the client's guesses and compares it in
//! constant time. No answer is stored server-side.
//!
//! Replay and brute-force limiting live entirely
//! in [`ChallengeRegistry`]. Without one, nothing
//! limits guesses; a single round falls in nine.
//!
//! The [`SampleSet`] is the security boundary.
//! Supply your own corpus and rotate it.
//!
//! ```no_run
//! use geronimo_captcha::{
//!     CaptchaManager, ChallengeInMemoryRegistry, GenerationOptions,
//!     NoiseOptions, SampleFormat, SampleSet, SpriteUri,
//! };
//! use std::sync::Arc;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let ttl_secs = 60;
//!     let registry = Arc::new(ChallengeInMemoryRegistry::new(ttl_secs, 3, 10_000)?);
//!
//!     let images = std::fs::read_dir("/srv/captcha-corpus")?
//!         .filter_map(|e| std::fs::read(e.ok()?.path()).ok())
//!         .collect();
//!     let samples = SampleSet::new(SampleFormat::Jpeg, images)?;
//!
//!     let mgr = CaptchaManager::new(
//!         "an-example-secret-key-of-32-bytes".to_string(),
//!         ttl_secs,
//!         NoiseOptions::default(),
//!         Some(registry),
//!         GenerationOptions::default(),
//!         &samples,
//!     )?;
//!
//!     let challenge = mgr.generate_challenge::<SpriteUri>()?;
//!
//!     // One sprite per round, rendered into the form
//!     let img_srcs: Vec<String> = challenge.sprites.into_iter().map(|s| s.0).collect();
//!     let challenge_id = challenge.challenge_id;
//!
//!     // One index per round, in issue order
//!     let verified = mgr.verify_challenge(&challenge_id, &[5])?;
//!
//!     Ok(())
//! }
//! ```

mod challenge;
mod error;
mod image;
mod manager;
mod registry;
mod samples;
mod sprite;
mod utils;

pub use challenge::{CaptchaChallenge, DecodeLimits, GenerationOptions};
pub use error::{CaptchaError, RegistryRejection, Result};
pub use image::{NoiseOptions, NoisePattern};
pub use manager::CaptchaManager;
pub use registry::{ChallengeInMemoryRegistry, ChallengeRegistry};
pub use samples::{SampleFormat, SampleSet};
pub use sprite::{SpriteBinary, SpriteFormat, SpriteTarget, SpriteUri};

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct Readme;
