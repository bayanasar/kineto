//! Manual run of keyframe generation against the live MiniMax API on an
//! existing project, following its locked style and casts. Paid: three
//! `image-01` calls, about 0.0105 USD; never run by the test suite.
//!
//! ```text
//! KINETO_IMAGE_PROVIDER=minimax KINETO_MINIMAX_API_KEY=... \
//!     cargo run -p kineto-core --example keyframe_smoke -- <project-dir> <shot-id>
//! ```

use std::{path::PathBuf, time::Instant};

use kineto_core::generation::{self, ImageProvider, NoHooks};
use kineto_project::{
    manifest::CanonicalProject,
    shot::{ShotWorkflow, locate},
};
use kineto_providers::secret::EnvSecretSource;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: keyframe_smoke <dir> <shot-id>";
    let target = PathBuf::from(args.next().ok_or(usage)?);
    let shot_id = args.next().ok_or(usage)?;
    let provider = ImageProvider::select(
        std::env::var("KINETO_IMAGE_PROVIDER").ok().as_deref(),
        &EnvSecretSource::process(),
    )?;
    let project = CanonicalProject::open(&target)?;
    let key = locate(&project, &shot_id)?.ok_or("no scene holds that shot")?;
    let mut shot = ShotWorkflow::load(&project, &key)?;
    let started = Instant::now();
    generation::generate(&project, &mut shot, &provider, &mut NoHooks)?;
    println!("generated in {} ms", started.elapsed().as_millis());
    let artifacts: serde_json::Value = serde_json::from_slice(&std::fs::read(target.join(
        format!("scenes/{}/shots/{}/artifacts.json", key.scene(), key.shot()),
    ))?)?;
    for artifact in artifacts.as_array().into_iter().flatten().rev().take(3) {
        println!(
            "{} {} {} degradation {}",
            artifact["artifact_id"],
            artifact["media_path"],
            artifact["compiler_version"],
            artifact["degradation"]
        );
        println!("  prompt: {}", artifact["compiled_prompt"]);
        println!("  applied: {}", artifact["applied_constraints"]);
        println!("  dropped: {}", artifact["dropped_constraints"]);
        println!("  depends on: {}", artifact["dependencies"]);
    }
    println!(
        "stale: {:?}",
        generation::keyframe_staleness(&project, &shot)?
    );
    Ok(())
}
