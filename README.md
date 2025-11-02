# geronimo-captcha

[![CI](https://github.com/yoozzeek/geronimo-captcha/actions/workflows/ci.yml/badge.svg)](https://github.com/yoozzeek/geronimo-captcha/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/geronimo-captcha.svg)](https://crates.io/crates/geronimo-captcha)
[![Docs.rs](https://docs.rs/geronimo-captcha/badge.svg)](https://docs.rs/geronimo-captcha)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache2-yellow.svg)](./LICENSE)

Secure, AI-resistant, JavaScript-free CAPTCHA built in Rust.
Confuses bots, but delights humans.

<img src="./logo.jpg" alt="geronimo-captcha logo" width="260"/>

## What it does

- Renders a 3×3 sprite with one correctly oriented tile
- Random jitter, label offset, colored noise, JPEG artifacts
- Stateless HMAC-signed challenge id with TTL

### Challenge examples

<img src="./examples/examples-sprite.jpg" alt="Challenge examples" width="580"/>

## Todos

- [x] Captcha core, image and sprite generation helpers + in-memory challenge registry
- [x] Publish crate on crates.io
- [ ] Code examples, demo webpage
- [ ] Custom fonts and sample sets
- [ ] Redis challenge registry implementation

## Quick start

### Generate

```rust
use std::sync::Arc;
use geronimo_captcha::{
    CaptchaManager, ChallengeInMemoryRegistry, GenerationOptions, NoiseOptions,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let secret = "your-secret-key".to_string();
    let ttl_secs = 60;
    let noise = NoiseOptions::default();
    let gen = GenerationOptions {
        cell_size: 150,
        jpeg_quality: 20,
        limits: None,
    };
    let registry = Arc::new(ChallengeInMemoryRegistry::new(ttl_secs, 3));

    let mgr = CaptchaManager::new(secret, ttl_secs, noise, Some(registry), gen);
    let challenge = mgr.generate_challenge()?;

    // Render to client
    let img_src = challenge.sprite_uri;         // data:image/jpeg;base64,...
    let challenge_id = challenge.challenge_id;  // send/store with form

    println!("img_src prefix: {}", &img_src[..32.min(img_src.len())]);
    println!("challenge_id: {}", challenge_id);

    Ok(())
}
```

### Verify

```rust
use geronimo_captcha::{CaptchaManager, GenerationOptions, NoiseOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Must use the same secret and TTL as on generation side
    let secret = "your-secret-key".to_string();
    let ttl_secs = 60;
    let noise = NoiseOptions::default();
    let gen = GenerationOptions {
        cell_size: 150,
        jpeg_quality: 20,
        limits: None,
    };
    let mgr = CaptchaManager::new(secret, ttl_secs, noise, None, gen);

    // Normally you get these from the client
    let challenge_id = "nonce:1730534400:BASE64_HMAC".to_string();
    let user_choice: u8 = 7;

    let ok = mgr.verify_challenge(&challenge_id, user_choice)?;
    println!("verified: {ok}");

    Ok(())
}
```

## License

This project is licensed under the Apache 2.0 License. See [LICENSE](./LICENSE) for details.
