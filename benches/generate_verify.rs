use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use geronimo_captcha::{
    CaptchaManager, DecodeLimits, GenerationOptions, NoiseOptions, SampleSet, SpriteBinary,
    SpriteFormat, SpriteUri,
};
use std::hint::black_box;

const SECRET: &str = "bench-secret-key-at-least-32-bytes";

fn make_mgr(cell: u32, format: SpriteFormat, ttl: u64) -> CaptchaManager {
    let opts = GenerationOptions {
        cell_size: cell,
        sprite_format: format,
        rounds: 1,
        limits: DecodeLimits::default(),
    };

    CaptchaManager::new(
        SECRET.to_string(),
        ttl,
        NoiseOptions::default(),
        None,
        opts,
        &SampleSet::demo_insecure(),
    )
    .expect("valid manager configuration")
}

fn expired_challenge_id() -> String {
    let nonce = "00000000-0000-4000-8000-000000000000";
    let code = format!("{}=", "A".repeat(43));

    format!("{nonce}:1000000000:{code}")
}

fn bench_verify(c: &mut Criterion) {
    let mgr = make_mgr(150, SpriteFormat::Jpeg { quality: 20 }, 60);
    let expired = expired_challenge_id();

    c.bench_function("verify_e2e/wrong_guess", |b| {
        b.iter_batched(
            || mgr.generate_challenge::<SpriteUri>().unwrap(),
            |ch| black_box(mgr.verify_challenge(&ch.challenge_id, &[5])),
            BatchSize::SmallInput,
        )
    });

    c.bench_function("verify_e2e/expired_fast_path", |b| {
        b.iter(|| black_box(mgr.verify_challenge(&expired, &[5])))
    });
}

fn bench_generate(c: &mut Criterion, name: &str, format: impl Fn(u8) -> SpriteFormat) {
    let mut group = c.benchmark_group(name);

    for (cell, q) in [(100u32, 70u8), (150, 70), (200, 70)] {
        let mgr_uri = make_mgr(cell, format(q), 60);
        let mgr_bin = make_mgr(cell, format(q), 60);

        group.throughput(Throughput::Elements(1));

        group.bench_function(format!("cell{cell}_q{q}/uri"), |b| {
            b.iter(|| {
                let ch = mgr_uri.generate_challenge::<SpriteUri>().unwrap();
                black_box(ch.challenge_id);
                black_box(ch.sprites);
            });
        });

        group.bench_function(format!("cell{cell}_q{q}/bin"), |b| {
            b.iter(|| {
                let ch = mgr_bin.generate_challenge::<SpriteBinary>().unwrap();
                black_box(ch.challenge_id);
                black_box(ch.sprites);
            });
        });
    }

    group.finish();
}

pub fn criterion_benches(c: &mut Criterion) {
    bench_generate(c, "generate_e2e_jpeg", |quality| SpriteFormat::Jpeg {
        quality,
    });
    bench_generate(c, "generate_e2e_webp", |quality| SpriteFormat::Webp {
        quality,
        lossless: false,
    });
    bench_verify(c);
}

criterion_group!(benches, criterion_benches);
criterion_main!(benches);
