//! Manual run of story analysis with a real model. Kimi Code uses its
//! subscription; Kimi's API is paid. Never run by the test suite.
//!
//! ```text
//! cargo run -p kineto-core --example story_smoke -- <new-project-dir> kimi-code <kimi> <kineto-kimi-home>
//! KINETO_KIMI_API_KEY=... cargo run -p kineto-core --example story_smoke -- <new-project-dir> kimi
//! ```

use std::{collections::BTreeMap, path::PathBuf, time::Instant};

use kineto_core::story;
use kineto_llm::{LlmBackend, acp::AcpAgent, openai::OpenAiCompatible};
use kineto_project::{
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    revisions,
    story::StoryAnalysis,
};
use kineto_providers::secret::{EnvSecretSource, SecretSource};

const STORY: &str = "The interview had gone on for two hours. Mara closed her notebook, looked \
at the filmmaker across the table, and said the thing she had been saving: \"Your brother called \
me last week. He wanted to know if you ever talk about him.\" Daniel did not answer. Outside the \
window the street lights came on one by one. He put on his coat, thanked her for her time, and \
walked out into the corridor, where the cleaner was already switching off the lights.\n";

fn run<B: LlmBackend>(
    project: &CanonicalProject,
    backend: &B,
) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let record = story::analyze(project, backend)?;
    let analysis: StoryAnalysis = revisions::read(project, &record)?;
    println!(
        "{} by {:?}/{:?} in {} ms",
        record.artifact_id,
        record.provider,
        record.model,
        started.elapsed().as_millis()
    );
    println!("logline: {}", analysis.logline);
    for character in &analysis.characters {
        println!(
            "  character {} ({:?}): {}",
            character.id, character.role, character.name
        );
    }
    for scene in &analysis.candidate_scenes {
        println!(
            "  scene {} at {}: {}",
            scene.id, scene.location, scene.summary
        );
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let target = PathBuf::from(
        args.next()
            .ok_or("usage: story_smoke <dir> <backend> ...")?,
    );
    let backend = args.next().ok_or("backend: kimi-code or kimi")?;
    let project = CanonicalProject::create(
        &target,
        ProjectManifest {
            format_version: PROJECT_FORMAT_VERSION,
            project_id: "story_smoke".to_owned(),
            title: "Story Smoke".to_owned(),
            created_at: "2026-10-01T00:00:00Z".to_owned(),
            source: ProjectSource {
                kind: "text".to_owned(),
                path: "source/story.txt".to_owned(),
                extra: BTreeMap::new(),
            },
            defaults: ProjectDefaults {
                language: "en".to_owned(),
                extra: BTreeMap::new(),
            },
            workflow: ProjectWorkflow {
                recipe_id: "default-film".to_owned(),
                recipe_version: 1,
                extra: BTreeMap::new(),
            },
            extra: BTreeMap::new(),
        },
        STORY.as_bytes(),
    )?;
    match backend.as_str() {
        "kimi-code" => {
            let executable = PathBuf::from(args.next().ok_or("kimi executable")?);
            let home = PathBuf::from(args.next().ok_or("kineto Kimi Code data root")?);
            run(&project, &AcpAgent::kimi_code(executable, home))
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
