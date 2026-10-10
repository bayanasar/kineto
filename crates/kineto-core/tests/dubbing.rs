//! Dubbing end to end, offline: a cast and voiced character's line spoken
//! in takes with the local demo, refined, chosen and locked, its length
//! measured, the workflow's voice and dialogue steps, staleness, and the
//! production package carrying the line. A package is never made inside the
//! project, or over it.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::Engine as _;
use kineto_core::{
    bible, cast, dialogue,
    dubbing::{self, DubRequest},
    export::{self, ExportRequest},
    generation::{GenerationError, ImageProvider, NoHooks, Prerequisite},
    minimax::MinimaxKeyframes,
    screenplay, shot_plan,
    speech::{LocalDemoVoices, NORMAL_SPEED, VoiceSettings, estimate_duration_ms},
    story,
    style::{self, SampleRequest, Submission},
    video::{self, LocalDemoVideos},
    voice::{self, VoiceChoice},
    workflow::{self, StepState},
};
use kineto_llm::fake::FakeLlm;
use kineto_project::{
    ContentHash,
    bible::SAMPLE_BIBLE_JSON,
    cast as cast_document,
    dubbing::{self as document, DubbingError, TakeStatus},
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    revisions::{self, DocumentKind},
    screenplay::{Block, BlockKind, SAMPLE_SCREENPLAY_JSON, Screenplay},
    shot::{ShotKey, ShotWorkflow},
    shot_plan::SAMPLE_SHOT_PLAN_JSON,
    story::SAMPLE_ANALYSIS_JSON,
    style as style_document, voice as voice_document,
};
use kineto_provider_minimax::{HttpResponse, MinimaxClient, Transport, TransportFailure};
use kineto_providers::{Emotion, secret::SecretString};

/// MiniMax `image-01`, offline: a distinct JPEG for every call.
#[derive(Default)]
struct FakeImages(AtomicU32);

impl Transport for FakeImages {
    fn post_json(
        &self,
        _url: &str,
        _api_key: &SecretString,
        _body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        let call = self.0.fetch_add(1, Ordering::SeqCst);
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x43, u8::try_from(call).unwrap()];
        Ok(HttpResponse {
            status: 200,
            body: format!(
                r#"{{"id":"c{call}","data":{{"image_base64":["{}"]}},"base_resp":{{"status_code":0}}}}"#,
                base64::engine::general_purpose::STANDARD.encode(jpeg)
            )
            .into_bytes(),
        })
    }

    fn get(&self, url: &str, _api_key: &SecretString) -> Result<HttpResponse, TransportFailure> {
        unreachable!("no GET request: {url}")
    }

    fn download(&self, url: &str, _max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
        unreachable!("nothing is downloaded: {url}")
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = env::temp_dir().join(format!(
            "kineto-dubbing-{}-{nonce}-{}",
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

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "dubbing".to_owned(),
        title: "Dubbing".to_owned(),
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
            recipe_version: 2,
            extra: BTreeMap::new(),
        },
        extra: BTreeMap::new(),
    }
}

const SCENE: &str = "office_night";

/// A film on recipe v2: its screenplay, bible and plan locked, the
/// interviewer cast and, when `voiced`, given the demo's low voice.
fn film(target: &Path, voiced: bool) -> CanonicalProject {
    let project =
        CanonicalProject::create(target, manifest(), b"A filmmaker hears the last line.\n")
            .unwrap();
    story::analyze(&project, &FakeLlm::answering([SAMPLE_ANALYSIS_JSON])).unwrap();
    screenplay::write(&project, &FakeLlm::answering([SAMPLE_SCREENPLAY_JSON])).unwrap();
    kineto_project::screenplay::lock(&project, 1).unwrap();
    bible::write(&project, &FakeLlm::answering([SAMPLE_BIBLE_JSON])).unwrap();
    let client = MinimaxClient::with_transport(
        FakeImages::default(),
        SecretString::new("sk-test".to_owned()).unwrap(),
        "https://example.test",
    );
    let adapter = || MinimaxKeyframes::new(&project, Some(&client));
    let sample = style::sample_with(
        &project,
        adapter(),
        SampleRequest {
            preset: "film_noir",
            ..SampleRequest::default()
        },
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    style_document::select(&project, &sample.artifact_id).unwrap();
    style_document::lock(&project, &sample.artifact_id).unwrap();
    let portraits = cast::audition_with(
        &project,
        adapter(),
        "interviewer",
        1,
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    cast_document::select(&project, "interviewer", &portraits[0].artifact_id).unwrap();
    cast::lock(
        &project,
        &ImageProvider::Minimax(Some(MinimaxClient::shared(
            SecretString::new("sk-test".to_owned()).unwrap(),
        ))),
        "interviewer",
    )
    .unwrap();
    kineto_project::bible::lock(&project, 1).unwrap();
    shot_plan::write(&project, &FakeLlm::answering([SAMPLE_SHOT_PLAN_JSON])).unwrap();
    kineto_project::shot_plan::lock(&project, 1).unwrap();
    if voiced {
        voice::audition(
            &project,
            &LocalDemoVoices,
            "interviewer",
            &[VoiceChoice {
                voice: VoiceSettings::new("demo_low", "en"),
                name: "Low".to_owned(),
            }],
            None,
            Submission::Once,
            &mut NoHooks,
        )
        .unwrap();
        voice_document::select(&project, "interviewer", "voice.interviewer.0001").unwrap();
        voice::lock(&project, "interviewer").unwrap();
    }
    project
}

/// Shot `shot`'s demo keyframe locked, and its demo video made and locked.
fn shoot(project: &CanonicalProject, shot: &str) {
    let mut shot = ShotWorkflow::load(project, &ShotKey::new(SCENE, shot).unwrap()).unwrap();
    shot.generate(project).unwrap();
    shot.select(project, 0).unwrap();
    shot.lock(project).unwrap();
    let job = video::start(project, &shot, &LocalDemoVideos).unwrap();
    let done = video::watch(
        &LocalDemoVideos,
        project.root(),
        job,
        &AtomicBool::new(false),
        Duration::ZERO,
    )
    .unwrap();
    video::finish(project, &mut shot, done).unwrap();
    shot.lock_video(project).unwrap();
}

fn dub(
    project: &CanonicalProject,
    request: &DubRequest,
) -> Result<Vec<document::LineTake>, GenerationError> {
    dub_line(project, 1, request)
}

fn dub_line(
    project: &CanonicalProject,
    line: u32,
    request: &DubRequest,
) -> Result<Vec<document::LineTake>, GenerationError> {
    dubbing::dub(
        project,
        &LocalDemoVoices,
        SCENE,
        line,
        request,
        Submission::Once,
        &mut NoHooks,
    )
}

/// Change the scene's blocks in a new screenplay revision, and lock it.
fn rewrite(project: &CanonicalProject, change: impl FnOnce(&mut Vec<Block>)) {
    let record = revisions::current(project, DocumentKind::Screenplay)
        .unwrap()
        .unwrap();
    let mut screenplay: Screenplay = revisions::read(project, &record).unwrap();
    let scene = screenplay
        .scenes
        .iter_mut()
        .find(|scene| scene.id == SCENE)
        .unwrap();
    change(&mut scene.blocks);
    kineto_project::screenplay::edit(project, record.revision, &screenplay).unwrap();
    kineto_project::screenplay::lock(project, record.revision + 1).unwrap();
}

/// The interviewer saying `text`.
fn interviewer(text: &str) -> Block {
    Block {
        kind: BlockKind::Dialogue,
        character: Some("interviewer".to_owned()),
        parenthetical: None,
        text: text.to_owned(),
    }
}

/// The digits of the hash of `text` that its takes' IDs carry.
fn digits(text: &str) -> String {
    document::line_text_hash(text)["sha256:".len()..][..12].to_owned()
}

fn no_selection(result: Result<document::LockedLine, GenerationError>, line: u32) -> bool {
    matches!(
        result,
        Err(GenerationError::Provider { source, .. })
            if matches!(
                source.downcast_ref::<DubbingError>(),
                Some(DubbingError::NoSelection(found)) if *found == line
            )
    )
}

fn takes(count: usize) -> DubRequest {
    DubRequest {
        takes: count,
        ..DubRequest::default()
    }
}

fn state(project: &CanonicalProject, step: &str) -> StepState {
    workflow::status(project)
        .unwrap()
        .into_iter()
        .find(|status| status.id == step)
        .unwrap()
        .state
}

#[test]
fn a_line_is_dubbed_in_its_characters_voice_chosen_locked_and_carried_into_the_package() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);
    assert_eq!(state(&project, "voices"), StepState::Done);
    assert_eq!(state(&project, "dialogue"), StepState::Ready);
    let lines = dubbing::current_lines(&project).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "Was it worth it?");
    assert_eq!(lines[0].parenthetical.as_deref(), Some("quietly"));

    let made = dub(&project, &takes(2)).unwrap();
    assert_eq!(
        made.iter()
            .map(|take| take.artifact_id.as_str())
            .collect::<Vec<_>>(),
        [
            "line.office_night.01-2c40ae8c8fe4.0001",
            "line.office_night.01-2c40ae8c8fe4.0002"
        ]
    );
    let take = &made[0];
    // "quietly" is a calm delivery, which the demo takes.
    assert_eq!(take.voice.emotion.as_deref(), Some("calm"));
    assert_eq!(take.voice.voice_id, "demo_low");
    assert!(
        take.provenance
            .applied_constraints
            .contains(&"line.parenthetical".to_owned())
    );
    assert_eq!(take.provenance.compiler_version, "speech-prompt/1");
    assert_eq!(
        take.media_path,
        "scenes/office_night/voice/line.office_night.01-2c40ae8c8fe4.0001.wav"
    );
    let audio = fs::read(target.join(&take.media_path)).unwrap();
    assert_eq!(ContentHash::of(&audio).as_str(), take.content_hash);
    let depends_on: Vec<_> = take
        .dependencies
        .iter()
        .map(|dependency| dependency.artifact_id.as_str())
        .collect();
    assert_eq!(depends_on, ["voice.interviewer.0001"]);
    assert_eq!(state(&project, "dialogue"), StepState::AwaitingApproval);

    // Two takes made this way are there: none is paid for again. A refined
    // take is made another way.
    assert_eq!(dub(&project, &takes(2)).unwrap(), made);
    let angry = dub(
        &project,
        &DubRequest {
            takes: 1,
            delivery: Some(Emotion::Angry),
            speed_permille: Some(1250),
            note: Some("Harder.".to_owned()),
        },
    )
    .unwrap();
    assert_eq!(
        angry[0].artifact_id,
        "line.office_night.01-2c40ae8c8fe4.0003"
    );
    assert_eq!(angry[0].voice.emotion.as_deref(), Some("angry"));
    assert_eq!(angry[0].voice.speed_permille, 1250);
    assert_eq!(angry[0].note.as_deref(), Some("Harder."));
    assert!(angry[0].duration_ms < take.duration_ms);

    // Shot office_night_02 carries the line: under this recipe it is not
    // ready until the line is locked.
    shoot(&project, "office_night_02");
    let readiness = export::readiness(&project).unwrap();
    let missing = |shot: &str| {
        readiness
            .missing
            .iter()
            .find(|missing| missing.shot.shot() == shot)
            .map(|missing| missing.reason.clone())
    };
    assert_eq!(
        missing("office_night_02").as_deref(),
        Some("line 1 is not dubbed and locked")
    );

    assert!(matches!(
        dubbing::lock(&project, SCENE, 1),
        Err(GenerationError::Provider { source, .. })
            if matches!(source.downcast_ref::<DubbingError>(), Some(DubbingError::NoSelection(1)))
    ));
    document::select(&project, SCENE, "line.office_night.01-2c40ae8c8fe4.0002").unwrap();
    document::reject(&project, SCENE, "line.office_night.01-2c40ae8c8fe4.0003").unwrap();
    let locked = dubbing::lock(&project, SCENE, 1).unwrap();
    assert_eq!(locked.artifact_id, "line.office_night.01-2c40ae8c8fe4.0002");
    assert!(matches!(
        document::reject(&project, SCENE, "line.office_night.01-2c40ae8c8fe4.0002"),
        Err(DubbingError::Locked(_))
    ));
    assert_eq!(state(&project, "dialogue"), StepState::Done);

    // Takes are the scene's media: the locked one is pinned; the rejected
    // one is the newest take, which the recent window keeps.
    let records = kineto_project::media::catalog(&project).unwrap();
    let classes = kineto_project::media::retention(&records, 1);
    let class = |id: &str| {
        classes
            .iter()
            .find(|retention| retention.record.artifact_id == id)
            .unwrap()
            .class
    };
    assert_eq!(
        class("line.office_night.01-2c40ae8c8fe4.0002"),
        kineto_project::media::RetentionClass::Pinned
    );
    assert_eq!(
        class("line.office_night.01-2c40ae8c8fe4.0003"),
        kineto_project::media::RetentionClass::Recent
    );

    // The locked take is how long the line is now.
    let screenplay: Screenplay = revisions::read(
        &project,
        &revisions::current(&project, DocumentKind::Screenplay)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let durations = dialogue::line_durations(&project, &screenplay).unwrap();
    assert_eq!(durations[0].duration_ms, made[1].duration_ms);
    assert_eq!(durations[0].source, dialogue::DurationSource::Measured);

    let out = temp.0.join("out");
    fs::create_dir_all(&out).unwrap();
    let package = export::export(
        &project,
        &ExportRequest {
            destination: out.clone(),
            name: "dailies".to_owned(),
            partial: true,
            replace: false,
            timeline: true,
        },
        UNIX_EPOCH + Duration::from_secs(1_791_015_045),
    )
    .unwrap();
    assert_eq!((package.shots, package.lines), (1, 1));
    let folder = out.join("dailies");
    assert_eq!(
        fs::read(folder.join("office_night_02_line01.wav")).unwrap(),
        fs::read(target.join(&made[1].media_path)).unwrap()
    );
    let json = |name: &str| -> serde_json::Value {
        serde_json::from_slice(&fs::read(folder.join(name)).unwrap()).unwrap()
    };
    let sidecar = json("office_night_02.json");
    assert_eq!(sidecar["schema_version"], 3);
    let line = &sidecar["lines"][0];
    assert_eq!(line["line"], 1);
    assert_eq!(line["character"], "interviewer");
    assert_eq!(line["text"], "Was it worth it?");
    assert_eq!(line["file"], "office_night_02_line01.wav");
    assert_eq!(
        line["artifact_id"],
        "line.office_night.01-2c40ae8c8fe4.0002"
    );
    assert_eq!(line["voice_id"], "demo_low");
    assert_eq!(line["emotion"], "calm");
    let manifest = json("manifest.json");
    assert_eq!(
        manifest["scenes"][0]["shots"][0]["lines"],
        serde_json::json!(["office_night_02_line01.wav"])
    );

    // The timeline: the shot as long as its file, its line at its start.
    assert_eq!(
        manifest["timeline"],
        serde_json::json!({
            "otio": "timeline.otio",
            "edl": "timeline.edl",
            "frame_rate": 24,
            "trimmed_lines": []
        })
    );
    let list = fs::read_to_string(folder.join("timeline.edl")).unwrap();
    assert!(list.contains("* FROM CLIP NAME: office_night_02.mp4"));
    assert!(list.contains("* FROM CLIP NAME: office_night_02_line01.wav"));
    let timeline = json("timeline.otio");
    assert_eq!(timeline["OTIO_SCHEMA"], "Timeline.1");
    let tracks = &timeline["tracks"]["children"];
    let clip = &tracks[0]["children"][0];
    assert_eq!(clip["media_reference"]["target_url"], "office_night_02.mp4");
    let seconds = kineto_core::timeline::mp4_duration_ms(
        &fs::read(folder.join("office_night_02.mp4")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        clip["source_range"]["duration"]["value"],
        kineto_core::timeline::frames(u64::from(seconds)) as f64
    );
    let spoken = &tracks[1]["children"][0];
    assert_eq!(
        spoken["media_reference"]["target_url"],
        "office_night_02_line01.wav"
    );
    assert_eq!(
        spoken["metadata"]["kineto"]["artifact_id"],
        "line.office_night.01-2c40ae8c8fe4.0002"
    );
    drop(project);

    // The lock is canonical: nothing of it is runtime state.
    fs::remove_dir_all(target.join(".kineto")).unwrap();
    let project = CanonicalProject::open(&target).unwrap();
    assert_eq!(document::locks(&project, SCENE).unwrap(), [locked]);
}

#[test]
fn a_take_goes_stale_with_its_line_or_its_voice_and_a_stale_choice_is_not_locked() {
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"), true);
    dub(&project, &takes(1)).unwrap();
    document::select(&project, SCENE, "line.office_night.01-2c40ae8c8fe4.0001").unwrap();
    dubbing::lock(&project, SCENE, 1).unwrap();
    let stale =
        |project: &CanonicalProject| dubbing::dubbed_lines(project).unwrap()[0].stale.clone();
    assert!(stale(&project).is_empty());

    // The interviewer's voice is locked again.
    voice::audition(
        &project,
        &LocalDemoVoices,
        "interviewer",
        &[VoiceChoice {
            voice: VoiceSettings::new("demo_high", "en"),
            name: "High".to_owned(),
        }],
        None,
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    voice_document::select(&project, "interviewer", "voice.interviewer.0002").unwrap();
    voice::lock(&project, "interviewer").unwrap();
    assert_eq!(stale(&project), ["voice.interviewer"]);
    assert!(matches!(
        dubbing::lock(&project, SCENE, 1),
        Err(GenerationError::Provider { source, .. })
            if matches!(source.downcast_ref::<DubbingError>(), Some(DubbingError::StaleSelection(1)))
    ));
    assert_eq!(
        state(&project, "dialogue"),
        StepState::Done,
        "stale, still locked"
    );
    assert!(
        workflow::status(&project)
            .unwrap()
            .iter()
            .find(|status| status.id == "dialogue")
            .unwrap()
            .stale
            .contains(&"office_night line 1: voice.interviewer".to_owned())
    );

    // The screenplay directs the line otherwise: the same words, the same
    // line, its take stale.
    rewrite(&project, |blocks| {
        let line = blocks
            .iter_mut()
            .find(|block| block.text == "Was it worth it?")
            .unwrap();
        line.parenthetical = Some("flatly".to_owned());
    });
    assert_eq!(stale(&project), ["line.parenthetical", "voice.interviewer"]);

    // It rewords the line: that is another line, with no takes and no lock
    // of its own. The old take and lock stay, under the old words.
    let old_lock = document::locks(&project, SCENE).unwrap();
    rewrite(&project, |blocks| {
        let line = blocks
            .iter_mut()
            .find(|block| block.text == "Was it worth it?")
            .unwrap();
        line.text = "Was it worth all of it?".to_owned();
    });
    let reworded = dubbing::dubbed_lines(&project).unwrap().remove(0);
    assert!(reworded.takes.is_empty());
    assert_eq!(
        (reworded.lock, reworded.stale),
        (None, Vec::<String>::new())
    );
    assert_eq!(state(&project, "dialogue"), StepState::Ready);
    assert_eq!(document::locks(&project, SCENE).unwrap(), old_lock);
    assert_eq!(
        old_lock[0].text_hash,
        document::line_text_hash("Was it worth it?")
    );

    // A new take of the new words in the new voice fits, and locks; the
    // old words' selection is not this line's.
    let fresh = dub(&project, &takes(1)).unwrap();
    assert_eq!(
        fresh[0].artifact_id,
        format!(
            "line.office_night.01-{}.0001",
            digits("Was it worth all of it?")
        )
    );
    assert_eq!(fresh[0].text, "Was it worth all of it?");
    assert_eq!(fresh[0].voice.voice_id, "demo_high");
    assert!(no_selection(dubbing::lock(&project, SCENE, 1), 1));
    document::select(&project, SCENE, &fresh[0].artifact_id).unwrap();
    let new_lock = dubbing::lock(&project, SCENE, 1).unwrap();
    assert!(stale(&project).is_empty());
    assert_eq!(
        document::locks(&project, SCENE).unwrap(),
        [old_lock[0].clone(), new_lock]
    );
}

#[test]
fn a_line_put_ahead_of_a_dubbed_line_inherits_none_of_its_takes_or_its_lock() {
    const B: &str = "Did anyone see you leave the building that night?";
    const X: &str = "Sit.";
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);

    // B is line 2, dubbed and locked.
    rewrite(&project, |blocks| blocks.push(interviewer(B)));
    let b = dub_line(&project, 2, &takes(1)).unwrap().remove(0);
    assert_eq!(
        b.artifact_id,
        format!("line.office_night.02-{}.0001", digits(B))
    );
    document::select(&project, SCENE, &b.artifact_id).unwrap();
    let b_lock = dubbing::lock(&project, SCENE, 2).unwrap();

    // X goes in ahead of it: X is line 2 now, and B line 3.
    rewrite(&project, |blocks| {
        let at = blocks.iter().position(|block| block.text == B).unwrap();
        blocks.insert(at, interviewer(X));
    });
    let lines = dubbing::dubbed_lines(&project).unwrap();
    assert_eq!((lines[1].line.line, lines[1].line.text.as_str()), (2, X));
    assert_eq!((lines[2].line.line, lines[2].line.text.as_str()), (3, B));
    for line in &lines[1..] {
        assert!(line.takes.is_empty(), "{:?} has takes", line.line.text);
        assert_eq!((&line.lock, &line.stale), (&None, &Vec::new()));
    }
    // B's selected take does not lock X.
    assert!(no_selection(dubbing::lock(&project, SCENE, 2), 2));

    // X's takes are numbered under its own words, and locking X leaves B's
    // lock as it was.
    let x = dub_line(&project, 2, &takes(1)).unwrap().remove(0);
    assert_eq!(
        x.artifact_id,
        format!("line.office_night.02-{}.0001", digits(X))
    );
    assert_eq!(x.text, X);
    document::select(&project, SCENE, &x.artifact_id).unwrap();
    let x_lock = dubbing::lock(&project, SCENE, 2).unwrap();
    assert_eq!(
        document::locks(&project, SCENE).unwrap(),
        [b_lock.clone(), x_lock.clone()]
    );
    let lines = dubbing::dubbed_lines(&project).unwrap();
    assert_eq!(
        lines[1]
            .takes
            .iter()
            .map(|take| take.artifact_id.as_str())
            .collect::<Vec<_>>(),
        [x.artifact_id.as_str()]
    );
    assert_eq!(lines[1].lock.as_ref(), Some(&x_lock));
    assert!(lines[2].takes.is_empty() && lines[2].lock.is_none());

    // B's take is still B's: its words, its hash, its selection.
    let recorded = document::takes(&project, SCENE).unwrap();
    let kept = recorded
        .iter()
        .find(|take| take.artifact_id == b.artifact_id)
        .unwrap();
    assert_eq!(
        (kept.line, kept.text.as_str(), kept.status),
        (2, B, TakeStatus::Selected)
    );
    assert_eq!(kept.text_hash, document::line_text_hash(B));
    assert_eq!(b_lock.text_hash, kept.text_hash);

    // Line 2 is as long as X's take says, and B, now line 3, is estimated.
    assert_ne!(x.duration_ms, b.duration_ms);
    let screenplay: Screenplay = revisions::read(
        &project,
        &revisions::current(&project, DocumentKind::Screenplay)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let durations = dialogue::line_durations(&project, &screenplay).unwrap();
    assert_eq!(
        (durations[1].duration_ms, durations[1].source),
        (x.duration_ms, dialogue::DurationSource::Measured)
    );
    assert_eq!(durations[2].source, dialogue::DurationSource::Estimated);
    drop(project);

    // The files alone say the same.
    fs::remove_dir_all(target.join(".kineto")).unwrap();
    let project = CanonicalProject::open(&target).unwrap();
    assert_eq!(
        document::locks(&project, SCENE).unwrap(),
        [b_lock, x_lock.clone()]
    );
    assert_eq!(
        dubbing::dubbed_lines(&project).unwrap()[1].lock.as_ref(),
        Some(&x_lock)
    );
}

#[test]
fn a_version_1_record_is_read_by_the_words_its_takes_say_and_kept_when_written() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);
    let made = dub(&project, &takes(2)).unwrap();
    document::select(&project, SCENE, &made[0].artifact_id).unwrap();
    dubbing::lock(&project, SCENE, 1).unwrap();
    drop(project);

    // As schema version 1 wrote it: takes named by their place alone, and
    // no text hashes. The job records named the takes otherwise.
    let path = target.join("scenes/office_night/voice/lines.json");
    let mut log: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let unhashed = |id: &serde_json::Value| {
        id.as_str()
            .unwrap()
            .replace(&format!("-{}", digits("Was it worth it?")), "")
    };
    for take in log["takes"].as_array_mut().unwrap() {
        let media = unhashed(&take["media_path"]);
        fs::rename(
            target.join(take["media_path"].as_str().unwrap()),
            target.join(&media),
        )
        .unwrap();
        take["artifact_id"] = unhashed(&take["artifact_id"]).into();
        take["media_path"] = media.into();
        take.as_object_mut().unwrap().remove("text_hash").unwrap();
    }
    for lock in log["locks"].as_array_mut().unwrap() {
        lock["artifact_id"] = unhashed(&lock["artifact_id"]).into();
        lock["media_path"] = unhashed(&lock["media_path"]).into();
        lock.as_object_mut().unwrap().remove("text_hash").unwrap();
    }
    log["schema_version"] = 1.into();
    fs::write(&path, serde_json::to_vec_pretty(&log).unwrap()).unwrap();
    fs::remove_dir_all(target.join(".kineto")).unwrap();

    let project = CanonicalProject::open(&target).unwrap();
    let line = dubbing::dubbed_lines(&project).unwrap().remove(0);
    assert_eq!(
        line.takes
            .iter()
            .map(|take| take.artifact_id.as_str())
            .collect::<Vec<_>>(),
        ["line.office_night.01.0001", "line.office_night.01.0002"]
    );
    assert_eq!(
        line.lock.as_ref().map(|lock| lock.artifact_id.as_str()),
        Some("line.office_night.01.0001")
    );
    assert!(line.stale.is_empty());
    let screenplay: Screenplay = revisions::read(
        &project,
        &revisions::current(&project, DocumentKind::Screenplay)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        dialogue::line_durations(&project, &screenplay).unwrap()[0].source,
        dialogue::DurationSource::Measured
    );

    // Asked for the same way again, the two old takes count; a third is
    // numbered under the line's words, beside them.
    assert_eq!(dub(&project, &takes(2)).unwrap().len(), 2);
    let three = dub(&project, &takes(3)).unwrap();
    assert_eq!(
        three[2].artifact_id,
        "line.office_night.01-2c40ae8c8fe4.0001"
    );

    // Written again, the record is version 2 and keeps the old IDs.
    let log: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let hash = document::line_text_hash("Was it worth it?");
    assert_eq!(log["schema_version"], 2);
    assert_eq!(log["takes"][0]["artifact_id"], "line.office_night.01.0001");
    assert_eq!(log["takes"][0]["text_hash"], hash.as_str());
    assert_eq!(log["locks"][0]["text_hash"], hash.as_str());

    // Reworded, the line inherits none of them.
    rewrite(&project, |blocks| {
        let line = blocks
            .iter_mut()
            .find(|block| block.text == "Was it worth it?")
            .unwrap();
        line.text = "Was it worth all of it?".to_owned();
    });
    let line = dubbing::dubbed_lines(&project).unwrap().remove(0);
    assert!(line.takes.is_empty() && line.lock.is_none());
}

#[test]
fn nothing_is_spoken_without_a_locked_voice_of_this_provider_or_a_line() {
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"), false);
    // Casting is not done (the filmmaker is not cast), so voices wait.
    assert_eq!(state(&project, "voices"), StepState::Waiting);
    assert!(matches!(
        dub(&project, &takes(1)),
        Err(GenerationError::Prerequisite(Prerequisite::LockedVoice))
    ));
    assert!(matches!(
        dubbing::dub(
            &project,
            &LocalDemoVoices,
            SCENE,
            7,
            &takes(1),
            Submission::Once,
            &mut NoHooks
        ),
        Err(GenerationError::Prerequisite(Prerequisite::Line))
    ));
    drop(project);

    // A voice from the demo's list means nothing to MiniMax.
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"), true);
    let minimax = MinimaxClient::with_transport(
        FakeImages::default(),
        SecretString::new("sk-test".to_owned()).unwrap(),
        "https://example.test",
    );
    assert!(matches!(
        dubbing::dub(
            &project,
            &minimax,
            SCENE,
            1,
            &takes(1),
            Submission::Once,
            &mut NoHooks
        ),
        Err(GenerationError::Incapable(missing)) if missing == ["speech.voice"]
    ));
    assert!(document::takes(&project, SCENE).unwrap().is_empty());
}

#[test]
fn a_lines_estimate_follows_the_pace_of_its_speakers_locked_voice() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);
    let screenplay: Screenplay = revisions::read(
        &project,
        &revisions::current(&project, DocumentKind::Screenplay)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let estimate = || dialogue::line_durations(&project, &screenplay).unwrap()[0].clone();
    let plain = estimate_duration_ms("Was it worth it?", NORMAL_SPEED);
    // The demo's audition ran exactly as long as estimated: the pace is 1.
    let line = estimate();
    assert_eq!(line.source, dialogue::DurationSource::Estimated);
    assert_eq!(line.duration_ms, plain);

    // The locked voice turns out slower or faster than estimated; every
    // estimate of its lines follows, within twice either way.
    let path = target.join("characters/interviewer/voice_auditions.json");
    let original = fs::read(&path).unwrap();
    let paced = |numerator: u32, denominator: u32| {
        let mut log: serde_json::Value = serde_json::from_slice(&original).unwrap();
        let audition = &mut log["auditions"][0];
        let said = estimate_duration_ms(
            audition["line"].as_str().unwrap(),
            u16::try_from(audition["voice"]["speed_permille"].as_u64().unwrap()).unwrap(),
        );
        audition["duration_ms"] = (said * numerator / denominator).into();
        fs::write(&path, serde_json::to_vec_pretty(&log).unwrap()).unwrap();
        estimate().duration_ms
    };
    let near = |got: u32, want: u32| assert!(got.abs_diff(want) <= 2, "{got} is not about {want}");
    near(paced(3, 2), plain * 3 / 2);
    near(paced(10, 1), plain * 2);
    near(paced(1, 4), plain / 2);
}

/// Every file under `dir`, by its path there, with its bytes.
fn tree(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(
                    path.strip_prefix(dir).unwrap().to_owned(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    files
}

/// Replacing a package can remove what holds its name, so a package that
/// would land inside the project, or hold it, is refused before anything
/// is written, however its folder is spelled.
#[test]
fn a_package_is_never_made_inside_the_project_or_over_it() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, false);
    shoot(&project, "office_night_01");
    let before = tree(&target);
    let export = |destination: PathBuf, name: &str| {
        export::export(
            &project,
            &ExportRequest {
                destination,
                name: name.to_owned(),
                partial: true,
                replace: true,
                timeline: false,
            },
            UNIX_EPOCH,
        )
    };
    let refused = |result: Result<export::Exported, export::ExportError>| {
        matches!(result, Err(export::ExportError::OverlapsProject(_)))
    };

    // Over a canonical folder, in one, or over the project itself.
    assert!(refused(export(target.clone(), "scenes")));
    assert!(refused(export(target.join("scenes"), "dailies")));
    assert!(refused(export(temp.0.clone(), "film")));
    // Spelled with `..` through a folder that does not exist, or relative.
    assert!(refused(export(target.join("nowhere/.."), "scenes")));
    assert!(refused(export(
        target.join("scenes/office_night/../../.."),
        "film"
    )));
    #[cfg(unix)]
    {
        let alias = temp.0.join("alias");
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        assert!(refused(export(alias.clone(), "scenes")));
        assert!(refused(export(alias.join("scenes"), "dailies")));
        let linked = temp.0.join("out");
        fs::create_dir_all(&linked).unwrap();
        std::os::unix::fs::symlink(&target, linked.join("dailies")).unwrap();
        assert!(
            refused(export(linked.clone(), "dailies")),
            "a package name linked to the project"
        );
        fs::remove_file(linked.join("dailies")).unwrap();
    }
    assert_eq!(tree(&target), before, "the project is as it was");

    // Outside it, a package is made and replaced as before.
    let out = temp.0.join("out");
    assert_eq!(export(out.clone(), "dailies").unwrap().shots, 1);
    assert_eq!(export(out.clone(), "dailies").unwrap().shots, 1);
    assert!(out.join("dailies/manifest.json").is_file());
}
