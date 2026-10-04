//! The bible, the style and the cast across the C ABI.
//!
//! Commands queue on the session worker like document commands and answer
//! with a `COMPLETED` message whose shot is -1. Reads return what the worker
//! last published, one field at a time: counts, numbers and UTF-8 text, each
//! selected by a code the contract table publishes, with `group` choosing the
//! bible entry or the character and `item` the element within it. No record
//! crosses as JSON; an edited entry arrives as typed text fields.

use std::slice;

use kineto_core::{
    session::{
        DocumentCommand, ProductionCommand, PublishedCharacter, PublishedDocuments,
        PublishedProduction, SubmitError,
    },
    style_prompt::PRESETS,
};
use kineto_project::{
    bible::{EntryEdit, EntryKind, StaleCause},
    cast::{CastImageKind, PortraitStatus},
    revisions::{Origin, RevisionStatus},
    style::SampleStatus,
};
use kineto_providers::Price;

use crate::{
    contract::*,
    documents::{at, optional_text, published, revision_bits, session_ref, submit},
    guarded,
    project_session::{KinetoProjectSession, copy_utf8, utf8_owned},
};

/// The most forbidden changes one edited entry may list.
const MAX_LIST: u64 = 1_000;

/// The most kinds one pack request may ask to make again.
const MAX_REMAKE: u64 = 64;

/// One UTF-8 string, as Dart lays it out.
#[repr(C)]
pub struct KinetoText {
    pub ptr: *const u8,
    pub len: u64,
}

/// # Safety
/// `session` must be null or a live project-session handle.
pub(crate) unsafe fn submit_production(
    session: *const KinetoProjectSession,
    command: ProductionCommand,
    resubmit: bool,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    let Some(session) = (unsafe { session_ref(session) }) else {
        return PROJECT_ERR_INVALID_ARGUMENT;
    };
    let submitted = if resubmit {
        session.resubmit_production(command, token)
    } else {
        session.submit_production(command, token)
    };
    match submitted {
        Ok(()) => PROJECT_OK,
        Err(SubmitError::Closed | SubmitError::InvalidShot) => PROJECT_ERR_NOT_OPEN,
    }
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

/// Queue writing a bible of the locked screenplay with the session's
/// language model; `resubmit` as for `kineto_project_analyze_story`.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_write_bible(
    session: *const KinetoProjectSession,
    resubmit: bool,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(session, DocumentCommand::WriteBible, resubmit, token)
    })
}

/// Queue saving the director's edit of one entry of bible revision
/// `based_on`, which must be the latest. `kind` is a contract `entry_kind`
/// code; `wardrobe` is ignored for an entry that is not a character. The
/// entry keeps its ID, name and everything else.
///
/// # Safety
/// `session` must be a live project-session handle. Each non-empty text input
/// must be readable for its length and hold UTF-8. When `forbidden_count` is
/// non-zero, `forbidden` must be readable for that many texts, each readable
/// for its length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_save_bible_entry(
    session: *const KinetoProjectSession,
    based_on: u32,
    kind: u32,
    id_ptr: *const u8,
    id_len: u64,
    description_ptr: *const u8,
    description_len: u64,
    appearance_ptr: *const u8,
    appearance_len: u64,
    wardrobe_ptr: *const u8,
    wardrobe_len: u64,
    forbidden: *const KinetoText,
    forbidden_count: u64,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let kind = match name(&ENTRY_KINDS, kind) {
            Some("character") => EntryKind::Character,
            Some("location") => EntryKind::Location,
            Some("prop") => EntryKind::Prop,
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        let read = |ptr, len| {
            // SAFETY: forwarded caller contract.
            unsafe { utf8_owned(ptr, len) }
        };
        let fields = (|| {
            Ok::<_, i32>((
                // SAFETY: forwarded caller contract.
                unsafe { required(id_ptr, id_len) }?,
                read(description_ptr, description_len)?,
                read(appearance_ptr, appearance_len)?,
                read(wardrobe_ptr, wardrobe_len)?,
            ))
        })();
        let (id, description, appearance, wardrobe) = match fields {
            Ok(fields) => fields,
            Err(code) => return code,
        };
        if forbidden_count > MAX_LIST || (forbidden_count > 0 && forbidden.is_null()) {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        let raw = if forbidden_count == 0 {
            &[][..]
        } else {
            // SAFETY: the caller guarantees `forbidden_count` readable texts.
            unsafe { slice::from_raw_parts(forbidden, forbidden_count as usize) }
        };
        let mut forbidden_changes = Vec::with_capacity(raw.len());
        for text in raw {
            match read(text.ptr, text.len) {
                Ok(text) => forbidden_changes.push(text),
                Err(code) => return code,
            }
        }
        let command = DocumentCommand::SaveBibleEntry {
            based_on,
            edit: EntryEdit {
                kind,
                id,
                description,
                appearance,
                wardrobe,
                forbidden_changes,
            },
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit(session, command, false, token) }
    })
}

/// Queue locking bible revision `revision` as the one casting and keyframes
/// follow.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_lock_bible(
    session: *const KinetoProjectSession,
    revision: u32,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(
            session,
            DocumentCommand::LockBible { revision },
            false,
            token,
        )
    })
}

/// Queue one paid sample of style preset `preset`, with `note` (which may be
/// empty). While a style sample may have been charged without its result,
/// it completes with `paid_call_unresolved` unless `resubmit` is the
/// director's decision to risk paying twice; the same holds for the other
/// paid image commands, each for its own images.
///
/// # Safety
/// `session` must be a live project-session handle. Each non-empty input must
/// be readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_sample_style(
    session: *const KinetoProjectSession,
    preset_ptr: *const u8,
    preset_len: u64,
    note_ptr: *const u8,
    note_len: u64,
    resubmit: bool,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let (preset, note) = match unsafe {
            (
                required(preset_ptr, preset_len),
                optional_text(note_ptr, note_len),
            )
        } {
            (Ok(preset), Ok(note)) => (preset, note),
            (Err(code), _) | (_, Err(code)) => return code,
        };
        let command = ProductionCommand::SampleStyle { preset, note };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, resubmit, token) }
    })
}

/// Queue one paid sample of sample `sample`'s preset, with `note`.
///
/// # Safety
/// As [`kineto_project_sample_style`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_refine_style(
    session: *const KinetoProjectSession,
    sample_ptr: *const u8,
    sample_len: u64,
    note_ptr: *const u8,
    note_len: u64,
    resubmit: bool,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let (sample, note) = match unsafe {
            (
                required(sample_ptr, sample_len),
                required(note_ptr, note_len),
            )
        } {
            (Ok(sample), Ok(note)) => (sample, note),
            (Err(code), _) | (_, Err(code)) => return code,
        };
        let command = ProductionCommand::RefineStyle { sample, note };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, resubmit, token) }
    })
}

/// Queue the director's `decision` (a contract `decision` code: select,
/// reject or lock) about style sample `sample`.
///
/// # Safety
/// `session` must be a live project-session handle. A non-empty `sample` must
/// be readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_decide_style(
    session: *const KinetoProjectSession,
    decision: u32,
    sample_ptr: *const u8,
    sample_len: u64,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let sample = match unsafe { required(sample_ptr, sample_len) } {
            Ok(sample) => sample,
            Err(code) => return code,
        };
        let command = match name(&DECISIONS, decision) {
            Some("select") => ProductionCommand::SelectStyle(sample),
            Some("reject") => ProductionCommand::RejectStyle(sample),
            Some("lock") => ProductionCommand::LockStyle(sample),
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, false, token) }
    })
}

/// Queue having `count` paid portrait candidates of `character` that are not
/// rejected and follow its current bible entry and the locked style, making
/// only those missing.
///
/// # Safety
/// `session` must be a live project-session handle. A non-empty `character`
/// must be readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_audition(
    session: *const KinetoProjectSession,
    character_ptr: *const u8,
    character_len: u64,
    count: u32,
    resubmit: bool,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let character = match unsafe { required(character_ptr, character_len) } {
            Ok(character) => character,
            Err(code) => return code,
        };
        let command = ProductionCommand::Audition { character, count };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, resubmit, token) }
    })
}

/// Queue completing the paid reference pack of `character`'s selected
/// portrait, making the kinds in `remake` (contract `cast_image_kind` codes)
/// again although the pack holds them.
///
/// # Safety
/// `session` must be a live project-session handle. A non-empty `character`
/// must be readable for its length and hold UTF-8. When `remake_count` is
/// non-zero, `remake` must be readable for that many `u32`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_make_pack(
    session: *const KinetoProjectSession,
    character_ptr: *const u8,
    character_len: u64,
    remake: *const u32,
    remake_count: u64,
    resubmit: bool,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let character = match unsafe { required(character_ptr, character_len) } {
            Ok(character) => character,
            Err(code) => return code,
        };
        if remake_count > MAX_REMAKE || (remake_count > 0 && remake.is_null()) {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        let raw = if remake_count == 0 {
            &[][..]
        } else {
            // SAFETY: the caller guarantees `remake_count` readable codes.
            unsafe { slice::from_raw_parts(remake, remake_count as usize) }
        };
        let mut kinds = Vec::with_capacity(raw.len());
        for code in raw {
            match name(&CAST_IMAGE_KINDS, *code).and_then(cast_image_kind) {
                Some(kind) if kind != CastImageKind::Portrait => kinds.push(kind),
                _ => return PROJECT_ERR_INVALID_ARGUMENT,
            }
        }
        let command = ProductionCommand::MakePack {
            character,
            remake: kinds,
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, resubmit, token) }
    })
}

/// Queue the director's `decision` (select or reject) about `character`'s
/// portrait `portrait`.
///
/// # Safety
/// `session` must be a live project-session handle. Each non-empty input must
/// be readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_decide_portrait(
    session: *const KinetoProjectSession,
    decision: u32,
    character_ptr: *const u8,
    character_len: u64,
    portrait_ptr: *const u8,
    portrait_len: u64,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let (character, portrait) = match unsafe {
            (
                required(character_ptr, character_len),
                required(portrait_ptr, portrait_len),
            )
        } {
            (Ok(character), Ok(portrait)) => (character, portrait),
            (Err(code), _) | (_, Err(code)) => return code,
        };
        let command = match name(&DECISIONS, decision) {
            Some("select") => ProductionCommand::SelectPortrait {
                character,
                portrait,
            },
            Some("reject") => ProductionCommand::RejectPortrait {
                character,
                portrait,
            },
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, false, token) }
    })
}

/// Queue locking `character` as cast: its selected portrait and that
/// portrait's complete reference pack.
///
/// # Safety
/// `session` must be a live project-session handle. A non-empty `character`
/// must be readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_lock_character(
    session: *const KinetoProjectSession,
    character_ptr: *const u8,
    character_len: u64,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let character = match unsafe { required(character_ptr, character_len) } {
            Ok(character) => character,
            Err(code) => return code,
        };
        let command = ProductionCommand::LockCharacter { character };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, false, token) }
    })
}

/// Queue importing the image at `path`, which the director chose, as a
/// portrait candidate of `character`. Free; the image must be one the
/// session's image provider takes as an identity reference.
///
/// # Safety
/// `session` must be a live project-session handle. Each non-empty input must
/// be readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_import_portrait(
    session: *const KinetoProjectSession,
    character_ptr: *const u8,
    character_len: u64,
    path_ptr: *const u8,
    path_len: u64,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let (character, path) = match unsafe {
            (
                required(character_ptr, character_len),
                required(path_ptr, path_len),
            )
        } {
            (Ok(character), Ok(path)) => (character, std::path::PathBuf::from(path)),
            (Err(code), _) | (_, Err(code)) => return code,
        };
        if !path.is_absolute() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        let command = ProductionCommand::ImportPortrait { character, path };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, false, token) }
    })
}

fn cast_image_kind(name: &str) -> Option<CastImageKind> {
    std::iter::once(CastImageKind::Portrait)
        .chain(CastImageKind::PACK)
        .find(|kind| kind.as_str() == name)
}

/// What the worker last published: the documents (for the bible) and the
/// style and cast.
struct Published {
    documents: std::sync::Arc<PublishedDocuments>,
    production: std::sync::Arc<PublishedProduction>,
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
    })
}

fn encode_production(published: &Published) -> u64 {
    let documents = &published.documents;
    let production = &published.production;
    pack(&[
        (
            BIBLE_REVISION,
            revision_bits(documents.bible.as_ref().map(|(revision, _)| *revision)),
        ),
        (BIBLE_LOCKED_REVISION, revision_bits(documents.bible_locked)),
        (BIBLE_STALE, u64::from(!documents.bible_stale.is_empty())),
        (STYLE_LOCKED, u64::from(production.style.is_some())),
        (STYLE_UNRESOLVED, u64::from(production.style_unresolved)),
        (
            PRODUCTION_DAMAGED,
            u64::from(production.damaged.is_some() || documents.damaged.is_some()),
        ),
    ])
}

/// The bible, style and cast state as last published, packed as the
/// contract table's production fields describe. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_state` writable
/// storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_production_state(
    session: *const KinetoProjectSession,
    out_state: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_state.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        match unsafe { read(session) } {
            Ok(published) => {
                // SAFETY: checked non-null; the caller guarantees it is writable.
                unsafe { out_state.write(encode_production(&published)) };
                PROJECT_OK
            }
            Err(code) => code,
        }
    })
}

fn bible(published: &Published) -> Option<&kineto_project::bible::Bible> {
    published.documents.bible.as_ref().map(|(_, bible)| bible)
}

fn character(published: &Published, group: u32) -> Option<&PublishedCharacter> {
    at(&published.production.characters, group)
}

fn count(published: &Published, what: &str, group: u32) -> Option<usize> {
    let bible = bible(published);
    let characters = bible.map_or(&[][..], |bible| &bible.characters);
    let locations = bible.map_or(&[][..], |bible| &bible.locations);
    let props = bible.map_or(&[][..], |bible| &bible.props);
    Some(match what {
        "bible_revisions" => published.documents.bible_revisions.len(),
        "world_visual_rules" => bible.map_or(0, |bible| bible.world.visual_rules.len()),
        "world_continuity_rules" => bible.map_or(0, |bible| bible.world.continuity_rules.len()),
        "bible_characters" => characters.len(),
        "character_forbidden" => at(characters, group)?.forbidden_changes.len(),
        "character_notes" => at(characters, group)?.continuity_notes.len(),
        "character_wardrobe_changes" => at(characters, group)?.wardrobe_changes.len(),
        "bible_locations" => locations.len(),
        "location_forbidden" => at(locations, group)?.forbidden_changes.len(),
        "bible_props" => props.len(),
        "prop_forbidden" => at(props, group)?.forbidden_changes.len(),
        "bible_stale_causes" => published.documents.bible_stale.len(),
        "style_presets" => PRESETS.len(),
        "style_samples" => published.production.samples.len(),
        "cast_characters" => published.production.characters.len(),
        "cast_images" => character(published, group)?.images.len(),
        "cast_stale" => character(published, group)?.stale.len(),
        "cast_lock_references" => character(published, group)?
            .lock
            .as_ref()
            .map_or(0, |lock| lock.references.len()),
        _ => return None,
    })
}

/// How many of `what` (a contract `production_count` code) the published
/// bible, style and cast hold; `group` selects the entry or character for
/// per-group counts. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_count` writable
/// storage for one `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_production_count(
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
        let Some(found) = name(&PRODUCTION_COUNTS, what)
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

fn stale_cause(cause: &StaleCause) -> (&'static str, &str) {
    match cause {
        StaleCause::NoLockedScreenplay => ("no_locked_screenplay", ""),
        StaleCause::Material(id) => ("material", id),
        StaleCause::SceneChanged(id) => ("scene_changed", id),
        StaleCause::SceneRemoved(id) => ("scene_removed", id),
        StaleCause::SceneAdded(id) => ("scene_added", id),
        StaleCause::Reference(id) => ("reference", id),
    }
}

fn value(published: &Published, field: &str, group: u32, item: u32) -> Option<u64> {
    let revision = || at(&published.documents.bible_revisions, item);
    let image = || at(&character(published, group)?.images, item).map(|(image, _)| image);
    Some(u64::from(match field {
        "bible_revision_number" => revision()?.revision,
        "bible_revision_status" => code(
            &REVISION_STATUSES,
            match revision()?.status {
                RevisionStatus::Candidate => "candidate",
                RevisionStatus::Locked => "locked",
                RevisionStatus::Superseded => "superseded",
            },
        ),
        "bible_revision_origin" => code(
            &ORIGINS,
            match revision()?.origin {
                Origin::Generated => "generated",
                Origin::Edited => "edited",
            },
        ),
        "bible_revision_based_on" => revision()?.based_on.unwrap_or(0),
        "bible_stale_cause" => code(
            &STALE_CAUSES,
            stale_cause(at(&published.documents.bible_stale, item)?).0,
        ),
        "sample_status" => code(
            &CHOICE_STATUSES,
            match at(&published.production.samples, item)?.0.status {
                SampleStatus::Candidate => "candidate",
                SampleStatus::Selected => "selected",
                SampleStatus::Rejected => "rejected",
            },
        ),
        "cast_image_kind" => code(&CAST_IMAGE_KINDS, image()?.kind.as_str()),
        "cast_image_status" => match image()?.status {
            None => 0,
            Some(status) => code(
                &CHOICE_STATUSES,
                match status {
                    PortraitStatus::Candidate => "candidate",
                    PortraitStatus::Selected => "selected",
                    PortraitStatus::Rejected => "rejected",
                },
            ),
        },
        "cast_locked" => u32::from(character(published, group)?.lock.is_some()),
        "cast_unresolved" => u32::from(character(published, group)?.unresolved),
        _ => return None,
    }))
}

/// Number field `field` (a contract `production_value` code) of the
/// published bible, style and cast; `group` and `item` select what it
/// belongs to. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_value` writable
/// storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_production_value(
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
            name(&PRODUCTION_VALUES, field).and_then(|field| value(&published, field, group, item))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_value.write(found) };
        PROJECT_OK
    })
}

fn text(published: &Published, field: &str, group: u32, item: u32) -> Option<String> {
    let bible = bible(published);
    let world = || bible.map(|bible| &bible.world);
    let characters = bible.map_or(&[][..], |bible| &bible.characters);
    let locations = bible.map_or(&[][..], |bible| &bible.locations);
    let props = bible.map_or(&[][..], |bible| &bible.props);
    let entry = || at(characters, group);
    let location = || at(locations, group);
    let prop = || at(props, group);
    let revision = || at(&published.documents.bible_revisions, item);
    let preset = || at(&PRESETS, item);
    let sample = || at(&published.production.samples, item);
    let style = || published.production.style.as_ref();
    let cast = || character(published, group);
    let image = || at(&cast()?.images, item);
    let owned = |text: &str| Some(text.to_owned());
    match field {
        "world_setting" => owned(world().map_or("", |world| &world.setting)),
        "world_period" => owned(world().map_or("", |world| &world.period)),
        "world_visual_rule" => owned(at(&world()?.visual_rules, item)?),
        "world_continuity_rule" => owned(at(&world()?.continuity_rules, item)?),
        "character_id" => owned(&entry()?.id),
        "character_name" => owned(&entry()?.name),
        "character_description" => owned(&entry()?.description),
        "character_appearance" => owned(&entry()?.appearance),
        "character_wardrobe" => owned(&entry()?.wardrobe),
        "character_forbidden" => owned(at(&entry()?.forbidden_changes, item)?),
        "character_note" => owned(at(&entry()?.continuity_notes, item)?),
        "character_wardrobe_change_scenes" => {
            Some(at(&entry()?.wardrobe_changes, item)?.scenes.join(", "))
        }
        "character_wardrobe_change" => owned(&at(&entry()?.wardrobe_changes, item)?.wardrobe),
        "location_id" => owned(&location()?.id),
        "location_name" => owned(&location()?.name),
        "location_description" => owned(&location()?.description),
        "location_appearance" => owned(&location()?.appearance),
        "location_forbidden" => owned(at(&location()?.forbidden_changes, item)?),
        "prop_id" => owned(&prop()?.id),
        "prop_name" => owned(&prop()?.name),
        "prop_description" => owned(&prop()?.description),
        "prop_appearance" => owned(&prop()?.appearance),
        "prop_forbidden" => owned(at(&prop()?.forbidden_changes, item)?),
        "bible_revision_provider" => owned(revision()?.provider.as_deref().unwrap_or_default()),
        "bible_revision_model" => owned(revision()?.model.as_deref().unwrap_or_default()),
        "bible_stale_subject" => owned(stale_cause(at(&published.documents.bible_stale, item)?).1),
        "preset_id" => owned(preset()?.id),
        "preset_name" => owned(preset()?.name),
        "sample_id" => owned(&sample()?.0.artifact_id),
        "sample_preset" => owned(&sample()?.0.preset),
        "sample_note" => owned(sample()?.0.note.as_deref().unwrap_or_default()),
        "sample_refines" => owned(sample()?.0.refines.as_deref().unwrap_or_default()),
        "sample_scene" => owned(&sample()?.0.scene_id),
        "sample_media" => Some(sample()?.1.to_string_lossy().into_owned()),
        "sample_provider" => owned(&sample()?.0.provider),
        "sample_model" => owned(&sample()?.0.model),
        "style_lock_sample" => owned(style().map_or("", |style| &style.artifact_id)),
        "style_lock_preset" => owned(style().map_or("", |style| &style.preset)),
        "style_lock_note" => owned(
            style()
                .and_then(|style| style.note.as_deref())
                .unwrap_or_default(),
        ),
        "cast_character_id" => owned(&cast()?.id),
        "cast_character_name" => owned(&cast()?.name),
        "cast_image_id" => owned(&image()?.0.artifact_id),
        "cast_image_media" => Some(image()?.1.to_string_lossy().into_owned()),
        "cast_image_identity_reference" => {
            owned(image()?.0.identity_reference.as_deref().unwrap_or_default())
        }
        "cast_lock_portrait" => owned(
            cast()?
                .lock
                .as_ref()
                .map_or("", |lock| &lock.portrait.artifact_id),
        ),
        "cast_lock_reference" => owned(&at(&cast()?.lock.as_ref()?.references, item)?.artifact_id),
        "cast_stale" => owned(at(&cast()?.stale, item)?),
        _ => None,
    }
}

/// Copy text field `field` (a contract `production_text` code) of the
/// published bible, style and cast into caller-owned memory, with the sizing
/// contract of project metadata. An absent optional field copies as empty;
/// media fields are absolute paths. Never does I/O.
///
/// # Safety
/// `session` must be live. `out_len` must point to writable `u64` storage. If
/// `out_cap` is non-zero, `out_buf` must be writable for at least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_production_text_copy(
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
            name(&PRODUCTION_TEXTS, field).and_then(|field| text(&published, field, group, item))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(found.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Estimated cost of `what` (a contract `image_cost` code) in USD micros,
/// with how many images it takes. Returns `cost_unknown` when the session's
/// image provider cannot make it or does not say what it costs in USD.
///
/// # Safety
/// `session` must be a live project-session handle; `out_micros` and
/// `out_images` writable storage for one `u64` and one `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_image_cost(
    session: *const KinetoProjectSession,
    what: u32,
    out_micros: *mut u64,
    out_images: *mut u32,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_micros.is_null() || out_images.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let Some(session) = (unsafe { session_ref(session) }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let prices = session.image_prices();
        let priced: Option<(usize, Price)> = match name(&IMAGE_COSTS, what) {
            Some("sample") => prices.sample.map(|price| (1, price)),
            Some("portrait") => prices.portrait.map(|price| (1, price)),
            Some("pack") => prices.pack,
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        match priced {
            Some((images, price)) if price.currency == *b"USD" => {
                // SAFETY: checked non-null; the caller guarantees both are writable.
                unsafe {
                    out_micros.write(price.amount_micros);
                    out_images.write(u32::try_from(images).unwrap_or(u32::MAX));
                }
                PROJECT_OK
            }
            _ => PROJECT_ERR_COST_UNKNOWN,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pack_kind_and_the_portrait_cross_by_name() {
        for (name, _) in CAST_IMAGE_KINDS {
            assert_eq!(cast_image_kind(name).map(CastImageKind::as_str), Some(name));
        }
        assert_eq!(CAST_IMAGE_KINDS.len(), CastImageKind::PACK.len() + 1);
    }

    #[test]
    fn every_stale_cause_has_a_code() {
        for cause in [
            StaleCause::NoLockedScreenplay,
            StaleCause::Material("source_r0001".to_owned()),
            StaleCause::SceneChanged("a".to_owned()),
            StaleCause::SceneRemoved("a".to_owned()),
            StaleCause::SceneAdded("a".to_owned()),
            StaleCause::Reference("reference.0001".to_owned()),
        ] {
            assert!(name(&STALE_CAUSES, code(&STALE_CAUSES, stale_cause(&cause).0)).is_some());
        }
    }
}
