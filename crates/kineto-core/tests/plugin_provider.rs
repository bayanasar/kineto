//! Provider plugins (ADR-0006) end to end, offline, against the demo plugin.
//!
//! This test runs without the libtest harness so that its own binary can be
//! the plugin: started as `<test binary> --as-plugin <args>`, it serves one
//! request as `kineto-demo-plugin` does. A configured plugin's environment is
//! cleared, so its arguments are the only way to tell it what to be.
//!
//! What is proven: a plugin is selected only when it is registered and
//! enabled; what it declares routes it like a built-in, so an incapable one
//! is refused before anything is written; it makes style samples, casting
//! images, keyframes and speech recorded under `plugin.<id>`; a plugin cut
//! off after it took a call is never paid again unasked, and one that kept
//! the answer gives it back on reopen; refusals, failures and files that are
//! not what they claim fail closed; and no request carries a project path.

use std::{
    collections::BTreeMap,
    env, fs,
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use kineto_core::{
    bible, cast,
    generation::{self, GenerationError, ImageProvider, NoHooks, ProviderSelectionError},
    plugin::{self, PluginProvider, PluginSpeech, PluginVideos},
    screenplay,
    session::{OpenRequest, ProjectSession, Recovery, SessionError, SessionSink},
    speech::{SpeechGenerator, SpeechProvider, VoiceSettings},
    story,
    style::{self, SampleRequest, Submission},
    video::{self, VideoProvider},
    voice::{self, VoiceChoice},
};
use kineto_llm::fake::FakeLlm;
use kineto_plugins::{PluginConfig, PluginEntry};
use kineto_project::{
    bible::SAMPLE_BIBLE_JSON,
    cast as cast_document,
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    screenplay::SAMPLE_SCREENPLAY_JSON,
    shot::{AttachOutcome, ShotKey, ShotWorkflow, VideoState},
    story::SAMPLE_ANALYSIS_JSON,
    style as style_document,
};
use kineto_providers::secret::EnvSecretSource;

const WAIT: Duration = Duration::from_secs(20);

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--as-plugin") {
        kineto_plugins::demo::serve(&args[1..]);
        return;
    }
    let filter = args.iter().find(|arg| !arg.starts_with('-')).cloned();
    let tests: [(&str, fn()); 9] = [
        (
            "only_a_registered_enabled_plugin_is_selected",
            only_a_registered_enabled_plugin_is_selected,
        ),
        (
            "an_incapable_plugin_is_refused_before_anything_is_written",
            an_incapable_plugin_is_refused_before_anything_is_written,
        ),
        (
            "a_plugin_samples_casts_and_speaks_as_builtins_do",
            a_plugin_samples_casts_and_speaks_as_builtins_do,
        ),
        (
            "a_plugin_makes_keyframes_and_the_session_follows_it_for_speech",
            a_plugin_makes_keyframes_and_the_session_follows_it_for_speech,
        ),
        (
            "a_plugin_cut_off_after_taking_a_call_is_never_paid_again_unasked",
            a_plugin_cut_off_after_taking_a_call_is_never_paid_again_unasked,
        ),
        (
            "a_plugin_that_kept_its_answer_gives_it_back_on_reopen",
            a_plugin_that_kept_its_answer_gives_it_back_on_reopen,
        ),
        (
            "refusals_failures_and_false_files_fail_closed",
            refusals_failures_and_false_files_fail_closed,
        ),
        (
            "a_plugin_video_is_followed_by_its_job_after_a_restart",
            a_plugin_video_is_followed_by_its_job_after_a_restart,
        ),
        (
            "a_plugin_video_that_fails_or_is_not_an_mp4_fails_closed",
            a_plugin_video_that_fails_or_is_not_an_mp4_fails_closed,
        ),
    ];
    let mut failed = 0;
    let mut ran = 0;
    for (name, test) in tests {
        if filter
            .as_deref()
            .is_some_and(|filter| !name.contains(filter))
        {
            continue;
        }
        ran += 1;
        match panic::catch_unwind(AssertUnwindSafe(test)) {
            Ok(()) => println!("test {name} ... ok"),
            Err(_) => {
                println!("test {name} ... FAILED");
                failed += 1;
            }
        }
    }
    println!("\ntest result: {} passed; {failed} failed", ran - failed);
    if failed > 0 {
        std::process::exit(101);
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "kineto-plugin-provider-{}-{nonce}-{}",
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

/// The demo plugin as `id`, misbehaving as `args` say.
fn entry(id: &str, args: &[&str]) -> PluginEntry {
    PluginEntry {
        id: id.to_owned(),
        executable: env::current_exe().unwrap(),
        args: ["--as-plugin"]
            .iter()
            .chain(args)
            .map(|arg| (*arg).to_owned())
            .collect(),
        secrets: Vec::new(),
        enabled: true,
    }
}

fn demo(args: &[&str]) -> Arc<PluginProvider> {
    Arc::new(PluginProvider::connect(entry("demo", args)).unwrap())
}

/// The demo plugin keeping its answers and its requests in `state`.
fn keeping(state: &Path, args: &[&str]) -> Arc<PluginProvider> {
    let state = state.to_str().unwrap();
    demo(&[&["--state", state][..], args].concat())
}

/// The requests a keeping plugin was sent, as JSON lines.
fn requests(state: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(state.join("requests.log"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn invokes(state: &Path) -> usize {
    requests(state)
        .iter()
        .filter(|request| request["operation"] == "invoke")
        .count()
}

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "plugins".to_owned(),
        title: "Plugins".to_owned(),
        created_at: "2026-10-03T00:00:00Z".to_owned(),
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

#[derive(Debug)]
enum Event {
    Recovered(String),
    Opened(Result<(), String>),
    Video(Result<(), String>),
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
            result
                .as_ref()
                .map(|_| ())
                .map_err(|error| format!("{error:?}")),
        ));
    }

    fn completed(&self, _token: u64, _shot_index: u32, _result: &Result<(), SessionError>) {}

    fn video_finished(&self, _shot_index: u32, _revision: u16, result: &Result<(), SessionError>) {
        let _ = self.0.lock().unwrap().send(Event::Video(
            result
                .as_ref()
                .map(|_| ())
                .map_err(|error| format!("{error:?}")),
        ));
    }

    fn document_completed(&self, _token: u64, _result: &Result<(), SessionError>) {}
}

/// Open as the app does with `provider`; what recovery reported, once open.
fn open(request: OpenRequest, provider: ImageProvider) -> (ProjectSession, Vec<String>) {
    let (session, recovered, _) = open_watching(request, provider);
    (session, recovered)
}

/// [`open`], with what the session reports after it opened.
fn open_watching(
    request: OpenRequest,
    provider: ImageProvider,
) -> (ProjectSession, Vec<String>, mpsc::Receiver<Event>) {
    let (sender, events) = mpsc::channel();
    let session =
        ProjectSession::spawn(request, provider, Box::new(Sink(Mutex::new(sender)))).unwrap();
    let mut recovered = Vec::new();
    loop {
        match events.recv_timeout(WAIT).unwrap() {
            Event::Recovered(recovery) => recovered.push(recovery),
            Event::Opened(result) => {
                result.unwrap();
                break;
            }
            Event::Video(_) => {}
        }
    }
    (session, recovered, events)
}

/// A new project with its bootstrap shot, made as the app makes one.
fn created(target: &Path) -> CanonicalProject {
    drop(open(
        OpenRequest::CreateText {
            path: target.to_owned(),
            manifest: Box::new(manifest()),
            source: b"A filmmaker waits.\n".to_vec(),
        },
        ImageProvider::LocalDemo,
    ));
    CanonicalProject::open(target).unwrap()
}

fn shot(project: &CanonicalProject) -> ShotWorkflow {
    ShotWorkflow::load(project, &ShotKey::bootstrap(1)).unwrap()
}

fn intents(target: &Path) -> Vec<serde_json::Value> {
    let Ok(entries) = fs::read_dir(target.join(".kineto/jobs/intents")) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
        .collect()
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flat_map(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files_under(&path)
            } else {
                vec![path]
            }
        })
        .collect()
}

fn keyframes(target: &Path) -> Vec<PathBuf> {
    files_under(&target.join("scenes"))
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "jpg"))
        .collect()
}

fn only_a_registered_enabled_plugin_is_selected() {
    let temp = TempDir::new();
    let executable = env::current_exe().unwrap();
    let config = format!(
        "[[plugin]]\nid = \"demo\"\nexecutable = {executable:?}\nargs = [\"--as-plugin\"]\n\n\
         [[plugin]]\nid = \"off\"\nexecutable = {executable:?}\nargs = [\"--as-plugin\"]\nenabled = false\n\n\
         [[plugin]]\nid = \"broken\"\nexecutable = {executable:?}\nargs = [\"--as-plugin\", \"--garbage\"]\n"
    );
    fs::write(temp.0.join("plugins.toml"), &config).unwrap();
    let parsed = PluginConfig::parse(&config).unwrap();
    // SAFETY: this harness runs its tests one at a time, on one thread.
    unsafe { env::set_var("KINETO_CONFIG_HOME", &temp.0) };
    let secrets = EnvSecretSource::process();

    let ImageProvider::Plugin(selected) =
        ImageProvider::select(Some("plugin:demo"), &secrets).unwrap()
    else {
        panic!("plugin:demo selects the plugin");
    };
    assert_eq!(selected.key(), "plugin.demo");
    assert_eq!(selected.name(), "Kineto demo plugin");
    let info = ImageProvider::Plugin(Arc::clone(&selected)).info();
    assert!(info.ready);
    assert_eq!(info.generation_cost_micros, 0);
    assert_eq!(
        ImageProvider::Plugin(Arc::clone(&selected)).identity_mechanism(),
        Some(("plugin.demo", plugin::REFERENCE_MECHANISM))
    );
    assert!(matches!(
        SpeechProvider::select(Some("plugin:demo"), &secrets),
        Ok(Some(SpeechProvider::Plugin(_)))
    ));
    assert!(matches!(
        VideoProvider::select(Some("plugin:demo"), &secrets),
        Ok(Some(VideoProvider::Plugin(_)))
    ));
    assert!(matches!(
        VideoProvider::select(Some("plugin:off"), &secrets),
        Err(ProviderSelectionError::Plugin(_))
    ));
    for refused in ["plugin:off", "plugin:missing", "plugin:broken"] {
        assert!(
            matches!(
                ImageProvider::select(Some(refused), &secrets),
                Err(ProviderSelectionError::Plugin(_))
            ),
            "{refused}"
        );
    }
    // A name that is not a plugin selection is not looked up at all.
    assert!(matches!(
        ImageProvider::select(Some("demo"), &secrets),
        Err(ProviderSelectionError::Unknown(_))
    ));

    let listed = plugin::list(&parsed);
    assert_eq!(listed.len(), 3);
    assert!(
        matches!(&listed[0].described, Some(Ok(plugin)) if plugin.image_capabilities().is_some())
    );
    assert!(
        listed[1].described.is_none(),
        "a disabled plugin is not started"
    );
    assert!(matches!(&listed[2].described, Some(Err(problem)) if problem.contains("not JSON")));
    // SAFETY: as above.
    unsafe { env::remove_var("KINETO_CONFIG_HOME") };
}

fn an_incapable_plugin_is_refused_before_anything_is_written() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = created(&target);
    let square = ImageProvider::Plugin(demo(&["--square-only"]));
    assert!(matches!(
        generation::generate(&project, &mut shot(&project), &square, &mut NoHooks),
        Err(GenerationError::Incapable(missing)) if missing == ["image.aspect_ratios"]
    ));
    assert!(intents(&target).is_empty(), "nothing was prepared");
    assert!(keyframes(&target).is_empty());
}

/// Lock a screenplay and a bible, as a style sample needs.
fn written(target: &Path) -> CanonicalProject {
    let project =
        CanonicalProject::create(target, manifest(), b"A filmmaker hears the last line.\n")
            .unwrap();
    story::analyze(&project, &FakeLlm::answering([SAMPLE_ANALYSIS_JSON])).unwrap();
    screenplay::write(&project, &FakeLlm::answering([SAMPLE_SCREENPLAY_JSON])).unwrap();
    kineto_project::screenplay::lock(&project, 1).unwrap();
    bible::write(&project, &FakeLlm::answering([SAMPLE_BIBLE_JSON])).unwrap();
    project
}

fn a_plugin_samples_casts_and_speaks_as_builtins_do() {
    let temp = TempDir::new();
    let state = temp.0.join("state");
    let target = temp.0.join("film");
    let project = written(&target);
    let plugin = keeping(&state, &[]);
    let provider = ImageProvider::Plugin(Arc::clone(&plugin));

    // A style sample, in the plugin's own partition.
    let sample = style::sample(
        &project,
        &provider,
        SampleRequest {
            preset: "film_noir",
            ..SampleRequest::default()
        },
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    assert_eq!(sample.provider, "plugin.demo");
    assert_eq!(sample.model, "demo-image");
    assert!(sample.media_path.ends_with(".jpg"));
    let bytes = fs::read(target.join(&sample.media_path)).unwrap();
    assert_eq!(&bytes[..3], &[0xff, 0xd8, 0xff]);
    let sampled = intents(&target)
        .into_iter()
        .find(|intent| intent["job_id"] == sample.artifact_id.as_str())
        .expect("the sample's intent is kept until it is collected");
    assert_eq!(sampled["provider"], "plugin.demo-style");
    style_document::select(&project, &sample.artifact_id).unwrap();
    style_document::lock(&project, &sample.artifact_id).unwrap();

    // A portrait, its reference pack sent with the portrait as the
    // identity reference, and the cast locked with the plugin's mechanism.
    let portraits = cast::audition(
        &project,
        &provider,
        "interviewer",
        1,
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    assert_eq!(portraits[0].provider, "plugin.demo");
    cast_document::select(&project, "interviewer", &portraits[0].artifact_id).unwrap();
    let pack = cast::make_pack(
        &project,
        &provider,
        "interviewer",
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    assert!(!pack.is_empty());
    let sent_with_identity = requests(&state)
        .iter()
        .filter(|request| {
            request["operation"] == "invoke"
                && request["request"]["references"][0]["role"] == "identity"
        })
        .count();
    assert_eq!(sent_with_identity, pack.len());
    let locked = cast::lock(&project, &provider, "interviewer").unwrap();
    assert_eq!(locked.provider_reference.provider, "plugin.demo");
    assert_eq!(locked.provider_reference.mechanism, "references");

    // Speech, in the plugin's own voices.
    let speech = PluginSpeech::new(&plugin);
    let voices = speech.voices().unwrap();
    assert_eq!(voices[0].id, "demo_plain");
    let heard = voice::audition(
        &project,
        &speech,
        "interviewer",
        &[VoiceChoice {
            voice: VoiceSettings::new("demo_plain", "en"),
            name: "Plain".to_owned(),
        }],
        None,
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    assert_eq!(heard[0].provider, "plugin.demo");
    assert!(heard[0].media_path.ends_with(".wav"));
    assert!(heard[0].duration_ms > 0);

    // Nothing the plugin was sent names the project.
    let project_path = target.to_string_lossy().into_owned();
    for request in requests(&state) {
        assert!(
            !request.to_string().contains(&project_path),
            "a request names the project: {request}"
        );
    }
}

fn a_plugin_makes_keyframes_and_the_session_follows_it_for_speech() {
    let temp = TempDir::new();
    let state = temp.0.join("state");
    let target = temp.0.join("film");
    let project = created(&target);
    let plugin = keeping(&state, &[]);
    let provider = ImageProvider::Plugin(Arc::clone(&plugin));
    let mut shot = shot(&project);
    assert_eq!(
        generation::generate(&project, &mut shot, &provider, &mut NoHooks).unwrap(),
        AttachOutcome::Attached
    );
    let snapshot = shot.snapshot().unwrap();
    assert!(snapshot.generated);
    assert_eq!(snapshot.candidate_count, 3);
    assert_eq!(invokes(&state), 3);
    assert_eq!(keyframes(&target).len(), 3);
    assert!(
        intents(&target)
            .iter()
            .all(|intent| intent["provider"] == "plugin.demo" && intent["state"] == "reconciled")
            || intents(&target).is_empty()
    );
    // Every record a call kept is gone once its candidate is acknowledged.
    assert!(files_under(&target.join(".kineto/generation")).is_empty());
    drop(project);

    let (session, _) = open(OpenRequest::Open { path: target }, provider);
    assert_eq!(
        session.speech_provider().kind,
        kineto_core::speech::SpeechProviderKind::Plugin
    );
    assert!(session.speech_provider().ready);
    assert_eq!(
        session.video_provider().kind,
        kineto_core::video::VideoProviderKind::Plugin
    );
    assert!(session.video_provider().ready);
    assert_eq!(session.video_provider().clip_seconds, 6);
}

fn a_plugin_cut_off_after_taking_a_call_is_never_paid_again_unasked() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = created(&target);
    let crashing = ImageProvider::Plugin(demo(&["--crash"]));
    // The call may have been charged, so the generation is unresolved at once.
    let generated = generation::generate(&project, &mut shot(&project), &crashing, &mut NoHooks);
    assert!(
        matches!(&generated, Err(GenerationError::UnresolvedSubmission)),
        "{generated:?}"
    );
    let left = intents(&target);
    assert!(left.iter().all(|intent| intent["state"] == "prepared"));
    assert!(left.iter().any(|intent| intent["attempts"] == 1));
    assert!(keyframes(&target).is_empty());
    drop(project);

    // Reopened with a healthy plugin that kept nothing: the call may have
    // been charged, so nothing is made until the director risks it.
    let healthy_state = temp.0.join("healthy");
    let healthy = keeping(&healthy_state, &[]);
    let (session, _) = open(
        OpenRequest::Open {
            path: target.clone(),
        },
        ImageProvider::Plugin(Arc::clone(&healthy)),
    );
    assert!(session.keyframes_unresolved(0).unwrap());
    assert!(!session.snapshot(0).unwrap().generated);
    drop(session);
    let asked = requests(&healthy_state);
    assert!(
        asked
            .iter()
            .any(|request| request["operation"] == "reconcile")
    );
    assert_eq!(invokes(&healthy_state), 0, "recovery makes no new call");

    let project = CanonicalProject::open(&target).unwrap();
    let provider = ImageProvider::Plugin(healthy);
    assert!(matches!(
        generation::generate(&project, &mut shot(&project), &provider, &mut NoHooks),
        Err(GenerationError::UnresolvedSubmission)
    ));
    assert_eq!(invokes(&healthy_state), 0, "never paid twice unasked");
    assert_eq!(
        generation::resubmit(&project, &mut shot(&project), &provider, &mut NoHooks).unwrap(),
        AttachOutcome::Attached
    );
    assert_eq!(invokes(&healthy_state), 3);
}

fn a_plugin_that_kept_its_answer_gives_it_back_on_reopen() {
    let temp = TempDir::new();
    let state = temp.0.join("state");
    let target = temp.0.join("film");
    let project = created(&target);
    // It keeps each image, then dies before it answers.
    let crashing = ImageProvider::Plugin(keeping(&state, &["--crash"]));
    assert!(generation::generate(&project, &mut shot(&project), &crashing, &mut NoHooks).is_err());
    let taken = invokes(&state);
    assert!(taken >= 1);
    drop(project);

    let (session, recovered) = open(
        OpenRequest::Open {
            path: target.clone(),
        },
        ImageProvider::Plugin(keeping(&state, &[])),
    );
    assert!(
        recovered
            .iter()
            .any(|recovery| recovery.starts_with("Completed")),
        "{recovered:?}"
    );
    assert_eq!(invokes(&state), taken, "recovered without a new call");
    // The kept answers came back; any call that never reached the plugin is
    // retired, and the shot shows what was recovered.
    assert!(!session.keyframes_unresolved(0).unwrap());
    assert!(session.snapshot(0).unwrap().generated);
}

fn refusals_failures_and_false_files_fail_closed() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = created(&target);

    // A refusal proves nothing was charged: asking again needs no consent.
    let refusing = ImageProvider::Plugin(demo(&["--refuse"]));
    assert!(matches!(
        generation::generate(&project, &mut shot(&project), &refusing, &mut NoHooks),
        Err(GenerationError::Provider { .. })
    ));
    let state = temp.0.join("state");
    let healthy = ImageProvider::Plugin(keeping(&state, &[]));
    assert_eq!(
        generation::generate(&project, &mut shot(&project), &healthy, &mut NoHooks).unwrap(),
        AttachOutcome::Attached
    );
    drop(project);

    for (mode, why) in [
        ("--fail", "a failure after the provider may have taken it"),
        ("--wrong-hash", "a file that is not what it declares"),
        ("--sleep", "no answer by the deadline"),
    ] {
        let target = temp.0.join(mode.trim_start_matches('-'));
        let project = created(&target);
        let failing = ImageProvider::Plugin(demo(&[mode]));
        assert!(
            generation::generate(&project, &mut shot(&project), &failing, &mut NoHooks).is_err(),
            "{why}"
        );
        assert!(
            keyframes(&target).is_empty(),
            "{why}: nothing reaches the project"
        );
        assert!(
            matches!(
                generation::generate(&project, &mut shot(&project), &healthy, &mut NoHooks),
                Err(GenerationError::UnresolvedSubmission)
            ),
            "{why}: unresolved until the director risks it"
        );
    }
}

/// A project whose bootstrap shot has a keyframe made by `plugin`, locked.
fn keyframed(target: &Path, plugin: &Arc<PluginProvider>) -> (CanonicalProject, ShotWorkflow) {
    let project = created(target);
    let mut shot = shot(&project);
    generation::generate(
        &project,
        &mut shot,
        &ImageProvider::Plugin(Arc::clone(plugin)),
        &mut NoHooks,
    )
    .unwrap();
    shot.select(&project, 0).unwrap();
    shot.lock(&project).unwrap();
    (project, shot)
}

fn videos_under(target: &Path) -> Vec<PathBuf> {
    files_under(&target.join("scenes"))
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "mp4"))
        .collect()
}

fn a_plugin_video_is_followed_by_its_job_after_a_restart() {
    let temp = TempDir::new();
    let state = temp.0.join("state");
    let target = temp.0.join("film");
    let plugin = keeping(&state, &[]);
    let (project, shot) = keyframed(&target, &plugin);
    let videos = PluginVideos::new(Arc::clone(&plugin));
    let job = video::start(&project, &shot, &videos).unwrap();
    assert_eq!(job.provider, "plugin.demo-video");
    let submitted = || {
        requests(&state)
            .iter()
            .filter(|request| {
                request["operation"] == "invoke" && request["capability"] == "video.generate"
            })
            .count()
    };
    assert_eq!(submitted(), 1);
    let sent = requests(&state)
        .into_iter()
        .find(|request| request["capability"] == "video.generate")
        .unwrap();
    assert_eq!(sent["request"]["first_frame"], "inputs/first-frame.jpg");
    assert_eq!(sent["inputs"][0]["path"], "inputs/first-frame.jpg");
    drop((shot, project));

    // A session opened with the plugin follows the job by its ID and keeps
    // the clip; nothing is submitted again.
    let (session, _, events) = open_watching(
        OpenRequest::Open {
            path: target.clone(),
        },
        ImageProvider::Plugin(Arc::clone(&plugin)),
    );
    let finished = loop {
        if let Event::Video(result) = events.recv_timeout(WAIT).unwrap() {
            break result;
        }
    };
    finished.unwrap();
    assert_eq!(session.snapshot(0).unwrap().video, VideoState::Ready);
    assert_eq!(submitted(), 1);
    drop(session);

    // The same job, followed by hand on a project restored to before it
    // finished, is not submitted again either.
    let (project, shot) = keyframed(&temp.0.join("again"), &plugin);
    let job = video::start(&project, &shot, &videos).unwrap();
    drop((shot, project));
    let target = temp.0.join("again");
    let project = CanonicalProject::open(&target).unwrap();
    let mut found = video::in_flight(&project, "plugin.demo-video").unwrap();
    assert_eq!(found.jobs, [job]);
    let done = video::watch(
        &videos,
        project.root(),
        found.jobs.remove(0),
        &AtomicBool::new(false),
        Duration::ZERO,
    )
    .unwrap();
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
    video::finish(&project, &mut shot, done).unwrap();
    assert_eq!(shot.snapshot().unwrap().video, VideoState::Ready);
    let media = shot.video_media().unwrap().unwrap();
    assert_eq!(&project.root().read(&media).unwrap()[4..8], b"ftyp");
    assert_eq!(submitted(), 2, "each job submitted once, then followed");
    assert!(
        video::in_flight(&project, "plugin.demo-video")
            .unwrap()
            .jobs
            .is_empty()
    );
}

fn a_plugin_video_that_fails_or_is_not_an_mp4_fails_closed() {
    for mode in ["--video-fails", "--not-mp4"] {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let plugin = demo(&[mode]);
        let (project, mut shot) = keyframed(&target, &plugin);
        let videos = PluginVideos::new(Arc::clone(&plugin));
        let job = video::start(&project, &shot, &videos).unwrap();
        let done = video::watch(
            &videos,
            project.root(),
            job,
            &AtomicBool::new(false),
            Duration::ZERO,
        )
        .unwrap();
        assert!(done.result.is_err(), "{mode}");
        assert!(
            matches!(
                video::finish(&project, &mut shot, done),
                Err(GenerationError::Provider { .. })
            ),
            "{mode}: the failure is reported"
        );
        assert_ne!(shot.snapshot().unwrap().video, VideoState::Ready, "{mode}");
        assert!(
            videos_under(&target).is_empty(),
            "{mode}: nothing reaches the project"
        );
        // The job is retired: asking again submits afresh, unasked consent
        // not needed, since the plugin answered.
        let healthy = PluginVideos::new(demo(&[]));
        assert!(video::start(&project, &shot, &healthy).is_ok(), "{mode}");
    }
}
