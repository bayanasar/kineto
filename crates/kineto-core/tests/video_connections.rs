//! Video connections (ADR-0009) end to end, offline: a service on this
//! computer that speaks Ark's video tasks, as Seedance is served, reached
//! over real HTTP.
//!
//! What is proven: a connection submits a shot's clip from its locked
//! keyframe with the director's request fields, under
//! `connection.<name>`, with its key as a bearer; the clip is followed
//! across a restart by its task, never submitted again, fetched without
//! the key and attached; a refusal proves nothing was started, so asking
//! again needs no consent, while a failure after the service may have
//! started the task is never paid again unasked; a task refused for its
//! input is final; and the price is the director's, or unknown.
//!
//! The `planned` fixture's `office_night_02` has keyframe candidates with
//! images, and a plan that asks for 5 seconds.

use std::{
    collections::{BTreeMap, VecDeque},
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kineto_core::{
    connection::VideoConnection,
    generation::GenerationError,
    video::{self, DEMO_CLIP, VideoProvider, VideoProviderKind},
};
use kineto_jobs::ErrorClass;
use kineto_plugins::settings::{Connection, Protocol};
use kineto_project::{
    manifest::CanonicalProject,
    shot::{ShotKey, ShotWorkflow, VideoState},
};
use kineto_provider_ark::ArkError;
use kineto_providers::secret::{SecretError, SecretSource, SecretString};

const KEY: &str = "ark-local-test-key";
const MODEL: &str = "dreamina-seedance-2-0-260128";
const PROVIDER: &str = "connection.seedance";

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "kineto-video-connections-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn shot_key() -> ShotKey {
    ShotKey::new("office_night", "office_night_02").unwrap()
}

/// The planned film with both shots' keyframes locked: `office_night_02`'s
/// at its first candidate, which has an image.
fn film(target: &Path) -> CanonicalProject {
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/planned"),
        target,
    );
    let project = CanonicalProject::open(target).unwrap();
    let mut first = ShotWorkflow::load(
        &project,
        &ShotKey::new("office_night", "office_night_01").unwrap(),
    )
    .unwrap();
    first.generate(&project).unwrap();
    first.select(&project, 0).unwrap();
    first.lock(&project).unwrap();
    let mut shot = ShotWorkflow::load(&project, &shot_key()).unwrap();
    shot.select(&project, 0).unwrap();
    shot.lock(&project).unwrap();
    project
}

/// One request the service was sent.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    authorization: Option<String>,
    body: serde_json::Value,
}

/// How a task ends once it has said it is running.
#[derive(Debug, Clone, Copy)]
enum Ending {
    Clip,
    /// Refused for its input: a real person's face.
    Refused,
}

/// Ark's video tasks on this computer. A submission is answered with the
/// next of `submissions`, or with a new task; task `n` says it is running
/// once, then ends as the `n`th ending says; its clip is the demo clip.
struct Service {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    submissions: Arc<Mutex<VecDeque<(u16, String)>>>,
}

impl Service {
    fn start(endings: &[Ending]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let url = format!("{origin}/api/v3");
        let seen = Arc::new(Mutex::new(Vec::<Seen>::new()));
        let submissions = Arc::new(Mutex::new(VecDeque::<(u16, String)>::new()));
        let (record, next, endings) = (
            Arc::clone(&seen),
            Arc::clone(&submissions),
            endings.to_vec(),
        );
        thread::spawn(move || {
            let mut tasks = 0;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut words = line.split_whitespace();
                let method = words.next().unwrap_or_default().to_owned();
                let path = words.next().unwrap_or_default().to_owned();
                let mut length = 0;
                let mut authorization = None;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    let header = header.trim_end();
                    if header.is_empty() {
                        break;
                    }
                    let (name, value) = header.split_once(':').unwrap();
                    match name.to_ascii_lowercase().as_str() {
                        "content-length" => length = value.trim().parse().unwrap(),
                        "authorization" => authorization = Some(value.trim().to_owned()),
                        _ => {}
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let polled_before = record
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|seen: &Seen| seen.method == "GET" && seen.path == path);
                record.lock().unwrap().push(Seen {
                    method: method.clone(),
                    path: path.clone(),
                    authorization,
                    body: serde_json::from_slice(&body).unwrap_or_default(),
                });
                let (status, kind, answer): (u16, &str, Vec<u8>) = if method == "POST" {
                    match next.lock().unwrap().pop_front() {
                        Some((status, answer)) => (status, "application/json", answer.into_bytes()),
                        None => {
                            tasks += 1;
                            (
                                200,
                                "application/json",
                                format!(r#"{{"id":"cgt-{tasks}"}}"#).into_bytes(),
                            )
                        }
                    }
                } else if let Some(task) =
                    path.strip_prefix("/api/v3/contents/generations/tasks/cgt-")
                {
                    let n: usize = task.parse().unwrap();
                    let body = if !polled_before {
                        r#"{"status":"running"}"#.to_owned()
                    } else {
                        match endings[n - 1] {
                            Ending::Clip => format!(
                                r#"{{"id":"cgt-{n}","status":"succeeded","content":{{"video_url":"{origin}/files/cgt-{n}.mp4?sig=1"}},"usage":{{"completion_tokens":108900}}}}"#
                            ),
                            Ending::Refused => format!(
                                r#"{{"id":"cgt-{n}","status":"failed","error":{{"code":"InputImageSensitiveContentDetected.PrivacyInformation","message":"The input image may contain a real person."}}}}"#
                            ),
                        }
                    };
                    (200, "application/json", body.into_bytes())
                } else if path.starts_with("/files/") {
                    (200, "video/mp4", DEMO_CLIP.to_vec())
                } else {
                    (404, "text/plain", b"no".to_vec())
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    answer.len()
                );
                let _ = stream.write_all(&answer);
            }
        });
        Self {
            url,
            seen,
            submissions,
        }
    }

    fn answer_next_submission(&self, status: u16, body: &str) {
        self.submissions
            .lock()
            .unwrap()
            .push_back((status, body.to_owned()));
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn submitted(&self) -> Vec<Seen> {
        self.seen()
            .into_iter()
            .filter(|seen| seen.method == "POST")
            .collect()
    }
}

struct Keys(BTreeMap<&'static str, &'static str>);

impl SecretSource for Keys {
    fn api_key(&self, provider: &str) -> Result<Option<SecretString>, SecretError> {
        Ok(self
            .0
            .get(provider)
            .and_then(|key| SecretString::new((*key).to_owned())))
    }
}

fn keys() -> Keys {
    Keys(BTreeMap::from([("ark", KEY)]))
}

fn connection(service: &Service, price: Option<f64>) -> Connection {
    let mut connection = Connection::new("seedance", Protocol::ArkVideo, &service.url, MODEL);
    connection.key = Some("ark".to_owned());
    connection.price_per_second = price;
    connection
        .request
        .insert("generate_audio".to_owned(), toml::Value::Boolean(false));
    connection
}

fn videos(connection: &Connection, keys: &Keys) -> Arc<VideoConnection> {
    match VideoProvider::connection(connection, keys).unwrap() {
        VideoProvider::Connection(videos) => videos,
        other => panic!("a video connection: {other:?}"),
    }
}

fn follow(
    project: &CanonicalProject,
    videos: &Arc<VideoConnection>,
    job: video::VideoJob,
) -> Result<(), GenerationError> {
    let done = video::watch(
        videos,
        project.root(),
        job,
        &AtomicBool::new(false),
        Duration::ZERO,
    )
    .unwrap();
    let mut shot = ShotWorkflow::load(project, &shot_key()).unwrap();
    video::finish(project, &mut shot, done).map(|_| ())
}

fn ark_error(error: GenerationError) -> (ErrorClass, ArkError) {
    let GenerationError::Provider { source, class } = error else {
        panic!("a provider failure: {error:?}");
    };
    (class, source.downcast_ref::<ArkError>().unwrap().clone())
}

#[test]
fn a_seedance_clip_starts_from_the_keyframe_and_is_followed_across_a_restart() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target);
    let service = Service::start(&[Ending::Clip]);
    let seedance = videos(&connection(&service, Some(0.1)), &keys());
    let shot = ShotWorkflow::load(&project, &shot_key()).unwrap();
    let job = video::start(&project, &shot, &seedance).unwrap();
    assert_eq!(job.provider, PROVIDER);
    assert_eq!(job.task.as_str(), "cgt-1");

    // The locked keyframe as a data URL, the planned 5 seconds, the
    // keyframe's 16:9, 720p, and the director's silence.
    let (_, keyframe) = shot.first_frame().unwrap().unwrap();
    let keyframe = fs::read(project.root().resolve(&keyframe.unwrap())).unwrap();
    let submitted = service.submitted();
    assert_eq!(submitted.len(), 1);
    let sent = &submitted[0];
    assert_eq!(sent.path, "/api/v3/contents/generations/tasks");
    assert_eq!(
        sent.authorization.as_deref(),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert_eq!(sent.body["model"], MODEL);
    assert_eq!(sent.body["duration"], 5);
    assert_eq!(sent.body["ratio"], "16:9");
    assert_eq!(sent.body["resolution"], "720p");
    assert_eq!(sent.body["generate_audio"], false);
    let content = sent.body["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "text");
    assert!(!content[0]["text"].as_str().unwrap().is_empty());
    assert_eq!(content[1]["role"], "first_frame");
    let url = content[1]["image_url"]["url"].as_str().unwrap();
    assert!(url.ends_with(&STANDARD.encode(&keyframe)), "{}", &url[..40]);
    drop((shot, project));

    // A new session follows the same task; nothing is submitted again.
    let project = CanonicalProject::open(&target).unwrap();
    let mut found = video::in_flight(&project, PROVIDER).unwrap();
    assert_eq!(found.jobs, [job]);
    assert!(found.unresolved.is_empty());
    follow(&project, &seedance, found.jobs.remove(0)).unwrap();
    assert_eq!(service.submitted().len(), 1);
    let fetched = service
        .seen()
        .into_iter()
        .find(|seen| seen.path.starts_with("/files/"))
        .unwrap();
    assert_eq!(
        fetched.authorization, None,
        "a clip is fetched without the key"
    );

    let shot = ShotWorkflow::load(&project, &shot_key()).unwrap();
    assert_eq!(shot.snapshot().unwrap().video, VideoState::Ready);
    assert_eq!(
        project
            .root()
            .read(&shot.video_media().unwrap().unwrap())
            .unwrap(),
        DEMO_CLIP
    );
    let artifacts: serde_json::Value = serde_json::from_slice(
        &fs::read(target.join("scenes/office_night/shots/office_night_02/artifacts.json")).unwrap(),
    )
    .unwrap();
    let record = artifacts
        .as_array()
        .unwrap()
        .iter()
        .find(|artifact| artifact["type"] == "shot_video")
        .unwrap();
    assert_eq!(record["provider"], PROVIDER);
    assert_eq!(record["model"], MODEL);
    assert!(
        video::in_flight(&project, PROVIDER)
            .unwrap()
            .jobs
            .is_empty()
    );
}

#[test]
fn a_refusal_needs_no_consent_and_a_lost_answer_does() {
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"));
    let service = Service::start(&[Ending::Clip]);
    let seedance = videos(&connection(&service, None), &keys());
    let shot = || ShotWorkflow::load(&project, &shot_key()).unwrap();

    service.answer_next_submission(
        400,
        r#"{"error":{"code":"InvalidParameter","message":"the parameter duration is invalid"}}"#,
    );
    let refused = video::start(&project, &shot(), &seedance).unwrap_err();
    let GenerationError::Provider { class, .. } = &refused else {
        panic!("a provider refusal: {refused:?}");
    };
    assert_eq!(*class, ErrorClass::Terminal);
    assert!(
        refused.to_string().contains("duration is invalid"),
        "{refused}"
    );

    // Refused, so nothing was started: asking again needs no consent. A
    // server failure may have started the task, so it is unresolved, and
    // never paid again unasked.
    service.answer_next_submission(500, "<html>busy</html>");
    for _ in 0..2 {
        let error = video::start(&project, &shot(), &seedance).unwrap_err();
        assert!(
            matches!(error, GenerationError::UnresolvedSubmission),
            "{error:?}"
        );
    }
    assert_eq!(service.submitted().len(), 2);
    let job = video::resubmit(&project, &shot(), &seedance).unwrap();
    assert_eq!(job.task.as_str(), "cgt-1");
    follow(&project, &seedance, job).unwrap();
    assert_eq!(shot().snapshot().unwrap().video, VideoState::Ready);
}

#[test]
fn a_task_refused_for_its_input_is_final() {
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"));
    let service = Service::start(&[Ending::Refused]);
    let seedance = videos(&connection(&service, None), &keys());
    let shot = ShotWorkflow::load(&project, &shot_key()).unwrap();
    let job = video::start(&project, &shot, &seedance).unwrap();
    let (class, failed) = ark_error(follow(&project, &seedance, job).unwrap_err());
    assert_eq!(class, ErrorClass::Terminal);
    assert!(failed.to_string().contains("real person"), "{failed}");
    assert!(
        video::in_flight(&project, PROVIDER)
            .unwrap()
            .jobs
            .is_empty()
    );
}

#[test]
fn the_price_is_the_directors_or_unknown_and_no_key_makes_nothing() {
    let service = Service::start(&[]);
    let priced = VideoProvider::connection(&connection(&service, Some(0.1)), &keys()).unwrap();
    let info = priced.info();
    assert_eq!(info.kind, VideoProviderKind::Connection);
    assert!(info.ready);
    let estimate = priced.estimate(Some(5.0), None).unwrap();
    assert_eq!(
        (estimate.clip_seconds, estimate.cost_micros),
        (5, Some(500_000))
    );
    assert!(
        !priced.capabilities().audio,
        "the director turned sound off"
    );
    assert_eq!(priced.capabilities().max_prompt_chars, Some(2_000));

    let unpriced = VideoProvider::connection(&connection(&service, None), &keys()).unwrap();
    assert_eq!(
        unpriced.estimate(Some(5.0), None).unwrap().cost_micros,
        None
    );
    assert!(unpriced.key() == PROVIDER);

    let keyless =
        VideoProvider::connection(&connection(&service, None), &Keys(BTreeMap::new())).unwrap();
    assert!(!keyless.info().ready);

    let mut images = connection(&service, None);
    images.protocol = Protocol::OpenAiImages.name().to_owned();
    images.price_per_second = None;
    images.request.clear();
    assert!(VideoProvider::connection(&images, &keys()).is_err());
    assert!(service.seen().is_empty());
}
