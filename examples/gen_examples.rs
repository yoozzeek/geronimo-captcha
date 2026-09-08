use geronimo_captcha::{
    CaptchaManager, DecodeLimits, GenerationOptions, NoiseOptions, SampleFormat, SampleSet,
    SpriteBinary, SpriteFormat,
};

use std::path::{Path, PathBuf};

const SECRET: &str = "example-generation-secret-32-bytes";
const ASSET_DIR: &str = "assets";
const OUT_DIR: &str = "assets/examples";

fn sample_paths() -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(ASSET_DIR)?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?;

            match name.starts_with("sample") && name.ends_with(".jpg") {
                true => Some(path),
                false => None,
            }
        })
        .collect();

    paths.sort();

    Ok(paths)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(OUT_DIR)?;

    let paths = sample_paths()?;
    if paths.is_empty() {
        return Err(format!("no sample images under {ASSET_DIR}").into());
    }

    for path in paths {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("sample path has no stem")?;

        let samples = SampleSet::new(SampleFormat::Jpeg, vec![std::fs::read(&path)?])?;

        let mgr = CaptchaManager::new(
            SECRET.to_string(),
            60,
            NoiseOptions::default(),
            None,
            GenerationOptions {
                cell_size: 150,
                sprite_format: SpriteFormat::Jpeg { quality: 70 },
                rounds: 1,
                limits: DecodeLimits::default(),
            },
            &samples,
        )?;

        let challenge = mgr.generate_challenge::<SpriteBinary>()?;
        let out = Path::new(OUT_DIR).join(format!("{stem}.jpg"));

        std::fs::write(&out, &challenge.sprites[0].bytes)?;

        println!(
            "{} -> {} ({} bytes)",
            path.display(),
            out.display(),
            challenge.sprites[0].bytes.len()
        );
    }

    Ok(())
}
