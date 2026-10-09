//! Manual run of screenplay writing with a real model, on a project that
//! `story_smoke` has analysed. Kimi Code uses its subscription; Kimi's API
//! is paid. Never run by the test suite.
//!
//! ```text
//! cargo run -p kineto-core --example screenplay_smoke -- <project-dir> kimi-code <kimi> [<scene-id> <direction>]
//! KINETO_KIMI_API_KEY=... cargo run -p kineto-core --example screenplay_smoke -- <project-dir> kimi [<scene-id> <direction>]
//! ```

use std::{path::PathBuf, time::Instant};

use kineto_core::screenplay;
use kineto_llm::{
    LlmBackend,
    agents::{AgentCli, AgentKind},
    openai::OpenAiCompatible,
};
use kineto_project::{manifest::CanonicalProject, revisions, screenplay::Screenplay};
use kineto_providers::secret::{EnvSecretSource, SecretSource};

fn run<B: LlmBackend>(
    project: &CanonicalProject,
    backend: &B,
    rewrite: Option<(String, String)>,
) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let record = match &rewrite {
        None => screenplay::write(project, backend)?,
        Some((scene, direction)) => {
            screenplay::rewrite_scene(project, backend, scene, Some(direction))?
        }
    };
    let document: Screenplay = revisions::read(project, &record)?;
    println!(
        "{} by {:?}/{:?} in {} ms, based on {:?}",
        record.artifact_id,
        record.provider,
        record.model,
        started.elapsed().as_millis(),
        record.based_on
    );
    println!("title: {}", document.title);
    for scene in &document.scenes {
        println!(
            "  {} [{}] {} blocks, hash {}: {}",
            scene.id,
            scene.location,
            scene.blocks.len(),
            &scene.content_hash().as_str()[..15],
            scene.heading
        );
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let target = PathBuf::from(
        args.next()
            .ok_or("usage: screenplay_smoke <dir> <backend> ...")?,
    );
    let backend = args.next().ok_or("backend: kimi-code or kimi")?;
    let project = CanonicalProject::open(&target)?;
    match backend.as_str() {
        "kimi-code" => {
            let executable = PathBuf::from(args.next().ok_or("kimi executable")?);
            let rewrite = args.next().zip(args.next());
            run(
                &project,
                &AgentCli::new(AgentKind::KimiCode, executable),
                rewrite,
            )
        }
        "kimi" => {
            let rewrite = args.next().zip(args.next());
            run(
                &project,
                &OpenAiCompatible::kimi(
                    EnvSecretSource::process()
                        .api_key("kimi")?
                        .ok_or("KINETO_KIMI_API_KEY is not set")?,
                    None,
                    None,
                ),
                rewrite,
            )
        }
        other => Err(format!("unknown backend {other}").into()),
    }
}
