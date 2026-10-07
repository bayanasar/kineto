//! Casting voices end to end, offline: auditions of a cast character with
//! the local demo, the default line, choosing, locking, staleness, and the
//! auditions as media.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::Engine as _;
use kineto_core::{
    bible, cast,
    generation::{GenerationError, ImageProvider, NoHooks, Prerequisite},
    minimax::MinimaxKeyframes,
    screenplay,
    speech::{LocalDemoVoices, SpeechHooks, SpeechPoint, VoiceSettings},
    story,
    style::{self, SampleRequest, Submission},
    voice::{self, VoiceChoice},
};
use kineto_llm::fake::FakeLlm;
use kineto_project::{
    ContentHash,
    bible::SAMPLE_BIBLE_JSON,
    cast as cast_document,
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    media::{self, MediaKind, RetentionClass, Standing},
    revisions::{self, DocumentKind},
    screenplay::SAMPLE_SCREENPLAY_JSON,
    story::SAMPLE_ANALYSIS_JSON,
    style as style_document,
    voice::{self as document, VoiceError},
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
        unreachable!("casting makes no GET request: {url}")
    }

    fn download(&self, url: &str, _max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
        unreachable!("casting downloads nothing: {url}")
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
            "kineto-voice-casting-{}-{nonce}-{}",
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
        project_id: "voices".to_owned(),
        title: "Voices".to_owned(),
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

/// A project with a locked screenplay, a bible and a locked style, and
/// `cast` characters cast.
fn film(target: &Path, cast_characters: &[&str]) -> CanonicalProject {
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
    let provider = ImageProvider::Minimax(Some(MinimaxClient::shared(
        SecretString::new("sk-test".to_owned()).unwrap(),
    )));
    for character in cast_characters {
        let portraits = cast::audition_with(
            &project,
            adapter(),
            character,
            1,
            Submission::Once,
            &mut NoHooks,
        )
        .unwrap();
        cast_document::select(&project, character, &portraits[0].artifact_id).unwrap();
        cast::make_pack_with(
            &project,
            adapter(),
            character,
            Submission::Once,
            &mut NoHooks,
        )
        .unwrap();
        cast::lock(&project, &provider, character).unwrap();
    }
    project
}

fn choice(voice_id: &str) -> VoiceChoice {
    VoiceChoice {
        voice: VoiceSettings::new(voice_id, "en"),
        name: voice_id.to_uppercase(),
    }
}

fn audition(
    project: &CanonicalProject,
    character: &str,
    choices: &[VoiceChoice],
    line: Option<&str>,
) -> Result<Vec<document::Audition>, GenerationError> {
    voice::audition(
        project,
        &LocalDemoVoices,
        character,
        choices,
        line,
        Submission::Once,
        &mut NoHooks,
    )
}

#[test]
fn a_cast_character_auditions_voices_saying_its_line_and_locks_one() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, &["filmmaker", "interviewer"]);

    let heard = audition(
        &project,
        "interviewer",
        &[choice("demo_low"), choice("demo_high")],
        None,
    )
    .unwrap();
    assert_eq!(
        heard
            .iter()
            .map(|audition| audition.artifact_id.as_str())
            .collect::<Vec<_>>(),
        ["voice.interviewer.0001", "voice.interviewer.0002"]
    );
    // The interviewer's first line in the screenplay.
    assert!(
        heard
            .iter()
            .all(|audition| audition.line == "Was it worth it?")
    );
    assert_eq!(heard[0].voice_name, "DEMO_LOW");
    assert_eq!(heard[0].provider, "local-demo-speech");
    assert_eq!(
        heard[0].media_path,
        "characters/interviewer/voice/voice.interviewer.0001.wav"
    );
    let audio = fs::read(target.join(&heard[0].media_path)).unwrap();
    assert_eq!(ContentHash::of(&audio).as_str(), heard[0].content_hash);
    assert_eq!(heard[0].duration_ms, 1520);
    let depends_on: Vec<_> = heard[0]
        .dependencies
        .iter()
        .map(|dependency| dependency.artifact_id.as_str())
        .collect();
    assert_eq!(
        depends_on,
        ["bible_character.interviewer", "cast.interviewer.0001"]
    );

    // A voice already heard saying the line is not asked again; another
    // delivery is a new audition.
    let calm = VoiceChoice {
        voice: VoiceSettings {
            emotion: Some(Emotion::Calm),
            ..VoiceSettings::new("demo_low", "en")
        },
        name: "DEMO_LOW".to_owned(),
    };
    let again = audition(&project, "interviewer", &[choice("demo_low"), calm], None).unwrap();
    assert_eq!(again[0], heard[0]);
    assert_eq!(again[1].artifact_id, "voice.interviewer.0003");
    assert_eq!(again[1].voice.emotion.as_deref(), Some("calm"));
    // The director's own line, and a character without lines.
    let written = audition(
        &project,
        "interviewer",
        &[choice("demo_low")],
        Some("Again."),
    )
    .unwrap();
    assert_eq!(written[0].line, "Again.");
    let silent = audition(&project, "filmmaker", &[choice("demo_middle")], None).unwrap();
    assert_eq!(silent[0].line, "This is how I sound.");
    assert_eq!(silent[0].artifact_id, "voice.filmmaker.0001");

    assert!(matches!(
        voice::lock(&project, "interviewer"),
        Err(GenerationError::Provider { source, .. })
            if matches!(source.downcast_ref::<VoiceError>(), Some(VoiceError::NoSelection))
    ));
    document::select(&project, "interviewer", "voice.interviewer.0002").unwrap();
    document::reject(&project, "interviewer", "voice.interviewer.0001").unwrap();
    let locked = voice::lock(&project, "interviewer").unwrap();
    assert_eq!(locked.artifact_id, "voice.interviewer.0002");
    assert_eq!(locked.voice.voice_id, "demo_high");
    assert_eq!(locked.provider, "local-demo-speech");
    assert!(
        voice::staleness(&project, "interviewer")
            .unwrap()
            .is_empty()
    );

    // The auditions are the character's media; the locked one is pinned.
    let records = media::catalog(&project).unwrap();
    let voices: Vec<_> = records
        .iter()
        .filter(|record| record.kind == MediaKind::Voice)
        .collect();
    assert_eq!(voices.len(), 5);
    let standing = |id: &str| {
        voices
            .iter()
            .find(|record| record.artifact_id == id)
            .unwrap()
            .standing
    };
    assert_eq!(standing("voice.interviewer.0002"), Standing::Locked);
    assert_eq!(standing("voice.interviewer.0001"), Standing::Passed);
    let classes = media::retention(&records, 1);
    let class = |id: &str| {
        classes
            .iter()
            .find(|retention| retention.record.artifact_id == id)
            .unwrap()
            .class
    };
    assert_eq!(class("voice.interviewer.0002"), RetentionClass::Pinned);
    assert_eq!(class("voice.interviewer.0001"), RetentionClass::Collectable);
    let collected = media::collect(&project, &media::plan(&project, &records, 1), 1).unwrap();
    assert!(
        collected
            .evicted
            .contains(&"voice.interviewer.0001".to_owned())
    );
    assert!(!target.join(&heard[0].media_path).exists());
    assert_eq!(
        document::auditions(&project, "interviewer").unwrap()[0].media_state,
        Some(kineto_project::media::MediaState::Evicted)
    );
    drop(project);

    fs::remove_dir_all(target.join(".kineto")).unwrap();
    let project = CanonicalProject::open(&target).unwrap();
    assert_eq!(
        document::locked(&project, "interviewer").unwrap(),
        Some(locked)
    );
}

#[test]
fn a_voice_goes_stale_with_its_character_and_a_stale_choice_is_not_locked() {
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"), &["interviewer"]);
    audition(&project, "interviewer", &[choice("demo_low")], None).unwrap();
    document::select(&project, "interviewer", "voice.interviewer.0001").unwrap();
    voice::lock(&project, "interviewer").unwrap();

    let record = revisions::latest(&project, DocumentKind::Bible)
        .unwrap()
        .unwrap();
    let mut edited: kineto_project::bible::Bible = revisions::read(&project, &record).unwrap();
    let index = edited
        .characters
        .iter()
        .position(|character| character.id == "interviewer")
        .unwrap();
    edited.characters[index].description = "Older now, and tired.".to_owned();
    kineto_project::bible::edit(&project, record.revision, &edited).unwrap();
    assert_eq!(
        voice::staleness(&project, "interviewer").unwrap(),
        ["bible_character.interviewer"]
    );
    // The selected audition was made for the character as it was.
    assert!(matches!(
        voice::lock(&project, "interviewer"),
        Err(GenerationError::Provider { source, .. })
            if matches!(source.downcast_ref::<VoiceError>(), Some(VoiceError::StaleSelection))
    ));
    // A new audition hears the character as it is now, and locks.
    let fresh = audition(&project, "interviewer", &[choice("demo_low")], None).unwrap();
    assert_eq!(fresh[0].artifact_id, "voice.interviewer.0002");
    document::select(&project, "interviewer", "voice.interviewer.0002").unwrap();
    voice::lock(&project, "interviewer").unwrap();
    assert!(
        voice::staleness(&project, "interviewer")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn nothing_is_spoken_before_the_character_is_cast_or_by_a_provider_that_cannot() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, &[]);
    assert!(matches!(
        audition(&project, "interviewer", &[choice("demo_low")], None),
        Err(GenerationError::Prerequisite(Prerequisite::LockedCast))
    ));
    drop(project);

    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, &["interviewer"]);
    let klingon = VoiceChoice {
        voice: VoiceSettings::new("demo_low", "tlh"),
        name: "Low".to_owned(),
    };
    assert!(matches!(
        audition(&project, "interviewer", &[choice("demo_low"), klingon], None),
        Err(GenerationError::Incapable(missing)) if missing == ["speech.language"]
    ));
    // Checked before anything is spoken: the first choice was not made
    // either.
    assert!(
        document::auditions(&project, "interviewer")
            .unwrap()
            .is_empty()
    );
    assert!(!target.join("characters/interviewer/voice").exists());
}

#[test]
fn an_audition_written_before_an_interruption_is_recorded_as_the_project_opens() {
    struct StopAt(SpeechPoint);

    impl SpeechHooks for StopAt {
        fn reached(&mut self, point: SpeechPoint) {
            assert_ne!(point, self.0, "stopped at {point:?}");
        }
    }

    let temp = TempDir::new();
    let project = film(&temp.0.join("film"), &["interviewer"]);
    let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        voice::audition(
            &project,
            &LocalDemoVoices,
            "interviewer",
            &[choice("demo_low")],
            None,
            Submission::Once,
            &mut StopAt(SpeechPoint::Written),
        )
    }));
    assert!(stopped.is_err());
    assert!(
        document::auditions(&project, "interviewer")
            .unwrap()
            .is_empty()
    );

    let recovery = voice::recover(&project).unwrap();
    assert_eq!(recovery.attached, ["voice.interviewer.0001"]);
    assert!(recovery.unresolved.is_empty());
    let recorded = document::auditions(&project, "interviewer").unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].line, "Was it worth it?");
    // Asking for the same voice again finds it heard.
    let again = audition(&project, "interviewer", &[choice("demo_low")], None).unwrap();
    assert_eq!(again, recorded);
}
