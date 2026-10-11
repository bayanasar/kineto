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
pub(crate) const PROJECT_ERR_IDENTITY_DRIFT: i32 = 144;
pub(crate) const PROJECT_ERR_NEEDS_LOCKED_SCREENPLAY: i32 = 145;
pub(crate) const PROJECT_ERR_IMAGE_PROVIDER_UNSUITABLE: i32 = 146;
pub(crate) const PROJECT_ERR_NO_SUCH_ARTIFACT: i32 = 147;
pub(crate) const PROJECT_ERR_NEEDS_BIBLE: i32 = 148;
pub(crate) const PROJECT_ERR_NEEDS_LOCKED_STYLE: i32 = 149;
pub(crate) const PROJECT_ERR_NEEDS_SELECTED_PORTRAIT: i32 = 150;
pub(crate) const PROJECT_ERR_NO_IDENTITY_REFERENCE: i32 = 151;
pub(crate) const PROJECT_ERR_REJECTED: i32 = 152;
// 153 was `pack_incomplete`, retired with the reference pack.
pub(crate) const PROJECT_ERR_NEEDS_LOCKED_BIBLE: i32 = 154;
pub(crate) const PROJECT_ERR_NEEDS_SHOT_PLAN: i32 = 155;
pub(crate) const PROJECT_ERR_PROVIDER_INCAPABLE: i32 = 156;
pub(crate) const PROJECT_ERR_MEDIA_UNREADABLE: i32 = 157;
pub(crate) const PROJECT_ERR_NO_COLLECTION_PLAN: i32 = 158;
pub(crate) const PROJECT_ERR_EXPORT_INCOMPLETE: i32 = 159;
pub(crate) const PROJECT_ERR_EXPORT_EXISTS: i32 = 160;
pub(crate) const PROJECT_ERR_NOTHING_TO_EXPORT: i32 = 161;
pub(crate) const PROJECT_ERR_MEDIA_CHANGED: i32 = 162;
pub(crate) const PROJECT_ERR_NEEDS_LOCKED_CAST: i32 = 163;
pub(crate) const PROJECT_ERR_NEEDS_LOCKED_VOICE: i32 = 164;
pub(crate) const PROJECT_ERR_PLUGIN_CONFIG_INVALID: i32 = 165;
pub(crate) const PROJECT_ERR_PLUGIN_UNAVAILABLE: i32 = 166;
pub(crate) const PROJECT_ERR_MCP_NOT_ALLOWED: i32 = 167;
pub(crate) const PROJECT_ERR_INVALID_REFERENCE: i32 = 168;
pub(crate) const PROJECT_ERR_PLANNED_DIRECTION: i32 = 169;
pub(crate) const PROJECT_ERR_KEYCHAIN_UNAVAILABLE: i32 = 170;
pub(crate) const PROJECT_ERR_KEYCHAIN_FAILED: i32 = 171;
pub(crate) const PROJECT_ERR_STYLE_REFERENCE_UNUSABLE: i32 = 172;
pub(crate) const PROJECT_ERR_SETTING_INVALID: i32 = 173;
pub(crate) const PROJECT_ERR_SETTINGS_UNWRITABLE: i32 = 174;
pub(crate) const PROJECT_ERR_MCP_SERVER_UNAVAILABLE: i32 = 175;
pub(crate) const PROJECT_ERR_PANIC: i32 = 199;

/// Every project status code, by the name Dart knows it by.
const STATUSES: [(&str, i32); 77] = [
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
    ("identity_drift", PROJECT_ERR_IDENTITY_DRIFT),
    (
        "needs_locked_screenplay",
        PROJECT_ERR_NEEDS_LOCKED_SCREENPLAY,
    ),
    (
        "image_provider_unsuitable",
        PROJECT_ERR_IMAGE_PROVIDER_UNSUITABLE,
    ),
    ("no_such_artifact", PROJECT_ERR_NO_SUCH_ARTIFACT),
    ("needs_bible", PROJECT_ERR_NEEDS_BIBLE),
    ("needs_locked_style", PROJECT_ERR_NEEDS_LOCKED_STYLE),
    (
        "needs_selected_portrait",
        PROJECT_ERR_NEEDS_SELECTED_PORTRAIT,
    ),
    ("no_identity_reference", PROJECT_ERR_NO_IDENTITY_REFERENCE),
    ("rejected", PROJECT_ERR_REJECTED),
    ("needs_locked_bible", PROJECT_ERR_NEEDS_LOCKED_BIBLE),
    ("needs_shot_plan", PROJECT_ERR_NEEDS_SHOT_PLAN),
    ("provider_incapable", PROJECT_ERR_PROVIDER_INCAPABLE),
    ("media_unreadable", PROJECT_ERR_MEDIA_UNREADABLE),
    ("no_collection_plan", PROJECT_ERR_NO_COLLECTION_PLAN),
    ("export_incomplete", PROJECT_ERR_EXPORT_INCOMPLETE),
    ("export_exists", PROJECT_ERR_EXPORT_EXISTS),
    ("nothing_to_export", PROJECT_ERR_NOTHING_TO_EXPORT),
    ("media_changed", PROJECT_ERR_MEDIA_CHANGED),
    ("needs_locked_cast", PROJECT_ERR_NEEDS_LOCKED_CAST),
    ("needs_locked_voice", PROJECT_ERR_NEEDS_LOCKED_VOICE),
    ("plugin_config_invalid", PROJECT_ERR_PLUGIN_CONFIG_INVALID),
    ("plugin_unavailable", PROJECT_ERR_PLUGIN_UNAVAILABLE),
    ("mcp_not_allowed", PROJECT_ERR_MCP_NOT_ALLOWED),
    ("invalid_reference", PROJECT_ERR_INVALID_REFERENCE),
    ("planned_direction", PROJECT_ERR_PLANNED_DIRECTION),
    ("keychain_unavailable", PROJECT_ERR_KEYCHAIN_UNAVAILABLE),
    ("keychain_failed", PROJECT_ERR_KEYCHAIN_FAILED),
    (
        "style_reference_unusable",
        PROJECT_ERR_STYLE_REFERENCE_UNUSABLE,
    ),
    ("setting_invalid", PROJECT_ERR_SETTING_INVALID),
    ("settings_unwritable", PROJECT_ERR_SETTINGS_UNWRITABLE),
    ("mcp_server_unavailable", PROJECT_ERR_MCP_SERVER_UNAVAILABLE),
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
pub(crate) const IMAGE_PROVIDER_PLUGIN: u32 = 3;
pub(crate) const IMAGE_PROVIDER_AGENT: u32 = 4;
pub(crate) const IMAGE_PROVIDER_CONNECTION: u32 = 5;

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
const IMAGE_PROVIDERS: [(&str, u32); 5] = [
    ("local_demo", IMAGE_PROVIDER_LOCAL_DEMO),
    ("minimax", IMAGE_PROVIDER_MINIMAX),
    ("plugin", IMAGE_PROVIDER_PLUGIN),
    ("agent", IMAGE_PROVIDER_AGENT),
    ("connection", IMAGE_PROVIDER_CONNECTION),
];

pub(crate) const VIDEO_PROVIDER_LOCAL_DEMO: u32 = 1;
pub(crate) const VIDEO_PROVIDER_MINIMAX: u32 = 2;
pub(crate) const VIDEO_PROVIDER_PLUGIN: u32 = 3;
pub(crate) const VIDEO_PROVIDER_VEO: u32 = 4;
pub(crate) const VIDEO_PROVIDER_OMNI: u32 = 5;
pub(crate) const VIDEO_PROVIDER_CONNECTION: u32 = 6;

/// The built-in providers whose keys settings list, by the name each key is
/// stored under. Any other key is stored by its own name: one a plugin's
/// entry names, or the one an OpenAI-compatible server takes.
pub(crate) const KEY_PROVIDERS: [(&str, u32); 4] =
    [("minimax", 1), ("gemini", 2), ("kimi", 3), ("veo", 4)];
pub(crate) const KEY_STATE_STORED: u32 = 1;
pub(crate) const KEY_STATE_ENVIRONMENT: u32 = 2;
/// The bits of `kineto_keys_state`: where a provider's key is.
const KEY_STATES: [(&str, u32); 2] = [
    ("stored", KEY_STATE_STORED),
    ("environment", KEY_STATE_ENVIRONMENT),
];

/// The provider settings `kineto_settings_*` read and change.
pub(crate) const PROVIDER_SETTINGS: [(&str, u32); 4] = [
    ("image", 1),
    ("video", 2),
    ("speech", 3),
    ("language_model", 4),
];

/// The fields of a connection `kineto_settings_connection_*` read and put,
/// each as text.
pub(crate) const CONNECTION_FIELDS: [(&str, u32); 11] = [
    ("name", 1),
    ("protocol", 2),
    ("url", 3),
    ("model", 4),
    ("key", 5),
    ("price_per_million_input", 6),
    ("price_per_million_output", 7),
    ("price_per_image", 8),
    ("references", 9),
    // The names of the fields the settings file adds to each request, read
    // only: settings show them and never change them.
    ("request_fields", 10),
    ("price_per_second", 11),
];

/// The protocols a connection may speak, by their names in the settings
/// file with `_` for `-`.
pub(crate) const CONNECTION_PROTOCOLS: [(&str, u32); 5] = [
    ("openai_chat", 1),
    ("anthropic_messages", 2),
    ("openai_images", 3),
    ("gemini_images", 4),
    ("ark_video", 5),
];

/// The agent CLIs settings look for.
pub(crate) const AGENTS: [(&str, u32); 5] = [
    ("claude_code", 1),
    ("codex", 2),
    ("kimi_code", 3),
    ("agy", 4),
    ("mini_agent", 5),
];

/// Video provider codes returned by `kineto_project_video_provider`.
const VIDEO_PROVIDERS: [(&str, u32); 6] = [
    ("local_demo", VIDEO_PROVIDER_LOCAL_DEMO),
    ("minimax", VIDEO_PROVIDER_MINIMAX),
    ("plugin", VIDEO_PROVIDER_PLUGIN),
    ("veo", VIDEO_PROVIDER_VEO),
    ("omni", VIDEO_PROVIDER_OMNI),
    ("connection", VIDEO_PROVIDER_CONNECTION),
];

/// What `kineto_project_video_take_count` counts.
pub(crate) const VIDEO_TAKE_COUNTS: [(&str, u32); 3] =
    [("video_history", 1), ("video_notes", 2), ("video_stale", 3)];

/// Number fields `kineto_project_video_take_value` reads. A candidate is its
/// place in its generation from 1; 0 for a video made before generations had
/// several. Statuses are `take_status` codes.
pub(crate) const VIDEO_TAKE_VALUES: [(&str, u32); 3] = [
    ("video_generation", 1),
    ("video_candidate", 2),
    ("video_status", 3),
];

/// Text fields `kineto_project_video_take_text_copy` copies.
pub(crate) const VIDEO_TAKE_TEXTS: [(&str, u32); 7] = [
    ("video_id", 1),
    ("video_media", 2),
    ("video_provider", 3),
    ("video_model", 4),
    ("video_note", 5),
    ("video_branched_from", 6),
    ("video_stale_reason", 7),
];

/// Number fields `kineto_project_progress_value` reads.
pub(crate) const PROGRESS_VALUES: [(&str, u32); 3] = [
    ("scene_complete", 1),
    ("keyframe_stage", 2),
    ("video_stage", 3),
];

/// Text fields `kineto_project_progress_text_copy` copies.
pub(crate) const PROGRESS_TEXTS: [(&str, u32); 2] = [("scene_id", 1), ("shot_id", 2)];

/// How far a shot's keyframe or video has come.
pub(crate) const STAGES: [(&str, u32); 4] =
    [("missing", 0), ("made", 1), ("locked", 2), ("stale", 3)];

/// Storage commands `kineto_project_storage` takes.
pub(crate) const STORAGE_COMMANDS: [(&str, u32); 3] =
    [("plan_collection", 1), ("collect", 2), ("verify", 3)];

/// Number fields `kineto_project_media_value` reads.
pub(crate) const MEDIA_VALUES: [(&str, u32); 5] = [
    ("usage_bytes", 1),
    ("recent_takes", 2),
    ("planned", 3),
    ("plan_bytes", 4),
    ("media_damaged", 5),
];

/// Lists `kineto_project_media_count` counts and its item reads index.
pub(crate) const MEDIA_LISTS: [(&str, u32); 4] = [
    ("plan", 1),
    ("missing", 2),
    ("mismatched", 3),
    ("owners", 4),
];

/// Text fields `kineto_project_media_text_copy` copies from a list item.
pub(crate) const MEDIA_TEXTS: [(&str, u32); 4] = [
    ("media_artifact", 1),
    ("media_owner", 2),
    ("media_reason", 3),
    ("media_problem", 4),
];

/// Number fields `kineto_project_export_value` reads.
pub(crate) const EXPORT_VALUES: [(&str, u32); 8] = [
    ("export_ready", 1),
    ("export_missing", 2),
    ("export_complete", 3),
    ("last_export", 4),
    ("last_shots", 5),
    ("last_complete", 6),
    ("last_bytes", 7),
    ("last_lines", 8),
];

/// Text fields `kineto_project_export_text_copy` copies.
pub(crate) const EXPORT_TEXTS: [(&str, u32); 4] = [
    ("missing_scene", 1),
    ("missing_shot", 2),
    ("missing_reason", 3),
    ("last_path", 4),
];

/// Language model codes returned by `kineto_project_llm_provider`.
pub(crate) const LLM_PROVIDERS: [(&str, u32); 9] = [
    ("none", 0),
    ("kimi_code", 1),
    ("kimi", 2),
    ("local", 3),
    ("claude_code", 4),
    ("codex", 5),
    ("agy", 6),
    ("mini_agent", 7),
    ("connection", 8),
];

/// Source format codes `kineto_project_import_source` takes.
pub(crate) const SOURCE_FORMATS: [(&str, u32); 2] = [("text", 1), ("markdown", 2)];

/// Documents whose cost `kineto_project_document_cost` estimates.
pub(crate) const DOCUMENT_COSTS: [(&str, u32); 6] = [
    ("analysis", 1),
    ("screenplay", 2),
    ("bible", 3),
    ("shot_plan", 4),
    ("preference_suggestions", 5),
    ("voice_recommendations", 6),
];

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

/// What `kineto_project_production_count` counts; `group` selects the entry,
/// sample or character for per-group counts.
pub(crate) const PRODUCTION_COUNTS: [(&str, u32); 18] = [
    ("bible_revisions", 1),
    ("world_visual_rules", 2),
    ("world_continuity_rules", 3),
    ("bible_characters", 4),
    ("character_forbidden", 5),
    ("character_notes", 6),
    ("character_wardrobe_changes", 7),
    ("bible_locations", 8),
    ("location_forbidden", 9),
    ("bible_props", 10),
    ("prop_forbidden", 11),
    ("bible_stale_causes", 12),
    ("style_presets", 13),
    ("style_samples", 14),
    ("cast_characters", 15),
    ("cast_images", 16),
    ("cast_stale", 17),
    ("cast_lock_references", 18),
];

/// Number fields `kineto_project_production_value` reads.
pub(crate) const PRODUCTION_VALUES: [(&str, u32); 10] = [
    ("bible_revision_number", 1),
    ("bible_revision_status", 2),
    ("bible_revision_origin", 3),
    ("bible_revision_based_on", 4),
    ("bible_stale_cause", 5),
    ("sample_status", 6),
    ("cast_image_kind", 7),
    ("cast_image_status", 8),
    ("cast_locked", 9),
    ("cast_unresolved", 10),
];

/// Text fields `kineto_project_production_text_copy` copies.
pub(crate) const PRODUCTION_TEXTS: [(&str, u32); 48] = [
    ("world_setting", 1),
    ("world_period", 2),
    ("world_visual_rule", 3),
    ("world_continuity_rule", 4),
    ("character_id", 5),
    ("character_name", 6),
    ("character_description", 7),
    ("character_appearance", 8),
    ("character_wardrobe", 9),
    ("character_forbidden", 10),
    ("character_note", 11),
    ("character_wardrobe_change_scenes", 12),
    ("character_wardrobe_change", 13),
    ("location_id", 14),
    ("location_name", 15),
    ("location_description", 16),
    ("location_appearance", 17),
    ("location_forbidden", 18),
    ("prop_id", 19),
    ("prop_name", 20),
    ("prop_description", 21),
    ("prop_appearance", 22),
    ("prop_forbidden", 23),
    ("bible_revision_provider", 24),
    ("bible_revision_model", 25),
    ("bible_stale_subject", 26),
    ("preset_id", 27),
    ("preset_name", 28),
    ("sample_id", 29),
    ("sample_preset", 30),
    ("sample_note", 31),
    ("sample_refines", 32),
    ("sample_scene", 33),
    ("sample_media", 34),
    ("sample_provider", 35),
    ("sample_model", 36),
    ("style_lock_sample", 37),
    ("style_lock_preset", 38),
    ("style_lock_note", 39),
    ("cast_character_id", 40),
    ("cast_character_name", 41),
    ("cast_image_id", 42),
    ("cast_image_media", 43),
    ("cast_image_identity_reference", 44),
    ("cast_lock_portrait", 45),
    ("cast_lock_reference", 46),
    ("cast_stale", 47),
    ("sample_directives", 48),
];

/// Why a bible may no longer describe the locked screenplay.
pub(crate) const STALE_CAUSES: [(&str, u32); 6] = [
    ("no_locked_screenplay", 1),
    ("material", 2),
    ("scene_changed", 3),
    ("scene_removed", 4),
    ("scene_added", 5),
    ("reference", 6),
];

/// A style sample's or a portrait's standing; 0 is none, for an image that
/// is not a portrait.
pub(crate) const CHOICE_STATUSES: [(&str, u32); 3] =
    [("candidate", 1), ("selected", 2), ("rejected", 3)];

pub(crate) const CAST_IMAGE_KINDS: [(&str, u32); 8] = [
    ("portrait", 1),
    ("front", 2),
    ("profile_left", 3),
    ("profile_right", 4),
    ("full_body", 5),
    ("costume", 6),
    ("expression_smile", 7),
    ("expression_tense", 8),
];

/// Which list of the bible an edited entry is in.
pub(crate) const ENTRY_KINDS: [(&str, u32); 3] = [("character", 1), ("location", 2), ("prop", 3)];

/// What the director decides about a style sample or a portrait.
pub(crate) const DECISIONS: [(&str, u32); 3] = [("select", 1), ("reject", 2), ("lock", 3)];

/// Paid images whose cost `kineto_project_image_cost` gives.
pub(crate) const IMAGE_COSTS: [(&str, u32); 2] = [("sample", 1), ("portrait", 2)];

/// What `kineto_project_plan_count` counts; `group` selects the scene (or
/// the step), `item` the shot.
pub(crate) const PLAN_COUNTS: [(&str, u32); 10] = [
    ("plan_revisions", 1),
    ("plan_scenes", 2),
    ("plan_shots", 3),
    ("shot_characters", 4),
    ("shot_continuity", 5),
    ("plan_stale_causes", 6),
    ("workflow_steps", 7),
    ("step_waiting", 8),
    ("step_stale", 9),
    ("step_skipped", 10),
];

/// Number fields `kineto_project_plan_value` reads.
pub(crate) const PLAN_VALUES: [(&str, u32); 13] = [
    ("plan_revision_number", 1),
    ("plan_revision_status", 2),
    ("plan_revision_origin", 3),
    ("plan_revision_based_on", 4),
    ("plan_stale_cause", 5),
    ("shot_direction", 6),
    ("shot_framing", 7),
    ("shot_camera_motion", 8),
    ("shot_duration_millis", 9),
    ("step_state", 10),
    ("step_approval", 11),
    ("shot_dialogue_millis", 12),
    ("shot_too_short", 13),
];

/// Text fields `kineto_project_plan_text_copy` copies.
pub(crate) const PLAN_TEXTS: [(&str, u32); 19] = [
    ("plan_revision_provider", 1),
    ("plan_revision_model", 2),
    ("plan_stale_subject", 3),
    ("scene_id", 4),
    ("shot_id", 5),
    ("shot_purpose", 6),
    ("shot_lens", 7),
    ("shot_action", 8),
    ("shot_dialogue", 9),
    ("shot_emotion", 10),
    ("shot_continuity_from", 11),
    ("shot_character", 12),
    ("shot_continuity", 13),
    ("step_id", 14),
    ("step_description", 15),
    ("step_waiting", 16),
    ("step_stale", 17),
    ("workflow_problem", 18),
    ("step_skipped", 19),
];

pub(crate) const FRAMINGS: [(&str, u32); 10] = [
    ("extreme_wide", 1),
    ("wide", 2),
    ("full", 3),
    ("medium_wide", 4),
    ("medium", 5),
    ("medium_close_up", 6),
    ("close_up", 7),
    ("extreme_close_up", 8),
    ("over_the_shoulder", 9),
    ("insert", 10),
];

pub(crate) const CAMERA_MOTIONS: [(&str, u32); 11] = [
    ("static", 1),
    ("slow_dolly_in", 2),
    ("dolly_in", 3),
    ("slow_dolly_out", 4),
    ("dolly_out", 5),
    ("pan_left", 6),
    ("pan_right", 7),
    ("tilt_up", 8),
    ("tilt_down", 9),
    ("tracking", 10),
    ("handheld", 11),
];

/// Why a shot plan may no longer follow the locked screenplay and bible.
pub(crate) const PLAN_STALE_CAUSES: [(&str, u32); 5] = [
    ("not_locked", 1),
    ("scene_changed", 2),
    ("scene_removed", 3),
    ("scene_added", 4),
    ("entry", 5),
];

/// Where a recipe step stands.
pub(crate) const STEP_STATES: [(&str, u32); 5] = [
    ("waiting", 1),
    ("ready", 2),
    ("awaiting_approval", 3),
    ("done", 4),
    ("skipped", 5),
];

/// What `kineto_project_take_count` counts for a shot.
pub(crate) const TAKE_COUNTS: [(&str, u32); 2] = [("history", 1), ("staleness", 2)];

/// Number fields `kineto_project_take_value` reads.
pub(crate) const TAKE_VALUES: [(&str, u32); 3] = [
    ("take_generation", 1),
    ("take_status", 2),
    ("take_refined", 3),
];

/// Text fields `kineto_project_take_text_copy` copies.
pub(crate) const TAKE_TEXTS: [(&str, u32); 5] = [
    ("take_id", 1),
    ("take_media", 2),
    ("take_provider", 3),
    ("take_model", 4),
    ("stale_reason", 5),
];

/// A take's standing in its shot.
pub(crate) const TAKE_STATUSES: [(&str, u32); 5] = [
    ("draft", 1),
    ("candidate", 2),
    ("selected", 3),
    ("locked", 4),
    ("superseded", 5),
];

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

/// The latest bible revision still in play.
pub(crate) const BIBLE_REVISION: Field = field("bible_revision", 0, 14);
pub(crate) const BIBLE_LOCKED_REVISION: Field = field("bible_locked_revision", 14, 14);
pub(crate) const BIBLE_STALE: Field = field("bible_stale", 28, 1);
pub(crate) const STYLE_LOCKED: Field = field("style_locked", 29, 1);
pub(crate) const STYLE_UNRESOLVED: Field = field("style_unresolved", 30, 1);
pub(crate) const PRODUCTION_DAMAGED: Field = field("production_damaged", 31, 1);

/// The latest shot plan revision still in play.
pub(crate) const PLAN_REVISION: Field = field("plan_revision", 0, 14);
pub(crate) const PLAN_LOCKED_REVISION: Field = field("plan_locked_revision", 14, 14);
pub(crate) const PLAN_STALE: Field = field("plan_stale", 28, 1);
pub(crate) const WORKFLOW_UNKNOWN: Field = field("workflow_unknown", 29, 1);

/// The packed `u64` plan state; 0 in a revision field means none.
const PLAN_FIELDS: [Field; 4] = [
    PLAN_REVISION,
    PLAN_LOCKED_REVISION,
    PLAN_STALE,
    WORKFLOW_UNKNOWN,
];

/// The packed `u64` production state; 0 in a revision field means none.
const PRODUCTION_FIELDS: [Field; 6] = [
    BIBLE_REVISION,
    BIBLE_LOCKED_REVISION,
    BIBLE_STALE,
    STYLE_LOCKED,
    STYLE_UNRESOLVED,
    PRODUCTION_DAMAGED,
];

/// Pack `(field, value)` pairs. A value wider than its field is a broken
/// invariant, never cut to fit: the call fails, in every build, rather than
/// hand Dart another value.
pub(crate) fn pack(values: &[(Field, u64)]) -> u64 {
    values.iter().fold(0, |bits, (field, value)| {
        let mask = (1_u64 << field.width) - 1;
        assert!(*value <= mask, "{} does not fit its field", field.name);
        bits | (value << field.shift)
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

/// What `kineto_project_voice_count` counts. `group` selects the character
/// (in cast order), the line, the preference or the suggestion.
pub(crate) const VOICE_COUNTS: [(&str, u32); 12] = [
    ("voices", 1),
    ("voice_auditions", 2),
    ("voice_stale", 3),
    ("voice_recommended", 4),
    ("lines", 5),
    ("line_takes", 6),
    ("line_stale", 7),
    ("preferences", 8),
    ("preference_evidence", 9),
    ("preference_revisions", 10),
    ("suggestions", 11),
    ("suggestion_evidence", 12),
];

/// Number fields `kineto_project_voice_value` reads.
pub(crate) const VOICE_VALUES: [(&str, u32); 34] = [
    ("voice_locked", 1),
    ("voice_unresolved", 2),
    ("audition_status", 3),
    ("audition_duration_millis", 4),
    ("audition_speed_permille", 5),
    ("line_number", 6),
    ("line_locked", 7),
    ("line_unresolved", 8),
    ("line_take_cost_known", 9),
    ("line_take_cost_micros", 10),
    ("line_duration_millis", 11),
    ("line_duration_source", 12),
    ("take_status", 13),
    ("take_duration_millis", 14),
    ("take_speed_permille", 15),
    ("preference_dimension", 16),
    ("preference_stance", 17),
    ("preference_strength_permille", 18),
    ("preference_from_suggestion", 19),
    ("preferences_revision", 20),
    ("preference_revision_number", 21),
    ("preference_revision_origin", 22),
    ("suggestion_batch", 23),
    ("suggestion_index", 24),
    ("suggestion_dimension", 25),
    ("suggestion_stance", 26),
    ("suggestion_compared_images", 27),
    ("unconsidered_choices", 28),
    ("suggestions_ready", 29),
    ("plan_preferences_changed", 30),
    ("speech_provider", 31),
    ("speech_ready", 32),
    ("speech_character_cost_known", 33),
    ("speech_character_cost_micros", 34),
];

/// Text fields `kineto_project_voice_text_copy` copies.
pub(crate) const VOICE_TEXTS: [(&str, u32); 34] = [
    ("voice_id", 1),
    ("voice_name", 2),
    ("voice_description", 3),
    ("voices_problem", 4),
    ("voice_default_line", 5),
    ("audition_id", 6),
    ("audition_voice_id", 7),
    ("audition_voice_name", 8),
    ("audition_line", 9),
    ("audition_media", 10),
    ("audition_emotion", 11),
    ("voice_lock_audition", 12),
    ("voice_lock_voice_id", 13),
    ("voice_lock_voice_name", 14),
    ("voice_stale", 15),
    ("recommended_voice_id", 16),
    ("recommended_voice_name", 17),
    ("recommended_reason", 18),
    ("line_scene", 19),
    ("line_character", 20),
    ("line_text", 21),
    ("line_parenthetical", 22),
    ("take_id", 23),
    ("take_media", 24),
    ("take_note", 25),
    ("take_emotion", 26),
    ("line_lock_take", 27),
    ("line_stale", 28),
    ("preference_value", 29),
    ("preference_reason", 30),
    ("preference_evidence", 31),
    ("suggestion_value", 32),
    ("suggestion_reason", 33),
    ("suggestion_evidence", 34),
];

/// Deliveries a line or an audition can be asked for.
pub(crate) const EMOTIONS: [(&str, u32); 9] = [
    ("happy", 1),
    ("sad", 2),
    ("angry", 3),
    ("fearful", 4),
    ("disgusted", 5),
    ("surprised", 6),
    ("calm", 7),
    ("fluent", 8),
    ("whisper", 9),
];

/// What a preference is about.
pub(crate) const DIMENSIONS: [(&str, u32); 8] = [
    ("camera_motion", 1),
    ("lighting", 2),
    ("framing", 3),
    ("lens", 4),
    ("color", 5),
    ("composition", 6),
    ("performance", 7),
    ("pacing", 8),
];

pub(crate) const STANCES: [(&str, u32); 2] = [("prefer", 1), ("avoid", 2)];

/// Where a line's length comes from.
pub(crate) const DURATION_SOURCES: [(&str, u32); 2] = [("estimated", 1), ("measured", 2)];

pub(crate) const SPEECH_PROVIDERS: [(&str, u32); 3] =
    [("local_demo", 1), ("minimax", 2), ("plugin", 3)];

/// How a tool takes an argument, as `KinetoToolArgument.kind` says.
pub(crate) const TOOL_ARGUMENT_KINDS: [(&str, u32); 3] = [("text", 1), ("number", 2), ("flag", 3)];

/// Text fields `kineto_project_reference_text_copy` copies of a reference.
pub(crate) const REFERENCE_TEXTS: [(&str, u32); 7] = [
    ("id", 1),
    ("server", 2),
    ("tool", 3),
    ("arguments", 4),
    ("text", 5),
    ("attached_to", 6),
    ("content_hash", 7),
];

/// Number fields `kineto_mcp_value` reads of a server or a tool parameter.
pub(crate) const MCP_VALUES: [(&str, u32); 4] =
    [("enabled", 1), ("listed", 2), ("required", 3), ("kind", 4)];

/// Text fields `kineto_mcp_text_copy` copies of a server, a tool or a tool
/// parameter.
pub(crate) const MCP_TEXTS: [(&str, u32); 9] = [
    ("id", 1),
    ("executable", 2),
    ("secrets", 3),
    ("allowed", 4),
    ("problem", 5),
    ("tool_name", 6),
    ("tool_description", 7),
    ("parameter_name", 8),
    ("parameter_description", 9),
];

/// Number fields `kineto_plugins_value` reads of a registered plugin.
pub(crate) const PLUGIN_VALUES: [(&str, u32); 13] = [
    ("enabled", 1),
    ("described", 2),
    ("selected", 3),
    ("makes_images", 4),
    ("speaks", 5),
    ("identity_references", 6),
    ("composition_reference", 7),
    ("image_price_known", 8),
    ("image_price_micros", 9),
    ("speech_price_known", 10),
    ("speech_character_price_micros", 11),
    ("makes_videos", 12),
    ("style_reference", 13),
];

/// Text fields `kineto_plugins_text_copy` copies of a registered plugin.
/// Lists are one item per line.
pub(crate) const PLUGIN_TEXTS: [(&str, u32); 12] = [
    ("id", 1),
    ("name", 2),
    ("version", 3),
    ("executable", 4),
    ("secrets", 5),
    ("image_model", 6),
    ("speech_model", 7),
    ("problem", 8),
    ("aspect_ratios", 9),
    ("languages", 10),
    ("video_model", 11),
    ("clip_seconds", 12),
];

/// What the director decides about a suggestion.
pub(crate) const SUGGESTION_DECISIONS: [(&str, u32); 2] = [("accept", 1), ("reject", 2)];

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
const ENTRY_PRODUCTION_FIELD: u32 = 17;
const ENTRY_PRODUCTION_COUNT: u32 = 18;
const ENTRY_PRODUCTION_VALUE: u32 = 19;
const ENTRY_PRODUCTION_TEXT: u32 = 20;
const ENTRY_STALE_CAUSE: u32 = 21;
const ENTRY_CHOICE_STATUS: u32 = 22;
const ENTRY_CAST_IMAGE_KIND: u32 = 23;
const ENTRY_ENTRY_KIND: u32 = 24;
const ENTRY_DECISION: u32 = 25;
const ENTRY_IMAGE_COST: u32 = 26;
const ENTRY_PLAN_FIELD: u32 = 27;
const ENTRY_PLAN_COUNT: u32 = 28;
const ENTRY_PLAN_VALUE: u32 = 29;
const ENTRY_PLAN_TEXT: u32 = 30;
const ENTRY_FRAMING: u32 = 31;
const ENTRY_CAMERA_MOTION: u32 = 32;
const ENTRY_PLAN_STALE_CAUSE: u32 = 33;
const ENTRY_STEP_STATE: u32 = 34;
const ENTRY_TAKE_COUNT: u32 = 35;
const ENTRY_TAKE_VALUE: u32 = 36;
const ENTRY_TAKE_TEXT: u32 = 37;
const ENTRY_TAKE_STATUS: u32 = 38;
const ENTRY_VIDEO_PROVIDER: u32 = 39;
const ENTRY_VIDEO_TAKE_COUNT: u32 = 40;
const ENTRY_VIDEO_TAKE_VALUE: u32 = 41;
const ENTRY_VIDEO_TAKE_TEXT: u32 = 42;
const ENTRY_PROGRESS_VALUE: u32 = 43;
const ENTRY_PROGRESS_TEXT: u32 = 44;
const ENTRY_STAGE: u32 = 45;
const ENTRY_STORAGE_COMMAND: u32 = 46;
const ENTRY_MEDIA_VALUE: u32 = 47;
const ENTRY_MEDIA_LIST: u32 = 48;
const ENTRY_MEDIA_TEXT: u32 = 49;
const ENTRY_EXPORT_VALUE: u32 = 50;
const ENTRY_EXPORT_TEXT: u32 = 51;
const ENTRY_VOICE_COUNT: u32 = 52;
const ENTRY_VOICE_VALUE: u32 = 53;
const ENTRY_VOICE_TEXT: u32 = 54;
const ENTRY_EMOTION: u32 = 55;
const ENTRY_DIMENSION: u32 = 56;
const ENTRY_STANCE: u32 = 57;
const ENTRY_DURATION_SOURCE: u32 = 58;
const ENTRY_SPEECH_PROVIDER: u32 = 59;
const ENTRY_SUGGESTION_DECISION: u32 = 60;
const ENTRY_PLUGIN_VALUE: u32 = 61;
const ENTRY_PLUGIN_TEXT: u32 = 62;
const ENTRY_TOOL_ARGUMENT_KIND: u32 = 63;
const ENTRY_REFERENCE_TEXT: u32 = 64;
const ENTRY_MCP_VALUE: u32 = 65;
const ENTRY_MCP_TEXT: u32 = 66;
const ENTRY_KEY_PROVIDER: u32 = 67;
const ENTRY_KEY_STATE: u32 = 68;
const ENTRY_PROVIDER_SETTING: u32 = 69;
const ENTRY_AGENT: u32 = 70;
const ENTRY_CONNECTION_FIELD: u32 = 71;
const ENTRY_CONNECTION_PROTOCOL: u32 = 72;

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
        fields(&mut entries, ENTRY_PRODUCTION_FIELD, &PRODUCTION_FIELDS);
        codes(&mut entries, ENTRY_PRODUCTION_COUNT, &PRODUCTION_COUNTS);
        codes(&mut entries, ENTRY_PRODUCTION_VALUE, &PRODUCTION_VALUES);
        codes(&mut entries, ENTRY_PRODUCTION_TEXT, &PRODUCTION_TEXTS);
        codes(&mut entries, ENTRY_STALE_CAUSE, &STALE_CAUSES);
        codes(&mut entries, ENTRY_CHOICE_STATUS, &CHOICE_STATUSES);
        codes(&mut entries, ENTRY_CAST_IMAGE_KIND, &CAST_IMAGE_KINDS);
        codes(&mut entries, ENTRY_ENTRY_KIND, &ENTRY_KINDS);
        codes(&mut entries, ENTRY_DECISION, &DECISIONS);
        codes(&mut entries, ENTRY_IMAGE_COST, &IMAGE_COSTS);
        fields(&mut entries, ENTRY_PLAN_FIELD, &PLAN_FIELDS);
        codes(&mut entries, ENTRY_PLAN_COUNT, &PLAN_COUNTS);
        codes(&mut entries, ENTRY_PLAN_VALUE, &PLAN_VALUES);
        codes(&mut entries, ENTRY_PLAN_TEXT, &PLAN_TEXTS);
        codes(&mut entries, ENTRY_FRAMING, &FRAMINGS);
        codes(&mut entries, ENTRY_CAMERA_MOTION, &CAMERA_MOTIONS);
        codes(&mut entries, ENTRY_PLAN_STALE_CAUSE, &PLAN_STALE_CAUSES);
        codes(&mut entries, ENTRY_STEP_STATE, &STEP_STATES);
        codes(&mut entries, ENTRY_TAKE_COUNT, &TAKE_COUNTS);
        codes(&mut entries, ENTRY_TAKE_VALUE, &TAKE_VALUES);
        codes(&mut entries, ENTRY_TAKE_TEXT, &TAKE_TEXTS);
        codes(&mut entries, ENTRY_TAKE_STATUS, &TAKE_STATUSES);
        codes(&mut entries, ENTRY_VIDEO_PROVIDER, &VIDEO_PROVIDERS);
        codes(&mut entries, ENTRY_VIDEO_TAKE_COUNT, &VIDEO_TAKE_COUNTS);
        codes(&mut entries, ENTRY_VIDEO_TAKE_VALUE, &VIDEO_TAKE_VALUES);
        codes(&mut entries, ENTRY_VIDEO_TAKE_TEXT, &VIDEO_TAKE_TEXTS);
        codes(&mut entries, ENTRY_PROGRESS_VALUE, &PROGRESS_VALUES);
        codes(&mut entries, ENTRY_PROGRESS_TEXT, &PROGRESS_TEXTS);
        codes(&mut entries, ENTRY_STAGE, &STAGES);
        codes(&mut entries, ENTRY_STORAGE_COMMAND, &STORAGE_COMMANDS);
        codes(&mut entries, ENTRY_MEDIA_VALUE, &MEDIA_VALUES);
        codes(&mut entries, ENTRY_MEDIA_LIST, &MEDIA_LISTS);
        codes(&mut entries, ENTRY_MEDIA_TEXT, &MEDIA_TEXTS);
        codes(&mut entries, ENTRY_EXPORT_VALUE, &EXPORT_VALUES);
        codes(&mut entries, ENTRY_EXPORT_TEXT, &EXPORT_TEXTS);
        codes(&mut entries, ENTRY_VOICE_COUNT, &VOICE_COUNTS);
        codes(&mut entries, ENTRY_VOICE_VALUE, &VOICE_VALUES);
        codes(&mut entries, ENTRY_VOICE_TEXT, &VOICE_TEXTS);
        codes(&mut entries, ENTRY_EMOTION, &EMOTIONS);
        codes(&mut entries, ENTRY_DIMENSION, &DIMENSIONS);
        codes(&mut entries, ENTRY_STANCE, &STANCES);
        codes(&mut entries, ENTRY_DURATION_SOURCE, &DURATION_SOURCES);
        codes(&mut entries, ENTRY_SPEECH_PROVIDER, &SPEECH_PROVIDERS);
        codes(
            &mut entries,
            ENTRY_SUGGESTION_DECISION,
            &SUGGESTION_DECISIONS,
        );
        codes(&mut entries, ENTRY_PLUGIN_VALUE, &PLUGIN_VALUES);
        codes(&mut entries, ENTRY_PLUGIN_TEXT, &PLUGIN_TEXTS);
        codes(&mut entries, ENTRY_TOOL_ARGUMENT_KIND, &TOOL_ARGUMENT_KINDS);
        codes(&mut entries, ENTRY_REFERENCE_TEXT, &REFERENCE_TEXTS);
        codes(&mut entries, ENTRY_MCP_VALUE, &MCP_VALUES);
        codes(&mut entries, ENTRY_MCP_TEXT, &MCP_TEXTS);
        codes(&mut entries, ENTRY_KEY_PROVIDER, &KEY_PROVIDERS);
        codes(&mut entries, ENTRY_KEY_STATE, &KEY_STATES);
        codes(&mut entries, ENTRY_PROVIDER_SETTING, &PROVIDER_SETTINGS);
        codes(&mut entries, ENTRY_AGENT, &AGENTS);
        codes(&mut entries, ENTRY_CONNECTION_FIELD, &CONNECTION_FIELDS);
        codes(
            &mut entries,
            ENTRY_CONNECTION_PROTOCOL,
            &CONNECTION_PROTOCOLS,
        );
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

    fn is_field_kind(kind: u32) -> bool {
        [
            ENTRY_SNAPSHOT_FIELD,
            ENTRY_DOCUMENT_FIELD,
            ENTRY_PRODUCTION_FIELD,
            ENTRY_PLAN_FIELD,
        ]
        .contains(&kind)
    }

    #[test]
    fn packed_fields_do_not_overlap_and_fit_in_u64() {
        for table in [
            &SNAPSHOT_FIELDS[..],
            &DOCUMENT_FIELDS[..],
            &PRODUCTION_FIELDS[..],
            &PLAN_FIELDS[..],
        ] {
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

    /// Every revision a project can hold fits the fields that carry one.
    #[test]
    fn revision_fields_hold_every_revision() {
        for field in [
            SOURCE_REVISION,
            ANALYSIS_REVISION,
            SCREENPLAY_REVISION,
            LOCKED_REVISION,
            BIBLE_REVISION,
            BIBLE_LOCKED_REVISION,
            PLAN_REVISION,
            PLAN_LOCKED_REVISION,
        ] {
            assert_eq!(
                (1_u64 << field.width) - 1,
                u64::from(kineto_project::revisions::MAX_REVISION),
                "{}",
                field.name
            );
        }
        let last = u64::from(kineto_project::revisions::MAX_REVISION);
        assert_eq!(
            pack(&[(SOURCE_REVISION, last), (ANALYSIS_REVISION, 1)]),
            last | (1 << 14)
        );
    }

    #[test]
    #[should_panic(expected = "source_revision does not fit its field")]
    fn a_value_wider_than_its_field_fails_rather_than_wrapping() {
        pack(&[(SOURCE_REVISION, 16_384)]);
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
            .filter(|(kind, ..)| !is_field_kind(*kind))
            .map(|(kind, _, value, _)| (*kind, *value))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            codes_per_kind.len(),
            entries()
                .iter()
                .filter(|(kind, ..)| !is_field_kind(*kind))
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
        assert_eq!(kind, ENTRY_CONNECTION_PROTOCOL);
        assert_eq!(&name[..len as usize], b"ark_video");
        assert_eq!((value, extra), (5, 0));
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
