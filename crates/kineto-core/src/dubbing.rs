//! Dubbing: takes of each line of the screenplay in its character's locked
//! voice, the one the director selects, and the lock.
//!
//! A take is paid speech (see [`crate::speech`]) in its provider's `line`
//! partition, numbered per line, so the director's decision to risk paying
//! twice for one line's takes retires none of another's. What the line is
//! asked to sound like is compiled by [`crate::speech_prompt`]. Asking for
//! takes says how many there should be, not how many to add: takes already
//! made the same way, and not rejected, count.
//!
//! A line is known by its place in its scene and the hash of its words:
//! inserting, removing or rewording a line ahead of it makes another line,
//! which never inherits its takes or its lock. A take goes stale when its
//! line is gone, someone else says it or its direction changes in the
//! screenplay, or its character's voice is locked again; a stale take is
//! not locked. A locked line's length is measured, not estimated, from then
//! on (see [`crate::dialogue`]).

use kineto_project::{
    InputHash, InputHashMaterial, cast as cast_document,
    dubbing::{self as document, DubbingError, LineTake, LockedLine, SpanRecord, TakeStatus},
    manifest::CanonicalProject,
    revisions::{self, DependencyRecord, DocumentKind},
    screenplay::{BlockKind, Screenplay},
    shot::PromptProvenance,
    voice::{self as voice_document, VoiceSpec},
};
use kineto_providers::Emotion;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    documents::document_error,
    generation::{GenerationError, Prerequisite},
    paid_image::Submission,
    speech::{
        DEMO_SPEECH_PROVIDER, MINIMAX_SPEECH_PROVIDER, SpeechGenerator, SpeechHooks, SpeechJobs,
        SpeechLedger, SpeechRecovery, SpokenOutput, check_speech,
    },
    speech_prompt::{self, LineDirection},
    voice,
};

const INTENT_DOMAIN: &[u8] = b"kineto.line-intent.v1";

const PLAN_SCHEMA_VERSION: u32 = 1;

/// Every key speech is recorded under, which recovery looks through.
const SPEECH_PROVIDERS: [&str; 2] = [DEMO_SPEECH_PROVIDER, MINIMAX_SPEECH_PROVIDER];

/// A line of the screenplay, as dubbing addresses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenplayLine {
    pub scene_id: String,
    /// Its place among the scene's lines, from 1.
    pub line: u32,
    pub character: Option<String>,
    pub text: String,
    /// The hash of `text`, which with `line` names the line.
    pub text_hash: String,
    pub parenthetical: Option<String>,
}

/// Every line of `screenplay`, in order.
#[must_use]
pub fn screenplay_lines(screenplay: &Screenplay) -> Vec<ScreenplayLine> {
    screenplay
        .scenes
        .iter()
        .flat_map(|scene| {
            (1..)
                .zip(
                    scene
                        .blocks
                        .iter()
                        .filter(|block| block.kind == BlockKind::Dialogue),
                )
                .map(|(line, block)| ScreenplayLine {
                    scene_id: scene.id.clone(),
                    line,
                    character: block.character.clone(),
                    text: block.text.trim().to_owned(),
                    text_hash: document::line_text_hash(block.text.trim()),
                    parenthetical: block
                        .parenthetical
                        .as_deref()
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .map(str::to_owned),
                })
        })
        .collect()
}

/// Every line of the screenplay work builds on: the locked one, or the
/// latest.
pub fn current_lines(project: &CanonicalProject) -> Result<Vec<ScreenplayLine>, GenerationError> {
    let Some(record) =
        revisions::current(project, DocumentKind::Screenplay).map_err(document_error)?
    else {
        return Ok(Vec::new());
    };
    let screenplay: Screenplay = revisions::read(project, &record).map_err(document_error)?;
    Ok(screenplay_lines(&screenplay))
}

fn line_in(
    project: &CanonicalProject,
    scene_id: &str,
    line: u32,
) -> Result<ScreenplayLine, GenerationError> {
    current_lines(project)?
        .into_iter()
        .find(|found| found.scene_id == scene_id && found.line == line)
        .ok_or(GenerationError::Prerequisite(Prerequisite::Line))
}

/// What the director asks of a line's takes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DubRequest {
    /// How many takes made this way there should be, at least 1.
    pub takes: usize,
    /// The delivery to speak it with instead of the screenplay's.
    pub delivery: Option<Emotion>,
    /// The rate to speak it at, in thousandths of normal.
    pub speed_permille: Option<u16>,
    /// The director's note, kept with the takes.
    pub note: Option<String>,
}

/// What a take's record needs beyond the speech, kept before the call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TakePlan {
    schema_version: u32,
    artifact_id: String,
    input_hash: String,
    scene_id: String,
    line: u32,
    character: String,
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parenthetical: Option<String>,
    voice: VoiceSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    provenance: PromptProvenance,
    dependencies: Vec<DependencyRecord>,
}

struct LineTakes;

impl SpeechLedger for LineTakes {
    type Plan = TakePlan;

    const KIND: &'static str = "line";

    fn artifact_id(plan: &TakePlan) -> &str {
        &plan.artifact_id
    }

    fn input_hash(plan: &TakePlan) -> &str {
        &plan.input_hash
    }

    fn media_dir(plan: &TakePlan) -> String {
        document::media_dir(&plan.scene_id).unwrap_or_default()
    }

    fn attach(
        project: &CanonicalProject,
        plan: &TakePlan,
        spoken: &SpokenOutput,
    ) -> Result<(), GenerationError> {
        if spoken.artifact_id.as_str() != plan.artifact_id
            || spoken.input_hash.as_str() != plan.input_hash
            || plan.schema_version != PLAN_SCHEMA_VERSION
        {
            return Err(GenerationError::Unexpected(
                "a take's speech belongs to another plan",
            ));
        }
        document::attach(
            project,
            &plan.scene_id,
            LineTake {
                artifact_id: plan.artifact_id.clone(),
                line: plan.line,
                character: plan.character.clone(),
                text: plan.text.clone(),
                text_hash: document::line_text_hash(&plan.text),
                parenthetical: plan.parenthetical.clone(),
                status: TakeStatus::Candidate,
                voice: plan.voice.clone(),
                note: plan.note.clone(),
                content_hash: spoken.content_hash.as_str().to_owned(),
                input_hash: plan.input_hash.clone(),
                provider: spoken.provider.clone(),
                model: spoken.model.clone(),
                parameters: spoken.parameters.clone(),
                media_path: spoken
                    .media_path
                    .as_path()
                    .to_string_lossy()
                    .replace('\\', "/"),
                duration_ms: spoken.duration_ms,
                sentences: spoken
                    .sentences
                    .iter()
                    .map(|span| SpanRecord {
                        text: span.text.clone(),
                        start_ms: span.start_ms,
                        end_ms: span.end_ms,
                    })
                    .collect(),
                provenance: plan.provenance.clone(),
                dependencies: plan.dependencies.clone(),
                media_state: None,
            },
        )
        .map(|_| ())
        .map_err(document_error)
    }
}

/// Takes are numbered per line.
fn numbered<'a>(spoken: &'a ScreenplayLine) -> impl Fn(&str) -> bool + 'a {
    move |artifact_id| {
        document::is_take_of(
            artifact_id,
            &spoken.scene_id,
            spoken.line,
            &spoken.text_hash,
        )
    }
}

/// Record takes an interrupted session left, as a project opens, for every
/// speech provider. Never calls one.
pub fn recover(project: &CanonicalProject) -> Result<SpeechRecovery, GenerationError> {
    let mut all = SpeechRecovery::default();
    for provider in SPEECH_PROVIDERS {
        let recovery = crate::speech::recover::<LineTakes>(project, provider)?;
        all.attached.extend(recovery.attached);
        all.unresolved.extend(recovery.unresolved);
        all.needs_attention += recovery.needs_attention;
    }
    Ok(all)
}

fn canonical_intent(line: &ScreenplayLine, note: Option<&str>) -> Vec<u8> {
    let mut hasher = Sha256::new();
    let mut field = |bytes: &[u8]| {
        hasher.update(
            u64::try_from(bytes.len())
                .expect("length fits in u64")
                .to_be_bytes(),
        );
        hasher.update(bytes);
    };
    field(INTENT_DOMAIN);
    field(line.scene_id.as_bytes());
    field(&line.line.to_be_bytes());
    field(line.text.as_bytes());
    field(line.parenthetical.as_deref().unwrap_or("").as_bytes());
    field(note.unwrap_or("").as_bytes());
    hasher.finalize().to_vec()
}

/// Have `request.takes` takes of line `line` of `scene_id` made as
/// `request` says, in its character's locked voice: takes already made the
/// same way, and not rejected, count. Returns them, oldest first.
pub fn dub<G: SpeechGenerator>(
    project: &CanonicalProject,
    generator: &G,
    scene_id: &str,
    line: u32,
    request: &DubRequest,
    submission: Submission,
    hooks: &mut dyn SpeechHooks,
) -> Result<Vec<LineTake>, GenerationError> {
    let spoken_line = line_in(project, scene_id, line)?;
    let character = spoken_line
        .character
        .clone()
        .ok_or(GenerationError::Prerequisite(Prerequisite::Line))?;
    let lock = voice_document::locked(project, &character)
        .map_err(document_error)?
        .ok_or(GenerationError::Prerequisite(Prerequisite::LockedVoice))?;
    if !voice::staleness(project, &character)?.is_empty() {
        return Err(GenerationError::Prerequisite(Prerequisite::LockedVoice));
    }
    // A voice ID is the provider's own: another provider cannot speak it.
    if lock.provider != generator.provider() {
        return Err(GenerationError::Incapable(vec!["speech.voice"]));
    }
    let capabilities = generator.capabilities();
    let locked_voice = voice::settings_of(&lock.voice)?;
    let note = request
        .note
        .as_deref()
        .map(str::trim)
        .filter(|note| !note.is_empty());
    let prompt = speech_prompt::compile(LineDirection {
        text: &spoken_line.text,
        parenthetical: spoken_line.parenthetical.as_deref(),
        voice: &locked_voice,
        delivery: request.delivery,
        speed_permille: request.speed_permille,
        note,
        capabilities: &capabilities,
    });
    let format = check_speech(&capabilities, &prompt.voice, &spoken_line.text)?;
    let dependencies =
        vec![
            voice_document::lock_dependency(&lock).ok_or(GenerationError::Unexpected(
                "a voice lock has no dependency",
            ))?,
        ];
    let input_hash = InputHash::derive(InputHashMaterial {
        dependencies: &dependencies,
        canonical_intent: &canonical_intent(&spoken_line, note),
        provider: generator.provider(),
        model: generator.model(),
        canonical_params: generator.canonical_params(&prompt.voice, format).as_bytes(),
        compiler_version: speech_prompt::SPEECH_COMPILER_VERSION,
    });
    // Takes an interruption left are recorded first, so they count.
    let (mut jobs, _) =
        SpeechJobs::<LineTakes, G>::open(project, generator, numbered(&spoken_line), submission)?;
    let made_this_way = |project: &CanonicalProject| -> Result<Vec<LineTake>, GenerationError> {
        Ok(document::takes(project, scene_id)
            .map_err(document_error)?
            .into_iter()
            .filter(|take| {
                take.line == line
                    && take.text_hash == spoken_line.text_hash
                    && take.input_hash == input_hash.as_str()
                    && take.status != TakeStatus::Rejected
            })
            .collect())
    };
    let wanted = request.takes.max(1);
    for _ in made_this_way(project)?.len()..wanted {
        let artifact_id =
            document::next_artifact_id(project, scene_id, line, &spoken_line.text_hash)
                .map_err(document_error)?;
        let plan = TakePlan {
            schema_version: PLAN_SCHEMA_VERSION,
            artifact_id: artifact_id.as_str().to_owned(),
            input_hash: input_hash.as_str().to_owned(),
            scene_id: scene_id.to_owned(),
            line,
            character: character.clone(),
            text: spoken_line.text.clone(),
            parenthetical: spoken_line.parenthetical.clone(),
            voice: voice::spec_of(&prompt.voice),
            note: note.map(str::to_owned),
            provenance: prompt.provenance.clone(),
            dependencies: dependencies.iter().map(DependencyRecord::from).collect(),
        };
        jobs.speak(&plan, &spoken_line.text, &prompt.voice, hooks)?;
    }
    made_this_way(project)
}

/// What `takes` takes of a line of `text` cost with `generator`, in USD
/// micros; `None` when the provider does not say.
#[must_use]
pub fn dub_cost<G: SpeechGenerator>(generator: &G, takes: usize, text: &str) -> Option<u64> {
    generator
        .capabilities()
        .cost_micros(text.chars().count())
        .map(|cost| cost.saturating_mul(u64::try_from(takes).unwrap_or(u64::MAX)))
}

/// Why `take` no longer fits its line: `line.missing` when the screenplay
/// no longer has those words in that place, `line.character` when someone
/// else says them, `line.parenthetical` when their direction changed, and
/// `voice.<character>` when the character's voice was locked again or went.
/// Empty when it fits.
pub fn staleness(
    project: &CanonicalProject,
    scene_id: &str,
    take: &LineTake,
) -> Result<Vec<String>, GenerationError> {
    let lines = current_lines(project)?;
    staleness_among(project, &lines, scene_id, take)
}

fn staleness_among(
    project: &CanonicalProject,
    lines: &[ScreenplayLine],
    scene_id: &str,
    take: &LineTake,
) -> Result<Vec<String>, GenerationError> {
    let mut causes = Vec::new();
    match lines.iter().find(|line| {
        line.scene_id == scene_id && line.line == take.line && line.text_hash == take.text_hash
    }) {
        None => causes.push("line.missing".to_owned()),
        Some(line) => {
            if line.character.as_deref() != Some(&take.character) {
                causes.push("line.character".to_owned());
            }
            if line.parenthetical != take.parenthetical {
                causes.push("line.parenthetical".to_owned());
            }
        }
    }
    let current: Vec<_> = voice_document::locked(project, &take.character)
        .map_err(document_error)?
        .as_ref()
        .and_then(voice_document::lock_dependency)
        .into_iter()
        .collect();
    if !cast_document::stale_parents(&take.dependencies, &current).is_empty() {
        causes.push(format!("voice.{}", take.character));
    }
    Ok(causes)
}

/// Lock line `line` of `scene_id`, as the screenplay now has it, to its
/// selected take. Refused while the take is stale.
pub fn lock(
    project: &CanonicalProject,
    scene_id: &str,
    line: u32,
) -> Result<LockedLine, GenerationError> {
    let spoken = line_in(project, scene_id, line)?;
    let selected = document::selected(project, scene_id, line, &spoken.text_hash)
        .map_err(document_error)?
        .ok_or_else(|| document_error(DubbingError::NoSelection(line)))?;
    if !staleness(project, scene_id, &selected)?.is_empty() {
        return Err(document_error(DubbingError::StaleSelection(line)));
    }
    document::lock(project, scene_id, line, &spoken.text_hash).map_err(document_error)
}

/// A line and where its dubbing stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DubbedLine {
    pub line: ScreenplayLine,
    /// Every take of it, oldest first.
    pub takes: Vec<LineTake>,
    pub lock: Option<LockedLine>,
    /// Why the locked take no longer fits, when it does not.
    pub stale: Vec<String>,
}

/// Every line of the screenplay work builds on, with its takes and lock.
pub fn dubbed_lines(project: &CanonicalProject) -> Result<Vec<DubbedLine>, GenerationError> {
    let lines = current_lines(project)?;
    let mut scenes: Vec<(String, Vec<LineTake>, Vec<LockedLine>)> = Vec::new();
    let mut dubbed = Vec::with_capacity(lines.len());
    for line in &lines {
        if !scenes.iter().any(|(scene, ..)| *scene == line.scene_id) {
            scenes.push((
                line.scene_id.clone(),
                document::takes(project, &line.scene_id).map_err(document_error)?,
                document::locks(project, &line.scene_id).map_err(document_error)?,
            ));
        }
        let (_, takes, locks) = scenes
            .iter()
            .find(|(scene, ..)| *scene == line.scene_id)
            .expect("the scene was just read");
        let lock = locks
            .iter()
            .find(|lock| lock.line == line.line && lock.text_hash == line.text_hash)
            .cloned();
        let stale = match lock.as_ref().and_then(|lock| {
            takes
                .iter()
                .find(|take| take.artifact_id == lock.artifact_id)
        }) {
            Some(take) => staleness_among(project, &lines, &line.scene_id, take)?,
            None => Vec::new(),
        };
        dubbed.push(DubbedLine {
            line: line.clone(),
            takes: takes
                .iter()
                .filter(|take| take.line == line.line && take.text_hash == line.text_hash)
                .cloned()
                .collect(),
            lock,
            stale,
        });
    }
    Ok(dubbed)
}
