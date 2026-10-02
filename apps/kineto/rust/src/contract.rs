//! The native contract Dart reads instead of copying.
//!
//! Project status codes, shot direction codes and the packed shot-snapshot
//! layout are defined here once. Dart loads them through `kineto_contract_*`
//! when first needed and decodes snapshots with the layout it read. Its tests
//! compare the named constants it keeps for control flow against this table,
//! so a change made on one side only fails a test instead of silently
//! mis-decoding. Symbol signatures remain written on both sides.

use std::ptr;

use crate::guarded;
use kineto_project::shot::{ShotApprovalSnapshot, ShotDirection, VideoState};

pub(crate) const PROJECT_OK: i32 = 0;
pub(crate) const PROJECT_ERR_INVALID_ARGUMENT: i32 = 100;
pub(crate) const PROJECT_ERR_INVALID_UTF8: i32 = 101;
pub(crate) const PROJECT_ERR_IO: i32 = 102;
pub(crate) const PROJECT_ERR_ALREADY_EXISTS: i32 = 103;
pub(crate) const PROJECT_ERR_INVALID_MANIFEST: i32 = 104;
pub(crate) const PROJECT_ERR_UNSUPPORTED_FORMAT: i32 = 105;
pub(crate) const PROJECT_ERR_BUFFER_TOO_SMALL: i32 = 106;
pub(crate) const PROJECT_ERR_READ_ONLY: i32 = 107;
pub(crate) const PROJECT_ERR_INVALID_SHOT: i32 = 108;
pub(crate) const PROJECT_ERR_NOT_GENERATED: i32 = 109;
pub(crate) const PROJECT_ERR_INVALID_CANDIDATE: i32 = 110;
pub(crate) const PROJECT_ERR_LOCKED: i32 = 111;
pub(crate) const PROJECT_ERR_ALREADY_LOCKED: i32 = 112;
pub(crate) const PROJECT_ERR_NO_SELECTION: i32 = 113;
pub(crate) const PROJECT_ERR_STALE_SELECTION: i32 = 114;
pub(crate) const PROJECT_ERR_INVALID_SHOT_STATE: i32 = 115;
pub(crate) const PROJECT_ERR_GENERATION_OVERFLOW: i32 = 116;
pub(crate) const PROJECT_ERR_BUSY: i32 = 117;
pub(crate) const PROJECT_ERR_MIGRATION_REQUIRED: i32 = 118;
pub(crate) const PROJECT_ERR_LOCK_UNAVAILABLE: i32 = 119;
pub(crate) const PROJECT_ERR_NOT_OPEN: i32 = 120;
pub(crate) const PROJECT_ERR_GENERATION_JOB: i32 = 121;
pub(crate) const PROJECT_ERR_PROVIDER_UNKNOWN: i32 = 122;
pub(crate) const PROJECT_ERR_PROVIDER_NOT_CONFIGURED: i32 = 123;
pub(crate) const PROJECT_ERR_SHOT_INTENT_MISSING: i32 = 124;
pub(crate) const PROJECT_ERR_PROVIDER_REFUSED: i32 = 125;
pub(crate) const PROJECT_ERR_PAID_CALL_UNRESOLVED: i32 = 126;
pub(crate) const PROJECT_ERR_VIDEO_IN_FLIGHT: i32 = 127;
pub(crate) const PROJECT_ERR_KEYFRAME_NOT_LOCKED: i32 = 128;
pub(crate) const PROJECT_ERR_VIDEO_LOCKED: i32 = 129;
pub(crate) const PROJECT_ERR_NO_VIDEO: i32 = 130;
pub(crate) const PROJECT_ERR_STALE_VIDEO: i32 = 131;
pub(crate) const PROJECT_ERR_NO_FIRST_FRAME: i32 = 132;
pub(crate) const PROJECT_ERR_NEEDS_ANALYSIS: i32 = 133;
pub(crate) const PROJECT_ERR_NEEDS_SCREENPLAY: i32 = 134;
pub(crate) const PROJECT_ERR_NO_SUCH_SCENE: i32 = 135;
pub(crate) const PROJECT_ERR_NOT_LATEST: i32 = 136;
pub(crate) const PROJECT_ERR_INVALID_DOCUMENT: i32 = 137;
pub(crate) const PROJECT_ERR_INVALID_REVISION: i32 = 138;
pub(crate) const PROJECT_ERR_INVALID_SOURCE: i32 = 139;
pub(crate) const PROJECT_ERR_MODEL_NOT_CONFIGURED: i32 = 140;
pub(crate) const PROJECT_ERR_MODEL_UNKNOWN: i32 = 141;
pub(crate) const PROJECT_ERR_DOCUMENT_DAMAGED: i32 = 142;
pub(crate) const PROJECT_ERR_COST_UNKNOWN: i32 = 143;
pub(crate) const PROJECT_ERR_PANIC: i32 = 199;

/// Every project status code, by the name Dart knows it by.
const STATUSES: [(&str, i32); 46] = [
    ("ok", PROJECT_OK),
    ("invalid_argument", PROJECT_ERR_INVALID_ARGUMENT),
    ("invalid_utf8", PROJECT_ERR_INVALID_UTF8),
    ("io", PROJECT_ERR_IO),
    ("already_exists", PROJECT_ERR_ALREADY_EXISTS),
    ("invalid_manifest", PROJECT_ERR_INVALID_MANIFEST),
    ("unsupported_format", PROJECT_ERR_UNSUPPORTED_FORMAT),
    ("buffer_too_small", PROJECT_ERR_BUFFER_TOO_SMALL),
    ("read_only", PROJECT_ERR_READ_ONLY),
    ("invalid_shot", PROJECT_ERR_INVALID_SHOT),
    ("not_generated", PROJECT_ERR_NOT_GENERATED),
    ("invalid_candidate", PROJECT_ERR_INVALID_CANDIDATE),
    ("locked", PROJECT_ERR_LOCKED),
    ("already_locked", PROJECT_ERR_ALREADY_LOCKED),
    ("no_selection", PROJECT_ERR_NO_SELECTION),
    ("stale_selection", PROJECT_ERR_STALE_SELECTION),
    ("invalid_shot_state", PROJECT_ERR_INVALID_SHOT_STATE),
    ("generation_overflow", PROJECT_ERR_GENERATION_OVERFLOW),
    ("busy", PROJECT_ERR_BUSY),
    ("migration_required", PROJECT_ERR_MIGRATION_REQUIRED),
    ("lock_unavailable", PROJECT_ERR_LOCK_UNAVAILABLE),
    ("not_open", PROJECT_ERR_NOT_OPEN),
    ("generation_job", PROJECT_ERR_GENERATION_JOB),
    ("provider_unknown", PROJECT_ERR_PROVIDER_UNKNOWN),
    (
        "provider_not_configured",
        PROJECT_ERR_PROVIDER_NOT_CONFIGURED,
    ),
    ("shot_intent_missing", PROJECT_ERR_SHOT_INTENT_MISSING),
    ("provider_refused", PROJECT_ERR_PROVIDER_REFUSED),
    ("paid_call_unresolved", PROJECT_ERR_PAID_CALL_UNRESOLVED),
    ("video_in_flight", PROJECT_ERR_VIDEO_IN_FLIGHT),
    ("keyframe_not_locked", PROJECT_ERR_KEYFRAME_NOT_LOCKED),
    ("video_locked", PROJECT_ERR_VIDEO_LOCKED),
    ("no_video", PROJECT_ERR_NO_VIDEO),
    ("stale_video", PROJECT_ERR_STALE_VIDEO),
    ("no_first_frame", PROJECT_ERR_NO_FIRST_FRAME),
    ("needs_analysis", PROJECT_ERR_NEEDS_ANALYSIS),
    ("needs_screenplay", PROJECT_ERR_NEEDS_SCREENPLAY),
    ("no_such_scene", PROJECT_ERR_NO_SUCH_SCENE),
    ("not_latest", PROJECT_ERR_NOT_LATEST),
    ("invalid_document", PROJECT_ERR_INVALID_DOCUMENT),
    ("invalid_revision", PROJECT_ERR_INVALID_REVISION),
    ("invalid_source", PROJECT_ERR_INVALID_SOURCE),
    ("model_not_configured", PROJECT_ERR_MODEL_NOT_CONFIGURED),
    ("model_unknown", PROJECT_ERR_MODEL_UNKNOWN),
    ("document_damaged", PROJECT_ERR_DOCUMENT_DAMAGED),
    ("cost_unknown", PROJECT_ERR_COST_UNKNOWN),
    ("panic", PROJECT_ERR_PANIC),
];

const DIRECTIONS: [(&str, ShotDirection); 4] = [
    ("reaction", ShotDirection::Reaction),
    ("spatial_clarity", ShotDirection::SpatialClarity),
    ("intimacy", ShotDirection::Intimacy),
    ("tension", ShotDirection::Tension),
];

pub(crate) const IMAGE_PROVIDER_LOCAL_DEMO: u32 = 1;
pub(crate) const IMAGE_PROVIDER_MINIMAX: u32 = 2;

/// Video state codes in the snapshot's `video_state` field.
const VIDEO_STATES: [(&str, u8); 6] = [
    ("none", 0),
    ("generating", 1),
    ("ready", 2),
    ("locked", 3),
    ("stale", 4),
    ("unresolved", 5),
];

/// Image provider codes returned by `kineto_project_image_provider`.
const IMAGE_PROVIDERS: [(&str, u32); 2] = [
    ("local_demo", IMAGE_PROVIDER_LOCAL_DEMO),
    ("minimax", IMAGE_PROVIDER_MINIMAX),
];

/// Language model codes returned by `kineto_project_llm_provider`.
pub(crate) const LLM_PROVIDERS: [(&str, u32); 4] =
    [("none", 0), ("kimi_code", 1), ("kimi", 2), ("local", 3)];

/// Source format codes `kineto_project_import_source` takes.
pub(crate) const SOURCE_FORMATS: [(&str, u32); 2] = [("text", 1), ("markdown", 2)];

/// Documents whose cost `kineto_project_document_cost` estimates.
pub(crate) const DOCUMENT_COSTS: [(&str, u32); 2] = [("analysis", 1), ("screenplay", 2)];

/// What `kineto_project_screenplay_count` counts.
pub(crate) const SCREENPLAY_COUNTS: [(&str, u32); 6] = [
    ("cast", 1),
    ("scenes", 2),
    ("scene_characters", 3),
    ("scene_blocks", 4),
    ("revisions", 5),
    ("analysis_characters", 6),
];

/// Number fields `kineto_project_screenplay_value` reads.
pub(crate) const SCREENPLAY_VALUES: [(&str, u32); 6] = [
    ("scene_setting", 1),
    ("block_kind", 2),
    ("revision_number", 3),
    ("revision_status", 4),
    ("revision_origin", 5),
    ("revision_based_on", 6),
];

/// Text fields `kineto_project_screenplay_text_copy` copies.
pub(crate) const SCREENPLAY_TEXTS: [(&str, u32); 16] = [
    ("title", 1),
    ("cast_id", 2),
    ("cast_name", 3),
    ("scene_id", 4),
    ("scene_heading", 5),
    ("scene_location", 6),
    ("scene_time", 7),
    ("scene_goal", 8),
    ("scene_character", 9),
    ("block_character", 10),
    ("block_parenthetical", 11),
    ("block_text", 12),
    ("revision_provider", 13),
    ("revision_model", 14),
    ("analysis_logline", 15),
    ("analysis_character_name", 16),
];

pub(crate) const BLOCK_KINDS: [(&str, u32); 3] =
    [("action", 1), ("dialogue", 2), ("transition", 3)];

pub(crate) const SETTINGS: [(&str, u32); 3] =
    [("interior", 1), ("exterior", 2), ("interior_exterior", 3)];

pub(crate) const REVISION_STATUSES: [(&str, u32); 3] =
    [("candidate", 1), ("locked", 2), ("superseded", 3)];

pub(crate) const ORIGINS: [(&str, u32); 2] = [("generated", 1), ("edited", 2)];

/// The code `table` gives `name`.
pub(crate) fn code(table: &[(&str, u32)], name: &str) -> u32 {
    table
        .iter()
        .find(|(entry, _)| *entry == name)
        .map(|(_, code)| *code)
        .unwrap_or_else(|| panic!("{name} is not in its contract table"))
}

/// The name `table` gives `code`, if any.
pub(crate) fn name(table: &[(&'static str, u32)], code: u32) -> Option<&'static str> {
    table
        .iter()
        .find(|(_, entry)| *entry == code)
        .map(|(name, _)| *name)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Field {
    pub(crate) name: &'static str,
    pub(crate) shift: u32,
    pub(crate) width: u32,
}

const GENERATED: Field = field("generated", 0, 1);
const LOCKED: Field = field("locked", 1, 1);
const STALE: Field = field("stale", 2, 1);
const MIGRATION_PENDING: Field = field("migration_pending", 3, 1);
const INVALID_HASHES: Field = field("invalid_hashes", 4, 1);
/// One of [`VIDEO_STATES`]; `generating` and then `unresolved` override the
/// canonical state.
const VIDEO_STATE: Field = field("video_state", 5, 3);
const CANDIDATE_COUNT: Field = field("candidate_count", 8, 8);
/// Selected candidate index + 1; 0 means no selection.
const SELECTED_PLUS_ONE: Field = field("selected_plus_one", 16, 8);
const DIRECTION: Field = field("direction", 24, 8);
const GENERATION_REVISION: Field = field("generation_revision", 32, 16);
const SUPERSEDED_COUNT: Field = field("superseded_count", 48, 16);

/// The packed `u64` shot snapshot. This is its only definition.
const SNAPSHOT_FIELDS: [Field; 11] = [
    GENERATED,
    LOCKED,
    STALE,
    MIGRATION_PENDING,
    INVALID_HASHES,
    VIDEO_STATE,
    CANDIDATE_COUNT,
    SELECTED_PLUS_ONE,
    DIRECTION,
    GENERATION_REVISION,
    SUPERSEDED_COUNT,
];

const fn field(name: &'static str, shift: u32, width: u32) -> Field {
    Field { name, shift, width }
}

pub(crate) const SOURCE_REVISION: Field = field("source_revision", 0, 14);
pub(crate) const ANALYSIS_REVISION: Field = field("analysis_revision", 14, 14);
/// The latest screenplay revision still in play.
pub(crate) const SCREENPLAY_REVISION: Field = field("screenplay_revision", 28, 14);
pub(crate) const LOCKED_REVISION: Field = field("locked_revision", 42, 14);
pub(crate) const ANALYSIS_STALE: Field = field("analysis_stale", 56, 1);
pub(crate) const SCREENPLAY_STALE: Field = field("screenplay_stale", 57, 1);
pub(crate) const DOCUMENTS_DAMAGED: Field = field("documents_damaged", 58, 1);

/// The packed `u64` documents state; 0 in a revision field means none.
const DOCUMENT_FIELDS: [Field; 7] = [
    SOURCE_REVISION,
    ANALYSIS_REVISION,
    SCREENPLAY_REVISION,
    LOCKED_REVISION,
    ANALYSIS_STALE,
    SCREENPLAY_STALE,
    DOCUMENTS_DAMAGED,
];

/// Pack `(field, value)` pairs; values wider than their field are cut.
pub(crate) fn pack(values: &[(Field, u64)]) -> u64 {
    values.iter().fold(0, |bits, (field, value)| {
        let mask = (1_u64 << field.width) - 1;
        debug_assert!(*value <= mask, "{} does not fit its field", field.name);
        bits | ((value & mask) << field.shift)
    })
}

/// Pack a shot snapshot. `video_in_flight` and `video_unresolved` are runtime
/// state the canonical snapshot does not carry.
pub(crate) fn encode_shot_snapshot(
    snapshot: ShotApprovalSnapshot,
    video_in_flight: bool,
    video_unresolved: bool,
) -> u64 {
    let video = if video_in_flight {
        1
    } else if video_unresolved {
        5
    } else {
        match snapshot.video {
            VideoState::None => 0,
            VideoState::Ready => 2,
            VideoState::Locked => 3,
            VideoState::Stale => 4,
        }
    };
    let selected = snapshot
        .selected_index
        .map_or(0, |index| u64::from(index) + 1);
    pack(&[
        (GENERATED, u64::from(snapshot.generated)),
        (LOCKED, u64::from(snapshot.locked)),
        (STALE, u64::from(snapshot.stale)),
        (MIGRATION_PENDING, u64::from(snapshot.migration_pending)),
        (INVALID_HASHES, u64::from(snapshot.invalid_hash_count != 0)),
        (VIDEO_STATE, video),
        (CANDIDATE_COUNT, u64::from(snapshot.candidate_count)),
        (SELECTED_PLUS_ONE, selected),
        (DIRECTION, u64::from(snapshot.direction.code())),
        (GENERATION_REVISION, u64::from(snapshot.generation_revision)),
        (SUPERSEDED_COUNT, u64::from(snapshot.superseded_count)),
    ])
}

/// A fixed snapshot, encoded by the one encoder, that Dart decodes in its
/// contract test against the values it expects.
const SAMPLE: ShotApprovalSnapshot = ShotApprovalSnapshot {
    generated: true,
    stale: true,
    selected_index: Some(2),
    locked: false,
    candidate_count: 3,
    direction: ShotDirection::Tension,
    generation_revision: 7,
    superseded_count: 18,
    migration_pending: true,
    invalid_hash_count: 2,
    video: VideoState::Stale,
    video_revision: 3,
};

const ENTRY_STATUS: u32 = 1;
const ENTRY_DIRECTION: u32 = 2;
const ENTRY_SNAPSHOT_FIELD: u32 = 3;
const ENTRY_IMAGE_PROVIDER: u32 = 4;
const ENTRY_VIDEO_STATE: u32 = 5;
const ENTRY_LLM_PROVIDER: u32 = 6;
const ENTRY_DOCUMENT_FIELD: u32 = 7;
const ENTRY_SCREENPLAY_COUNT: u32 = 8;
const ENTRY_SCREENPLAY_VALUE: u32 = 9;
const ENTRY_SCREENPLAY_TEXT: u32 = 10;
const ENTRY_BLOCK_KIND: u32 = 11;
const ENTRY_SETTING: u32 = 12;
const ENTRY_REVISION_STATUS: u32 = 13;
const ENTRY_ORIGIN: u32 = 14;
const ENTRY_SOURCE_FORMAT: u32 = 15;
const ENTRY_DOCUMENT_COST: u32 = 16;

type Entry = (u32, &'static str, i64, u32);

/// Every contract entry, `(kind, name, value, extra)`: a code (value), or a
/// packed field (value = shift, extra = width).
fn entries() -> &'static [Entry] {
    static ENTRIES: std::sync::OnceLock<Vec<Entry>> = std::sync::OnceLock::new();
    ENTRIES.get_or_init(|| {
        let mut entries: Vec<Entry> = Vec::new();
        let codes = |entries: &mut Vec<Entry>, kind: u32, table: &[(&'static str, u32)]| {
            entries.extend(
                table
                    .iter()
                    .map(|(name, code)| (kind, *name, i64::from(*code), 0)),
            );
        };
        let fields = |entries: &mut Vec<Entry>, kind: u32, table: &[Field]| {
            entries.extend(
                table
                    .iter()
                    .map(|field| (kind, field.name, i64::from(field.shift), field.width)),
            );
        };
        entries.extend(
            STATUSES
                .iter()
                .map(|(name, code)| (ENTRY_STATUS, *name, i64::from(*code), 0)),
        );
        entries.extend(
            DIRECTIONS
                .iter()
                .map(|(name, direction)| (ENTRY_DIRECTION, *name, i64::from(direction.code()), 0)),
        );
        fields(&mut entries, ENTRY_SNAPSHOT_FIELD, &SNAPSHOT_FIELDS);
        codes(&mut entries, ENTRY_IMAGE_PROVIDER, &IMAGE_PROVIDERS);
        entries.extend(
            VIDEO_STATES
                .iter()
                .map(|(name, code)| (ENTRY_VIDEO_STATE, *name, i64::from(*code), 0)),
        );
        codes(&mut entries, ENTRY_LLM_PROVIDER, &LLM_PROVIDERS);
        fields(&mut entries, ENTRY_DOCUMENT_FIELD, &DOCUMENT_FIELDS);
        codes(&mut entries, ENTRY_SCREENPLAY_COUNT, &SCREENPLAY_COUNTS);
        codes(&mut entries, ENTRY_SCREENPLAY_VALUE, &SCREENPLAY_VALUES);
        codes(&mut entries, ENTRY_SCREENPLAY_TEXT, &SCREENPLAY_TEXTS);
        codes(&mut entries, ENTRY_BLOCK_KIND, &BLOCK_KINDS);
        codes(&mut entries, ENTRY_SETTING, &SETTINGS);
        codes(&mut entries, ENTRY_REVISION_STATUS, &REVISION_STATUSES);
        codes(&mut entries, ENTRY_ORIGIN, &ORIGINS);
        codes(&mut entries, ENTRY_SOURCE_FORMAT, &SOURCE_FORMATS);
        codes(&mut entries, ENTRY_DOCUMENT_COST, &DOCUMENT_COSTS);
        entries
    })
}

fn entry(index: usize) -> Option<Entry> {
    entries().get(index).copied()
}

/// Number of entries in the native contract table.
#[unsafe(no_mangle)]
pub extern "C" fn kineto_contract_entry_count() -> u32 {
    u32::try_from(entries().len()).expect("contract table fits in u32")
}

/// Describe contract entry `index`. The UTF-8 name is copied into
/// caller-owned memory with the same sizing contract as project metadata.
///
/// # Safety
/// `out_kind`, `out_value`, `out_extra` and `out_len` must point to writable
/// storage. If `out_cap` is non-zero, `out_name` must be writable for at least
/// `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_contract_entry(
    index: u32,
    out_kind: *mut u32,
    out_value: *mut i64,
    out_extra: *mut u32,
    out_name: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_kind.is_null() || out_value.is_null() || out_extra.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        let Some((kind, name, value, extra)) = usize::try_from(index).ok().and_then(entry) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: the caller guarantees writable storage for each output.
        unsafe {
            ptr::write(out_kind, kind);
            ptr::write(out_value, value);
            ptr::write(out_extra, extra);
            crate::project_session::copy_utf8(name.as_bytes(), out_name, out_cap, out_len)
        }
    })
}

/// The fixed sample snapshot, packed by the native encoder.
#[unsafe(no_mangle)]
pub extern "C" fn kineto_contract_snapshot_sample() -> u64 {
    encode_shot_snapshot(SAMPLE, false, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn packed_fields_do_not_overlap_and_fit_in_u64() {
        for table in [&SNAPSHOT_FIELDS[..], &DOCUMENT_FIELDS[..]] {
            let mut used = 0_u64;
            for field in table {
                assert!(
                    field.width > 0 && field.shift + field.width <= 64,
                    "{}",
                    field.name
                );
                let mask = ((1_u128 << field.width) - 1) as u64;
                assert_eq!(used & (mask << field.shift), 0, "{} overlaps", field.name);
                used |= mask << field.shift;
            }
        }
    }

    #[test]
    fn contract_names_and_codes_are_unique() {
        let count = entries().len();
        let names = (0..count)
            .map(|index| entry(index).map(|(kind, name, _, _)| (kind, name)))
            .collect::<Option<BTreeSet<_>>>()
            .expect("every index below the count has an entry");
        assert_eq!(names.len(), count);
        assert!(entry(count).is_none());
        let codes_per_kind = entries()
            .iter()
            .filter(|(kind, ..)| *kind != ENTRY_SNAPSHOT_FIELD && *kind != ENTRY_DOCUMENT_FIELD)
            .map(|(kind, _, value, _)| (*kind, *value))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            codes_per_kind.len(),
            entries()
                .iter()
                .filter(|(kind, ..)| *kind != ENTRY_SNAPSHOT_FIELD && *kind != ENTRY_DOCUMENT_FIELD)
                .count(),
            "no two names of one kind share a code"
        );
        let codes = STATUSES
            .iter()
            .map(|(_, code)| code)
            .collect::<BTreeSet<_>>();
        assert_eq!(codes.len(), STATUSES.len());
        for (name, direction) in DIRECTIONS {
            assert_eq!(
                ShotDirection::from_code(direction.code()),
                Some(direction),
                "{name}"
            );
        }
    }

    #[test]
    fn the_sample_packs_every_field() {
        let bits = kineto_contract_snapshot_sample();
        let read = |field: Field| (bits >> field.shift) & ((1 << field.width) - 1);
        assert_eq!(read(GENERATED), 1);
        assert_eq!(read(LOCKED), 0);
        assert_eq!(read(STALE), 1);
        assert_eq!(read(MIGRATION_PENDING), 1);
        assert_eq!(read(INVALID_HASHES), 1);
        assert_eq!(read(VIDEO_STATE), 4);
        assert_eq!(read(CANDIDATE_COUNT), 3);
        assert_eq!(read(SELECTED_PLUS_ONE), 3);
        assert_eq!(read(DIRECTION), 4);
        assert_eq!(read(GENERATION_REVISION), 7);
        assert_eq!(read(SUPERSEDED_COUNT), 18);
    }

    #[test]
    fn entries_copy_their_names_through_the_abi() {
        let mut kind = 0;
        let mut value = 0;
        let mut extra = 0;
        let mut len = 0;
        let mut name = [0_u8; 64];
        let last = kineto_contract_entry_count() - 1;
        assert_eq!(
            unsafe {
                kineto_contract_entry(
                    last,
                    &raw mut kind,
                    &raw mut value,
                    &raw mut extra,
                    name.as_mut_ptr(),
                    name.len() as u64,
                    &raw mut len,
                )
            },
            PROJECT_OK
        );
        assert_eq!(kind, ENTRY_DOCUMENT_COST);
        assert_eq!(&name[..len as usize], b"screenplay");
        assert_eq!((value, extra), (2, 0));
        assert_eq!(
            unsafe {
                kineto_contract_entry(
                    last + 1,
                    &raw mut kind,
                    &raw mut value,
                    &raw mut extra,
                    name.as_mut_ptr(),
                    name.len() as u64,
                    &raw mut len,
                )
            },
            PROJECT_ERR_INVALID_ARGUMENT
        );
    }
}
