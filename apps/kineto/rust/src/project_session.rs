use std::{collections::BTreeMap, ptr, slice, str};

use crate::guarded;
use kineto_project::{
    fs::ProjectFsError,
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest,
        ProjectManifestError, ProjectSource, ProjectStoreError, ProjectWorkflow,
    },
    shot::{ShotApprovalSnapshot, ShotDirection, ShotWorkflow, ShotWorkflowError},
};

const PROJECT_OK: i32 = 0;
const PROJECT_ERR_INVALID_ARGUMENT: i32 = 100;
const PROJECT_ERR_INVALID_UTF8: i32 = 101;
const PROJECT_ERR_IO: i32 = 102;
const PROJECT_ERR_ALREADY_EXISTS: i32 = 103;
const PROJECT_ERR_INVALID_MANIFEST: i32 = 104;
const PROJECT_ERR_UNSUPPORTED_FORMAT: i32 = 105;
const PROJECT_ERR_BUFFER_TOO_SMALL: i32 = 106;
const PROJECT_ERR_READ_ONLY: i32 = 107;
const PROJECT_ERR_INVALID_SHOT: i32 = 108;
const PROJECT_ERR_NOT_GENERATED: i32 = 109;
const PROJECT_ERR_INVALID_CANDIDATE: i32 = 110;
const PROJECT_ERR_LOCKED: i32 = 111;
const PROJECT_ERR_ALREADY_LOCKED: i32 = 112;
const PROJECT_ERR_NO_SELECTION: i32 = 113;
const PROJECT_ERR_STALE_SELECTION: i32 = 114;
const PROJECT_ERR_INVALID_SHOT_STATE: i32 = 115;
const PROJECT_ERR_GENERATION_OVERFLOW: i32 = 116;
const PROJECT_ERR_PANIC: i32 = 199;
const PROJECT_SHOT_COUNT: usize = 2;

/// Opaque canonical-project handle. Dart never dereferences this allocation.
pub struct KinetoProjectSession {
    project: CanonicalProject,
    // Canonical JSON is loaded once per shot per open session. UI reads then
    // stay typed/in-process rather than repeatedly parsing project files.
    shots: [Option<ShotWorkflow>; PROJECT_SHOT_COUNT],
}

impl KinetoProjectSession {
    fn new(project: CanonicalProject) -> Self {
        Self {
            project,
            shots: [None, None],
        }
    }

    fn with_shot<R>(
        &mut self,
        shot_index: u32,
        operation: impl FnOnce(&CanonicalProject, &mut ShotWorkflow) -> Result<R, ShotWorkflowError>,
    ) -> Result<R, ShotWorkflowError> {
        let index =
            usize::try_from(shot_index).map_err(|_| ShotWorkflowError::InvalidShotNumber)?;
        if index >= PROJECT_SHOT_COUNT {
            return Err(ShotWorkflowError::InvalidShotNumber);
        }
        if self.shots[index].is_none() {
            let shot_number = u32::try_from(index + 1).expect("project shot count fits in u32");
            self.shots[index] = Some(ShotWorkflow::load(&self.project, shot_number)?);
        }
        let project = &self.project;
        let shot = self.shots[index]
            .as_mut()
            .expect("shot is installed immediately above");
        operation(project, shot)
    }
}

/// Open and validate an existing canonical Kineto project.
///
/// # Safety
/// `out_session` must point to writable storage for one pointer. When
/// `path_len` is non-zero, `path_ptr` must point to at least `path_len` readable
/// bytes containing UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_open(
    path_ptr: *const u8,
    path_len: u64,
    out_session: *mut *mut KinetoProjectSession,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_session.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: caller guarantees writable storage for one pointer.
        unsafe { ptr::write(out_session, ptr::null_mut()) };
        let path = match unsafe { utf8_owned(path_ptr, path_len) } {
            Ok(path) if !path.is_empty() => path,
            Ok(_) => return PROJECT_ERR_INVALID_ARGUMENT,
            Err(code) => return code,
        };
        match CanonicalProject::open(&path) {
            Ok(project) => {
                let session = Box::into_raw(Box::new(KinetoProjectSession::new(project)));
                // SAFETY: caller guarantees writable storage for one pointer.
                unsafe { ptr::write(out_session, session) };
                PROJECT_OK
            }
            Err(error) => project_error_code(&error),
        }
    })
}

/// Create a new canonical UTF-8 text-source project and return it opened.
///
/// All input buffers are borrowed only for this call and all must be valid
/// UTF-8. `source_ptr` is written to `source/story.txt`.
///
/// # Safety
/// `out_session` must point to writable storage for one pointer. Each non-empty
/// input buffer must be valid for reads of its declared length for the call.
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
    out_session: *mut *mut KinetoProjectSession,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_session.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: caller guarantees writable storage for one pointer.
        unsafe { ptr::write(out_session, ptr::null_mut()) };

        let path = match unsafe { utf8_owned(path_ptr, path_len) } {
            Ok(path) if !path.is_empty() => path,
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
        let source_len = match usize::try_from(source_len) {
            Ok(length) => length,
            Err(_) => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        if source_len != 0 && source_ptr.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        let source: &[u8] = if source_len == 0 {
            &[]
        } else {
            // SAFETY: caller guarantees this buffer remains readable for the call.
            unsafe { slice::from_raw_parts(source_ptr, source_len) }
        };
        if str::from_utf8(source).is_err() {
            return PROJECT_ERR_INVALID_UTF8;
        }

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

        match CanonicalProject::create(&path, manifest, source) {
            Ok(project) => {
                let session = Box::into_raw(Box::new(KinetoProjectSession::new(project)));
                // SAFETY: caller guarantees writable storage for one pointer.
                unsafe { ptr::write(out_session, session) };
                PROJECT_OK
            }
            Err(error) => project_error_code(&error),
        }
    })
}

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
        session.project.manifest().format_version
    })
}

/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_is_read_only(session: *const KinetoProjectSession) -> u8 {
    guarded(1, || {
        let Some(session) = (unsafe { session.as_ref() }) else {
            return 1;
        };
        u8::from(session.project.is_read_only())
    })
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
        // SAFETY: forwarded caller-owned output contract.
        unsafe {
            copy_utf8(
                session.project.manifest().project_id.as_bytes(),
                out_buf,
                out_cap,
                out_len,
            )
        }
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
        // SAFETY: forwarded caller-owned output contract.
        unsafe {
            copy_utf8(
                session.project.manifest().title.as_bytes(),
                out_buf,
                out_cap,
                out_len,
            )
        }
    })
}

/// Return one shot's typed canonical approval state as a fixed-width snapshot.
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
    session: *mut KinetoProjectSession,
    shot_index: u32,
    out_state: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(session) = (unsafe { session.as_mut() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        if out_state.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        match session.with_shot(shot_index, |_, shot| shot.snapshot()) {
            Ok(snapshot) => {
                // SAFETY: caller guarantees writable storage for one u64.
                unsafe { ptr::write(out_state, encode_shot_snapshot(snapshot)) };
                PROJECT_OK
            }
            Err(error) => shot_error_code(&error),
        }
    })
}

/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_set_direction(
    session: *mut KinetoProjectSession,
    shot_index: u32,
    direction: u32,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(session) = (unsafe { session.as_mut() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Some(direction) = u8::try_from(direction)
            .ok()
            .and_then(ShotDirection::from_code)
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        shot_result_code(session.with_shot(shot_index, |project, shot| {
            shot.set_direction(project, direction)
        }))
    })
}

/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_generate(
    session: *mut KinetoProjectSession,
    shot_index: u32,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(session) = (unsafe { session.as_mut() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        shot_result_code(session.with_shot(shot_index, |project, shot| shot.generate(project)))
    })
}

/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_select(
    session: *mut KinetoProjectSession,
    shot_index: u32,
    candidate_index: u32,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(session) = (unsafe { session.as_mut() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Ok(candidate_index) = usize::try_from(candidate_index) else {
            return PROJECT_ERR_INVALID_CANDIDATE;
        };
        shot_result_code(session.with_shot(shot_index, |project, shot| {
            shot.select(project, candidate_index)
        }))
    })
}

/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_lock(
    session: *mut KinetoProjectSession,
    shot_index: u32,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(session) = (unsafe { session.as_mut() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        shot_result_code(session.with_shot(shot_index, |project, shot| shot.lock(project)))
    })
}

/// Explicitly discard this shot's active approval state while retaining
/// superseded lineage. This is never called implicitly by an upstream edit.
///
/// # Safety
/// `session` must be a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_shot_reset(
    session: *mut KinetoProjectSession,
    shot_index: u32,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(session) = (unsafe { session.as_mut() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        shot_result_code(session.with_shot(shot_index, |project, shot| shot.reset(project)))
    })
}

fn encode_shot_snapshot(snapshot: ShotApprovalSnapshot) -> u64 {
    let selected_code = snapshot
        .selected_index
        .map_or(0, |index| u64::from(index) + 1);
    u64::from(snapshot.generated)
        | (u64::from(snapshot.locked) << 1)
        | (u64::from(snapshot.stale) << 2)
        | (u64::from(snapshot.migration_pending) << 3)
        | (u64::from(snapshot.invalid_hash_count != 0) << 4)
        | (u64::from(snapshot.candidate_count) << 8)
        | (selected_code << 16)
        | (u64::from(snapshot.direction.code()) << 24)
        | (u64::from(snapshot.generation_revision) << 32)
        | (u64::from(snapshot.superseded_count) << 48)
}

fn shot_result_code(result: Result<(), ShotWorkflowError>) -> i32 {
    match result {
        Ok(()) => PROJECT_OK,
        Err(error) => shot_error_code(&error),
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
        | ShotWorkflowError::MigrationRequired
        | ShotWorkflowError::InvalidStoredHash { .. }
        | ShotWorkflowError::MigrationBackupConflict(_)
        | ShotWorkflowError::InvalidTarget(_)
        | ShotWorkflowError::Path(_)
        | ShotWorkflowError::Json(_)
        | ShotWorkflowError::ArtifactId(_)
        | ShotWorkflowError::Hash(_)
        | ShotWorkflowError::Lifecycle(_)
        | ShotWorkflowError::Selection(_) => PROJECT_ERR_INVALID_SHOT_STATE,
    }
}

unsafe fn copy_utf8(bytes: &[u8], out_buf: *mut u8, out_cap: u64, out_len: *mut u64) -> i32 {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_target() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kineto-project-ffi-{}-{nonce}", std::process::id()))
    }

    unsafe fn create_text(path: &str, out: *mut *mut KinetoProjectSession) -> i32 {
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
                out,
            )
        }
    }

    unsafe fn snapshot(session: *mut KinetoProjectSession, shot: u32) -> u64 {
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
        let mut created = ptr::null_mut();
        assert_eq!(unsafe { create_text(&path, &raw mut created) }, PROJECT_OK);
        assert!(!created.is_null());
        assert_eq!(unsafe { kineto_project_format_version(created) }, 1);
        assert_eq!(unsafe { kineto_project_is_read_only(created) }, 0);
        unsafe { kineto_project_destroy(created) };

        let mut reopened = ptr::null_mut();
        assert_eq!(
            unsafe { kineto_project_open(path.as_ptr(), path.len() as u64, &raw mut reopened) },
            PROJECT_OK
        );
        let mut required = 0_u64;
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
    fn shot_approval_survives_reopen_and_dot_kineto_deletion() {
        let target = temp_target();
        let path = target.to_string_lossy();
        let mut session = ptr::null_mut();
        assert_eq!(unsafe { create_text(&path, &raw mut session) }, PROJECT_OK);

        assert_eq!(
            unsafe { kineto_project_shot_set_direction(session, 0, 4) },
            PROJECT_OK
        );
        assert_eq!(
            unsafe { kineto_project_shot_generate(session, 0) },
            PROJECT_OK
        );
        assert_eq!(
            unsafe { kineto_project_shot_select(session, 0, 2) },
            PROJECT_OK
        );
        assert_eq!(unsafe { kineto_project_shot_lock(session, 0) }, PROJECT_OK);
        let before = unsafe { snapshot(session, 0) };
        unsafe { kineto_project_destroy(session) };

        fs::create_dir_all(target.join(".kineto")).unwrap();
        fs::write(target.join(".kineto/project.db"), b"derived").unwrap();
        fs::remove_dir_all(target.join(".kineto")).unwrap();

        let mut reopened = ptr::null_mut();
        assert_eq!(
            unsafe { kineto_project_open(path.as_ptr(), path.len() as u64, &raw mut reopened) },
            PROJECT_OK
        );
        assert_eq!(unsafe { snapshot(reopened, 0) }, before);
        unsafe { kineto_project_destroy(reopened) };
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn direction_change_preserves_stale_selection_until_regenerate() {
        let target = temp_target();
        let path = target.to_string_lossy();
        let mut session = ptr::null_mut();
        assert_eq!(unsafe { create_text(&path, &raw mut session) }, PROJECT_OK);
        assert_eq!(
            unsafe { kineto_project_shot_generate(session, 0) },
            PROJECT_OK
        );
        assert_eq!(
            unsafe { kineto_project_shot_select(session, 0, 1) },
            PROJECT_OK
        );
        assert_eq!(
            unsafe { kineto_project_shot_set_direction(session, 0, 3) },
            PROJECT_OK
        );

        let stale = unsafe { snapshot(session, 0) };
        assert_eq!((stale >> 2) & 1, 1);
        assert_eq!((stale >> 16) & 0xff, 2);
        assert_eq!(
            unsafe { kineto_project_shot_lock(session, 0) },
            PROJECT_ERR_STALE_SELECTION
        );

        assert_eq!(
            unsafe { kineto_project_shot_generate(session, 0) },
            PROJECT_OK
        );
        let regenerated = unsafe { snapshot(session, 0) };
        assert_eq!((regenerated >> 2) & 1, 0);
        assert_eq!((regenerated >> 16) & 0xff, 0);
        assert_eq!((regenerated >> 32) & 0xffff, 2);
        assert_eq!((regenerated >> 48) & 0xffff, 3);

        unsafe { kineto_project_destroy(session) };
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn ffi_rejects_invalid_inputs_and_existing_target() {
        let invalid = [0xff_u8];
        let mut out = ptr::null_mut();
        assert_eq!(
            unsafe { kineto_project_open(invalid.as_ptr(), invalid.len() as u64, &raw mut out) },
            PROJECT_ERR_INVALID_UTF8
        );
        assert!(out.is_null());

        let target = temp_target();
        let path = target.to_string_lossy();
        assert_eq!(unsafe { create_text(&path, &raw mut out) }, PROJECT_OK);
        unsafe { kineto_project_destroy(out) };
        out = ptr::null_mut();
        assert_eq!(
            unsafe { create_text(&path, &raw mut out) },
            PROJECT_ERR_ALREADY_EXISTS
        );
        assert!(out.is_null());
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn snapshot_packing_includes_stale_lineage_and_migration_fields() {
        let bits = encode_shot_snapshot(ShotApprovalSnapshot {
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
        });
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
