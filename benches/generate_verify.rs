use criterion::{BatchSize, Criterion, Throughput, black_box, criterion_group, criterion_main};
use geronimo_captcha::{
    CaptchaManager, GenerationOptions, NoiseOptions, SpriteBinary, SpriteFormat, SpriteUri,
};

fn make_mgr(cell: u32, q: u8, ttl: u64) -> CaptchaManager {
    make_mgr_with(cell, SpriteFormat::Jpeg { quality: q }, ttl)
}

fn make_mgr_with(cell: u32, format: SpriteFormat, ttl: u64) -> CaptchaManager {
    let opts = GenerationOptions {
        cell_size: cell,
        sprite_format: format,
        limits: None,
    };
    let noise = NoiseOptions::default();
    let secret = String::from("bench-secret");

    CaptchaManager::new(secret, ttl, noise, None, opts)
}

fn bench_verify(c: &mut Criterion) {
    let mgr_ok = make_mgr(150, 20, 60);
    let mgr_expired = make_mgr(150, 20, 0);

    c.bench_function("verify_e2e/ok_vs_wrong_and_expired", |b| {
        b.iter_batched(
            || mgr_ok.generate_challenge::<SpriteUri>().unwrap(),
            |ch| {
                let _ = mgr_ok.verify_challenge(&ch.challenge_id, 5); // wrong guess
                let _ = mgr_expired.verify_challenge(&ch.challenge_id, 5); // expired fast-path
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_generate_jpeg(c: &mut Criterion) {
    let mut group = c.benchmark_group("generate_e2e_jpeg");

    let configs = [(100u32, 70u8), (150u32, 70u8), (200u32, 70u8)];

    for (cell, q) in configs {
        let mgr_uri = make_mgr_with(cell, SpriteFormat::Jpeg { quality: q }, 60);
        let mgr_bin = make_mgr_with(cell, SpriteFormat::Jpeg { quality: q }, 60);

        group.throughput(Throughput::Elements(1));
        group.bench_function(format!("cell{cell}_q{q}/uri"), |b| {
            b.iter(|| {
                let ch = mgr_uri.generate_challenge::<SpriteUri>().unwrap();
                black_box(ch.challenge_id);
                black_box(ch.sprite.0);
            });
        });
        group.bench_function(format!("cell{cell}_q{q}/bin"), |b| {
            b.iter(|| {
                let ch = mgr_bin.generate_challenge::<SpriteBinary>().unwrap();
                black_box(ch.challenge_id);
                black_box(ch.sprite.bytes.len());
            });
        });
    }

    group.finish();
}

fn bench_generate_webp(c: &mut Criterion) {
    let mut group = c.benchmark_group("generate_e2e_webp");

    let configs = [(100u32, 70u8), (150u32, 70u8), (200u32, 70u8)];

    for (cell, q) in configs {
        let fmt = SpriteFormat::Webp {
            quality: q,
            lossless: false,
        };
        let mgr_uri = make_mgr_with(cell, fmt, 60);
        let mgr_bin = make_mgr_with(cell, fmt, 60);

        group.throughput(Throughput::Elements(1));
        group.bench_function(format!("cell{cell}_q{q}/uri"), |b| {
            b.iter(|| {
                let ch = mgr_uri.generate_challenge::<SpriteUri>().unwrap();
                black_box(ch.challenge_id);
                black_box(ch.sprite.0);
            });
        });
        group.bench_function(format!("cell{cell}_q{q}/bin"), |b| {
            b.iter(|| {
                let ch = mgr_bin.generate_challenge::<SpriteBinary>().unwrap();
                black_box(ch.challenge_id);
                black_box(ch.sprite.bytes.len());
            });
        });
    }

    group.finish();
}

pub fn criterion_benches(c: &mut Criterion) {
    bench_generate_jpeg(c);
    bench_generate_webp(c);
    bench_verify(c);
}

criterion_group!(benches, criterion_benches);
criterion_main!(benches);
