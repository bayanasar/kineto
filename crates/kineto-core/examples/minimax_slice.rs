//! Manual end-to-end run of the keyframe slice against the live MiniMax API,
//! through the same project session the desktop app uses. Paid (three
//! `image-01` calls); never run by the test suite.
//!
//! ```text
//! KINETO_IMAGE_PROVIDER=minimax KINETO_MINIMAX_API_KEY=... \
//!     cargo run -p kineto-core --example minimax_slice -- <new-project-dir>
//! ```
//!
//! Creates the project, generates shot 1, selects take B and locks it, then
//! reopens without credentials, deletes `.kineto/`, and reopens again to show
//! the locked keyframe survives both.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};

use kineto_core::{
    generation::ImageProvider,
    session::{OpenRequest, ProjectSession, Recovery, SessionError, SessionSink, ShotCommand},
};
use kineto_project::manifest::{
    PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource, ProjectWorkflow,
};
use kineto_providers::secret::EnvSecretSource;

const WAIT: Duration = Duration::from_secs(300);

enum Event {
    Opened(Result<(), String>),
    Completed(u64, Result<(), String>),
    Recovered(String),
}

struct Sink(Mutex<mpsc::Sender<Event>>);

impl SessionSink for Sink {
    fn recovered(&self, recovery: &Recovery) {
        let _ = self
            .0
            .lock()
            .unwrap()
            .send(Event::Recovered(format!("{recovery:?}")));
    }

    fn opened(&self, result: &Result<(), SessionError>) {
        let _ = self.0.lock().unwrap().send(Event::Opened(
            result.as_ref().map(|_| ()).map_err(ToString::to_string),
        ));
    }

    fn completed(&self, token: u64, _shot: u32, result: &Result<(), SessionError>) {
        let _ = self.0.lock().unwrap().send(Event::Completed(
            token,
            result.as_ref().map(|_| ()).map_err(ToString::to_string),
        ));
    }
}

fn spawn(
    request: OpenRequest,
    provider: ImageProvider,
) -> Result<(ProjectSession, mpsc::Receiver<Event>), Box<dyn std::error::Error>> {
    let (sender, events) = mpsc::channel();
    let session = ProjectSession::spawn(request, provider, Box::new(Sink(Mutex::new(sender))))?;
    loop {
        match events.recv_timeout(WAIT)? {
            Event::Recovered(recovery) => println!("  recovered: {recovery}"),
            Event::Opened(result) => {
                result?;
                return Ok((session, events));
            }
            Event::Completed(..) => {}
        }
    }
}

fn run(
    session: &ProjectSession,
    events: &mpsc::Receiver<Event>,
    token: u64,
    command: ShotCommand,
) -> Result<(), Box<dyn std::error::Error>> {
    session
        .submit(0, command, token)
        .map_err(|error| format!("{error:?}"))?;
    match events.recv_timeout(WAIT)? {
        Event::Completed(answered, result) if answered == token => Ok(result?),
        _ => Err("unexpected session event".into()),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: minimax_slice <new-project-dir>")?,
    );
    let provider = ImageProvider::select(
        std::env::var("KINETO_IMAGE_PROVIDER").ok().as_deref(),
        &EnvSecretSource::process(),
    )?;
    let info = provider.info();
    println!(
        "provider {:?}, ready {}, ≈ {} USD micros per generation",
        info.kind, info.ready, info.generation_cost_micros
    );

    let (session, events) = spawn(
        OpenRequest::CreateText {
            path: target.clone(),
            manifest: Box::new(ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                project_id: "minimax_slice".to_owned(),
                title: "MiniMax Slice".to_owned(),
                created_at: "2026-09-30T00:00:00Z".to_owned(),
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
            }),
            source: b"A filmmaker waits across the table. The final line lands.\n".to_vec(),
        },
        provider,
    )?;

    let started = Instant::now();
    run(&session, &events, 1, ShotCommand::Generate)?;
    println!("generated in {} ms", started.elapsed().as_millis());
    for candidate in 0..3 {
        let media = session.candidate_media(0, candidate)?.ok_or("no media")?;
        println!(
            "  take {}: {} ({} bytes)",
            char::from(b'A' + u8::try_from(candidate)?),
            media.display(),
            std::fs::metadata(&media)?.len()
        );
    }
    run(&session, &events, 2, ShotCommand::Select(1))?;
    run(&session, &events, 3, ShotCommand::Lock)?;
    drop(session);

    // No credentials and no runtime state: the lock is canonical.
    let (reopened, _) = spawn(
        OpenRequest::Open {
            path: target.clone(),
        },
        ImageProvider::LocalDemo,
    )?;
    let snapshot = reopened.snapshot(0)?;
    println!(
        "reopened: locked {}, selected {:?}",
        snapshot.locked, snapshot.selected_index
    );
    drop(reopened);
    std::fs::remove_dir_all(target.join(".kineto"))?;
    let (rebuilt, _) = spawn(OpenRequest::Open { path: target }, ImageProvider::LocalDemo)?;
    let snapshot = rebuilt.snapshot(0)?;
    let media = rebuilt.candidate_media(0, 1)?.ok_or("no media")?;
    println!(
        "after deleting .kineto: locked {}, selected {:?}, take B present {}",
        snapshot.locked,
        snapshot.selected_index,
        media.is_file()
    );
    Ok(())
}
