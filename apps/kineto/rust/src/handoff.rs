//! Video takes, scene progress, stored media and the production package
//! across the C ABI.
//!
//! Commands queue on the session worker. Shot commands answer with a
//! `COMPLETED` message for their shot; storage and export commands with one
//! whose shot is -1. Reads return what the worker last published, one field
//! at a time, each selected by a code the contract table publishes; nothing
//! crosses as JSON.

use kineto_core::{
    export::ExportRequest,
    session::{ProjectSession, PublishedMedia, ShotCommand, StorageCommand, SubmitError},
    video::VideoAsk,
    workflow::Stage,
};
use kineto_project::{
    ArtifactStatus,
    media::{MediaOwner, MediaRecord},
    shot::MAX_VIDEO_CANDIDATES,
};

use crate::{
    contract::*,
    documents::{at, optional_text, session_ref},
    guarded,
    project_session::{KinetoProjectSession, copy_utf8, submit_shot, utf8_owned},
};

/// Queue a video generation of shot `shot_index`: `count` candidates (1 to
/// 4), with the director's `note` and a take to `branch` from, either empty
/// for none. With `resubmit`, a candidate whose last submission may have
/// been charged without leaving a task is submitted again.
///
/// # Safety
/// `session` must be a live project-session handle. A non-empty `note` or
/// `branch` must be readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_generate_videos(
    session: *const KinetoProjectSession,
    shot_index: u32,
    count: u32,
    note_ptr: *const u8,
    note_len: u64,
    branch_ptr: *const u8,
    branch_len: u64,
    resubmit: bool,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(count) = u8::try_from(count)
            .ok()
            .filter(|count| (1..=MAX_VIDEO_CANDIDATES).contains(count))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller contract.
        let note = match unsafe { optional_text(note_ptr, note_len) } {
            Ok(note) => note.filter(|note| !note.trim().is_empty()),
            Err(code) => return code,
        };
        // SAFETY: forwarded caller contract.
        let branch_from = match unsafe { optional_text(branch_ptr, branch_len) } {
            Ok(branch) => branch,
            Err(code) => return code,
        };
        let ask = VideoAsk {
            count,
            note,
            branch_from,
        };
        let command = if resubmit {
            ShotCommand::ResubmitVideos(ask)
        } else {
            ShotCommand::GenerateVideos(ask)
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit_shot(session, shot_index, command, token) }
    })
}

/// Queue choosing video `video` of the latest generation of shot
/// `shot_index` to lock.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_select_video(
    session: *const KinetoProjectSession,
    shot_index: u32,
    video: u32,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Ok(video) = usize::try_from(video) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit_shot(session, shot_index, ShotCommand::SelectVideo(video), token) }
    })
}

/// How many of `what` (a contract `video_take_count` code) shot
/// `shot_index` has: every video it has had, or the notes or stale reasons
/// of take `take`. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_count` writable
/// storage for one `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_video_take_count(
    session: *const KinetoProjectSession,
    what: u32,
    shot_index: u32,
    take: u32,
    out_count: *mut u32,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_count.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let Some(session) = (unsafe { session_ref(session) }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let history = match session.video_history(shot_index) {
            Ok(history) => history,
            Err(error) => return crate::project_session::snapshot_error_code(&error),
        };
        let found = match name(&VIDEO_TAKE_COUNTS, what) {
            Some("video_history") => Some(history.len()),
            Some("video_notes") => at(&history, take).map(|(take, _)| take.notes.len()),
            Some("video_stale") => at(&history, take).map(|(take, _)| take.stale.len()),
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        let Some(found) = found else {
            return PROJECT_ERR_INVALID_CANDIDATE;
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_count.write(u32::try_from(found).unwrap_or(u32::MAX)) };
        PROJECT_OK
    })
}

/// What one take of shot `shot_index`'s next video costs: the clip length
/// its plan asks for, as the session's video provider renders it, in
/// `out_seconds` (0 when the provider declares none), and that clip's price
/// in USD micros in `out_micros` (`u64::MAX` when unknown). Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle; `out_seconds` and
/// `out_micros` writable storage for one `u32` and one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_video_estimate(
    session: *const KinetoProjectSession,
    shot_index: u32,
    out_seconds: *mut u32,
    out_micros: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_seconds.is_null() || out_micros.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let Some(session) = (unsafe { session_ref(session) }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let estimate = match session.video_estimate(shot_index) {
            Ok(estimate) => estimate,
            Err(error) => return crate::project_session::snapshot_error_code(&error),
        };
        // SAFETY: checked non-null; the caller guarantees both are writable.
        unsafe {
            out_seconds.write(estimate.map_or(0, |estimate| u32::from(estimate.clip_seconds)));
            out_micros.write(
                estimate
                    .and_then(|estimate| estimate.cost_micros)
                    .unwrap_or(u64::MAX),
            );
        }
        PROJECT_OK
    })
}

/// Number field `field` (a contract `video_take_value` code) of take
/// `take` of shot `shot_index`'s videos. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_value` writable
/// storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_video_take_value(
    session: *const KinetoProjectSession,
    field: u32,
    shot_index: u32,
    take: u32,
    out_value: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_value.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let Some(session) = (unsafe { session_ref(session) }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let history = match session.video_history(shot_index) {
            Ok(history) => history,
            Err(error) => return crate::project_session::snapshot_error_code(&error),
        };
        let Some((record, _)) = at(&history, take) else {
            return PROJECT_ERR_INVALID_CANDIDATE;
        };
        let found = match name(&VIDEO_TAKE_VALUES, field) {
            Some("video_generation") => u64::from(record.generation),
            Some("video_candidate") => record
                .candidate
                .map_or(0, |candidate| u64::from(candidate) + 1),
            Some("video_status") => u64::from(code(&TAKE_STATUSES, status_name(record.status))),
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_value.write(found) };
        PROJECT_OK
    })
}

/// Copy text field `field` (a contract `video_take_text` code) of take
/// `take` of shot `shot_index`'s videos: its ID, media (an absolute path,
/// empty when its bytes were let go), provider, model or the take it was
/// branched from; or, for a note or stale reason, the one at `item`. Same
/// sizing contract as project metadata. Never does I/O.
///
/// # Safety
/// `session` must be live. `out_len` must point to writable `u64` storage. If
/// `out_cap` is non-zero, `out_buf` must be writable for at least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_video_take_text_copy(
    session: *const KinetoProjectSession,
    field: u32,
    shot_index: u32,
    take: u32,
    item: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let Some(session) = (unsafe { session_ref(session) }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let history = match session.video_history(shot_index) {
            Ok(history) => history,
            Err(error) => return crate::project_session::snapshot_error_code(&error),
        };
        let Some((record, media)) = at(&history, take) else {
            return PROJECT_ERR_INVALID_CANDIDATE;
        };
        let found = match name(&VIDEO_TAKE_TEXTS, field) {
            Some("video_id") => Some(record.artifact_id.clone()),
            Some("video_media") => Some(
                media
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            Some("video_provider") => Some(record.origin.provider.clone()),
            Some("video_model") => Some(record.origin.model.clone()),
            Some("video_branched_from") => Some(record.branched_from.clone().unwrap_or_default()),
            Some("video_note") => at(&record.notes, item).cloned(),
            Some("video_stale_reason") => at(&record.stale, item).cloned(),
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        let Some(found) = found else {
            return PROJECT_ERR_INVALID_CANDIDATE;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(found.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// How many scenes have shots in play (`scene` is `u32::MAX`), or how many
/// shots scene `scene` has. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_count` writable
/// storage for one `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_progress_count(
    session: *const KinetoProjectSession,
    scene: u32,
    out_count: *mut u32,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_count.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let workflow = match unsafe { workflow(session) } {
            Ok(workflow) => workflow,
            Err(code) => return code,
        };
        let found = if scene == u32::MAX {
            Some(workflow.scenes.len())
        } else {
            at(&workflow.scenes, scene).map(|scene| scene.shots.len())
        };
        let Some(found) = found else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_count.write(u32::try_from(found).unwrap_or(u32::MAX)) };
        PROJECT_OK
    })
}

/// Number field `field` (a contract `progress_value` code) of scene
/// `scene`, or of its shot `shot`: whether the scene is complete, or where
/// the shot's keyframe or video stands as a `stage` code. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_value` writable
/// storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_progress_value(
    session: *const KinetoProjectSession,
    field: u32,
    scene: u32,
    shot: u32,
    out_value: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_value.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let workflow = match unsafe { workflow(session) } {
            Ok(workflow) => workflow,
            Err(code) => return code,
        };
        let Some(scene) = at(&workflow.scenes, scene) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let found = match name(&PROGRESS_VALUES, field) {
            Some("scene_complete") => Some(u64::from(scene.complete())),
            Some("keyframe_stage") => {
                at(&scene.shots, shot).map(|shot| u64::from(stage_code(shot.keyframe)))
            }
            Some("video_stage") => {
                at(&scene.shots, shot).map(|shot| u64::from(stage_code(shot.video)))
            }
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        let Some(found) = found else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_value.write(found) };
        PROJECT_OK
    })
}

/// Copy scene `scene`'s ID, or the ID of its shot `shot`. Same sizing
/// contract as project metadata. Never does I/O.
///
/// # Safety
/// `session` must be live. `out_len` must point to writable `u64` storage. If
/// `out_cap` is non-zero, `out_buf` must be writable for at least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_progress_text_copy(
    session: *const KinetoProjectSession,
    field: u32,
    scene: u32,
    shot: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let workflow = match unsafe { workflow(session) } {
            Ok(workflow) => workflow,
            Err(code) => return code,
        };
        let Some(scene) = at(&workflow.scenes, scene) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let found = match name(&PROGRESS_TEXTS, field) {
            Some("scene_id") => Some(scene.scene_id.clone()),
            Some("shot_id") => at(&scene.shots, shot).map(|shot| shot.shot_id.clone()),
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        let Some(found) = found else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(found.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Queue storage command `command` (a contract `storage_command` code): a
/// dry run of what collecting lets go, collecting it, or reading every kept
/// file against its record.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_storage(
    session: *const KinetoProjectSession,
    command: u32,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let command = match name(&STORAGE_COMMANDS, command) {
            Some("plan_collection") => StorageCommand::PlanCollection,
            Some("collect") => StorageCommand::Collect,
            Some("verify") => StorageCommand::Verify,
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        // SAFETY: forwarded caller contract.
        let Some(session) = (unsafe { session_ref(session) }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        submitted(session.submit_storage(command, token))
    })
}

/// Number field `field` (a contract `media_value` code) of the project's
/// media as last checked. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_value` writable
/// storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_media_value(
    session: *const KinetoProjectSession,
    field: u32,
    out_value: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_value.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let media = match unsafe { media(session) } {
            Ok(media) => media,
            Err(code) => return code,
        };
        let found = match name(&MEDIA_VALUES, field) {
            Some("usage_bytes") => media.usage.bytes,
            Some("recent_takes") => u64::try_from(media.recent).unwrap_or(u64::MAX),
            Some("planned") => u64::from(media.plan.is_some()),
            Some("plan_bytes") => media.plan.as_ref().map_or(0, |plan| plan.bytes),
            Some("media_damaged") => u64::from(media.problem.is_some()),
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_value.write(found) };
        PROJECT_OK
    })
}

/// How many items list `list` (a contract `media_list` code) has: the dry
/// run's files, the missing or mismatched ones, or the owners of media.
/// Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_count` writable
/// storage for one `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_media_count(
    session: *const KinetoProjectSession,
    list: u32,
    out_count: *mut u32,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_count.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let media = match unsafe { media(session) } {
            Ok(media) => media,
            Err(code) => return code,
        };
        let found = match name(&MEDIA_LISTS, list) {
            Some("plan") => media.plan.as_ref().map_or(0, |plan| plan.items.len()),
            Some("missing") => media.missing.len(),
            Some("mismatched") => media.mismatched.len(),
            Some("owners") => media.usage.by_owner.len(),
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_count.write(u32::try_from(found).unwrap_or(u32::MAX)) };
        PROJECT_OK
    })
}

/// The bytes of owner `index` of the `owners` list. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_value` writable
/// storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_media_owner_bytes(
    session: *const KinetoProjectSession,
    index: u32,
    out_value: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_value.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let media = match unsafe { media(session) } {
            Ok(media) => media,
            Err(code) => return code,
        };
        let Some((_, bytes)) = at(&media.usage.by_owner, index) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_value.write(*bytes) };
        PROJECT_OK
    })
}

/// Copy text field `field` (a contract `media_text` code) of item `index` of
/// list `list`: its artifact ID, its owner (`shot <scene>/<shot>`,
/// `character <id>` or `style`), or why the dry run lets it go; or, for
/// `media_problem`, why the media could not be listed. Same sizing contract
/// as project metadata. Never does I/O.
///
/// # Safety
/// `session` must be live. `out_len` must point to writable `u64` storage. If
/// `out_cap` is non-zero, `out_buf` must be writable for at least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_media_text_copy(
    session: *const KinetoProjectSession,
    list: u32,
    index: u32,
    field: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let media = match unsafe { media(session) } {
            Ok(media) => media,
            Err(code) => return code,
        };
        let field = name(&MEDIA_TEXTS, field);
        let found = if field == Some("media_problem") {
            Some(media.problem.clone().unwrap_or_default())
        } else {
            let (record, reason): (Option<&MediaRecord>, Option<&str>) =
                match name(&MEDIA_LISTS, list) {
                    Some("plan") => media
                        .plan
                        .as_ref()
                        .and_then(|plan| at(&plan.items, index))
                        .map_or((None, None), |item| {
                            (Some(&item.record), Some(item.reason.as_str()))
                        }),
                    Some("missing") => (at(&media.missing, index), None),
                    Some("mismatched") => (at(&media.mismatched, index), None),
                    Some("owners") => {
                        let owner = at(&media.usage.by_owner, index).map(|(owner, _)| owner);
                        let found = match field {
                            Some("media_owner") => owner.map(owner_label),
                            _ => return PROJECT_ERR_INVALID_ARGUMENT,
                        };
                        let Some(found) = found else {
                            return PROJECT_ERR_INVALID_ARGUMENT;
                        };
                        // SAFETY: forwarded caller-owned output contract.
                        return unsafe { copy_utf8(found.as_bytes(), out_buf, out_cap, out_len) };
                    }
                    _ => return PROJECT_ERR_INVALID_ARGUMENT,
                };
            let Some(record) = record else {
                return PROJECT_ERR_INVALID_ARGUMENT;
            };
            match field {
                Some("media_artifact") => Some(record.artifact_id.clone()),
                Some("media_owner") => Some(owner_label(&record.owner)),
                Some("media_reason") => Some(reason.unwrap_or_default().to_owned()),
                _ => None,
            }
        };
        let Some(found) = found else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(found.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Queue writing a production package named `name` in folder `destination`
/// (an absolute path), with only the ready shots when `partial`, replacing
/// one of the same name when `replace`.
///
/// # Safety
/// `session` must be a live project-session handle. `destination` and
/// `name` must be readable for their lengths and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_export(
    session: *const KinetoProjectSession,
    destination_ptr: *const u8,
    destination_len: u64,
    name_ptr: *const u8,
    name_len: u64,
    partial: bool,
    replace: bool,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let destination = match unsafe { utf8_owned(destination_ptr, destination_len) } {
            Ok(destination) if !destination.is_empty() => destination,
            Ok(_) => return PROJECT_ERR_INVALID_ARGUMENT,
            Err(code) => return code,
        };
        // SAFETY: forwarded caller contract.
        let package = match unsafe { utf8_owned(name_ptr, name_len) } {
            Ok(package) => package,
            Err(code) => return code,
        };
        // SAFETY: forwarded caller contract.
        let Some(session) = (unsafe { session_ref(session) }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        submitted(session.submit_export(
            ExportRequest {
                destination: destination.into(),
                name: package,
                partial,
                replace,
            },
            token,
        ))
    })
}

/// Number field `field` (a contract `export_value` code): how many shots in
/// play are ready or missing and whether all are ready, and what the
/// session's last package held. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_value` writable
/// storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_export_value(
    session: *const KinetoProjectSession,
    field: u32,
    out_value: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_value.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let workflow = match unsafe { workflow(session) } {
            Ok(workflow) => workflow,
            Err(code) => return code,
        };
        let count = |count: usize| u64::try_from(count).unwrap_or(u64::MAX);
        let last = workflow.last_export.as_ref();
        let found = match name(&EXPORT_VALUES, field) {
            Some("export_ready") => count(workflow.export.ready.len()),
            Some("export_missing") => count(workflow.export.missing.len()),
            Some("export_complete") => u64::from(workflow.export.complete()),
            Some("last_export") => u64::from(last.is_some()),
            Some("last_shots") => last.map_or(0, |last| count(last.shots)),
            Some("last_complete") => u64::from(last.is_some_and(|last| last.complete)),
            Some("last_bytes") => last.map_or(0, |last| last.bytes),
            Some("last_lines") => last.map_or(0, |last| count(last.lines)),
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_value.write(found) };
        PROJECT_OK
    })
}

/// Copy text field `field` (a contract `export_text` code): the scene, shot
/// or reason of missing shot `index`, or the last package's folder. Same
/// sizing contract as project metadata. Never does I/O.
///
/// # Safety
/// `session` must be live. `out_len` must point to writable `u64` storage. If
/// `out_cap` is non-zero, `out_buf` must be writable for at least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_export_text_copy(
    session: *const KinetoProjectSession,
    field: u32,
    index: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let workflow = match unsafe { workflow(session) } {
            Ok(workflow) => workflow,
            Err(code) => return code,
        };
        let missing = at(&workflow.export.missing, index);
        let found = match name(&EXPORT_TEXTS, field) {
            Some("missing_scene") => missing.map(|missing| missing.shot.scene().to_owned()),
            Some("missing_shot") => missing.map(|missing| missing.shot.shot().to_owned()),
            Some("missing_reason") => missing.map(|missing| missing.reason.clone()),
            Some("last_path") => Some(
                workflow
                    .last_export
                    .as_ref()
                    .map(|last| last.path.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        let Some(found) = found else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(found.as_bytes(), out_buf, out_cap, out_len) }
    })
}

fn submitted(result: Result<(), SubmitError>) -> i32 {
    match result {
        Ok(()) => PROJECT_OK,
        Err(SubmitError::Closed | SubmitError::InvalidShot) => PROJECT_ERR_NOT_OPEN,
    }
}

/// # Safety
/// `session` must be null or a live project-session handle.
unsafe fn workflow(
    session: *const KinetoProjectSession,
) -> Result<std::sync::Arc<kineto_core::session::PublishedWorkflow>, i32> {
    // SAFETY: forwarded caller contract.
    let session: &ProjectSession =
        unsafe { session_ref(session) }.ok_or(PROJECT_ERR_INVALID_ARGUMENT)?;
    session
        .workflow()
        .map_err(|error| crate::project_session::snapshot_error_code(&error))
}

/// # Safety
/// `session` must be null or a live project-session handle.
unsafe fn media(
    session: *const KinetoProjectSession,
) -> Result<std::sync::Arc<PublishedMedia>, i32> {
    // SAFETY: forwarded caller contract.
    let session: &ProjectSession =
        unsafe { session_ref(session) }.ok_or(PROJECT_ERR_INVALID_ARGUMENT)?;
    session
        .media()
        .map_err(|error| crate::project_session::snapshot_error_code(&error))
}

fn owner_label(owner: &MediaOwner) -> String {
    match owner {
        MediaOwner::Shot(key) => format!("shot {}/{}", key.scene(), key.shot()),
        MediaOwner::Character(id) => format!("character {id}"),
        MediaOwner::Style => "style".to_owned(),
        MediaOwner::Scene(id) => format!("scene {id}"),
    }
}

fn stage_code(stage: Stage) -> u32 {
    code(
        &STAGES,
        match stage {
            Stage::Missing => "missing",
            Stage::Made => "made",
            Stage::Locked => "locked",
            Stage::Stale => "stale",
        },
    )
}

fn status_name(status: ArtifactStatus) -> &'static str {
    match status {
        ArtifactStatus::Draft => "draft",
        ArtifactStatus::Candidate => "candidate",
        ArtifactStatus::Selected => "selected",
        ArtifactStatus::Locked => "locked",
        ArtifactStatus::Superseded => "superseded",
    }
}
