//! The shot plan, the recipe's steps and each shot's takes across the C ABI.
//!
//! Commands queue on the session worker like document commands and answer
//! with a `COMPLETED` message whose shot is -1. Reads return what the worker
//! last published, one field at a time: counts, numbers and UTF-8 text, each
//! selected by a code the contract table publishes. For the plan, `group`
//! chooses the scene (or the recipe step), `item` the shot and `sub` an
//! element of it; for takes, the shot index and the take. No record crosses
//! as JSON; an edited scene's shots arrive as a typed array.

use std::{slice, sync::Arc};

use kineto_core::{
    generation::Consent,
    session::{DocumentCommand, PublishedDocuments, PublishedWorkflow, ShotCommand},
    workflow::StepState,
};
use kineto_project::{
    ArtifactStatus,
    revisions::{Origin, RevisionStatus},
    shot::ShotDirection,
    shot_plan::{CameraMotion, Framing, PlannedShot, ScenePlan, ShotPlan, StaleCause},
};

use crate::{
    contract::*,
    documents::{at, optional_text, published, revision_bits, session_ref, submit},
    guarded,
    production::KinetoText,
    project_session::{KinetoProjectSession, copy_utf8, submit_shot, utf8_owned},
};

/// The most shots one saved scene may have.
const MAX_SHOTS: u64 = 500;

/// The most entries one list of a shot may have.
const MAX_LIST: u64 = 1_000;

/// One shot of an edited scene, as Dart lays it out. An empty
/// `continuity_from` means none.
#[repr(C)]
pub struct KinetoPlannedShot {
    pub id_ptr: *const u8,
    pub id_len: u64,
    pub purpose_ptr: *const u8,
    pub purpose_len: u64,
    pub direction: u32,
    pub framing: u32,
    pub lens_ptr: *const u8,
    pub lens_len: u64,
    pub camera_motion: u32,
    pub action_ptr: *const u8,
    pub action_len: u64,
    pub dialogue_ptr: *const u8,
    pub dialogue_len: u64,
    pub emotion_ptr: *const u8,
    pub emotion_len: u64,
    pub duration_target: f64,
    pub characters: *const KinetoText,
    pub character_count: u64,
    pub continuity: *const KinetoText,
    pub continuity_count: u64,
    pub continuity_from_ptr: *const u8,
    pub continuity_from_len: u64,
}

/// Queue planning the shots of the locked screenplay with the session's
/// language model; `resubmit` as for `kineto_project_analyze_story`.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_plan_shots(
    session: *const KinetoProjectSession,
    resubmit: bool,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(session, DocumentCommand::PlanShots, resubmit, token)
    })
}

/// Queue planning scene `scene_id` of the latest plan again, as `direction`
/// (which may be empty) asks.
///
/// # Safety
/// `session` must be a live project-session handle. Each non-empty input must
/// be readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_replan_scene(
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
        let (scene_id, direction) = match unsafe {
            (
                utf8_owned(scene_id_ptr, scene_id_len),
                optional_text(direction_ptr, direction_len),
            )
        } {
            (Ok(scene_id), Ok(direction)) if !scene_id.is_empty() => (scene_id, direction),
            (Ok(_), Ok(_)) => return PROJECT_ERR_INVALID_ARGUMENT,
            (Err(code), _) | (_, Err(code)) => return code,
        };
        let command = DocumentCommand::ReplanScene {
            scene_id,
            direction,
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit(session, command, resubmit, token) }
    })
}

/// # Safety
/// When `count` is non-zero, `texts` must be readable for that many texts,
/// each readable for its length.
unsafe fn texts(texts: *const KinetoText, count: u64) -> Result<Vec<String>, i32> {
    if count > MAX_LIST || (count > 0 && texts.is_null()) {
        return Err(PROJECT_ERR_INVALID_ARGUMENT);
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    // SAFETY: the caller guarantees `count` readable texts.
    let raw = unsafe { slice::from_raw_parts(texts, count as usize) };
    raw.iter()
        // SAFETY: forwarded caller contract for each text.
        .map(|text| unsafe { utf8_owned(text.ptr, text.len) })
        .collect()
}

/// # Safety
/// Every pointer of `shot` must follow [`kineto_project_save_scene_plan`]'s
/// contract.
unsafe fn planned_shot(shot: &KinetoPlannedShot) -> Result<PlannedShot, i32> {
    let read = |ptr, len| {
        // SAFETY: forwarded caller contract.
        unsafe { utf8_owned(ptr, len) }
    };
    let direction = name(&directions(), shot.direction)
        .and_then(|name| {
            [
                ShotDirection::Reaction,
                ShotDirection::SpatialClarity,
                ShotDirection::Intimacy,
                ShotDirection::Tension,
            ]
            .into_iter()
            .find(|direction| direction_name(*direction) == name)
        })
        .ok_or(PROJECT_ERR_INVALID_ARGUMENT)?;
    let framing = name(&FRAMINGS, shot.framing)
        .and_then(framing)
        .ok_or(PROJECT_ERR_INVALID_ARGUMENT)?;
    let camera_motion = name(&CAMERA_MOTIONS, shot.camera_motion)
        .and_then(camera_motion)
        .ok_or(PROJECT_ERR_INVALID_ARGUMENT)?;
    let continuity_from = read(shot.continuity_from_ptr, shot.continuity_from_len)?;
    Ok(PlannedShot {
        id: read(shot.id_ptr, shot.id_len)?,
        purpose: read(shot.purpose_ptr, shot.purpose_len)?,
        direction,
        framing,
        lens: read(shot.lens_ptr, shot.lens_len)?,
        camera_motion,
        action: read(shot.action_ptr, shot.action_len)?,
        dialogue: read(shot.dialogue_ptr, shot.dialogue_len)?,
        emotion: read(shot.emotion_ptr, shot.emotion_len)?,
        duration_target: shot.duration_target,
        // SAFETY: forwarded caller contract.
        characters: unsafe { texts(shot.characters, shot.character_count) }?,
        // SAFETY: forwarded caller contract.
        continuity: unsafe { texts(shot.continuity, shot.continuity_count) }?,
        continuity_from: Some(continuity_from).filter(|from| !from.is_empty()),
    })
}

/// Queue saving the director's edit of scene `scene_id`'s shots of plan
/// revision `based_on`, which must be the latest. `direction`, `framing` and
/// `camera_motion` are contract codes; the edit is held to the plan's rules
/// and to the locked screenplay and bible.
///
/// # Safety
/// `session` must be a live project-session handle. Each non-empty text must
/// be readable for its length and hold UTF-8. When `shot_count` is non-zero,
/// `shots` must be readable for that many shots, each with readable texts and
/// lists of their stated lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_save_scene_plan(
    session: *const KinetoProjectSession,
    based_on: u32,
    scene_id_ptr: *const u8,
    scene_id_len: u64,
    shots: *const KinetoPlannedShot,
    shot_count: u64,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let scene_id = match unsafe { utf8_owned(scene_id_ptr, scene_id_len) } {
            Ok(id) if !id.is_empty() => id,
            Ok(_) => return PROJECT_ERR_INVALID_ARGUMENT,
            Err(code) => return code,
        };
        if shot_count > MAX_SHOTS || (shot_count > 0 && shots.is_null()) {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        let raw = if shot_count == 0 {
            &[][..]
        } else {
            // SAFETY: the caller guarantees `shot_count` readable shots.
            unsafe { slice::from_raw_parts(shots, shot_count as usize) }
        };
        let mut planned = Vec::with_capacity(raw.len());
        for shot in raw {
            // SAFETY: forwarded caller contract for each shot.
            match unsafe { planned_shot(shot) } {
                Ok(shot) => planned.push(shot),
                Err(code) => return code,
            }
        }
        let command = DocumentCommand::SaveScenePlan {
            based_on,
            scene: ScenePlan {
                scene_id,
                shots: planned,
            },
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit(session, command, false, token) }
    })
}

/// Queue locking plan revision `revision`: its shots become the ones in
/// play, written into the project.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_lock_shot_plan(
    session: *const KinetoProjectSession,
    revision: u32,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(
            session,
            DocumentCommand::LockShotPlan { revision },
            false,
            token,
        )
    })
}

/// Queue generating shot `shot_index`'s keyframes again as `note` asks,
/// with what the director accepted (`risk_paying_twice`, `identity_drift`, as
/// for `kineto_project_shot_generate_consenting`).
///
/// # Safety
/// `session` must be a live project-session handle. A non-empty `note` must be
/// readable for its length and hold UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_refine(
    session: *const KinetoProjectSession,
    shot_index: u32,
    note_ptr: *const u8,
    note_len: u64,
    risk_paying_twice: bool,
    identity_drift: bool,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let note = match unsafe { utf8_owned(note_ptr, note_len) } {
            Ok(note) if !note.trim().is_empty() => note,
            Ok(_) => return PROJECT_ERR_INVALID_ARGUMENT,
            Err(code) => return code,
        };
        let command = ShotCommand::Refine {
            note,
            consent: Consent {
                risk_paying_twice,
                identity_drift,
            },
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit_shot(session, shot_index, command, token) }
    })
}

/// What the worker last published: the documents (for the plan) and the
/// recipe's steps.
struct Published {
    documents: Arc<PublishedDocuments>,
    workflow: Arc<PublishedWorkflow>,
}

/// # Safety
/// `session` must be null or a live project-session handle.
unsafe fn read(session: *const KinetoProjectSession) -> Result<Published, i32> {
    // SAFETY: forwarded caller contract.
    let documents = unsafe { published(session) }?;
    // SAFETY: forwarded caller contract.
    let handle = unsafe { session_ref(session) }.ok_or(PROJECT_ERR_INVALID_ARGUMENT)?;
    let workflow = handle.workflow().map_err(|_| PROJECT_ERR_NOT_OPEN)?;
    Ok(Published {
        documents,
        workflow,
    })
}

fn plan(published: &Published) -> Option<&ShotPlan> {
    published.documents.shot_plan.as_ref().map(|(_, plan)| plan)
}

/// Shot `item` of scene `group` of the plan in play.
fn planned(published: &Published, group: u32, item: u32) -> Option<&PlannedShot> {
    at(&at(&plan(published)?.scenes, group)?.shots, item)
}

/// The plan and recipe state as last published, packed as the contract
/// table's plan fields describe. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_state` writable
/// storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_plan_state(
    session: *const KinetoProjectSession,
    out_state: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_state.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let published = match unsafe { read(session) } {
            Ok(published) => published,
            Err(code) => return code,
        };
        let documents = &published.documents;
        let bits = pack(&[
            (
                PLAN_REVISION,
                revision_bits(documents.shot_plan.as_ref().map(|(revision, _)| *revision)),
            ),
            (
                PLAN_LOCKED_REVISION,
                revision_bits(documents.shot_plan_locked),
            ),
            (PLAN_STALE, u64::from(!documents.shot_plan_stale.is_empty())),
            (
                WORKFLOW_UNKNOWN,
                u64::from(published.workflow.problem.is_some()),
            ),
        ]);
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_state.write(bits) };
        PROJECT_OK
    })
}

fn count(published: &Published, what: &str, group: u32, item: u32) -> Option<usize> {
    let steps = &published.workflow.steps;
    Some(match what {
        "plan_revisions" => published.documents.shot_plan_revisions.len(),
        "plan_scenes" => plan(published).map_or(0, |plan| plan.scenes.len()),
        "plan_shots" => at(&plan(published)?.scenes, group)?.shots.len(),
        "shot_characters" => planned(published, group, item)?.characters.len(),
        "shot_continuity" => planned(published, group, item)?.continuity.len(),
        "plan_stale_causes" => published.documents.shot_plan_stale.len(),
        "workflow_steps" => steps.len(),
        "step_waiting" => at(steps, group)?.waiting_on.len(),
        "step_stale" => at(steps, group)?.stale.len(),
        "step_skipped" => at(steps, group)?.skipped_when.len(),
        _ => return None,
    })
}

/// How many of `what` (a contract `plan_count` code) the published plan and
/// recipe hold; `group` and `item` select the scene and shot, or the step.
/// Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_count` writable
/// storage for one `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_plan_count(
    session: *const KinetoProjectSession,
    what: u32,
    group: u32,
    item: u32,
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
        let Some(found) = name(&PLAN_COUNTS, what)
            .and_then(|what| count(&published, what, group, item))
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
        StaleCause::NotLocked => ("not_locked", ""),
        StaleCause::SceneChanged(id) => ("scene_changed", id),
        StaleCause::SceneRemoved(id) => ("scene_removed", id),
        StaleCause::SceneAdded(id) => ("scene_added", id),
        StaleCause::Entry(id) => ("entry", id),
    }
}

fn step_state(state: StepState) -> &'static str {
    match state {
        StepState::Waiting => "waiting",
        StepState::Ready => "ready",
        StepState::AwaitingApproval => "awaiting_approval",
        StepState::Done => "done",
        StepState::Skipped => "skipped",
    }
}

fn value(published: &Published, field: &str, group: u32, item: u32) -> Option<u64> {
    let revision = || at(&published.documents.shot_plan_revisions, item);
    let shot = || planned(published, group, item);
    let step = || at(&published.workflow.steps, group);
    Some(match field {
        "plan_revision_number" => u64::from(revision()?.revision),
        "plan_revision_status" => u64::from(code(
            &REVISION_STATUSES,
            match revision()?.status {
                RevisionStatus::Candidate => "candidate",
                RevisionStatus::Locked => "locked",
                RevisionStatus::Superseded => "superseded",
            },
        )),
        "plan_revision_origin" => u64::from(code(
            &ORIGINS,
            match revision()?.origin {
                Origin::Generated => "generated",
                Origin::Edited => "edited",
            },
        )),
        "plan_revision_based_on" => u64::from(revision()?.based_on.unwrap_or(0)),
        "plan_stale_cause" => u64::from(code(
            &PLAN_STALE_CAUSES,
            stale_cause(at(&published.documents.shot_plan_stale, item)?).0,
        )),
        "shot_direction" => u64::from(shot()?.direction.code()),
        "shot_framing" => u64::from(code(&FRAMINGS, shot()?.framing.as_str())),
        "shot_camera_motion" => u64::from(code(&CAMERA_MOTIONS, shot()?.camera_motion.as_str())),
        // Durations are 1–30 seconds, so milliseconds never overflow.
        "shot_duration_millis" => (shot()?.duration_target * 1000.0).round() as u64,
        "step_state" => u64::from(code(&STEP_STATES, step_state(step()?.state))),
        "step_approval" => u64::from(step()?.approval),
        _ => return None,
    })
}

/// Number field `field` (a contract `plan_value` code) of the published plan
/// and recipe; `group` and `item` select what it belongs to. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_value` writable
/// storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_plan_value(
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
            name(&PLAN_VALUES, field).and_then(|field| value(&published, field, group, item))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_value.write(found) };
        PROJECT_OK
    })
}

fn text(published: &Published, field: &str, group: u32, item: u32, sub: u32) -> Option<String> {
    let revision = || at(&published.documents.shot_plan_revisions, item);
    let shot = || planned(published, group, item);
    let step = || at(&published.workflow.steps, group);
    let owned = |text: &str| Some(text.to_owned());
    match field {
        "plan_revision_provider" => owned(revision()?.provider.as_deref().unwrap_or_default()),
        "plan_revision_model" => owned(revision()?.model.as_deref().unwrap_or_default()),
        "plan_stale_subject" => {
            owned(stale_cause(at(&published.documents.shot_plan_stale, item)?).1)
        }
        "scene_id" => owned(&at(&plan(published)?.scenes, group)?.scene_id),
        "shot_id" => owned(&shot()?.id),
        "shot_purpose" => owned(&shot()?.purpose),
        "shot_lens" => owned(&shot()?.lens),
        "shot_action" => owned(&shot()?.action),
        "shot_dialogue" => owned(&shot()?.dialogue),
        "shot_emotion" => owned(&shot()?.emotion),
        "shot_continuity_from" => owned(shot()?.continuity_from.as_deref().unwrap_or_default()),
        "shot_character" => owned(at(&shot()?.characters, sub)?),
        "shot_continuity" => owned(at(&shot()?.continuity, sub)?),
        "step_id" => owned(&step()?.id),
        "step_description" => owned(&step()?.description),
        "step_waiting" => owned(at(&step()?.waiting_on, item)?),
        "step_stale" => owned(at(&step()?.stale, item)?),
        "step_skipped" => Some(at(&step()?.skipped_when, item)?.as_str().to_owned()),
        "workflow_problem" => owned(published.workflow.problem.as_deref().unwrap_or_default()),
        _ => None,
    }
}

/// Copy text field `field` (a contract `plan_text` code) of the published
/// plan and recipe into caller-owned memory, with the sizing contract of
/// project metadata. An absent optional field copies as empty. Never does
/// I/O.
///
/// # Safety
/// `session` must be live. `out_len` must point to writable `u64` storage. If
/// `out_cap` is non-zero, `out_buf` must be writable for at least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_plan_text_copy(
    session: *const KinetoProjectSession,
    field: u32,
    group: u32,
    item: u32,
    sub: u32,
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
            name(&PLAN_TEXTS, field).and_then(|field| text(&published, field, group, item, sub))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(found.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// How many of `what` (a contract `take_count` code) shot `shot_index` has:
/// every take of its history, or the reasons take `take` is stale. Never
/// does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_count` writable
/// storage for one `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_take_count(
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
        let found = match name(&TAKE_COUNTS, what) {
            Some("history") => session
                .candidate_history(shot_index)
                .map(|history| Some(history.len())),
            Some("staleness") => session.candidate_staleness(shot_index).map(|staleness| {
                usize::try_from(take)
                    .ok()
                    .and_then(|take| staleness.get(take))
                    .map(Vec::len)
            }),
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        match found {
            Ok(Some(found)) => {
                // SAFETY: checked non-null; the caller guarantees it is writable.
                unsafe { out_count.write(u32::try_from(found).unwrap_or(u32::MAX)) };
                PROJECT_OK
            }
            Ok(None) => PROJECT_ERR_INVALID_CANDIDATE,
            Err(error) => crate::project_session::snapshot_error_code(&error),
        }
    })
}

/// Number field `field` (a contract `take_value` code) of take `take` of
/// shot `shot_index`'s history. Never does I/O.
///
/// # Safety
/// `session` must be a live project-session handle and `out_value` writable
/// storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_take_value(
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
        let history = match session.candidate_history(shot_index) {
            Ok(history) => history,
            Err(error) => return crate::project_session::snapshot_error_code(&error),
        };
        let Some((record, _)) = usize::try_from(take)
            .ok()
            .and_then(|take| history.get(take))
        else {
            return PROJECT_ERR_INVALID_CANDIDATE;
        };
        let found = match name(&TAKE_VALUES, field) {
            Some("take_generation") => u64::from(record.generation),
            Some("take_status") => u64::from(code(
                &TAKE_STATUSES,
                match record.status {
                    ArtifactStatus::Draft => "draft",
                    ArtifactStatus::Candidate => "candidate",
                    ArtifactStatus::Selected => "selected",
                    ArtifactStatus::Locked => "locked",
                    ArtifactStatus::Superseded => "superseded",
                },
            )),
            Some("take_refined") => u64::from(
                record
                    .applied
                    .iter()
                    .any(|applied| applied == "director.note"),
            ),
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_value.write(found) };
        PROJECT_OK
    })
}

/// Copy text field `field` (a contract `take_text` code) of take `take` of
/// shot `shot_index`: its ID, media (an absolute path), provider or model;
/// or, for `stale_reason`, reason `reason` the current take `take` is stale.
/// Same sizing contract as project metadata. Never does I/O.
///
/// # Safety
/// `session` must be live. `out_len` must point to writable `u64` storage. If
/// `out_cap` is non-zero, `out_buf` must be writable for at least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_take_text_copy(
    session: *const KinetoProjectSession,
    field: u32,
    shot_index: u32,
    take: u32,
    reason: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let Some(session) = (unsafe { session_ref(session) }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let take = usize::try_from(take).unwrap_or(usize::MAX);
        let found = match name(&TAKE_TEXTS, field) {
            Some("stale_reason") => match session.candidate_staleness(shot_index) {
                Ok(staleness) => staleness
                    .get(take)
                    .and_then(|reasons| reasons.get(usize::try_from(reason).ok()?))
                    .cloned(),
                Err(error) => return crate::project_session::snapshot_error_code(&error),
            },
            Some(field) => match session.candidate_history(shot_index) {
                Ok(history) => history.get(take).map(|(record, media)| match field {
                    "take_id" => record.artifact_id.clone(),
                    "take_media" => media
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    "take_provider" => record.origin.provider.clone(),
                    _ => record.origin.model.clone(),
                }),
                Err(error) => return crate::project_session::snapshot_error_code(&error),
            },
            None => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        let Some(found) = found else {
            return PROJECT_ERR_INVALID_CANDIDATE;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(found.as_bytes(), out_buf, out_cap, out_len) }
    })
}

fn framing(name: &str) -> Option<Framing> {
    [
        Framing::ExtremeWide,
        Framing::Wide,
        Framing::Full,
        Framing::MediumWide,
        Framing::Medium,
        Framing::MediumCloseUp,
        Framing::CloseUp,
        Framing::ExtremeCloseUp,
        Framing::OverTheShoulder,
        Framing::Insert,
    ]
    .into_iter()
    .find(|framing| framing.as_str() == name)
}

fn camera_motion(name: &str) -> Option<CameraMotion> {
    [
        CameraMotion::Static,
        CameraMotion::SlowDollyIn,
        CameraMotion::DollyIn,
        CameraMotion::SlowDollyOut,
        CameraMotion::DollyOut,
        CameraMotion::PanLeft,
        CameraMotion::PanRight,
        CameraMotion::TiltUp,
        CameraMotion::TiltDown,
        CameraMotion::Tracking,
        CameraMotion::Handheld,
    ]
    .into_iter()
    .find(|motion| motion.as_str() == name)
}

fn direction_name(direction: ShotDirection) -> &'static str {
    match direction {
        ShotDirection::Reaction => "reaction",
        ShotDirection::SpatialClarity => "spatial_clarity",
        ShotDirection::Intimacy => "intimacy",
        ShotDirection::Tension => "tension",
    }
}

/// Direction codes as the contract table publishes them.
fn directions() -> Vec<(&'static str, u32)> {
    [
        ShotDirection::Reaction,
        ShotDirection::SpatialClarity,
        ShotDirection::Intimacy,
        ShotDirection::Tension,
    ]
    .into_iter()
    .map(|direction| (direction_name(direction), u32::from(direction.code())))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_framing_and_camera_motion_crosses_by_name() {
        for (name, _) in FRAMINGS {
            assert_eq!(framing(name).map(Framing::as_str), Some(name));
        }
        for (name, _) in CAMERA_MOTIONS {
            assert_eq!(camera_motion(name).map(CameraMotion::as_str), Some(name));
        }
        for (name, code) in directions() {
            assert_eq!(
                ShotDirection::from_code(u8::try_from(code).unwrap()).map(direction_name),
                Some(name)
            );
        }
    }
}
