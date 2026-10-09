//! Manual run of bible writing with a real model, on a project whose
//! screenplay `screenplay_smoke` has written and the director has locked
//! (pass `--lock <revision>` to lock it here). Kimi Code uses its
//! subscription; Kimi's API is paid. Never run by the test suite.
//!
//! ```text
//! cargo run -p kineto-core --example bible_smoke -- <project-dir> [--lock <revision>] kimi-code <kimi>
//! KINETO_KIMI_API_KEY=... cargo run -p kineto-core --example bible_smoke -- <project-dir> [--lock <revision>] kimi
//! ```

use std::{path::PathBuf, time::Instant};

use kineto_core::bible;
use kineto_llm::{
    LlmBackend,
    agents::{AgentCli, AgentKind},
    openai::OpenAiCompatible,
};
use kineto_project::{
    bible::{Bible, staleness},
    manifest::CanonicalProject,
    revisions,
};
use kineto_providers::secret::{EnvSecretSource, SecretSource};

fn run<B: LlmBackend>(
    project: &CanonicalProject,
    backend: &B,
) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let record = bible::write(project, backend)?;
    let document: Bible = revisions::read(project, &record)?;
    println!(
        "{} by {:?}/{:?} in {} ms, stale: {:?}",
        record.artifact_id,
        record.provider,
        record.model,
        started.elapsed().as_millis(),
        staleness(project, &record)?
    );
    println!(
        "world: {} / {}",
        document.world.setting, document.world.period
    );
    for character in &document.characters {
        println!(
            "  character {}: {} | {} | {} | forbidden {:?}",
            character.id,
            character.name,
            character.appearance,
            character.wardrobe,
            character.forbidden_changes
        );
    }
    for location in &document.locations {
        println!("  location {}: {}", location.id, location.appearance);
    }
    for prop in &document.props {
        println!(
            "  prop {} in {:?}: {}",
            prop.id, prop.scenes, prop.appearance
        );
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1).peekable();
    let target = PathBuf::from(
        args.next()
            .ok_or("usage: bible_smoke <dir> <backend> ...")?,
    );
    let project = CanonicalProject::open(&target)?;
    if args.peek().is_some_and(|arg| arg == "--lock") {
        args.next();
        let revision = args.next().ok_or("--lock needs a revision")?.parse()?;
        kineto_project::screenplay::lock(&project, revision)?;
    }
    let backend = args.next().ok_or("backend: kimi-code or kimi")?;
    match backend.as_str() {
        "kimi-code" => {
            let executable = PathBuf::from(args.next().ok_or("kimi executable")?);
            run(&project, &AgentCli::new(AgentKind::KimiCode, executable))
        }
        "kimi" => run(
            &project,
            &OpenAiCompatible::kimi(
                EnvSecretSource::process()
                    .api_key("kimi")?
                    .ok_or("KINETO_KIMI_API_KEY is not set")?,
                None,
                None,
            ),
        ),
        other => Err(format!("unknown backend {other}").into()),
    }
}
