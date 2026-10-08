//! Documents across the C ABI: the source, the story analysis and the
//! screenplay.
//!
//! Commands queue on the session worker like shot commands and answer with a
//! `COMPLETED` message whose shot is -1. Reads return what the worker last
//! published, one field at a time: counts, numbers and UTF-8 text, each
//! selected by a code the contract table publishes. No document crosses as
//! JSON; an edited scene arrives as a typed array of blocks.

use std::slice;

use kineto_core::{
    llm::DocumentModelKind,
    session::{DocumentCommand, ProjectSession, PublishedDocuments, SubmitError},
};
use kineto_llm::Billing;
use kineto_project::{
    revisions::{Origin, RevisionStatus},
    screenplay::{Block, BlockKind, SceneEdit, Screenplay, Setting},
    source::SourceFormat,
};

use crate::{
    contract::*,
    guarded,
    project_session::{KinetoProjectSession, copy_utf8, utf8_owned},
};

/// The most blocks one saved scene may have.
const MAX_BLOCKS: u64 = 10_000;

/// One block of an edited scene, as Dart lays it out. Empty `character` or
/// `parenthetical` means none.
#[repr(C)]
pub struct KinetoScreenplayBlock {
    pub kind: u32,
    pub character_ptr: *const u8,
    pub character_len: u64,
    pub parenthetical_ptr: *const u8,
    pub parenthetical_len: u64,
    pub text_ptr: *const u8,
    pub text_len: u64,
}

/// # Safety
/// `session` must be null or a live project-session handle.
pub(crate) unsafe fn session_ref<'a>(
    session: *const KinetoProjectSession,
) -> Option<&'a ProjectSession> {
    // SAFETY: forwarded caller contract.
    unsafe { session.as_ref() }.map(|session| &session.session)
}

/// # Safety
/// `session` must be null or a live project-session handle.
pub(crate) unsafe fn submit(
    session: *const KinetoProjectSession,
    command: DocumentCommand,
    resubmit: bool,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    let Some(session) = (unsafe { session_ref(session) }) else {
        return PROJECT_ERR_INVALID_ARGUMENT;
    };
    let submitted = if resubmit {
        session.resubmit_document(command, token)
    } else {
        session.submit_document(command, token)
    };
    match submitted {
        Ok(()) => PROJECT_OK,
        Err(SubmitError::Closed | SubmitError::InvalidShot) => PROJECT_ERR_NOT_OPEN,
    }
}

/// UTF-8 text, or `None` when empty.
///
/// # Safety
/// When `len` is non-zero, `ptr` must be readable for `len` bytes.
pub(crate) unsafe fn optional_text(ptr: *const u8, len: u64) -> Result<Option<String>, i32> {
    // SAFETY: forwarded caller contract.
    unsafe { utf8_owned(ptr, len) }.map(|text| Some(text).filter(|text| !text.is_empty()))
}

/// Queue importing `text` as the next source revision. `format` is a
/// contract `source_format` code; `name`, which may be empty, is the file it
/// came from.
///
/// # Safety
/// `session` must be a live project-session handle. Each non-empty input must
/// be readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_import_source(
    session: *const KinetoProjectSession,
    text_ptr: *const u8,
    text_len: u64,
    name_ptr: *const u8,
    name_len: u64,
    format: u32,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let format = match name(&SOURCE_FORMATS, format) {
            Some("text") => SourceFormat::Text,
            Some("markdown") => SourceFormat::Markdown,
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        // SAFETY: forwarded caller contract.
        let text = match unsafe { utf8_owned(text_ptr, text_len) } {
            Ok(text) => text,
            Err(code) => return code,
        };
        // SAFETY: forwarded caller contract.
        let original_name = match unsafe { optional_text(name_ptr, name_len) } {
            Ok(name) => name,
            Err(code) => return code,
        };
        let command = DocumentCommand::ImportSource {
            text,
            format,
            original_name,
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit(session, command, false, token) }
    })
}

/// Queue analysing the current source with the session's language model.
/// When the last paid request for the analysis may have been answered
/// without its answer being kept, it completes with `paid_call_unresolved`
/// unless `resubmit` is the director's decision to risk paying twice; the
/// same holds for the other model commands.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_analyze_story(
    session: *const KinetoProjectSession,
    resubmit: bool,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(session, DocumentCommand::AnalyzeStory, resubmit, token)
    })
}

/// Queue writing a screenplay of the current story analysis.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_write_screenplay(
    session: *const KinetoProjectSession,
    resubmit: bool,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(session, DocumentCommand::WriteScreenplay, resubmit, token)
    })
}

/// Queue writing one scene of the latest screenplay again, as `direction`
/// (which may be empty) asks.
///
/// # Safety
/// `session` must be a live project-session handle. Each non-empty input must
/// be readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_rewrite_scene(
    session: *const KinetoProjectSession,
    scene_id_ptr: *const u8,
    scene_id_len: u64,
    direction_ptr: *const u8,
    direction_len: u64,
    resubmit: bool,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let scene_id = match unsafe { utf8_owned(scene_id_ptr, scene_id_len) } {
            Ok(id) if !id.is_empty() => id,
            Ok(_) => return PROJECT_ERR_INVALID_ARGUMENT,
            Err(code) => return code,
        };
        // SAFETY: forwarded caller contract.
        let direction = match unsafe { optional_text(direction_ptr, direction_len) } {
            Ok(direction) => direction,
            Err(code) => return code,
        };
        let command = DocumentCommand::RewriteScene {
            scene_id,
            direction,
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit(session, command, resubmit, token) }
    })
}

/// Queue saving the director's edit of one scene of screenplay revision
/// `based_on`, which must be the latest. The scene keeps its ID, setting and
/// location; its heading, time, dramatic goal and blocks are replaced.
///
/// # Safety
/// `session` must be a live project-session handle. Each non-empty text input
/// must be readable for its length and hold UTF-8. When `block_count` is
/// non-zero, `blocks` must be readable for that many blocks, each with
/// readable text of its stated lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_save_scene(
    session: *const KinetoProjectSession,
    based_on: u32,
    scene_id_ptr: *const u8,
    scene_id_len: u64,
    heading_ptr: *const u8,
    heading_len: u64,
    time_ptr: *const u8,
    time_len: u64,
    goal_ptr: *const u8,
    goal_len: u64,
    blocks: *const KinetoScreenplayBlock,
    block_count: u64,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let read = |ptr, len| {
            // SAFETY: forwarded caller contract.
            unsafe { utf8_owned(ptr, len) }
        };
        let fields = (|| {
            Ok::<_, i32>((
                read(scene_id_ptr, scene_id_len)?,
                read(heading_ptr, heading_len)?,
                read(time_ptr, time_len)?,
                read(goal_ptr, goal_len)?,
            ))
        })();
        let (scene_id, heading, time, dramatic_goal) = match fields {
            Ok(fields) => fields,
            Err(code) => return code,
        };
        if block_count > MAX_BLOCKS || (block_count > 0 && blocks.is_null()) {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        let raw = if block_count == 0 {
            &[][..]
        } else {
            // SAFETY: the caller guarantees `block_count` readable blocks.
            unsafe { slice::from_raw_parts(blocks, block_count as usize) }
        };
        let mut parsed = Vec::with_capacity(raw.len());
        for block in raw {
            let kind = match name(&BLOCK_KINDS, block.kind) {
                Some("action") => BlockKind::Action,
                Some("dialogue") => BlockKind::Dialogue,
                Some("transition") => BlockKind::Transition,
                _ => return PROJECT_ERR_INVALID_ARGUMENT,
            };
            // SAFETY: forwarded caller contract for each block's text.
            let texts = unsafe {
                (
                    optional_text(block.character_ptr, block.character_len),
                    optional_text(block.parenthetical_ptr, block.parenthetical_len),
                    utf8_owned(block.text_ptr, block.text_len),
                )
            };
            let (Ok(character), Ok(parenthetical), Ok(text)) = texts else {
                return PROJECT_ERR_INVALID_UTF8;
            };
            parsed.push(Block {
                kind,
                character,
                parenthetical,
                text,
            });
        }
        let command = DocumentCommand::SaveScene {
            based_on,
            edit: SceneEdit {
                scene_id,
                heading,
                time,
                dramatic_goal,
                blocks: parsed,
            },
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit(session, command, false, token) }
    })
}

/// Queue locking screenplay revision `revision` as the one production
/// follows.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_lock_screenplay(
    session: *const KinetoProjectSession,
    revision: u32,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(
            session,
            DocumentCommand::LockScreenplay { revision },
            false,
            token,
        )
    })
}

/// # Safety
/// `session` must be null or a live project-session handle.
pub(crate) unsafe fn published(
    session: *const KinetoProjectSession,
) -> Result<std::sync::Arc<PublishedDocuments>, i32> {
    // SAFETY: forwarded caller contract.
    let session = unsafe { session_ref(session) }.ok_or(PROJECT_ERR_INVALID_ARGUMENT)?;
    session.documents().map_err(|_| PROJECT_ERR_NOT_OPEN)
}

/// A revision as its field carries it: 0 for none. Every revision a
/// project holds fits ([`kineto_project::revisions::MAX_REVISION`]); [`pack`]
/// fails on one that does not.
pub(crate) fn revision_bits(revision: Option<u32>) -> u64 {
    revision.map_or(0, u64::from)
}

fn encode_documents(documents: &PublishedDocuments) -> u64 {
    pack(&[
        (SOURCE_REVISION, revision_bits(documents.source_revision)),
        (
            ANALYSIS_REVISION,
            revision_bits(
                documents
                    .analysis
                    .as_ref()
                    .map(|analysis| analysis.revision),
            ),
        ),
        (
            SCREENPLAY_REVISION,
            revision_bits(documents.screenplay.as_ref().map(|(revision, _)| *revision)),
        ),
        (LOCKED_REVISION, revision_bits(documents.locked)),
        (ANALYSIS_STALE, u64::from(documents.analysis_stale)),
        (SCREENPLAY_STALE, u64::from(documents.screenplay_stale)),
        (DOCUMENTS_DAMAGED, u64::from(documents.damaged.is_some())),
    ])
}

/// The documents' state as last published, packed as the contract table's
/// document fields describe. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_state` writable
/// storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_documents_state(
    session: *const KinetoProjectSession,
    out_state: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_state.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        match unsafe { published(session) } {
            Ok(documents) => {
                // SAFETY: checked non-null; the caller guarantees it is writable.
                unsafe { out_state.write(encode_documents(&documents)) };
                PROJECT_OK
            }
            Err(code) => code,
        }
    })
}

fn screenplay(documents: &PublishedDocuments) -> Option<&Screenplay> {
    documents
        .screenplay
        .as_ref()
        .map(|(_, screenplay)| screenplay)
}

pub(crate) fn at<T>(items: &[T], index: u32) -> Option<&T> {
    items.get(usize::try_from(index).ok()?)
}

fn count(documents: &PublishedDocuments, what: &str, scene: u32) -> Option<usize> {
    let scenes = screenplay(documents).map_or(&[][..], |screenplay| &screenplay.scenes);
    Some(match what {
        "cast" => screenplay(documents).map_or(0, |screenplay| screenplay.cast.len()),
        "scenes" => scenes.len(),
        "scene_characters" => at(scenes, scene)?.characters.len(),
        "scene_blocks" => at(scenes, scene)?.blocks.len(),
        "revisions" => documents.revisions.len(),
        "analysis_characters" => documents
            .analysis
            .as_ref()
            .map_or(0, |analysis| analysis.analysis.characters.len()),
        _ => return None,
    })
}

/// How many of `what` (a contract `screenplay_count` code) the published
/// documents hold; `scene` selects the scene for per-scene counts. Never
/// does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_count` writable
/// storage for one `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_screenplay_count(
    session: *const KinetoProjectSession,
    what: u32,
    scene: u32,
    out_count: *mut u32,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_count.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let documents = match unsafe { published(session) } {
            Ok(documents) => documents,
            Err(code) => return code,
        };
        let Some(found) = name(&SCREENPLAY_COUNTS, what)
            .and_then(|what| count(&documents, what, scene))
            .and_then(|found| u32::try_from(found).ok())
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_count.write(found) };
        PROJECT_OK
    })
}

fn value(documents: &PublishedDocuments, field: &str, scene: u32, item: u32) -> Option<u64> {
    let scenes = screenplay(documents).map_or(&[][..], |screenplay| &screenplay.scenes);
    let revision = || at(&documents.revisions, item);
    Some(u64::from(match field {
        "scene_setting" => code(
            &SETTINGS,
            match at(scenes, scene)?.setting {
                Setting::Interior => "interior",
                Setting::Exterior => "exterior",
                Setting::InteriorExterior => "interior_exterior",
            },
        ),
        "block_kind" => code(
            &BLOCK_KINDS,
            match at(&at(scenes, scene)?.blocks, item)?.kind {
                BlockKind::Action => "action",
                BlockKind::Dialogue => "dialogue",
                BlockKind::Transition => "transition",
            },
        ),
        "revision_number" => revision()?.revision,
        "revision_status" => code(
            &REVISION_STATUSES,
            match revision()?.status {
                RevisionStatus::Candidate => "candidate",
                RevisionStatus::Locked => "locked",
                RevisionStatus::Superseded => "superseded",
            },
        ),
        "revision_origin" => code(
            &ORIGINS,
            match revision()?.origin {
                Origin::Generated => "generated",
                Origin::Edited => "edited",
            },
        ),
        "revision_based_on" => revision()?.based_on.unwrap_or(0),
        _ => return None,
    }))
}

/// Number field `field` (a contract `screenplay_value` code) of the
/// published documents; `scene` and `item` select what it belongs to.
/// Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_value` writable
/// storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_screenplay_value(
    session: *const KinetoProjectSession,
    field: u32,
    scene: u32,
    item: u32,
    out_value: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_value.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let documents = match unsafe { published(session) } {
            Ok(documents) => documents,
            Err(code) => return code,
        };
        let Some(found) =
            name(&SCREENPLAY_VALUES, field).and_then(|field| value(&documents, field, scene, item))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_value.write(found) };
        PROJECT_OK
    })
}

fn text<'a>(
    documents: &'a PublishedDocuments,
    field: &str,
    scene: u32,
    item: u32,
) -> Option<&'a str> {
    let screenplay = screenplay(documents);
    let scenes = screenplay.map_or(&[][..], |screenplay| &screenplay.scenes);
    let cast = screenplay.map_or(&[][..], |screenplay| &screenplay.cast);
    let block = || at(&at(scenes, scene)?.blocks, item);
    let revision = || at(&documents.revisions, item);
    Some(match field {
        "title" => screenplay.map_or("", |screenplay| &screenplay.title),
        "cast_id" => &at(cast, item)?.id,
        "cast_name" => &at(cast, item)?.name,
        "scene_id" => &at(scenes, scene)?.id,
        "scene_heading" => &at(scenes, scene)?.heading,
        "scene_location" => &at(scenes, scene)?.location,
        "scene_time" => &at(scenes, scene)?.time,
        "scene_goal" => &at(scenes, scene)?.dramatic_goal,
        "scene_character" => at(&at(scenes, scene)?.characters, item)?,
        "block_character" => block()?.character.as_deref().unwrap_or_default(),
        "block_parenthetical" => block()?.parenthetical.as_deref().unwrap_or_default(),
        "block_text" => &block()?.text,
        "revision_provider" => revision()?.provider.as_deref().unwrap_or_default(),
        "revision_model" => revision()?.model.as_deref().unwrap_or_default(),
        "analysis_logline" => documents
            .analysis
            .as_ref()
            .map_or("", |analysis| &analysis.analysis.logline),
        "analysis_character_name" => {
            &at(&documents.analysis.as_ref()?.analysis.characters, item)?.name
        }
        _ => return None,
    })
}

/// Copy text field `field` (a contract `screenplay_text` code) of the
/// published documents into caller-owned memory, with the sizing contract of
/// project metadata. An absent optional field copies as empty. Never does
/// I/O.
///
/// # Safety
/// `session` must be live. `out_len` must point to writable `u64` storage. If
/// `out_cap` is non-zero, `out_buf` must be writable for at least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_screenplay_text_copy(
    session: *const KinetoProjectSession,
    field: u32,
    scene: u32,
    item: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let documents = match unsafe { published(session) } {
            Ok(documents) => documents,
            Err(code) => return code,
        };
        let Some(found) =
            name(&SCREENPLAY_TEXTS, field).and_then(|field| text(&documents, field, scene, item))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(found.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// The session's language model as a contract `llm_provider` code; 0 (none)
/// for a null handle.
///
/// # Safety
/// `session` must be null or a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_llm_provider(session: *const KinetoProjectSession) -> u32 {
    guarded(0, || {
        // SAFETY: forwarded caller contract.
        let Some(model) =
            (unsafe { session_ref(session) }).and_then(|session| session.document_model())
        else {
            return 0;
        };
        code(
            &LLM_PROVIDERS,
            match model.kind {
                DocumentModelKind::None => "none",
                DocumentModelKind::KimiCode => "kimi_code",
                DocumentModelKind::Kimi => "kimi",
                DocumentModelKind::Local => "local",
            },
        )
    })
}

/// 1 when the session's language model can be asked for a document, 0 when
/// it cannot (none named, or one without its key or CLI).
///
/// # Safety
/// `session` must be null or a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_llm_ready(session: *const KinetoProjectSession) -> u8 {
    guarded(0, || {
        // SAFETY: forwarded caller contract.
        unsafe { session_ref(session) }
            .and_then(|session| session.document_model())
            .map_or(0, |model| u8::from(model.ready))
    })
}

/// Copy the session model's name, as far as kineto knows it, into
/// caller-owned memory; an agent reports `agent-default`.
///
/// # Safety
/// Same output-buffer contract as [`kineto_project_screenplay_text_copy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_llm_model_copy(
    session: *const KinetoProjectSession,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let Some(session) = (unsafe { session_ref(session) }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Some(model) = session.document_model() else {
            return PROJECT_ERR_NOT_OPEN;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(model.model.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Estimated cost of asking for `document` (a contract `document_cost` code)
/// in USD micros, counting the model's full output allowance. Returns
/// `cost_unknown` when the model's billing gives no price (an agent on a
/// subscription), `model_not_configured` without a usable model,
/// `needs_analysis` for a screenplay with nothing to write from, and
/// `needs_locked_screenplay` for a bible, and `needs_locked_bible` for a
/// shot plan as well.
///
/// # Safety
/// `session` must be a live project-session handle and `out_micros` writable
/// storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_document_cost(
    session: *const KinetoProjectSession,
    document: u32,
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
        let documents = match unsafe { published(session) } {
            Ok(documents) => documents,
            Err(code) => return code,
        };
        let Some(model) = handle.document_model() else {
            return PROJECT_ERR_NOT_OPEN;
        };
        let estimate = match name(&DOCUMENT_COSTS, document) {
            Some("analysis") => documents.analysis_cost,
            Some("screenplay") if documents.analysis.is_none() => {
                return PROJECT_ERR_NEEDS_ANALYSIS;
            }
            Some("screenplay") => documents.screenplay_cost,
            Some("bible") if documents.locked.is_none() => {
                return PROJECT_ERR_NEEDS_LOCKED_SCREENPLAY;
            }
            Some("bible") => documents.bible_cost,
            Some("shot_plan") if documents.locked.is_none() => {
                return PROJECT_ERR_NEEDS_LOCKED_SCREENPLAY;
            }
            Some("shot_plan") if documents.bible_locked.is_none() => {
                return PROJECT_ERR_NEEDS_LOCKED_BIBLE;
            }
            Some("shot_plan") => documents.shot_plan_cost,
            Some("preference_suggestions") => documents.suggestions_cost,
            Some("voice_recommendations") => documents.recommendations_cost,
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        if !model.ready {
            return PROJECT_ERR_MODEL_NOT_CONFIGURED;
        }
        match (estimate, model.billing) {
            (Some(micros), Billing::PerToken { .. } | Billing::Free) => {
                // SAFETY: checked non-null; the caller guarantees it is writable.
                unsafe { out_micros.write(micros) };
                PROJECT_OK
            }
            _ => PROJECT_ERR_COST_UNKNOWN,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The last revision a project holds is carried as it is; none is cut
    /// to fit.
    #[test]
    fn every_revision_a_project_holds_is_carried_exactly() {
        let last = kineto_project::revisions::MAX_REVISION;
        let documents = PublishedDocuments {
            source_revision: Some(1),
            locked: Some(last),
            analysis_stale: true,
            ..PublishedDocuments::default()
        };
        let bits = encode_documents(&documents);
        let read = |field: Field| (bits >> field.shift) & ((1 << field.width) - 1);
        assert_eq!(read(SOURCE_REVISION), 1);
        assert_eq!(read(ANALYSIS_REVISION), 0);
        assert_eq!(read(LOCKED_REVISION), u64::from(last));
        assert_eq!(read(ANALYSIS_STALE), 1);
        assert_eq!(read(SCREENPLAY_STALE), 0);
    }

    /// A revision past the last one breaks the project's own limit: the
    /// summary fails instead of handing Dart another number.
    #[test]
    #[should_panic(expected = "locked_revision does not fit its field")]
    fn a_revision_past_the_last_fails_the_summary() {
        encode_documents(&PublishedDocuments {
            locked: Some(kineto_project::revisions::MAX_REVISION + 1),
            ..PublishedDocuments::default()
        });
    }
}
