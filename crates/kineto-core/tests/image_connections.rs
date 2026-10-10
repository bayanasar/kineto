//! Image connections (ADR-0009) end to end, offline: a service on this
//! computer that speaks OpenAI's images protocol, reached over real HTTP.
//!
//! What is proven: a connection makes keyframes recorded under
//! `connection.<name>`, with its key as a bearer, cut to the shot's frame;
//! the price the director gave is the estimate, and none given is unknown;
//! a refusal proves nothing was charged, so asking again needs no consent,
//! while a failure after the service may have made the image is never paid
//! again unasked; and a connection whose key is not kept makes nothing.

use std::{
    collections::{BTreeMap, VecDeque},
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kineto_core::{
    generation::{self, GenerationError, ImageProvider, ImageProviderKind, NoHooks},
    session::{OpenRequest, ProjectSession, Recovery, SessionError, SessionSink},
};
use kineto_plugins::settings::{Connection, Protocol};
use kineto_project::{
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    shot::{AttachOutcome, ShotKey, ShotWorkflow},
};
use kineto_providers::secret::{SecretError, SecretSource, SecretString};

const KEY: &str = "sk-images-local-test";

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "kineto-image-connections-{}-{nonce}-{}",
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

/// One request the service was sent: path, authorization and body.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    authorization: Option<String>,
    body: serde_json::Value,
}

/// A service on this computer answering each request with the next of its
/// answers, or with an image when it has none left.
struct Service {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    answers: Arc<Mutex<VecDeque<(u16, String)>>>,
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(width, height, image::Rgb([40, 90, 160]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

impl Service {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let answers = Arc::new(Mutex::new(VecDeque::<(u16, String)>::new()));
        let image = format!(
            r#"{{"created":1,"data":[{{"b64_json":"{}"}}]}}"#,
            STANDARD.encode(png(1536, 1024))
        );
        let (record, next) = (Arc::clone(&seen), Arc::clone(&answers));
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
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
                record.lock().unwrap().push(Seen {
                    path,
                    authorization,
                    body: serde_json::from_slice(&body).unwrap_or_default(),
                });
                let (status, answer) = next
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| (200, image.clone()));
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                );
            }
        });
        Self { url, seen, answers }
    }

    fn answer_next(&self, status: u16, body: &str) {
        self.answers
            .lock()
            .unwrap()
            .push_back((status, body.to_owned()));
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
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
    Keys(BTreeMap::from([("studio", KEY)]))
}

fn connection(service: &Service, price: Option<f64>) -> Connection {
    let mut connection = Connection::new("studio", Protocol::OpenAiImages, &service.url, "image-1");
    connection.key = Some("studio".to_owned());
    connection.price_per_image = price;
    connection
}

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "connections".to_owned(),
        title: "Connections".to_owned(),
        created_at: "2026-10-10T00:00:00Z".to_owned(),
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
    }
}

struct Quiet(Mutex<std::sync::mpsc::Sender<()>>);

impl SessionSink for Quiet {
    fn recovered(&self, _recovery: &Recovery) {}

    fn video_finished(&self, _shot_index: u32, _revision: u16, _result: &Result<(), SessionError>) {
    }

    fn opened(&self, result: &Result<(), SessionError>) {
        assert!(result.is_ok(), "{result:?}");
        let _ = self.0.lock().unwrap().send(());
    }

    fn completed(&self, _token: u64, _shot_index: u32, _result: &Result<(), SessionError>) {}

    fn document_completed(&self, _token: u64, _result: &Result<(), SessionError>) {}
}

/// A new project with its bootstrap shot, made as the app makes one.
fn created(target: &Path) -> CanonicalProject {
    let (sender, opened) = std::sync::mpsc::channel();
    let session = ProjectSession::spawn(
        OpenRequest::CreateText {
            path: target.to_owned(),
            manifest: Box::new(manifest()),
            source: b"A filmmaker waits.\n".to_vec(),
        },
        ImageProvider::LocalDemo,
        Box::new(Quiet(Mutex::new(sender))),
    )
    .unwrap();
    opened
        .recv_timeout(std::time::Duration::from_secs(60))
        .unwrap();
    drop(session);
    CanonicalProject::open(target).unwrap()
}

fn shot(project: &CanonicalProject) -> ShotWorkflow {
    ShotWorkflow::load(project, &ShotKey::bootstrap(1)).unwrap()
}

fn keyframes(target: &Path) -> Vec<PathBuf> {
    fn under(root: &Path) -> Vec<PathBuf> {
        fs::read_dir(root)
            .map(|entries| {
                entries
                    .flat_map(|entry| {
                        let path = entry.unwrap().path();
                        if path.is_dir() {
                            under(&path)
                        } else {
                            vec![path]
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    under(&target.join("scenes"))
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
        .collect()
}

#[test]
fn a_connection_makes_keyframes_in_the_shots_frame_under_its_own_name() {
    let temp = TempDir::new();
    let service = Service::start();
    let target = temp.0.join("film");
    let project = created(&target);
    let provider = ImageProvider::connection(&connection(&service, Some(0.04)), &keys()).unwrap();
    let info = provider.info();
    assert_eq!(info.kind, ImageProviderKind::Connection);
    assert!(info.ready && info.cost_known);
    assert_eq!(info.generation_cost_micros, 3 * 40_000);
    assert_eq!(
        provider.identity_mechanism(),
        None,
        "it takes no references unless told"
    );

    let mut shot = shot(&project);
    assert_eq!(
        generation::generate(&project, &mut shot, &provider, &mut NoHooks).unwrap(),
        AttachOutcome::Attached
    );
    assert_eq!(shot.snapshot().unwrap().candidate_count, 3);
    let seen = service.seen();
    assert_eq!(seen.len(), 3);
    for request in &seen {
        assert_eq!(request.path, "/v1/images/generations");
        assert_eq!(
            request.authorization.as_deref(),
            Some(&*format!("Bearer {KEY}"))
        );
        assert_eq!(request.body["model"], "image-1");
        assert_eq!(request.body["size"], "1536x1024");
        let prompt = request.body["prompt"].as_str().unwrap();
        assert!(prompt.contains("Frame: 16:9 landscape"), "{prompt}");
        assert!(!prompt.contains(&target.to_string_lossy().into_owned()));
    }
    let made = keyframes(&target);
    assert_eq!(made.len(), 3);
    for keyframe in made {
        let image = image::open(&keyframe).unwrap();
        assert_eq!(
            (image.width(), image.height()),
            (1536, 864),
            "cut to the shot's 16:9"
        );
    }
    let artifacts =
        fs::read_to_string(target.join("scenes/scene_001/shots/shot_001/artifacts.json")).unwrap();
    assert!(artifacts.contains("connection.studio"), "{artifacts}");
    assert!(!artifacts.contains(KEY));
}

#[test]
fn a_connection_without_a_price_costs_what_it_costs() {
    let service = Service::start();
    let info = ImageProvider::connection(&connection(&service, None), &keys())
        .unwrap()
        .info();
    assert!(info.ready);
    assert!(!info.cost_known, "no price given is not free");
}

#[test]
fn a_connection_whose_key_is_not_kept_makes_nothing() {
    let temp = TempDir::new();
    let service = Service::start();
    let target = temp.0.join("film");
    let project = created(&target);
    let provider =
        ImageProvider::connection(&connection(&service, Some(0.04)), &Keys(BTreeMap::new()))
            .unwrap();
    assert!(!provider.info().ready);
    assert!(generation::generate(&project, &mut shot(&project), &provider, &mut NoHooks).is_err());
    assert!(
        service.seen().is_empty(),
        "nothing is asked without the key"
    );

    let text = Connection::new("glm", Protocol::OpenAiChat, &service.url, "glm-5");
    assert!(
        ImageProvider::connection(&text, &keys()).is_err(),
        "it makes no images"
    );
}

#[test]
fn a_refusal_needs_no_consent_and_a_lost_answer_is_never_paid_again_unasked() {
    let temp = TempDir::new();
    let service = Service::start();
    let target = temp.0.join("film");
    let project = created(&target);
    let provider = ImageProvider::connection(&connection(&service, Some(0.04)), &keys()).unwrap();

    // Refused: nothing was made, so the director asks again freely.
    for _ in 0..3 {
        service.answer_next(
            400,
            r#"{"error":{"message":"Your request was rejected by the safety system","code":"content_policy_violation"}}"#,
        );
    }
    let refused = generation::generate(&project, &mut shot(&project), &provider, &mut NoHooks);
    assert!(
        matches!(&refused, Err(GenerationError::Provider { .. })),
        "{refused:?}"
    );
    assert_eq!(
        generation::generate(&project, &mut shot(&project), &provider, &mut NoHooks).unwrap(),
        AttachOutcome::Attached
    );
    drop(project);

    // The service failed after it may have made the image: unresolved.
    let target = temp.0.join("lost");
    let project = created(&target);
    for _ in 0..3 {
        service.answer_next(500, "{}");
    }
    assert!(generation::generate(&project, &mut shot(&project), &provider, &mut NoHooks).is_err());
    let asked = service.seen().len();
    assert!(matches!(
        generation::generate(&project, &mut shot(&project), &provider, &mut NoHooks),
        Err(GenerationError::UnresolvedSubmission)
    ));
    assert_eq!(service.seen().len(), asked, "never paid twice unasked");
    assert_eq!(
        generation::resubmit(&project, &mut shot(&project), &provider, &mut NoHooks).unwrap(),
        AttachOutcome::Attached
    );
}
