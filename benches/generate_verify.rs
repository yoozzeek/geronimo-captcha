use criterion::{BatchSize, Criterion, Throughput, black_box, criterion_group, criterion_main};
use geronimo_captcha::{CaptchaManager, GenerationOptions, NoiseOptions};

fn make_mgr(cell: u32, q: u8, ttl: u64) -> CaptchaManager {
    let opts = GenerationOptions {
        cell_size: cell,
        jpeg_quality: q,
        limits: None,
    };
    let noise = NoiseOptions::default();
    let secret = String::from("bench-secret");

    // No registry for pure E2E latency;
    // add Some(reg) if needed in future
    CaptchaManager::new(secret, ttl, noise, None, opts)
}

fn bench_generate(c: &mut Criterion) {
    let mut group = c.benchmark_group("generate_e2e");

    let configs = [(100u32, 20u8), (150u32, 20u8), (200u32, 60u8)];

    for (cell, q) in configs {
        let mgr = make_mgr(cell, q, 60);
        group.throughput(Throughput::Elements(1));
        group.bench_function(format!("cell{cell}_q{q}"), |b| {
            b.iter(|| {
                let ch = mgr.generate_challenge().unwrap();
                black_box(ch.challenge_id);
                black_box(ch.sprite_uri);
            });
        });
    }

    group.finish();
}

fn bench_verify(c: &mut Criterion) {
    let mgr_ok = make_mgr(150, 20, 60);
    let mgr_expired = make_mgr(150, 20, 0);

    c.bench_function("verify_e2e/ok_vs_wrong_and_expired", |b| {
        b.iter_batched(
            || mgr_ok.generate_challenge().unwrap(),
            |ch| {
                let _ = mgr_ok.verify_challenge(&ch.challenge_id, 5); // wrong guess
                let _ = mgr_expired.verify_challenge(&ch.challenge_id, 5); // expired fast-path
            },
            BatchSize::SmallInput,
        )
    });
}

pub fn criterion_benches(c: &mut Criterion) {
    bench_generate(c);
    bench_verify(c);
}

criterion_group!(benches, criterion_benches);
criterion_main!(benches);
