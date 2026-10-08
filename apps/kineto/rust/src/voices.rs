//! Voices, lines and preferences across the C ABI.
//!
//! Commands queue on the session worker like production and document
//! commands and answer with a `COMPLETED` message whose shot is -1. Reads
//! return what the worker last published, one field at a time: counts,
//! numbers and UTF-8 text, each selected by a code the contract table
//! publishes, with `group` choosing the character (in cast order), the line,
//! the preference or the suggestion, and `item` the element within it. No
//! record crosses as JSON; a voice to audition and a preference arrive as
//! typed fields.

use std::{slice, sync::Arc};

use kineto_core::{
    dialogue::DurationSource,
    dubbing::DubRequest,
    session::{
        DocumentCommand, ProductionCommand, PublishedCharacter, PublishedDocuments, PublishedLine,
        PublishedProduction,
    },
    speech::{SpeechProviderInfo, SpeechProviderKind, VoiceSettings},
    voice::VoiceChoice,
};
use kineto_project::{
    dubbing::TakeStatus,
    preferences::{Compared, Dimension, Preference, Preferences, Source, Stance},
    revisions::Origin,
    voice::AuditionStatus,
};
use kineto_providers::Emotion;

use crate::{
    contract::*,
    documents::{at, optional_text, published, session_ref, submit},
    guarded,
    production::{KinetoText, submit_production},
    project_session::{KinetoProjectSession, copy_utf8, utf8_owned},
};

/// The most voices one audition may ask for.
const MAX_CHOICES: u64 = 32;

/// The most entries one preference profile may hold.
const MAX_PREFERENCES: u64 = 1_000;

/// The most takes one dubbing request may ask for.
const MAX_TAKES: u32 = 8;

/// One voice to audition, as Dart lays it out.
#[repr(C)]
pub struct KinetoVoiceChoice {
    /// The provider's ID of the voice.
    pub voice: KinetoText,
    /// The voice's name as the provider lists it.
    pub name: KinetoText,
    /// A contract `emotion` code, or 0 for the voice's own delivery.
    pub emotion: u32,
    /// In thousandths of the normal rate; 0 for normal.
    pub speed_permille: u32,
}

/// One entry of the director's preference profile, as Dart lays it out.
#[repr(C)]
pub struct KinetoPreference {
    /// A contract `dimension` code.
    pub dimension: u32,
    /// A contract `stance` code.
    pub stance: u32,
    pub value: KinetoText,
    /// How strongly, in thousandths; 0 when the director does not say.
    pub strength_permille: u32,
    /// The entry of the profile in force, from 1, whose source it keeps,
    /// such as the suggestion it was accepted from; 0 for the director's
    /// own.
    pub kept_from: u32,
}

/// UTF-8 text that must not be empty.
///
/// # Safety
/// When `len` is non-zero, `ptr` must be readable for `len` bytes.
unsafe fn required(ptr: *const u8, len: u64) -> Result<String, i32> {
    // SAFETY: forwarded caller contract.
    match unsafe { utf8_owned(ptr, len) } {
        Ok(text) if !text.is_empty() => Ok(text),
        Ok(_) => Err(PROJECT_ERR_INVALID_ARGUMENT),
        Err(code) => Err(code),
    }
}

/// # Safety
/// When `count` is non-zero, `items` must be readable for `count` items.
unsafe fn items<'a, T>(items: *const T, count: u64, max: u64) -> Result<&'a [T], i32> {
    if count > max || (count > 0 && items.is_null()) {
        return Err(PROJECT_ERR_INVALID_ARGUMENT);
    }
    Ok(if count == 0 {
        &[]
    } else {
        // SAFETY: the caller guarantees `count` readable items.
        unsafe { slice::from_raw_parts(items, usize::try_from(count).unwrap_or(usize::MAX)) }
    })
}

/// The delivery a contract `emotion` code names; `Ok(None)` for 0.
fn emotion(code: u32) -> Result<Option<Emotion>, i32> {
    if code == 0 {
        return Ok(None);
    }
    name(&EMOTIONS, code)
        .and_then(Emotion::parse)
        .map(Some)
        .ok_or(PROJECT_ERR_INVALID_ARGUMENT)
}

/// A rate in thousandths of normal; `Ok(None)` for 0.
fn speed(permille: u32) -> Result<Option<u16>, i32> {
    if permille == 0 {
        return Ok(None);
    }
    u16::try_from(permille)
        .map(Some)
        .map_err(|_| PROJECT_ERR_INVALID_ARGUMENT)
}

/// Queue asking the session's speech provider which voices it offers. Free.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_load_voices(
    session: *const KinetoProjectSession,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit_production(session, ProductionCommand::LoadVoices, false, token)
    })
}

/// Queue having each of `choices` say `line` (empty for the character's
/// default line) as `character`, in the project's language. Paid; a voice
/// already heard saying it is not asked again. `resubmit` is the director's
/// explicit decision to ask again although an audition of this character
/// may have been charged without its audio.
///
/// # Safety
/// `session` must be a live project-session handle. Each non-empty text must
/// be readable for its length and hold UTF-8. When `choice_count` is
/// non-zero, `choices` must be readable for that many choices.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_audition_voices(
    session: *const KinetoProjectSession,
    character_ptr: *const u8,
    character_len: u64,
    choices: *const KinetoVoiceChoice,
    choice_count: u64,
    line_ptr: *const u8,
    line_len: u64,
    resubmit: bool,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let Some(handle) = (unsafe { session_ref(session) }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Some(language) = handle.metadata().map(|metadata| metadata.language.clone()) else {
            return PROJECT_ERR_NOT_OPEN;
        };
        // SAFETY: forwarded caller contract.
        let (character, line, raw) = match unsafe {
            (
                required(character_ptr, character_len),
                optional_text(line_ptr, line_len),
                items(choices, choice_count, MAX_CHOICES),
            )
        } {
            (Ok(character), Ok(line), Ok(raw)) if !raw.is_empty() => (character, line, raw),
            (Err(code), _, _) | (_, Err(code), _) | (_, _, Err(code)) => return code,
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        let mut picked = Vec::with_capacity(raw.len());
        for choice in raw {
            // SAFETY: forwarded caller contract for each choice.
            let (voice_id, name) = match unsafe {
                (
                    required(choice.voice.ptr, choice.voice.len),
                    required(choice.name.ptr, choice.name.len),
                )
            } {
                (Ok(voice_id), Ok(name)) => (voice_id, name),
                (Err(code), _) | (_, Err(code)) => return code,
            };
            let mut voice = VoiceSettings::new(voice_id, language.clone());
            match (emotion(choice.emotion), speed(choice.speed_permille)) {
                (Ok(emotion), Ok(speed)) => {
                    voice.emotion = emotion;
                    if let Some(speed) = speed {
                        voice.speed_permille = speed;
                    }
                }
                (Err(code), _) | (_, Err(code)) => return code,
            }
            picked.push(VoiceChoice { voice, name });
        }
        let command = ProductionCommand::AuditionVoices {
            character,
            choices: picked,
            line,
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, resubmit, token) }
    })
}

/// Queue the director's `decision` about `character`'s voice: select or
/// reject audition `audition`, or lock the selected one as the character's
/// voice (`audition` may then be empty).
///
/// # Safety
/// `session` must be a live project-session handle. Each non-empty input must
/// be readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_decide_voice(
    session: *const KinetoProjectSession,
    decision: u32,
    character_ptr: *const u8,
    character_len: u64,
    audition_ptr: *const u8,
    audition_len: u64,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let (character, audition) = match unsafe {
            (
                required(character_ptr, character_len),
                utf8_owned(audition_ptr, audition_len),
            )
        } {
            (Ok(character), Ok(audition)) => (character, audition),
            (Err(code), _) | (_, Err(code)) => return code,
        };
        let command = match (name(&DECISIONS, decision), audition.is_empty()) {
            (Some("select"), false) => ProductionCommand::SelectVoice {
                character,
                audition,
            },
            (Some("reject"), false) => ProductionCommand::RejectVoice {
                character,
                audition,
            },
            (Some("lock"), _) => ProductionCommand::LockVoice { character },
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, false, token) }
    })
}

/// Queue having `takes` takes of line `line` (from 1) of `scene` spoken in
/// its character's locked voice. Paid. `delivery` is a contract `emotion`
/// code to speak it with instead of the screenplay's (0 for the
/// screenplay's), `speed_permille` a rate (0 for the voice's), `note` the
/// director's note kept with the takes (may be empty). `resubmit` as for
/// `kineto_project_audition_voices`, for this line.
///
/// # Safety
/// `session` must be a live project-session handle. Each non-empty text must
/// be readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_dub_line(
    session: *const KinetoProjectSession,
    scene_ptr: *const u8,
    scene_len: u64,
    line: u32,
    takes: u32,
    delivery: u32,
    speed_permille: u32,
    note_ptr: *const u8,
    note_len: u64,
    resubmit: bool,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if line == 0 || takes == 0 || takes > MAX_TAKES {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let (scene, note) = match unsafe {
            (
                required(scene_ptr, scene_len),
                optional_text(note_ptr, note_len),
            )
        } {
            (Ok(scene), Ok(note)) => (scene, note),
            (Err(code), _) | (_, Err(code)) => return code,
        };
        let (delivery, speed_permille) = match (emotion(delivery), speed(speed_permille)) {
            (Ok(delivery), Ok(speed)) => (delivery, speed),
            (Err(code), _) | (_, Err(code)) => return code,
        };
        let command = ProductionCommand::DubLine {
            scene,
            line,
            request: DubRequest {
                takes: usize::try_from(takes).unwrap_or(usize::MAX),
                delivery,
                speed_permille,
                note,
            },
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, resubmit, token) }
    })
}

/// Queue the director's `decision` about a line of `scene`: select or
/// reject take `take`, or lock line `line` (from 1) to its selected take
/// (`take` may then be empty).
///
/// # Safety
/// `session` must be a live project-session handle. Each non-empty input must
/// be readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_decide_take(
    session: *const KinetoProjectSession,
    decision: u32,
    scene_ptr: *const u8,
    scene_len: u64,
    take_ptr: *const u8,
    take_len: u64,
    line: u32,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let (scene, take) = match unsafe {
            (
                required(scene_ptr, scene_len),
                utf8_owned(take_ptr, take_len),
            )
        } {
            (Ok(scene), Ok(take)) => (scene, take),
            (Err(code), _) | (_, Err(code)) => return code,
        };
        let command = match (name(&DECISIONS, decision), take.is_empty()) {
            (Some("select"), false) => ProductionCommand::SelectTake { scene, take },
            (Some("reject"), false) => ProductionCommand::RejectTake { scene, take },
            (Some("lock"), _) if line > 0 => ProductionCommand::LockLine { scene, line },
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, false, token) }
    })
}

/// Queue saving `entries` as the director's preference profile, the next
/// revision after `based_on` (0 for the first). Each entry keeps the source
/// of the entry of the profile in force it names, or is the director's own.
///
/// # Safety
/// `session` must be a live project-session handle. When `entry_count` is
/// non-zero, `entries` must be readable for that many entries, each with
/// readable UTF-8 text of its stated length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_save_preferences(
    session: *const KinetoProjectSession,
    based_on: u32,
    entries: *const KinetoPreference,
    entry_count: u64,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let raw = match unsafe { items(entries, entry_count, MAX_PREFERENCES) } {
            Ok(raw) => raw,
            Err(code) => return code,
        };
        // SAFETY: forwarded caller contract.
        let documents = match unsafe { published(session) } {
            Ok(documents) => documents,
            Err(code) => return code,
        };
        let in_force = documents
            .preferences
            .as_ref()
            .filter(|(record, _)| record.revision == based_on)
            .map(|(_, preferences)| preferences);
        let mut saved = Vec::with_capacity(raw.len());
        for entry in raw {
            // SAFETY: forwarded caller contract for each entry.
            let value = match unsafe { required(entry.value.ptr, entry.value.len) } {
                Ok(value) => value,
                Err(code) => return code,
            };
            let (Some(dimension), Some(stance)) = (
                name(&DIMENSIONS, entry.dimension).and_then(Dimension::parse),
                name(&STANCES, entry.stance).and_then(stance),
            ) else {
                return PROJECT_ERR_INVALID_ARGUMENT;
            };
            let source = match entry.kept_from {
                0 => Source::Director,
                kept => match in_force.and_then(|preferences| at(&preferences.entries, kept - 1)) {
                    Some(kept) => kept.source.clone(),
                    None => return PROJECT_ERR_INVALID_ARGUMENT,
                },
            };
            let strength_permille = match entry.strength_permille {
                0 => None,
                strength => match u16::try_from(strength) {
                    Ok(strength) => Some(strength),
                    Err(_) => return PROJECT_ERR_INVALID_ARGUMENT,
                },
            };
            saved.push(Preference {
                dimension,
                stance,
                value,
                strength_permille,
                source,
            });
        }
        let command = DocumentCommand::SavePreferences {
            based_on: (based_on > 0).then_some(based_on),
            preferences: Preferences { entries: saved },
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit(session, command, false, token) }
    })
}

fn stance(name: &str) -> Option<Stance> {
    match name {
        "prefer" => Some(Stance::Prefer),
        "avoid" => Some(Stance::Avoid),
        _ => None,
    }
}

/// Queue asking the session's language model what the director's choices
/// show; `resubmit` as for `kineto_project_analyze_story`.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_suggest_preferences(
    session: *const KinetoProjectSession,
    resubmit: bool,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(
            session,
            DocumentCommand::SuggestPreferences,
            resubmit,
            token,
        )
    })
}

/// Queue the director's `decision` (a contract `suggestion_decision` code)
/// about suggestion `index` of batch `batch`: accept it, as suggested or,
/// with a non-empty `value`, in the director's words; or reject it.
///
/// # Safety
/// `session` must be a live project-session handle. A non-empty `value` must
/// be readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_decide_suggestion(
    session: *const KinetoProjectSession,
    decision: u32,
    batch: u32,
    index: u32,
    value_ptr: *const u8,
    value_len: u64,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let value = match unsafe { optional_text(value_ptr, value_len) } {
            Ok(value) => value,
            Err(code) => return code,
        };
        let command = match (name(&SUGGESTION_DECISIONS, decision), value) {
            (Some("accept"), value) => DocumentCommand::AcceptSuggestion {
                batch,
                index,
                value,
            },
            (Some("reject"), None) => DocumentCommand::RejectSuggestion { batch, index },
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit(session, command, false, token) }
    })
}

/// Queue asking the session's language model which of the speech
/// provider's voices fit the characters who speak; `resubmit` as for
/// `kineto_project_analyze_story`.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_recommend_voices(
    session: *const KinetoProjectSession,
    resubmit: bool,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(session, DocumentCommand::RecommendVoices, resubmit, token)
    })
}

/// What saying `text` `count` times costs with the session's speech
/// provider, in USD micros. Returns `cost_unknown` when the provider does
/// not say.
///
/// # Safety
/// `session` must be a live project-session handle; a non-empty `text`
/// readable for its length and UTF-8; `out_micros` writable storage for one
/// `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_speech_cost(
    session: *const KinetoProjectSession,
    text_ptr: *const u8,
    text_len: u64,
    count: u32,
    out_micros: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_micros.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let Some(handle) = (unsafe { session_ref(session) }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller contract.
        let text = match unsafe { utf8_owned(text_ptr, text_len) } {
            Ok(text) => text,
            Err(code) => return code,
        };
        let Some(per_character) = handle.speech_provider().character_cost_micros else {
            return PROJECT_ERR_COST_UNKNOWN;
        };
        let characters = u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
        let micros = per_character
            .saturating_mul(characters)
            .saturating_mul(u64::from(count));
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_micros.write(micros) };
        PROJECT_OK
    })
}

/// What the worker last published, and the session's speech provider.
struct Published {
    documents: Arc<PublishedDocuments>,
    production: Arc<PublishedProduction>,
    speech: SpeechProviderInfo,
}

/// # Safety
/// `session` must be null or a live project-session handle.
unsafe fn read(session: *const KinetoProjectSession) -> Result<Published, i32> {
    // SAFETY: forwarded caller contract.
    let documents = unsafe { published(session) }?;
    // SAFETY: forwarded caller contract.
    let handle = unsafe { session_ref(session) }.ok_or(PROJECT_ERR_INVALID_ARGUMENT)?;
    let production = handle.production().map_err(|_| PROJECT_ERR_NOT_OPEN)?;
    Ok(Published {
        documents,
        production,
        speech: handle.speech_provider(),
    })
}

fn character(published: &Published, group: u32) -> Option<&PublishedCharacter> {
    at(&published.production.characters, group)
}

fn line(published: &Published, group: u32) -> Option<&PublishedLine> {
    at(&published.production.lines, group)
}

fn preferences(published: &Published) -> &[Preference] {
    published
        .documents
        .preferences
        .as_ref()
        .map_or(&[][..], |(_, preferences)| &preferences.entries)
}

fn count(published: &Published, what: &str, group: u32) -> Option<usize> {
    let voice = || character(published, group).map(|character| &character.voice);
    let documents = &published.documents;
    Some(match what {
        "voices" => published.production.voices.len(),
        "voice_auditions" => voice()?.auditions.len(),
        "voice_stale" => voice()?.stale.len(),
        "voice_recommended" => voice()?.recommended.len(),
        "lines" => published.production.lines.len(),
        "line_takes" => line(published, group)?.dubbed.takes.len(),
        "line_stale" => line(published, group)?.dubbed.stale.len(),
        "preferences" => preferences(published).len(),
        "preference_evidence" => match &at(preferences(published), group)?.source {
            Source::Director => 0,
            Source::Suggestion { evidence, .. } => evidence.len(),
        },
        "preference_revisions" => documents.preferences_revisions.len(),
        "suggestions" => documents.open_suggestions.len(),
        "suggestion_evidence" => at(&documents.open_suggestions, group)?
            .suggestion
            .evidence
            .len(),
        _ => return None,
    })
}

/// How many of `what` (a contract `voice_count` code) the published voices,
/// lines and preferences hold; `group` selects the character, line,
/// preference or suggestion for per-group counts. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_count` writable
/// storage for one `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_voice_count(
    session: *const KinetoProjectSession,
    what: u32,
    group: u32,
    out_count: *mut u32,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_count.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let published = match unsafe { read(session) } {
            Ok(published) => published,
            Err(code) => return code,
        };
        let Some(found) = name(&VOICE_COUNTS, what)
            .and_then(|what| count(&published, what, group))
            .and_then(|found| u32::try_from(found).ok())
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_count.write(found) };
        PROJECT_OK
    })
}

fn choice_status(status: &str) -> u64 {
    u64::from(code(&CHOICE_STATUSES, status))
}

fn audition_status(status: AuditionStatus) -> &'static str {
    match status {
        AuditionStatus::Candidate => "candidate",
        AuditionStatus::Selected => "selected",
        AuditionStatus::Rejected => "rejected",
    }
}

fn take_status(status: TakeStatus) -> &'static str {
    match status {
        TakeStatus::Candidate => "candidate",
        TakeStatus::Selected => "selected",
        TakeStatus::Rejected => "rejected",
    }
}

fn stance_name(stance: Stance) -> &'static str {
    match stance {
        Stance::Prefer => "prefer",
        Stance::Avoid => "avoid",
    }
}

/// How long `line` takes to say, and where that comes from.
fn duration(published: &Published, line: &PublishedLine) -> Option<(u32, DurationSource)> {
    let said = &line.dubbed.line;
    published
        .documents
        .line_durations
        .iter()
        .find(|duration| duration.scene_id == said.scene_id && duration.line == said.line)
        .map(|duration| (duration.duration_ms, duration.source))
}

fn value(published: &Published, field: &str, group: u32, item: u32) -> Option<u64> {
    let documents = &published.documents;
    let voice = || character(published, group).map(|character| &character.voice);
    let audition = || at(&voice()?.auditions, item).map(|(audition, _)| audition);
    let line = || line(published, group);
    let take = || at(&line()?.dubbed.takes, item);
    let preference = || at(preferences(published), group);
    let revision = || at(&documents.preferences_revisions, item);
    let suggestion = || at(&documents.open_suggestions, group);
    Some(match field {
        "voice_locked" => u64::from(voice()?.lock.is_some()),
        "voice_unresolved" => u64::from(voice()?.unresolved),
        "audition_status" => choice_status(audition_status(audition()?.status)),
        "audition_duration_millis" => u64::from(audition()?.duration_ms),
        "audition_speed_permille" => u64::from(audition()?.voice.speed_permille),
        "line_number" => u64::from(line()?.dubbed.line.line),
        "line_locked" => u64::from(line()?.dubbed.lock.is_some()),
        "line_unresolved" => u64::from(line()?.unresolved),
        "line_take_cost_known" => u64::from(line()?.take_cost_micros.is_some()),
        "line_take_cost_micros" => line()?.take_cost_micros.unwrap_or(0),
        "line_duration_millis" => duration(published, line()?).map_or(0, |(ms, _)| u64::from(ms)),
        // A line not yet timed reads as estimated, at 0.
        "line_duration_source" => match duration(published, line()?)
            .map_or(DurationSource::Estimated, |(_, source)| source)
        {
            DurationSource::Estimated => u64::from(code(&DURATION_SOURCES, "estimated")),
            DurationSource::Measured => u64::from(code(&DURATION_SOURCES, "measured")),
        },
        "take_status" => choice_status(take_status(take()?.status)),
        "take_duration_millis" => u64::from(take()?.duration_ms),
        "take_speed_permille" => u64::from(take()?.voice.speed_permille),
        "preference_dimension" => u64::from(code(&DIMENSIONS, preference()?.dimension.as_str())),
        "preference_stance" => u64::from(code(&STANCES, stance_name(preference()?.stance))),
        "preference_strength_permille" => u64::from(preference()?.strength_permille.unwrap_or(0)),
        "preference_from_suggestion" => {
            u64::from(matches!(preference()?.source, Source::Suggestion { .. }))
        }
        "preferences_revision" => documents
            .preferences
            .as_ref()
            .map_or(0, |(record, _)| u64::from(record.revision)),
        "preference_revision_number" => u64::from(revision()?.revision),
        "preference_revision_origin" => u64::from(code(
            &ORIGINS,
            match revision()?.origin {
                Origin::Generated => "generated",
                Origin::Edited => "edited",
            },
        )),
        "suggestion_batch" => u64::from(suggestion()?.batch),
        "suggestion_index" => u64::from(suggestion()?.index),
        "suggestion_dimension" => u64::from(code(
            &DIMENSIONS,
            suggestion()?.suggestion.dimension.as_str(),
        )),
        "suggestion_stance" => {
            u64::from(code(&STANCES, stance_name(suggestion()?.suggestion.stance)))
        }
        "suggestion_compared_images" => {
            u64::from(suggestion()?.compared == Compared::ImagesAndText)
        }
        "unconsidered_choices" => u64::try_from(documents.unconsidered_choices).ok()?,
        "suggestions_ready" => u64::from(documents.suggestions_ready),
        "plan_preferences_changed" => u64::from(documents.shot_plan_preferences_changed),
        "speech_provider" => u64::from(code(
            &SPEECH_PROVIDERS,
            match published.speech.kind {
                SpeechProviderKind::LocalDemo => "local_demo",
                SpeechProviderKind::Minimax => "minimax",
                SpeechProviderKind::Plugin => "plugin",
            },
        )),
        "speech_ready" => u64::from(published.speech.ready),
        "speech_character_cost_known" => {
            u64::from(published.speech.character_cost_micros.is_some())
        }
        "speech_character_cost_micros" => published.speech.character_cost_micros.unwrap_or(0),
        _ => return None,
    })
}

/// Number field `field` (a contract `voice_value` code) of the published
/// voices, lines and preferences; `group` and `item` select what it belongs
/// to. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_value` writable
/// storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_voice_value(
    session: *const KinetoProjectSession,
    field: u32,
    group: u32,
    item: u32,
    out_value: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_value.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let published = match unsafe { read(session) } {
            Ok(published) => published,
            Err(code) => return code,
        };
        let Some(found) =
            name(&VOICE_VALUES, field).and_then(|field| value(&published, field, group, item))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_value.write(found) };
        PROJECT_OK
    })
}

fn text(published: &Published, field: &str, group: u32, item: u32) -> Option<String> {
    let production = &published.production;
    let documents = &published.documents;
    let listed = || at(&production.voices, item);
    let voice = || character(published, group).map(|character| &character.voice);
    let audition = || at(&voice()?.auditions, item);
    let lock = || voice()?.lock.as_ref();
    let recommended = || at(&voice()?.recommended, item);
    let line = || line(published, group);
    let take = || at(&line()?.dubbed.takes, item);
    let preference = || at(preferences(published), group);
    let suggestion = || at(&documents.open_suggestions, group).map(|open| &open.suggestion);
    let owned = |text: &str| Some(text.to_owned());
    match field {
        "voice_id" => owned(&listed()?.id),
        "voice_name" => owned(&listed()?.name),
        "voice_description" => owned(&listed()?.description),
        "voices_problem" => owned(production.voices_problem.as_deref().unwrap_or_default()),
        "voice_default_line" => owned(&voice()?.default_line),
        "audition_id" => owned(&audition()?.0.artifact_id),
        "audition_voice_id" => owned(&audition()?.0.voice.voice_id),
        "audition_voice_name" => owned(&audition()?.0.voice_name),
        "audition_line" => owned(&audition()?.0.line),
        "audition_media" => Some(audition()?.1.to_string_lossy().into_owned()),
        "audition_emotion" => owned(audition()?.0.voice.emotion.as_deref().unwrap_or_default()),
        "voice_lock_audition" => owned(lock().map_or("", |lock| &lock.artifact_id)),
        "voice_lock_voice_id" => owned(lock().map_or("", |lock| &lock.voice.voice_id)),
        "voice_lock_voice_name" => owned(lock().map_or("", |lock| &lock.voice_name)),
        "voice_stale" => owned(at(&voice()?.stale, item)?),
        "recommended_voice_id" => owned(&recommended()?.voice_id),
        "recommended_voice_name" => owned(recommended()?.name.as_deref().unwrap_or_default()),
        "recommended_reason" => owned(&recommended()?.reason),
        "line_scene" => owned(&line()?.dubbed.line.scene_id),
        "line_character" => owned(line()?.dubbed.line.character.as_deref().unwrap_or_default()),
        "line_text" => owned(&line()?.dubbed.line.text),
        "line_parenthetical" => owned(
            line()?
                .dubbed
                .line
                .parenthetical
                .as_deref()
                .unwrap_or_default(),
        ),
        "take_id" => owned(&take()?.artifact_id),
        "take_media" => Some(at(&line()?.media, item)?.to_string_lossy().into_owned()),
        "take_note" => owned(take()?.note.as_deref().unwrap_or_default()),
        "take_emotion" => owned(take()?.voice.emotion.as_deref().unwrap_or_default()),
        "line_lock_take" => owned(
            line()?
                .dubbed
                .lock
                .as_ref()
                .map_or("", |lock| &lock.artifact_id),
        ),
        "line_stale" => owned(at(&line()?.dubbed.stale, item)?),
        "preference_value" => owned(&preference()?.value),
        "preference_reason" => match &preference()?.source {
            Source::Director => owned(""),
            Source::Suggestion { reason, .. } => owned(reason),
        },
        "preference_evidence" => match &preference()?.source {
            Source::Director => None,
            Source::Suggestion { evidence, .. } => owned(at(evidence, item)?),
        },
        "suggestion_value" => owned(&suggestion()?.value),
        "suggestion_reason" => owned(&suggestion()?.reason),
        "suggestion_evidence" => owned(at(&suggestion()?.evidence, item)?),
        _ => None,
    }
}

/// Copy text field `field` (a contract `voice_text` code) of the published
/// voices, lines and preferences into caller-owned memory, with the sizing
/// contract of project metadata. An absent optional field copies as empty;
/// media fields are absolute paths. Never does I/O.
///
/// # Safety
/// `session` must be live. `out_len` must point to writable `u64` storage. If
/// `out_cap` is non-zero, `out_buf` must be writable for at least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_voice_text_copy(
    session: *const KinetoProjectSession,
    field: u32,
    group: u32,
    item: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let published = match unsafe { read(session) } {
            Ok(published) => published,
            Err(code) => return code,
        };
        let Some(found) =
            name(&VOICE_TEXTS, field).and_then(|field| text(&published, field, group, item))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(found.as_bytes(), out_buf, out_cap, out_len) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_delivery_dimension_and_stance_crosses_by_name() {
        fn names(table: &[(&'static str, u32)]) -> Vec<&'static str> {
            table.iter().map(|(name, _)| *name).collect()
        }
        assert_eq!(names(&EMOTIONS), Emotion::ALL.map(Emotion::name).to_vec());
        assert_eq!(
            names(&DIMENSIONS),
            Dimension::ALL.map(Dimension::as_str).to_vec()
        );
        for (name, code) in EMOTIONS {
            assert_eq!(emotion(code), Ok(Emotion::parse(name)));
        }
        assert_eq!(emotion(0), Ok(None));
        assert_eq!(emotion(99), Err(PROJECT_ERR_INVALID_ARGUMENT));
        assert_eq!(
            [Stance::Prefer, Stance::Avoid].map(stance_name).to_vec(),
            names(&STANCES)
        );
        assert!(
            names(&STANCES)
                .into_iter()
                .all(|name| stance(name).is_some())
        );
        assert_eq!(speed(0), Ok(None));
        assert_eq!(speed(1_250), Ok(Some(1_250)));
        assert_eq!(speed(70_000), Err(PROJECT_ERR_INVALID_ARGUMENT));
    }
}
