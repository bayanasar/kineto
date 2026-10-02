//! Manual run of style exploration against the live MiniMax API, on a
//! project whose screenplay is locked (and, ideally, has a bible). Paid: one
//! `image-01` call per preset named, about 0.0035 USD each; never run by
//! the test suite.
//!
//! ```text
//! KINETO_IMAGE_PROVIDER=minimax KINETO_MINIMAX_API_KEY=... \
//!     cargo run -p kineto-core --example style_smoke -- <project-dir> <preset>... [--lock]
//! ```
//!
//! Samples each preset, prints what was compiled and kept, and with
//! `--lock` selects and locks the last sample.

use std::{path::PathBuf, time::Instant};

use kineto_core::{
    generation::{ImageProvider, NoHooks},
    style::{self, SampleRequest, Submission},
    style_prompt::PRESETS,
};
use kineto_project::{manifest::CanonicalProject, style as document};
use kineto_providers::secret::EnvSecretSource;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let target = PathBuf::from(
        args.next()
            .ok_or("usage: style_smoke <dir> <preset>... [--lock]")?,
    );
    let rest: Vec<String> = args.collect();
    let lock = rest.iter().any(|arg| arg == "--lock");
    let presets: Vec<&str> = rest
        .iter()
        .map(String::as_str)
        .filter(|arg| *arg != "--lock")
        .collect();
    if presets.is_empty() {
        let known: Vec<_> = PRESETS.iter().map(|preset| preset.id).collect();
        return Err(format!("name at least one preset: {known:?}").into());
    }
    let provider = ImageProvider::select(
        std::env::var("KINETO_IMAGE_PROVIDER").ok().as_deref(),
        &EnvSecretSource::process(),
    )?;
    let project = CanonicalProject::open(&target)?;
    let recovery = style::recover(&project)?;
    println!("recovered before sampling: {recovery:?}");
    if let Some(price) = style::sample_cost(&provider) {
        println!(
            "{} call(s), about {} {} micros",
            presets.len(),
            price.amount_micros * presets.len() as u64,
            String::from_utf8_lossy(&price.currency)
        );
    }
    let mut last = None;
    for preset in presets {
        let started = Instant::now();
        let sample = style::sample(
            &project,
            &provider,
            SampleRequest {
                preset,
                ..SampleRequest::default()
            },
            Submission::Once,
            &mut NoHooks,
        )?;
        println!(
            "{} {} in {} ms: {} ({} chars), applied {:?}, dropped {:?}",
            sample.artifact_id,
            sample.preset,
            started.elapsed().as_millis(),
            sample.media_path,
            sample.provenance.compiled_prompt.chars().count(),
            sample.provenance.applied_constraints,
            sample
                .provenance
                .dropped_constraints
                .iter()
                .map(|dropped| dropped.constraint.as_str())
                .collect::<Vec<_>>()
        );
        last = Some(sample.artifact_id);
    }
    if lock && let Some(artifact_id) = last {
        document::select(&project, &artifact_id)?;
        let locked = document::lock(&project, &artifact_id)?;
        println!("locked {} ({})", locked.artifact_id, locked.preset);
    }
    Ok(())
}
