//! Learning from the director's choices, offline: what was chosen and
//! passed over gathered as evidence, suggestions from a scripted model
//! checked against it, accepted, reworded or rejected by the director, and
//! voices recommended from the provider's list.

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
    dubbing::{self, DubRequest},
    generation::{GenerationError, ImageProvider, NoHooks, Prerequisite},
    learning,
    minimax::MinimaxKeyframes,
    screenplay,
    speech::{LocalDemoVoices, SpeechGenerator, VoiceSettings},
    story,
    style::{self, SampleRequest, Submission},
    voice::{self, VoiceChoice},
};
use kineto_llm::fake::FakeLlm;
use kineto_project::{
    bible::SAMPLE_BIBLE_JSON,
    cast as cast_document,
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    preferences::{self, Compared, Outcome, Source, SuggestionBatch},
    revisions,
    screenplay::SAMPLE_SCREENPLAY_JSON,
    story::SAMPLE_ANALYSIS_JSON,
    style as style_document, voice as document,
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
            "kineto-learning-{}-{nonce}-{}",
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

const SUGGESTED: &str = r#"{"suggestions":[
  {"dimension":"performance","stance":"prefer","value":"raised voices","reason":"The chosen line take is angry where the passed-over ones are calm.","evidence":["line.office_night.01.0003"]},
  {"dimension":"pacing","stance":"prefer","value":"quick delivery","reason":"The chosen take is faster.","evidence":["line.office_night.01.0003"]},
  {"dimension":"lighting","stance":"avoid","value":"flat light","reason":"Unsupported.","evidence":["voice.interviewer.0002"]}
]}"#;

#[test]
fn choices_become_suggestions_the_director_accepts_rewords_or_turns_down() {
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"), &["interviewer"]);
    assert!(matches!(
        learning::suggest(&project, &FakeLlm::answering([SUGGESTED])),
        Err(GenerationError::Prerequisite(Prerequisite::Choices))
    ));

    // Two auditions, the high one chosen; three takes of the line, the
    // refined angry one chosen.
    voice::audition(
        &project,
        &LocalDemoVoices,
        "interviewer",
        &[choice("demo_low"), choice("demo_high")],
        None,
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    document::select(&project, "interviewer", "voice.interviewer.0002").unwrap();
    voice::lock(&project, "interviewer").unwrap();
    let dub = |request: &DubRequest| {
        dubbing::dub(
            &project,
            &LocalDemoVoices,
            "office_night",
            1,
            request,
            Submission::Once,
            &mut NoHooks,
        )
        .unwrap()
    };
    dub(&DubRequest {
        takes: 2,
        ..DubRequest::default()
    });
    dub(&DubRequest {
        takes: 1,
        delivery: Some(Emotion::Angry),
        speed_permille: Some(1300),
        note: Some("Harder.".to_owned()),
    });
    kineto_project::dubbing::select(&project, "office_night", "line.office_night.01.0003").unwrap();

    let choices = learning::choices(&project).unwrap();
    let kinds: Vec<_> = choices.iter().map(|choice| choice.kind).collect();
    assert_eq!(
        kinds,
        [learning::ChoiceKind::Voice, learning::ChoiceKind::Line]
    );
    assert_eq!(
        choices[1].chosen[0].artifact_id,
        "line.office_night.01.0003"
    );
    assert!(
        choices[1].chosen[0]
            .asked
            .contains("delivery angry; rate 1300‰; note: Harder.")
    );
    assert_eq!(choices[1].passed.len(), 2);
    assert_eq!(learning::unconsidered(&project).unwrap(), 2);
    assert!(!learning::suggestions_ready(&project).unwrap());

    // A suggestion citing work passed over, as if it showed the preference,
    // is asked for again.
    let passed_over = SUGGESTED.replace("voice.interviewer.0002", "voice.interviewer.0001");
    let model = FakeLlm::answering([passed_over.as_str(), SUGGESTED]);
    let record = learning::suggest(&project, &model).unwrap();
    assert_eq!(model.requests().len(), 2);
    assert!(
        format!("{:?}", model.requests()[1])
            .contains("voice.interviewer.0001 was passed over; cite only chosen work")
    );
    let input = &model.requests()[0].messages[1].content;
    assert!(input.contains("chosen line.office_night.01.0003: delivery angry"));
    assert!(input.contains("passed over voice.interviewer.0001: DEMO_LOW"));
    let batch: SuggestionBatch = revisions::read(&project, &record).unwrap();
    assert_eq!(batch.compared, Compared::Text);
    assert_eq!(
        batch.considered,
        ["voice.interviewer.0002", "line.office_night.01.0003"]
    );
    assert_eq!(learning::unconsidered(&project).unwrap(), 0);

    // Nothing takes effect until the director says.
    assert_eq!(preferences::current(&project).unwrap(), None);
    let open = learning::open_suggestions(&project).unwrap();
    assert_eq!(open.len(), 3);
    learning::accept(&project, record.revision, 0, None).unwrap();
    learning::accept(
        &project,
        record.revision,
        1,
        Some("brisk, clipped delivery"),
    )
    .unwrap();
    learning::reject(&project, record.revision, 2).unwrap();
    assert!(learning::open_suggestions(&project).unwrap().is_empty());
    assert!(learning::accept(&project, record.revision, 2, None).is_err());

    let (profile_record, profile) = preferences::current(&project).unwrap().unwrap();
    assert_eq!(profile_record.revision, 2, "each acceptance is a revision");
    assert_eq!(profile.entries.len(), 2);
    assert_eq!(profile.entries[0].value, "raised voices");
    assert_eq!(
        profile.entries[0].source,
        Source::Suggestion {
            reason: "The chosen line take is angry where the passed-over ones are calm.".to_owned(),
            evidence: vec!["line.office_night.01.0003".to_owned()],
        }
    );
    assert_eq!(profile.entries[1].value, "brisk, clipped delivery");
    let outcomes: Vec<_> = preferences::reviews(&project)
        .unwrap()
        .iter()
        .map(|review| review.outcome)
        .collect();
    assert_eq!(
        outcomes,
        [Outcome::Accepted, Outcome::Modified, Outcome::Rejected]
    );
}

#[test]
fn voices_are_recommended_only_from_the_providers_list() {
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"), &["interviewer"]);
    let catalog = LocalDemoVoices.voices().unwrap();
    let unlisted = r#"{"characters":[{"character_id":"interviewer","voices":[{"voice_id":"English_Wiselady","reason":"Measured."}]}]}"#;
    let listed = r#"{"characters":[{"character_id":"interviewer","voices":[
        {"voice_id":"demo_low","reason":"Low and measured, for a patient questioner."},
        {"voice_id":"demo_middle","reason":"Even, for restraint."}]}]}"#;
    let model = FakeLlm::answering([unlisted, listed]);
    voice::recommend(&project, &model, &catalog).unwrap();
    assert_eq!(
        model.requests().len(),
        2,
        "an unlisted voice is asked about again"
    );
    let input = &model.requests()[0].messages[1].content;
    assert!(input.contains("interviewer: "));
    assert!(input.contains("demo_high | High | A high tone, for a light voice."));
    assert!(!input.contains("filmmaker:"), "only characters who speak");
    let recommendations = voice::recommendations(&project).unwrap().unwrap();
    let voices = &recommendations.characters[0].voices;
    assert_eq!(voices[0].voice_id, "demo_low");
    assert_eq!(
        voices[0].name.as_deref(),
        Some("Low"),
        "the listed name is kept"
    );
}
