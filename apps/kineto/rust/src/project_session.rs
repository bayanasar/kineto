//! Canonical project sessions across the C ABI.
//!
//! A session runs on its own worker thread (`kineto_core::session`). Every call
//! that may touch the filesystem returns before doing so: open/create hand back
//! a handle at once, and each shot command is queued with a caller token. The
//! worker reports outcomes by posting `Int64List [kind, token, status, shot]`
//! messages to the Dart port supplied at open, through Dart's
//! `NativeApi.postCObject`. Posting to a closed port is defined behaviour, so a
//! Dart isolate that goes away first cannot be called into.
//!
//! Snapshot and metadata reads return state the worker has already published;
//! they never do I/O and never wait for a running command.

use std::{collections::BTreeMap, path::PathBuf, ptr, slice, str};

use crate::{contract::*, guarded};
use kineto_core::{
    generation::{GenerationError, ImageProvider, ImageProviderKind},
    keyframe_prompt::KeyframePromptError,
    session::{
        OpenRequest, ProjectSession, Recovery, SessionError, SessionSink, ShotCommand,
        SnapshotError, SubmitError,
    },
};
use kineto_jobs::ErrorClass;
use kineto_project::{
    fs::ProjectFsError,
    manifest::{
        PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectManifestError,
        ProjectSource, ProjectStoreError, ProjectWorkflow,
    },
    shot::{CandidateOrigin, ShotDirection, ShotWorkflowError},
};
use kineto_providers::secret::EnvSecretSource;

/// Message kinds posted to a session's Dart port.
const MESSAGE_OPENED: i64 = 1;
const MESSAGE_COMPLETED: i64 = 2;
/// `[RECOVERED, revision or record count, status, shot or -1]`, posted before
/// `OPENED` for each piece of unfinished work an earlier session left.
const MESSAGE_RECOVERED: i64 = 3;
/// `[VIDEO, video revision, status, shot]`, posted when a video generated in
/// the background was attached, or failed and was retired.
const MESSAGE_VIDEO: i64 = 4;

/// Dart's `NativeApi.postCObject` (`Dart_PostCObject`).
pub type PostCObject = unsafe extern "C" fn(port: i64, message: *mut DartCObject) -> bool;

// `Dart_CObject` from `dart_native_api.h`. Only the typed-data member is used;
// the padding member gives the union the header's size (40 bytes on 64-bit).
const DART_COBJECT_TYPED_DATA: i32 = 7;
const DART_TYPED_DATA_INT64: i32 = 8;

#[repr(C)]
pub struct DartCObject {
    kind: i32,
    value: DartCObjectValue,
}

#[repr(C)]
union DartCObjectValue {
    as_typed_data: DartTypedData,
    _size: [u64; 5],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DartTypedData {
    kind: i32,
    /// In elements, not bytes.
    length: isize,
    values: *const u8,
}

struct DartPort {
    post: PostCObject,
    port: i64,
}

impl DartPort {
    fn post(&self, message: [i64; 4]) {
        let mut object = DartCObject {
            kind: DART_COBJECT_TYPED_DATA,
            value: DartCObjectValue {
                as_typed_data: DartTypedData {
                    kind: DART_TYPED_DATA_INT64,
                    length: 4,
                    values: message.as_ptr().cast(),
                },
            },
        };
        // SAFETY: `post` is Dart's `NativeApi.postCObject`, which copies typed
        // data before returning, so `message` only needs to outlive the call.
        // A closed port makes it return false, which needs no handling.
        let _ = unsafe { (self.post)(self.port, &raw mut object) };
    }
}

impl SessionSink for DartPort {
    fn recovered(&self, recovery: &Recovery) {
        self.post(match recovery {
            Recovery::Completed {
                shot_index,
                revision,
            } => [
                MESSAGE_RECOVERED,
                i64::from(*revision),
                i64::from(PROJECT_OK),
                i64::from(*shot_index),
            ],
            Recovery::Failed {
                shot_index,
                revision,
                error,
            } => [
                MESSAGE_RECOVERED,
                i64::from(*revision),
                i64::from(session_error_code(error)),
                i64::from(*shot_index),
            ],
            Recovery::NeedsAttention { count } => [
                MESSAGE_RECOVERED,
                i64::try_from(*count).unwrap_or(i64::MAX),
                i64::from(PROJECT_ERR_GENERATION_JOB),
                -1,
            ],
        });
    }

    fn opened(&self, result: &Result<(), SessionError>) {
        self.post([MESSAGE_OPENED, 0, i64::from(session_status(result)), 0]);
    }

    fn completed(&self, token: u64, shot_index: u32, result: &Result<(), SessionError>) {
        self.post([
            MESSAGE_COMPLETED,
            i64::from_ne_bytes(token.to_ne_bytes()),
            i64::from(session_status(result)),
            i64::from(shot_index),
        ]);
    }

    fn video_finished(&self, shot_index: u32, revision: u16, result: &Result<(), SessionError>) {
        self.post([
            MESSAGE_VIDEO,
            i64::from(revision),
            i64::from(session_status(result)),
            i64::from(shot_index),
        ]);
    }
}

/// Opaque canonical-project handle. Dart never dereferences this allocation.
pub struct KinetoProjectSession {
    session: ProjectSession,
}

unsafe fn spawn(
    request: OpenRequest,
    post: Option<PostCObject>,
    port: i64,
    out_session: *mut *mut KinetoProjectSession,
) -> i32 {
    let Some(post) = post else {
        return PROJECT_ERR_INVALID_ARGUMENT;
    };
    let Ok(provider) = image_provider_from_environment() else {
        return PROJECT_ERR_PROVIDER_UNKNOWN;
    };
    match ProjectSession::spawn(request, provider, Box::new(DartPort { post, port })) {
        Ok(session) => {
            let session = Box::into_raw(Box::new(KinetoProjectSession { session }));
            // SAFETY: callers validated `out_session` as writable pointer storage.
            unsafe { ptr::write(out_session, session) };
            PROJECT_OK
        }
        Err(_) => PROJECT_ERR_IO,
    }
}

/// The environment variable that names the image provider a session uses.
/// Unset, empty or `local-demo` means the free demo generator; `minimax`
/// means MiniMax with the key in `KINETO_MINIMAX_API_KEY`. A paid provider is
/// used only when named here, never because a key happens to be set.
const IMAGE_PROVIDER_VARIABLE: &str = "KINETO_IMAGE_PROVIDER";

fn image_provider_from_environment() -> Result<ImageProvider, ()> {
    let selection = match std::env::var(IMAGE_PROVIDER_VARIABLE) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => return Err(()),
    };
    ImageProvider::select(selection.as_deref(), &EnvSecretSource::process()).map_err(|_| ())
}

/// Start opening an existing canonical Kineto project.
///
/// Returns at once. On `PROJECT_OK` a handle is written to `out_session` and
/// the open runs on the session worker; its result arrives on `port` as an
/// `OPENED` message. The handle must be destroyed whatever that result is.
///
/// # Safety
/// `out_session` must point to writable storage for one pointer. When
/// `path_len` is non-zero, `path_ptr` must point to at least `path_len` readable
/// bytes containing UTF-8. `post` must be Dart's `NativeApi.postCObject`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_open(
    path_ptr: *const u8,
    path_len: u64,
    post: Option<PostCObject>,
    port: i64,
    out_session: *mut *mut KinetoProjectSession,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_session.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: caller guarantees writable storage for one pointer.
        unsafe { ptr::write(out_session, ptr::null_mut()) };
        let path = match unsafe { utf8_owned(path_ptr, path_len) } {
            Ok(path) if !path.is_empty() => PathBuf::from(path),
            Ok(_) => return PROJECT_ERR_INVALID_ARGUMENT,
            Err(code) => return code,
        };
        // SAFETY: `out_session` was checked above.
        unsafe { spawn(OpenRequest::Open { path }, post, port, out_session) }
    })
}

/// Start creating a new canonical UTF-8 text-source project, then open it.
///
/// Input buffers are borrowed only for this call and must all be valid UTF-8;
/// they are validated and copied before it returns. `source_ptr` is written to
/// `source/story.txt`. The result arrives on `port` as an `OPENED` message.
///
/// # Safety
/// `out_session` must point to writable storage for one pointer. Each non-empty
/// input buffer must be valid for reads of its declared length for the call.
/// `post` must be Dart's `NativeApi.postCObject`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_create_text(
    path_ptr: *const u8,
    path_len: u64,
    project_id_ptr: *const u8,
    project_id_len: u64,
    title_ptr: *const u8,
    title_len: u64,
    created_at_ptr: *const u8,
    created_at_len: u64,
    language_ptr: *const u8,
    language_len: u64,
    source_ptr: *const u8,
    source_len: u64,
    post: Option<PostCObject>,
    port: i64,
    out_session: *mut *mut KinetoProjectSession,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_session.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: caller guarantees writable storage for one pointer.
        unsafe { ptr::write(out_session, ptr::null_mut()) };

        let path = match unsafe { utf8_owned(path_ptr, path_len) } {
            Ok(path) if !path.is_empty() => PathBuf::from(path),
            Ok(_) => return PROJECT_ERR_INVALID_ARGUMENT,
            Err(code) => return code,
        };
        let project_id = match unsafe { utf8_owned(project_id_ptr, project_id_len) } {
            Ok(value) => value,
            Err(code) => return code,
        };
        let title = match unsafe { utf8_owned(title_ptr, title_len) } {
            Ok(value) => value,
            Err(code) => return code,
        };
        let created_at = match unsafe { utf8_owned(created_at_ptr, created_at_len) } {
            Ok(value) => value,
            Err(code) => return code,
        };
        let language = match unsafe { utf8_owned(language_ptr, language_len) } {
            Ok(value) => value,
            Err(code) => return code,
        };
        let source = match unsafe { utf8_owned(source_ptr, source_len) } {
            Ok(value) => value.into_bytes(),
            Err(code) => return code,
        };

        let manifest = ProjectManifest {
            format_version: PROJECT_FORMAT_VERSION,
            project_id,
            title,
            created_at,
            source: ProjectSource {
                kind: "text".to_owned(),
                path: "source/story.txt".to_owned(),
                extra: BTreeMap::new(),
            },
            defaults: ProjectDefaults {
                language,
                extra: BTreeMap::new(),
            },
            workflow: ProjectWorkflow {
                recipe_id: "default-film".to_owned(),
                recipe_version: 1,
                extra: BTreeMap::new(),
            },
            extra: BTreeMap::new(),
        };
        let request = OpenRequest::CreateText {
            path,
            manifest: Box::new(manifest),
            source,
        };
        // SAFETY: `out_session` was checked above.
        unsafe { spawn(request, post, port, out_session) }
    })
}

/// Close a session. Stops message delivery before returning, discards queued
/// commands that have not started, waits for the running command (at most one
/// canonical write) and releases the project's writer lock.
///
/// # Safety
/// `session` must be null or a live pointer returned exactly once by project
/// open/create.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_destroy(session: *mut KinetoProjectSession) {
    if session.is_null() {
        return;
    }
    guarded((), || {
        // SAFETY: the caller contract requires ownership of this allocation.
        unsafe { drop(Box::from_raw(session)) };
    });
}

/// Project format version, or 0 until the open has succeeded.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_format_version(
    session: *const KinetoProjectSession,
) -> u32 {
    guarded(0, || {
        let Some(session) = (unsafe { session.as_ref() }) else {
            return 0;
        };
        session
            .session
            .metadata()
            .map_or(0, |metadata| metadata.format_version)
    })
}

/// 1 when the project is read-only or not yet open, 0 when it is writable.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_is_read_only(session: *const KinetoProjectSession) -> u8 {
    guarded(1, || {
        let Some(session) = (unsafe { session.as_ref() }) else {
            return 1;
        };
        session
            .session
            .metadata()
            .map_or(1, |metadata| u8::from(metadata.read_only))
    })
}

/// The session's image provider as a contract `image_provider` code: 1 for the
/// local demo generator, 2 for MiniMax; 0 for a null handle.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_image_provider(
    session: *const KinetoProjectSession,
) -> u32 {
    guarded(0, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(session) = (unsafe { session.as_ref() }) else {
            return 0;
        };
        image_provider_code(session.session.image_provider().kind)
    })
}

/// 1 when the session's image provider can generate, 0 when it cannot (a
/// paid provider without credentials) or for a null handle.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_image_provider_ready(
    session: *const KinetoProjectSession,
) -> u8 {
    guarded(0, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(session) = (unsafe { session.as_ref() }) else {
            return 0;
        };
        u8::from(session.session.image_provider().ready)
    })
}

/// Estimated cost of one shot generation with the session's image provider,
/// in USD micros; 0 for a free provider or a null handle.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_generation_cost_micros(
    session: *const KinetoProjectSession,
) -> u64 {
    guarded(0, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(session) = (unsafe { session.as_ref() }) else {
            return 0;
        };
        session.session.image_provider().generation_cost_micros
    })
}

/// Estimated cost of one shot video with the session's provider, in USD
/// micros; 0 when it cannot generate video, or for a null handle.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_video_cost_micros(
    session: *const KinetoProjectSession,
) -> u64 {
    guarded(0, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(session) = (unsafe { session.as_ref() }) else {
            return 0;
        };
        session.session.image_provider().video_cost_micros
    })
}

fn image_provider_code(kind: ImageProviderKind) -> u32 {
    match kind {
        ImageProviderKind::LocalDemo => IMAGE_PROVIDER_LOCAL_DEMO,
        ImageProviderKind::Minimax => IMAGE_PROVIDER_MINIMAX,
    }
}

/// Copy the UTF-8 project ID into caller-owned memory.
///
/// # Safety
/// `session` must be live. `out_len` must point to writable `u64` storage. If
/// `out_cap` is non-zero, `out_buf` must be writable for at least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_id_copy(
    session: *const KinetoProjectSession,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(session) = (unsafe { session.as_ref() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Some(metadata) = session.session.metadata() else {
            return PROJECT_ERR_NOT_OPEN;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(metadata.project_id.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Copy the UTF-8 project title into caller-owned memory.
///
/// # Safety
/// Same output-buffer contract as [`kineto_project_id_copy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_title_copy(
    session: *const KinetoProjectSession,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(session) = (unsafe { session.as_ref() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Some(metadata) = session.session.metadata() else {
            return PROJECT_ERR_NOT_OPEN;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(metadata.title.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Return one shot's most recently published approval state as a fixed-width
/// snapshot. Never does I/O and never waits for a running command.
///
/// Layout:
/// - bit 0: candidates generated
/// - bit 1: selection locked
/// - bit 2: current candidates are stale against current shot intent
/// - bit 3: a known legacy hash migration is pending explicit approval
/// - bit 4: one or more invalid/noncanonical stored hashes were diagnosed
/// - bits 8..15: candidate count
/// - bits 16..23: selected index + 1 (0 means none)
/// - bits 24..31: shot direction
/// - bits 32..47: current generation revision
/// - bits 48..63: superseded candidate count in this shot lineage
///
/// # Safety
/// `session` must be live and `out_state` must point to writable `u64` storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_state(
    session: *const KinetoProjectSession,
    shot_index: u32,
    out_state: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(session) = (unsafe { session.as_ref() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        if out_state.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        let in_flight = session.session.video_in_flight(shot_index);
        match session.session.snapshot(shot_index) {
            Ok(snapshot) => {
                let bits = encode_shot_snapshot(snapshot, in_flight.unwrap_or(false));
                // SAFETY: caller guarantees writable storage for one u64.
                unsafe { ptr::write(out_state, bits) };
                PROJECT_OK
            }
            Err(error) => snapshot_error_code(&error),
        }
    })
}

fn snapshot_error_code(error: &SnapshotError) -> i32 {
    match error {
        SnapshotError::InvalidShot => PROJECT_ERR_INVALID_SHOT,
        SnapshotError::InvalidCandidate => PROJECT_ERR_INVALID_CANDIDATE,
        SnapshotError::NotOpen => PROJECT_ERR_NOT_OPEN,
        SnapshotError::Failed(error) => shot_error_code(error),
    }
}

/// Copy the absolute path of a candidate's media into caller-owned memory,
/// with the same sizing contract as project metadata. A candidate without
/// media, such as a demo candidate, copies an empty path. Reads published
/// state only; never does I/O.
///
/// # Safety
/// `session` must be live. `out_len` must point to writable `u64` storage. If
/// `out_cap` is non-zero, `out_buf` must be writable for at least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_candidate_media_copy(
    session: *const KinetoProjectSession,
    shot_index: u32,
    candidate_index: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(session) = (unsafe { session.as_ref() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Ok(candidate_index) = usize::try_from(candidate_index) else {
            return PROJECT_ERR_INVALID_CANDIDATE;
        };
        let media = match session.session.candidate_media(shot_index, candidate_index) {
            Ok(media) => media,
            Err(error) => return snapshot_error_code(&error),
        };
        let path = match &media {
            None => "",
            Some(path) => match path.to_str() {
                Some(path) => path,
                None => return PROJECT_ERR_INVALID_UTF8,
            },
        };
        // SAFETY: the caller's sizing contract matches `copy_utf8`'s.
        unsafe { copy_utf8(path.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Copy the provider recorded on a candidate's artifact, for example
/// `minimax`, with the same sizing contract as project metadata. This is who
/// made the candidate, not the session's provider. Empty when the record names
/// none. Reads published state only; never does I/O.
///
/// # Safety
/// Same contract as [`kineto_project_candidate_media_copy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_candidate_provider_copy(
    session: *const KinetoProjectSession,
    shot_index: u32,
    candidate_index: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        unsafe {
            copy_candidate_origin(
                session,
                shot_index,
                candidate_index,
                |origin| origin.provider,
                out_buf,
                out_cap,
                out_len,
            )
        }
    })
}

/// Copy the model recorded on a candidate's artifact, for example
/// `image-01`. Otherwise as [`kineto_project_candidate_provider_copy`].
///
/// # Safety
/// Same contract as [`kineto_project_candidate_media_copy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_candidate_model_copy(
    session: *const KinetoProjectSession,
    shot_index: u32,
    candidate_index: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        unsafe {
            copy_candidate_origin(
                session,
                shot_index,
                candidate_index,
                |origin| origin.model,
                out_buf,
                out_cap,
                out_len,
            )
        }
    })
}

unsafe fn copy_candidate_origin(
    session: *const KinetoProjectSession,
    shot_index: u32,
    candidate_index: u32,
    field: impl FnOnce(CandidateOrigin) -> String,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    // SAFETY: the caller guarantees a live handle or null.
    let Some(session) = (unsafe { session.as_ref() }) else {
        return PROJECT_ERR_INVALID_ARGUMENT;
    };
    let Ok(candidate_index) = usize::try_from(candidate_index) else {
        return PROJECT_ERR_INVALID_CANDIDATE;
    };
    let origin = match session
        .session
        .candidate_origin(shot_index, candidate_index)
    {
        Ok(origin) => origin,
        Err(error) => return snapshot_error_code(&error),
    };
    // SAFETY: the caller's sizing contract matches `copy_utf8`'s.
    unsafe { copy_utf8(field(origin).as_bytes(), out_buf, out_cap, out_len) }
}

unsafe fn submit(
    session: *const KinetoProjectSession,
    shot_index: u32,
    command: ShotCommand,
    token: u64,
) -> i32 {
    let Some(session) = (unsafe { session.as_ref() }) else {
        return PROJECT_ERR_INVALID_ARGUMENT;
    };
    match session.session.submit(shot_index, command, token) {
        Ok(()) => PROJECT_OK,
        Err(SubmitError::InvalidShot) => PROJECT_ERR_INVALID_SHOT,
        Err(SubmitError::Closed) => PROJECT_ERR_NOT_OPEN,
    }
}

/// Queue a direction change. Returns once queued; the outcome arrives as a
/// `COMPLETED` message carrying `token`.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_set_direction(
    session: *const KinetoProjectSession,
    shot_index: u32,
    direction: u32,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(direction) = u8::try_from(direction)
            .ok()
            .and_then(ShotDirection::from_code)
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller contract.
        unsafe {
            submit(
                session,
                shot_index,
                ShotCommand::SetDirection(direction),
                token,
            )
        }
    })
}

/// Queue candidate generation. Returns once queued.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_generate(
    session: *const KinetoProjectSession,
    shot_index: u32,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(session, shot_index, ShotCommand::Generate, token)
    })
}

/// Queue a candidate selection. Returns once queued.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_select(
    session: *const KinetoProjectSession,
    shot_index: u32,
    candidate_index: u32,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Ok(candidate_index) = usize::try_from(candidate_index) else {
            return PROJECT_ERR_INVALID_CANDIDATE;
        };
        // SAFETY: forwarded caller contract.
        unsafe {
            submit(
                session,
                shot_index,
                ShotCommand::Select(candidate_index),
                token,
            )
        }
    })
}

/// Queue locking the current selection. Returns once queued.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_lock(
    session: *const KinetoProjectSession,
    shot_index: u32,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(session, shot_index, ShotCommand::Lock, token)
    })
}

/// Queue submitting a video from the shot's locked keyframe. Returns once
/// queued; the `COMPLETED` message arrives once MiniMax accepted the task, and
/// a `VIDEO` message once the video is attached or failed.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_generate_video(
    session: *const KinetoProjectSession,
    shot_index: u32,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(session, shot_index, ShotCommand::GenerateVideo, token)
    })
}

/// Queue locking the shot's current video. Returns once queued.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_lock_video(
    session: *const KinetoProjectSession,
    shot_index: u32,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(session, shot_index, ShotCommand::LockVideo, token)
    })
}

/// Copy the absolute path of the shot's current video into caller-owned
/// memory, with the same sizing contract as project metadata; an empty path
/// when the shot has no video. Reads published state only.
///
/// # Safety
/// `session` must be live. `out_len` must point to writable `u64` storage. If
/// `out_cap` is non-zero, `out_buf` must be writable for at least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_video_media_copy(
    session: *const KinetoProjectSession,
    shot_index: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(session) = (unsafe { session.as_ref() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let media = match session.session.video_media(shot_index) {
            Ok(media) => media,
            Err(error) => return snapshot_error_code(&error),
        };
        let path = match &media {
            None => "",
            Some(path) => match path.to_str() {
                Some(path) => path,
                None => return PROJECT_ERR_INVALID_UTF8,
            },
        };
        // SAFETY: the caller's sizing contract matches `copy_utf8`'s.
        unsafe { copy_utf8(path.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Queue an explicit reset of this shot's active approval state, retaining
/// superseded lineage. This is never triggered implicitly by an upstream edit.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_reset(
    session: *const KinetoProjectSession,
    shot_index: u32,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(session, shot_index, ShotCommand::Reset, token)
    })
}

/// Queue the explicit migration of one shot's known legacy demo hashes to
/// canonical `sha256:` form. The original `artifacts.json` bytes are written
/// to a backup file first. Loading a shot never does this on its own; this call
/// is the user's opt-in.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_migrate_legacy_hashes(
    session: *const KinetoProjectSession,
    shot_index: u32,
    token: u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    guarded(PROJECT_ERR_PANIC, || unsafe {
        submit(session, shot_index, ShotCommand::MigrateLegacyHashes, token)
    })
}

fn session_status(result: &Result<(), SessionError>) -> i32 {
    match result {
        Ok(()) => PROJECT_OK,
        Err(error) => session_error_code(error),
    }
}

fn session_error_code(error: &SessionError) -> i32 {
    match error {
        SessionError::Open(error) => project_error_code(error),
        SessionError::Shot(error) => shot_error_code(error),
        SessionError::Generation(error) => generation_error_code(error),
        SessionError::NotOpen => PROJECT_ERR_NOT_OPEN,
        SessionError::VideoInFlight => PROJECT_ERR_VIDEO_IN_FLIGHT,
        SessionError::Panicked => PROJECT_ERR_PANIC,
    }
}

fn generation_error_code(error: &GenerationError) -> i32 {
    match error {
        GenerationError::Shot(error) => shot_error_code(error),
        GenerationError::NotConfigured => PROJECT_ERR_PROVIDER_NOT_CONFIGURED,
        GenerationError::NoFirstFrame => PROJECT_ERR_NO_FIRST_FRAME,
        GenerationError::Prompt(KeyframePromptError::MissingIntent) => {
            PROJECT_ERR_SHOT_INTENT_MISSING
        }
        GenerationError::Prompt(_) => PROJECT_ERR_INVALID_SHOT_STATE,
        GenerationError::Provider {
            class: ErrorClass::Terminal,
            ..
        } => PROJECT_ERR_PROVIDER_REFUSED,
        GenerationError::Provider { .. }
        | GenerationError::NeedsProviderCall
        | GenerationError::Runtime(_)
        | GenerationError::JobId(_)
        | GenerationError::Unexpected(_) => PROJECT_ERR_GENERATION_JOB,
    }
}

fn shot_error_code(error: &ShotWorkflowError) -> i32 {
    match error {
        ShotWorkflowError::InvalidShotNumber => PROJECT_ERR_INVALID_SHOT,
        ShotWorkflowError::NotGenerated => PROJECT_ERR_NOT_GENERATED,
        ShotWorkflowError::InvalidCandidate => PROJECT_ERR_INVALID_CANDIDATE,
        ShotWorkflowError::Locked => PROJECT_ERR_LOCKED,
        ShotWorkflowError::AlreadyLocked => PROJECT_ERR_ALREADY_LOCKED,
        ShotWorkflowError::NoSelection => PROJECT_ERR_NO_SELECTION,
        ShotWorkflowError::StaleSelection => PROJECT_ERR_STALE_SELECTION,
        ShotWorkflowError::GenerationOverflow => PROJECT_ERR_GENERATION_OVERFLOW,
        ShotWorkflowError::MigrationRequired => PROJECT_ERR_MIGRATION_REQUIRED,
        ShotWorkflowError::KeyframeNotLocked => PROJECT_ERR_KEYFRAME_NOT_LOCKED,
        ShotWorkflowError::VideoLocked => PROJECT_ERR_VIDEO_LOCKED,
        ShotWorkflowError::NoVideo => PROJECT_ERR_NO_VIDEO,
        ShotWorkflowError::StaleVideo => PROJECT_ERR_STALE_VIDEO,
        ShotWorkflowError::Manifest(ProjectManifestError::ReadOnlyFormatVersion(_)) => {
            PROJECT_ERR_READ_ONLY
        }
        ShotWorkflowError::Manifest(ProjectManifestError::UnsupportedFormatVersion(_)) => {
            PROJECT_ERR_UNSUPPORTED_FORMAT
        }
        ShotWorkflowError::Manifest(_) => PROJECT_ERR_INVALID_MANIFEST,
        ShotWorkflowError::Fs(ProjectFsError::Io(_)) | ShotWorkflowError::Store(_) => {
            PROJECT_ERR_IO
        }
        ShotWorkflowError::Fs(_)
        | ShotWorkflowError::InvalidCanonicalState
        | ShotWorkflowError::StaleGeneration
        | ShotWorkflowError::InvalidStoredHash { .. }
        | ShotWorkflowError::MigrationBackupConflict(_)
        | ShotWorkflowError::MediaMismatch(_)
        | ShotWorkflowError::AttachedRecordMismatch(_)
        | ShotWorkflowError::InvalidTarget(_)
        | ShotWorkflowError::Path(_)
        | ShotWorkflowError::Json(_)
        | ShotWorkflowError::ArtifactId(_)
        | ShotWorkflowError::Hash(_)
        | ShotWorkflowError::Lifecycle(_)
        | ShotWorkflowError::Selection(_) => PROJECT_ERR_INVALID_SHOT_STATE,
    }
}

pub(crate) unsafe fn copy_utf8(
    bytes: &[u8],
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    if out_len.is_null() {
        return PROJECT_ERR_INVALID_ARGUMENT;
    }
    let required = match u64::try_from(bytes.len()) {
        Ok(length) => length,
        Err(_) => return PROJECT_ERR_INVALID_ARGUMENT,
    };
    // SAFETY: caller guarantees writable storage for one u64.
    unsafe { ptr::write(out_len, required) };
    if out_cap < required {
        return PROJECT_ERR_BUFFER_TOO_SMALL;
    }
    if required == 0 {
        return PROJECT_OK;
    }
    if out_buf.is_null() {
        return PROJECT_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the capacity check proves the destination can hold `bytes`.
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), out_buf, bytes.len()) };
    PROJECT_OK
}

unsafe fn utf8_owned(ptr: *const u8, len: u64) -> Result<String, i32> {
    let len = usize::try_from(len).map_err(|_| PROJECT_ERR_INVALID_ARGUMENT)?;
    if len == 0 {
        return Ok(String::new());
    }
    if ptr.is_null() {
        return Err(PROJECT_ERR_INVALID_ARGUMENT);
    }
    // SAFETY: exported callers guarantee readable input for `len` bytes.
    let bytes = unsafe { slice::from_raw_parts(ptr, len) };
    str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| PROJECT_ERR_INVALID_UTF8)
}

fn project_error_code(error: &ProjectStoreError) -> i32 {
    match error {
        ProjectStoreError::AlreadyExists(_) => PROJECT_ERR_ALREADY_EXISTS,
        ProjectStoreError::InvalidTarget(_) => PROJECT_ERR_INVALID_ARGUMENT,
        ProjectStoreError::Utf8(_) => PROJECT_ERR_INVALID_UTF8,
        ProjectStoreError::Manifest(ProjectManifestError::UnsupportedFormatVersion(_)) => {
            PROJECT_ERR_UNSUPPORTED_FORMAT
        }
        ProjectStoreError::Manifest(ProjectManifestError::ReadOnlyFormatVersion(_)) => {
            PROJECT_ERR_READ_ONLY
        }
        ProjectStoreError::Manifest(_) => PROJECT_ERR_INVALID_MANIFEST,
        ProjectStoreError::Fs(_) => PROJECT_ERR_IO,
        ProjectStoreError::Busy(_) => PROJECT_ERR_BUSY,
        ProjectStoreError::LockUnavailable { .. } => PROJECT_ERR_LOCK_UNAVAILABLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kineto_project::shot::ShotApprovalSnapshot;
    use std::{
        collections::VecDeque,
        fs,
        sync::{
            Condvar, Mutex,
            atomic::{AtomicI64, Ordering},
        },
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    /// Messages the fake `postCObject` received, per port.
    static POSTED: Mutex<Vec<(i64, [i64; 4])>> = Mutex::new(Vec::new());
    static POSTED_SIGNAL: Condvar = Condvar::new();
    static NEXT_PORT: AtomicI64 = AtomicI64::new(1);

    /// Stands in for Dart's `NativeApi.postCObject`: decodes the typed-data
    /// message exactly as the Dart VM would receive it.
    unsafe extern "C" fn record_post(port: i64, message: *mut DartCObject) -> bool {
        // SAFETY: the session posts a live typed-data object for the call.
        let object = unsafe { &*message };
        assert_eq!(object.kind, DART_COBJECT_TYPED_DATA);
        // SAFETY: the kind check selects the typed-data union member.
        let data = unsafe { object.value.as_typed_data };
        assert_eq!(data.kind, DART_TYPED_DATA_INT64);
        assert_eq!(data.length, 4);
        // SAFETY: four aligned i64 values live for the duration of the call.
        let values = unsafe { slice::from_raw_parts(data.values.cast::<i64>(), 4) };
        let mut posted = POSTED.lock().unwrap();
        posted.push((port, [values[0], values[1], values[2], values[3]]));
        POSTED_SIGNAL.notify_all();
        true
    }

    struct Port(i64);

    impl Port {
        fn new() -> Self {
            Self(NEXT_PORT.fetch_add(1, Ordering::SeqCst))
        }

        fn next(&self) -> [i64; 4] {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut posted = POSTED.lock().unwrap();
            loop {
                if let Some(index) = posted.iter().position(|(port, _)| *port == self.0) {
                    return posted.remove(index).1;
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                assert!(!remaining.is_zero(), "no message on port {}", self.0);
                posted = POSTED_SIGNAL.wait_timeout(posted, remaining).unwrap().0;
            }
        }

        fn opened(&self) -> i32 {
            let [kind, _, status, _] = self.next();
            assert_eq!(kind, MESSAGE_OPENED);
            i32::try_from(status).unwrap()
        }

        fn completed(&self, token: u64) -> i32 {
            let [kind, actual, status, _] = self.next();
            assert_eq!(kind, MESSAGE_COMPLETED);
            assert_eq!(actual, i64::try_from(token).unwrap());
            i32::try_from(status).unwrap()
        }

        fn pending(&self) -> VecDeque<[i64; 4]> {
            POSTED
                .lock()
                .unwrap()
                .iter()
                .filter(|(port, _)| *port == self.0)
                .map(|(_, message)| *message)
                .collect()
        }
    }

    fn temp_target() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // SystemTime is only microsecond-precise on macOS, so tests running in
        // parallel could otherwise share a directory name.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = format!(
            "{nonce}-{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        std::env::temp_dir().join(format!("kineto-project-ffi-{}-{nonce}", std::process::id()))
    }

    unsafe fn create_text(path: &str, port: &Port, out: *mut *mut KinetoProjectSession) -> i32 {
        let id = "ffi_project_001";
        let title = "FFI Project";
        let created_at = "2026-09-10T08:00:00Z";
        let language = "en";
        let source = "A filmmaker waits across the table.\n";
        // SAFETY: all borrowed string buffers outlive this call and `out` is
        // supplied by the test as writable pointer storage.
        unsafe {
            kineto_project_create_text(
                path.as_ptr(),
                path.len() as u64,
                id.as_ptr(),
                id.len() as u64,
                title.as_ptr(),
                title.len() as u64,
                created_at.as_ptr(),
                created_at.len() as u64,
                language.as_ptr(),
                language.len() as u64,
                source.as_ptr(),
                source.len() as u64,
                Some(record_post),
                port.0,
                out,
            )
        }
    }

    unsafe fn open(path: &str, port: &Port, out: *mut *mut KinetoProjectSession) -> i32 {
        // SAFETY: `path` outlives the call and `out` is writable test storage.
        unsafe {
            kineto_project_open(
                path.as_ptr(),
                path.len() as u64,
                Some(record_post),
                port.0,
                out,
            )
        }
    }

    /// Create a project and wait until its session reports open.
    fn created(path: &str) -> (*mut KinetoProjectSession, Port) {
        let port = Port::new();
        let mut session = ptr::null_mut();
        assert_eq!(
            unsafe { create_text(path, &port, &raw mut session) },
            PROJECT_OK
        );
        assert_eq!(port.opened(), PROJECT_OK);
        (session, port)
    }

    fn opened(path: &str) -> (*mut KinetoProjectSession, Port) {
        let port = Port::new();
        let mut session = ptr::null_mut();
        assert_eq!(unsafe { open(path, &port, &raw mut session) }, PROJECT_OK);
        assert_eq!(port.opened(), PROJECT_OK);
        (session, port)
    }

    unsafe fn snapshot(session: *const KinetoProjectSession, shot: u32) -> u64 {
        let mut state = 0_u64;
        // SAFETY: callers pass a live session and writable local state pointer.
        assert_eq!(
            unsafe { kineto_project_shot_state(session, shot, &raw mut state) },
            PROJECT_OK
        );
        state
    }

    #[test]
    fn project_session_create_and_open_round_trip() {
        let target = temp_target();
        let path = target.to_string_lossy();
        let title = "FFI Project";
        let (created, _port) = created(&path);
        assert_eq!(unsafe { kineto_project_format_version(created) }, 1);
        assert_eq!(unsafe { kineto_project_is_read_only(created) }, 0);

        // A second writer session is refused while the first one is live. The
        // handle exists at once; the refusal arrives as the open result.
        let contender_port = Port::new();
        let mut contender = ptr::null_mut();
        assert_eq!(
            unsafe { open(&path, &contender_port, &raw mut contender) },
            PROJECT_OK
        );
        assert_eq!(contender_port.opened(), PROJECT_ERR_BUSY);
        assert_eq!(unsafe { kineto_project_format_version(contender) }, 0);
        let mut required = 0_u64;
        assert_eq!(
            unsafe { kineto_project_title_copy(contender, ptr::null_mut(), 0, &raw mut required) },
            PROJECT_ERR_NOT_OPEN
        );
        unsafe { kineto_project_destroy(contender) };
        unsafe { kineto_project_destroy(created) };

        let (reopened, _port) = opened(&path);
        assert_eq!(
            unsafe { kineto_project_title_copy(reopened, ptr::null_mut(), 0, &raw mut required) },
            PROJECT_ERR_BUFFER_TOO_SMALL
        );
        assert_eq!(required, title.len() as u64);
        let mut title_bytes = vec![0_u8; required as usize];
        assert_eq!(
            unsafe {
                kineto_project_title_copy(
                    reopened,
                    title_bytes.as_mut_ptr(),
                    title_bytes.len() as u64,
                    &raw mut required,
                )
            },
            PROJECT_OK
        );
        assert_eq!(title_bytes, title.as_bytes());
        unsafe { kineto_project_destroy(reopened) };
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn shot_commands_complete_by_token_and_survive_reopen() {
        let target = temp_target();
        let path = target.to_string_lossy();
        let (session, port) = created(&path);

        assert_eq!(
            unsafe { kineto_project_shot_set_direction(session, 0, 4, 11) },
            PROJECT_OK
        );
        assert_eq!(
            unsafe { kineto_project_shot_generate(session, 0, 12) },
            PROJECT_OK
        );
        assert_eq!(
            unsafe { kineto_project_shot_select(session, 0, 2, 13) },
            PROJECT_OK
        );
        assert_eq!(
            unsafe { kineto_project_shot_lock(session, 0, 14) },
            PROJECT_OK
        );
        for token in 11..=14 {
            assert_eq!(port.completed(token), PROJECT_OK);
        }
        let before = unsafe { snapshot(session, 0) };
        assert_eq!(before & 0b11, 0b11);
        unsafe { kineto_project_destroy(session) };

        fs::create_dir_all(target.join(".kineto")).unwrap();
        fs::write(target.join(".kineto/project.db"), b"derived").unwrap();
        fs::remove_dir_all(target.join(".kineto")).unwrap();

        let (reopened, _port) = opened(&path);
        assert_eq!(unsafe { snapshot(reopened, 0) }, before);
        unsafe { kineto_project_destroy(reopened) };
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn refused_commands_carry_their_status_in_the_completion() {
        let target = temp_target();
        let path = target.to_string_lossy();
        let (session, port) = created(&path);

        assert_eq!(
            unsafe { kineto_project_shot_generate(session, 0, 1) },
            PROJECT_OK
        );
        assert_eq!(
            unsafe { kineto_project_shot_select(session, 0, 1, 2) },
            PROJECT_OK
        );
        assert_eq!(
            unsafe { kineto_project_shot_set_direction(session, 0, 3, 3) },
            PROJECT_OK
        );
        assert_eq!(
            unsafe { kineto_project_shot_lock(session, 0, 4) },
            PROJECT_OK
        );
        assert_eq!(port.completed(1), PROJECT_OK);
        assert_eq!(port.completed(2), PROJECT_OK);
        assert_eq!(port.completed(3), PROJECT_OK);
        assert_eq!(port.completed(4), PROJECT_ERR_STALE_SELECTION);

        let stale = unsafe { snapshot(session, 0) };
        assert_eq!((stale >> 2) & 1, 1);
        assert_eq!((stale >> 16) & 0xff, 2);

        // Arguments the ABI can reject without the worker are refused at once.
        assert_eq!(
            unsafe { kineto_project_shot_generate(session, 2, 5) },
            PROJECT_ERR_INVALID_SHOT
        );
        assert_eq!(
            unsafe { kineto_project_shot_set_direction(session, 0, 99, 6) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        unsafe { kineto_project_destroy(session) };
        assert!(port.pending().is_empty());
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn ffi_rejects_invalid_inputs_and_existing_target() {
        let port = Port::new();
        let invalid = [0xff_u8];
        let mut out = ptr::null_mut();
        assert_eq!(
            unsafe {
                kineto_project_open(
                    invalid.as_ptr(),
                    invalid.len() as u64,
                    Some(record_post),
                    port.0,
                    &raw mut out,
                )
            },
            PROJECT_ERR_INVALID_UTF8
        );
        assert!(out.is_null());
        let path = "/tmp/kineto-never-opened";
        assert_eq!(
            unsafe {
                kineto_project_open(path.as_ptr(), path.len() as u64, None, port.0, &raw mut out)
            },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        assert!(out.is_null());

        let target = temp_target();
        let path = target.to_string_lossy();
        let (session, _port) = created(&path);
        unsafe { kineto_project_destroy(session) };

        let port = Port::new();
        assert_eq!(
            unsafe { create_text(&path, &port, &raw mut out) },
            PROJECT_OK
        );
        assert_eq!(port.opened(), PROJECT_ERR_ALREADY_EXISTS);
        unsafe { kineto_project_destroy(out) };
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn legacy_hashes_block_writes_until_explicit_migration_with_backup() {
        let target = temp_target();
        let path = target.to_string_lossy().into_owned();
        let (session, port) = created(&path);
        assert_eq!(
            unsafe { kineto_project_shot_generate(session, 0, 1) },
            PROJECT_OK
        );
        assert_eq!(port.completed(1), PROJECT_OK);
        unsafe { kineto_project_destroy(session) };

        // Rewrite the shot's records in the pre-sha256 demo form an older
        // build produced.
        let shot_dir = target.join("scenes/scene_001/shots/shot_001");
        let artifacts_path = shot_dir.join("artifacts.json");
        let mut artifacts: serde_json::Value =
            serde_json::from_slice(&fs::read(&artifacts_path).unwrap()).unwrap();
        for artifact in artifacts.as_array_mut().unwrap() {
            let id = artifact["artifact_id"].as_str().unwrap().to_owned();
            artifact["content_hash"] = format!("demo:{id}").into();
            artifact["input_hash"] = "kineto:shot-intent:v1:shot_001:reaction".into();
        }
        let legacy = serde_json::to_vec_pretty(&artifacts).unwrap();
        fs::write(&artifacts_path, &legacy).unwrap();

        let (reopened, port) = opened(&path);
        assert_ne!(unsafe { snapshot(reopened, 0) } & (1 << 3), 0);
        assert_eq!(
            unsafe { kineto_project_shot_generate(reopened, 0, 2) },
            PROJECT_OK
        );
        assert_eq!(port.completed(2), PROJECT_ERR_MIGRATION_REQUIRED);
        assert_eq!(fs::read(&artifacts_path).unwrap(), legacy);

        assert_eq!(
            unsafe { kineto_project_shot_migrate_legacy_hashes(reopened, 0, 3) },
            PROJECT_OK
        );
        assert_eq!(port.completed(3), PROJECT_OK);
        assert_eq!(unsafe { snapshot(reopened, 0) } & (1 << 3), 0);
        assert_eq!(
            fs::read(shot_dir.join("artifacts.pre-sha256-migration.json")).unwrap(),
            legacy
        );
        assert_eq!(
            unsafe { kineto_project_shot_generate(reopened, 0, 4) },
            PROJECT_OK
        );
        assert_eq!(port.completed(4), PROJECT_OK);
        unsafe { kineto_project_destroy(reopened) };
        let _ = fs::remove_dir_all(target);
    }

    #[test]
    fn destroy_stops_delivery_before_returning() {
        let target = temp_target();
        let path = target.to_string_lossy();
        let (session, port) = created(&path);
        for token in 0..20 {
            assert_eq!(
                unsafe { kineto_project_shot_generate(session, 0, token) },
                PROJECT_OK
            );
        }
        unsafe { kineto_project_destroy(session) };
        let delivered = port.pending().len();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(port.pending().len(), delivered);
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn writer_lock_refusals_have_distinct_statuses() {
        assert_eq!(
            project_error_code(&ProjectStoreError::Busy(temp_target())),
            PROJECT_ERR_BUSY
        );
        assert_eq!(
            project_error_code(&ProjectStoreError::LockUnavailable {
                path: temp_target(),
                source: std::io::Error::from(std::io::ErrorKind::Unsupported),
            }),
            PROJECT_ERR_LOCK_UNAVAILABLE
        );
        assert_eq!(
            session_status(&Err(SessionError::NotOpen)),
            PROJECT_ERR_NOT_OPEN
        );
    }

    #[test]
    fn dart_cobject_layout_matches_the_c_header() {
        // dart_native_api.h: the enum tag, then a union whose largest member
        // (external typed data) is five pointer-sized fields.
        assert_eq!(std::mem::size_of::<DartCObjectValue>(), 40);
        assert_eq!(std::mem::size_of::<DartCObject>(), 48);
        assert_eq!(std::mem::offset_of!(DartCObject, value), 8);
        assert_eq!(std::mem::offset_of!(DartTypedData, length), 8);
        assert_eq!(std::mem::offset_of!(DartTypedData, values), 16);
    }

    #[test]
    fn snapshot_packing_includes_stale_lineage_and_migration_fields() {
        let bits = encode_shot_snapshot(
            ShotApprovalSnapshot {
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
                video: kineto_project::shot::VideoState::None,
                video_revision: 0,
            },
            false,
        );
        assert_eq!(bits & 1, 1);
        assert_eq!((bits >> 1) & 1, 0);
        assert_eq!((bits >> 2) & 1, 1);
        assert_eq!((bits >> 3) & 1, 1);
        assert_eq!((bits >> 4) & 1, 1);
        assert_eq!((bits >> 8) & 0xff, 3);
        assert_eq!((bits >> 16) & 0xff, 3);
        assert_eq!((bits >> 24) & 0xff, 4);
        assert_eq!((bits >> 32) & 0xffff, 7);
        assert_eq!((bits >> 48) & 0xffff, 18);
    }
}
