//! Manual run of casting against the live MiniMax API, on a project with a
//! bible and a locked style. Paid: `<portraits>` portrait calls, 0.0035 USD
//! each with MiniMax; never run by the test suite.
//!
//! ```text
//! KINETO_IMAGE_PROVIDER=minimax KINETO_MINIMAX_API_KEY=... \
//!     cargo run -p kineto-core --example cast_smoke -- <project-dir> <character-id> [<portraits>] [--portraits-only]
//! ```
//!
//! Makes the portraits, casts the character as the first and prints what
//! was kept; with `--portraits-only`, stops after the portraits. With `0`
//! portraits, the first portrait already made is used.

use std::{path::PathBuf, time::Instant};

use kineto_core::{
    cast::{self, Submission},
    generation::{ImageProvider, NoHooks},
};
use kineto_project::{cast as document, manifest::CanonicalProject};
use kineto_providers::secret::EnvSecretSource;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: cast_smoke <dir> <character-id> [<portraits>]";
    let target = PathBuf::from(args.next().ok_or(usage)?);
    let character = args.next().ok_or(usage)?;
    let rest: Vec<String> = args.collect();
    let portraits_only = rest.iter().any(|arg| arg == "--portraits-only");
    let portraits: usize = rest
        .iter()
        .find(|arg| *arg != "--portraits-only")
        .map_or(Ok(2), |count| count.parse())?;
    let provider = ImageProvider::select(
        std::env::var("KINETO_IMAGE_PROVIDER").ok().as_deref(),
        &EnvSecretSource::process(),
    )?;
    let project = CanonicalProject::open(&target)?;
    println!("main characters: {:?}", cast::main_characters(&project)?);
    println!("recovered before casting: {:?}", cast::recover(&project)?);
    if let Some(audition) = cast::audition_cost(&provider, portraits) {
        println!(
            "{portraits} portrait(s) about {} micros",
            audition.amount_micros
        );
    }

    let started = Instant::now();
    let made = if portraits == 0 {
        document::images(&project, &character)?
            .into_iter()
            .filter(|image| image.kind == document::CastImageKind::Portrait)
            .take(1)
            .collect()
    } else {
        cast::audition(
            &project,
            &provider,
            &character,
            portraits,
            Submission::Once,
            &mut NoHooks,
        )?
    };
    for image in &made {
        println!(
            "{} {} ({} chars, dropped {:?})",
            image.artifact_id,
            image.media_path,
            image.provenance.compiled_prompt.chars().count(),
            image
                .provenance
                .dropped_constraints
                .iter()
                .map(|dropped| dropped.constraint.as_str())
                .collect::<Vec<_>>()
        );
    }
    println!("portraits in {} ms", started.elapsed().as_millis());
    if portraits_only {
        return Ok(());
    }

    let locked = cast::cast_portrait(&project, &provider, &character, &made[0].artifact_id)?;
    println!(
        "cast {} as portrait {}; stale: {:?}",
        locked.character_id,
        locked.portrait.artifact_id,
        cast::staleness(&project, &character)?
    );
    Ok(())
}
