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
pub(crate) const PROJECT_ERR_VIDEO_IN_FLIGHT: i32 = 126;
pub(crate) const PROJECT_ERR_KEYFRAME_NOT_LOCKED: i32 = 127;
pub(crate) const PROJECT_ERR_VIDEO_LOCKED: i32 = 128;
pub(crate) const PROJECT_ERR_NO_VIDEO: i32 = 129;
pub(crate) const PROJECT_ERR_STALE_VIDEO: i32 = 130;
pub(crate) const PROJECT_ERR_NO_FIRST_FRAME: i32 = 131;
pub(crate) const PROJECT_ERR_PANIC: i32 = 199;

/// Every project status code, by the name Dart knows it by.
const STATUSES: [(&str, i32); 34] = [
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
    ("video_in_flight", PROJECT_ERR_VIDEO_IN_FLIGHT),
    ("keyframe_not_locked", PROJECT_ERR_KEYFRAME_NOT_LOCKED),
    ("video_locked", PROJECT_ERR_VIDEO_LOCKED),
    ("no_video", PROJECT_ERR_NO_VIDEO),
    ("stale_video", PROJECT_ERR_STALE_VIDEO),
    ("no_first_frame", PROJECT_ERR_NO_FIRST_FRAME),
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
const VIDEO_STATES: [(&str, u8); 5] = [
    ("none", 0),
    ("generating", 1),
    ("ready", 2),
    ("locked", 3),
    ("stale", 4),
];

/// Image provider codes returned by `kineto_project_image_provider`.
const IMAGE_PROVIDERS: [(&str, u32); 2] = [
    ("local_demo", IMAGE_PROVIDER_LOCAL_DEMO),
    ("minimax", IMAGE_PROVIDER_MINIMAX),
];

#[derive(Debug, Clone, Copy)]
struct Field {
    name: &'static str,
    shift: u32,
    width: u32,
}

const GENERATED: Field = field("generated", 0, 1);
const LOCKED: Field = field("locked", 1, 1);
const STALE: Field = field("stale", 2, 1);
const MIGRATION_PENDING: Field = field("migration_pending", 3, 1);
const INVALID_HASHES: Field = field("invalid_hashes", 4, 1);
/// One of [`VIDEO_STATES`]; `generating` overrides the canonical state.
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

/// Pack a shot snapshot. `video_in_flight` is runtime state the canonical
/// snapshot does not carry.
pub(crate) fn encode_shot_snapshot(snapshot: ShotApprovalSnapshot, video_in_flight: bool) -> u64 {
    let video = if video_in_flight {
        1
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
    [
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
    ]
    .into_iter()
    .fold(0, |bits, (field, value)| {
        let mask = (1_u64 << field.width) - 1;
        debug_assert!(value <= mask, "{} does not fit its field", field.name);
        bits | ((value & mask) << field.shift)
    })
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

/// `(kind, name, value, extra)` for contract entry `index`: a status (value =
/// code), a direction (value = wire code), a snapshot field (value = shift,
/// extra = width) or an image provider (value = code).
fn entry(index: usize) -> Option<(u32, &'static str, i64, u32)> {
    let directions_start = STATUSES.len();
    let fields_start = directions_start + DIRECTIONS.len();
    let providers_start = fields_start + SNAPSHOT_FIELDS.len();
    let video_states_start = providers_start + IMAGE_PROVIDERS.len();
    if let Some((name, code)) = index
        .checked_sub(video_states_start)
        .and_then(|index| VIDEO_STATES.get(index))
    {
        return Some((ENTRY_VIDEO_STATE, name, i64::from(*code), 0));
    }
    if let Some((name, code)) = index
        .checked_sub(providers_start)
        .and_then(|index| IMAGE_PROVIDERS.get(index))
    {
        return Some((ENTRY_IMAGE_PROVIDER, name, i64::from(*code), 0));
    }
    if let Some((name, code)) = STATUSES.get(index) {
        return Some((ENTRY_STATUS, name, i64::from(*code), 0));
    }
    if let Some((name, direction)) = index
        .checked_sub(directions_start)
        .and_then(|index| DIRECTIONS.get(index))
    {
        return Some((ENTRY_DIRECTION, name, i64::from(direction.code()), 0));
    }
    index
        .checked_sub(fields_start)
        .and_then(|index| SNAPSHOT_FIELDS.get(index))
        .map(|field| {
            (
                ENTRY_SNAPSHOT_FIELD,
                field.name,
                i64::from(field.shift),
                field.width,
            )
        })
}

const ENTRY_COUNT: usize = STATUSES.len()
    + DIRECTIONS.len()
    + SNAPSHOT_FIELDS.len()
    + IMAGE_PROVIDERS.len()
    + VIDEO_STATES.len();

/// Number of entries in the native contract table.
#[unsafe(no_mangle)]
pub extern "C" fn kineto_contract_entry_count() -> u32 {
    u32::try_from(ENTRY_COUNT).expect("contract table fits in u32")
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
    encode_shot_snapshot(SAMPLE, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn snapshot_fields_do_not_overlap_and_fit_in_u64() {
        let mut used = 0_u64;
        for field in SNAPSHOT_FIELDS {
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

    #[test]
    fn contract_names_and_codes_are_unique() {
        let names = (0..ENTRY_COUNT)
            .map(|index| entry(index).map(|(kind, name, _, _)| (kind, name)))
            .collect::<Option<BTreeSet<_>>>()
            .expect("every index below the count has an entry");
        assert_eq!(names.len(), ENTRY_COUNT);
        assert!(entry(ENTRY_COUNT).is_none());
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
        assert_eq!(kind, ENTRY_VIDEO_STATE);
        assert_eq!(&name[..len as usize], b"stale");
        assert_eq!((value, extra), (4, 0));
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
