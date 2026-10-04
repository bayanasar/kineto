//! A production package: the locked videos of the shots in play, each with
//! a sidecar saying what made it, and a project-level manifest with scene
//! grouping, shot order and runtime. Self-describing, so an editor needs no
//! Kineto to use it (`schemas/export-manifest.schema.json`,
//! `schemas/export-shot.schema.json`).
//!
//! ```text
//! <destination>/<name>/
//! ├── manifest.json
//! ├── timeline.otio                      (the shots and lines as a rough cut)
//! ├── timeline.edl                       (the same, as a CMX 3600 list)
//! ├── <scene>_<shot>.mp4
//! ├── <scene>_<shot>.json
//! ├── <scene>_<shot>_first_frame.<ext>   (when the keyframe has an image)
//! └── <scene>_<shot>_line<NN>.<ext>      (each locked line the shot carries)
//! ```
//!
//! A shot carries the screenplay lines its dialogue holds verbatim. Their
//! locked takes go with it, named by the line's place in its scene. Under a
//! recipe with a dialogue step, a shot whose lines are not all locked, or
//! whose locked takes no longer fit, is not ready.
//!
//! The package's timeline ([`crate::timeline`]) lays the shots end to end and
//! each one's lines within it, for an editor to open as a rough cut.
//!
//! Only locked videos that still follow their shot are exported, each
//! checked against its recorded hash. A shot that is not ready is reported
//! and, unless the director asks for a partial package, refuses the export.
//! The package is written beside its destination and renamed into place, so
//! an interruption never leaves half a package under its name.

use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use kineto_project::{
    ContentHash,
    fs::ProjectRelativePath,
    manifest::CanonicalProject,
    recipe::{self, ArtifactKind},
    revisions::RevisionError,
    shot::{LockedVideo, ShotKey, ShotWorkflow, ShotWorkflowError, VideoState, load_scene_intent},
    shot_plan::shots_in_play,
};
use serde_json::{Value, json};

use crate::{
    dubbing::{self, DubbedLine},
    generation::GenerationError,
    timeline::{self, TimelineLine, TimelineShot},
};

/// The manifest and sidecar schema version this module writes: 2 adds the
/// lines each shot carries, 3 the timeline.
pub const EXPORT_SCHEMA_VERSION: u32 = 3;

/// The package's OpenTimelineIO timeline.
pub const TIMELINE_FILE: &str = "timeline.otio";

/// The package's timeline as a CMX 3600 edit decision list.
pub const EDL_FILE: &str = "timeline.edl";

/// A shot in play that is not in the package, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    pub shot: ShotKey,
    /// Its place among the shots in play, from 1.
    pub order: usize,
    pub reason: String,
}

/// Which shots in play can be exported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Readiness {
    /// Shots with a current locked video, with their place among the shots
    /// in play.
    pub ready: Vec<(ShotKey, usize)>,
    pub missing: Vec<Missing>,
}

impl Readiness {
    /// Every shot in play can be exported.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.missing.is_empty() && !self.ready.is_empty()
    }
}

#[derive(Debug)]
pub enum ExportError {
    Shot(ShotWorkflowError),
    Plan(RevisionError),
    /// Shots are not ready and a partial package was not asked for.
    Incomplete(Vec<Missing>),
    /// No shot is ready.
    NothingToExport,
    /// The package name is not one folder name.
    InvalidName(String),
    /// A package of that name exists and replacing it was not asked for.
    Exists(PathBuf),
    /// A file's bytes are not what its record says.
    Mismatch(String),
    /// The lines and their takes could not be read.
    Lines(Box<GenerationError>),
    /// The timeline cannot be written in a format the package holds.
    Timeline(String),
    Io(std::io::Error),
}

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shot(error) => error.fmt(formatter),
            Self::Plan(error) => error.fmt(formatter),
            Self::Incomplete(missing) => write!(
                formatter,
                "{} shot(s) are not ready to export: {}",
                missing.len(),
                missing
                    .iter()
                    .map(|missing| format!("{} ({})", missing.shot.shot(), missing.reason))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::NothingToExport => formatter.write_str("no shot has a locked video to export"),
            Self::InvalidName(name) => write!(formatter, "{name:?} is not a folder name"),
            Self::Exists(path) => write!(formatter, "{} exists already", path.display()),
            Self::Mismatch(path) => write!(formatter, "{path} is not what its record says"),
            Self::Lines(error) => error.fmt(formatter),
            Self::Timeline(reason) => formatter.write_str(reason),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl Error for ExportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Shot(error) => Some(error),
            Self::Plan(error) => Some(error),
            Self::Lines(error) => Some(error.as_ref()),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ShotWorkflowError> for ExportError {
    fn from(error: ShotWorkflowError) -> Self {
        Self::Shot(error)
    }
}

impl From<std::io::Error> for ExportError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Which shots in play have a current locked video to export, and why the
/// others do not. Reads no media bytes.
pub fn readiness(project: &CanonicalProject) -> Result<Readiness, ExportError> {
    let mut readiness = Readiness::default();
    let lines = dubbed_lines(project)?;
    // An unknown recipe asks for nothing it cannot name.
    let lines_required = recipe::of(project).is_ok_and(|recipe| {
        recipe
            .steps
            .iter()
            .any(|step| step.produces == ArtifactKind::Line)
    });
    for (index, key) in shots_in_play(project)
        .map_err(ExportError::Plan)?
        .into_iter()
        .enumerate()
    {
        let order = index + 1;
        let shot = ShotWorkflow::load(project, &key)?;
        let snapshot = shot.snapshot()?;
        let reason = match snapshot.video {
            _ if !snapshot.locked => Some("its keyframe is not locked".to_owned()),
            VideoState::None => Some("it has no video".to_owned()),
            VideoState::Ready => Some("its video is not locked".to_owned()),
            VideoState::Stale => Some(format!(
                "its video no longer follows the shot: {}",
                shot.video_staleness()?.join(", ")
            )),
            VideoState::Locked => match shot.locked_video()? {
                None => Some("its locked video's bytes were let go".to_owned()),
                Some(video) if !project.root().resolve(&video.media_path).is_file() => {
                    Some("its locked video's file is missing".to_owned())
                }
                Some(_) if lines_required => {
                    let dialogue = shot.intent()?.and_then(|intent| intent.dialogue);
                    carried(&lines, key.scene(), dialogue.as_deref())
                        .into_iter()
                        .find_map(|line| match &line.lock {
                            None => {
                                Some(format!("line {} is not dubbed and locked", line.line.line))
                            }
                            Some(_) if !line.stale.is_empty() => Some(format!(
                                "the locked take of line {} no longer fits: {}",
                                line.line.line,
                                line.stale.join(", ")
                            )),
                            Some(lock) => present(project, &lock.media_path)
                                .is_none()
                                .then(|| format!("line {}'s take is missing", line.line.line)),
                        })
                }
                Some(_) => None,
            },
        };
        match reason {
            None => readiness.ready.push((key, order)),
            Some(reason) => readiness.missing.push(Missing {
                shot: key,
                order,
                reason,
            }),
        }
    }
    Ok(readiness)
}

/// What the director asks of an export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRequest {
    /// The folder the package is made in.
    pub destination: PathBuf,
    /// The package's folder name.
    pub name: String,
    /// Export the shots that are ready although others are not.
    pub partial: bool,
    /// Replace a package of the same name.
    pub replace: bool,
}

/// What an export wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exported {
    pub path: PathBuf,
    pub shots: usize,
    pub missing: Vec<Missing>,
    pub complete: bool,
    /// The bytes of every file in the package.
    pub bytes: u64,
    /// The dubbed lines it carries.
    pub lines: usize,
}

/// Write the package `request` asks for, stamped `exported_at`.
pub fn export(
    project: &CanonicalProject,
    request: &ExportRequest,
    exported_at: SystemTime,
) -> Result<Exported, ExportError> {
    let name = request.name.trim();
    if name.is_empty()
        || name.starts_with('.')
        || name.contains(['/', '\\'])
        || Path::new(name).components().count() != 1
    {
        return Err(ExportError::InvalidName(request.name.clone()));
    }
    let readiness = readiness(project)?;
    if !readiness.missing.is_empty() && !request.partial {
        return Err(ExportError::Incomplete(readiness.missing));
    }
    if readiness.ready.is_empty() {
        return Err(ExportError::NothingToExport);
    }
    let target = request.destination.join(name);
    if target.exists() && !request.replace {
        return Err(ExportError::Exists(target));
    }
    let staging = request.destination.join(format!(".{name}.partial"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    let written = write_package(project, &readiness, &staging, exported_at);
    let (bytes, lines) = match written {
        Ok(written) => written,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    // Swap the finished package into place; an earlier one of the same name
    // goes only once the new one is there.
    let replaced = request.destination.join(format!(".{name}.replaced"));
    if target.exists() {
        if replaced.exists() {
            std::fs::remove_dir_all(&replaced)?;
        }
        std::fs::rename(&target, &replaced)?;
    }
    if let Err(error) = std::fs::rename(&staging, &target) {
        if replaced.exists() {
            let _ = std::fs::rename(&replaced, &target);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error.into());
    }
    if replaced.exists() {
        let _ = std::fs::remove_dir_all(&replaced);
    }
    Ok(Exported {
        path: target,
        shots: readiness.ready.len(),
        complete: readiness.missing.is_empty(),
        missing: readiness.missing,
        bytes,
        lines,
    })
}

/// Write every file of the package into `folder`; the bytes written and
/// the dubbed lines carried.
fn write_package(
    project: &CanonicalProject,
    readiness: &Readiness,
    folder: &Path,
    exported_at: SystemTime,
) -> Result<(u64, usize), ExportError> {
    let mut bytes = 0;
    let mut carried_lines = 0;
    let mut scenes: Vec<(String, Vec<Value>)> = Vec::new();
    let mut runtime: u64 = 0;
    let mut cut: Vec<TimelineShot> = Vec::new();
    let lines = dubbed_lines(project)?;
    for (key, order) in &readiness.ready {
        let shot = ShotWorkflow::load(project, key)?;
        let video = shot
            .locked_video()?
            .ok_or(ExportError::Mismatch(key.shot().to_owned()))?;
        let stem = file_stem(key);
        let video_file = format!("{stem}.mp4");
        bytes += copy_checked(
            project,
            &video.media_path,
            &video.content_hash,
            &folder.join(&video_file),
        )?;
        let first_frame_file = match &video.first_frame_media {
            Some(media) => {
                let extension = media
                    .as_path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or("img");
                let file = format!("{stem}_first_frame.{extension}");
                bytes += copy_checked(
                    project,
                    media,
                    &video.first_frame.content_hash,
                    &folder.join(&file),
                )?;
                Some(file)
            }
            None => None,
        };
        let dialogue = shot.intent()?.and_then(|intent| intent.dialogue);
        let mut line_files = Vec::new();
        let mut spoken = Vec::new();
        // The file says how long it lasts; the length asked for, otherwise.
        let duration_ms = timeline::mp4_duration_ms(&std::fs::read(folder.join(&video_file))?)
            .unwrap_or_else(|| u32::from(video.clip_seconds.unwrap_or(0)) * 1000);
        let mut timed = Vec::new();
        for line in carried(&lines, key.scene(), dialogue.as_deref()) {
            let (Some(lock), true) = (&line.lock, line.stale.is_empty()) else {
                continue;
            };
            let Some(take) = line
                .takes
                .iter()
                .find(|take| take.artifact_id == lock.artifact_id)
            else {
                continue;
            };
            let Some(path) = present(project, &lock.media_path) else {
                continue;
            };
            let extension = path
                .as_path()
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("audio");
            let file = format!("{stem}_line{:02}.{extension}", line.line.line);
            bytes += copy_checked(
                project,
                &path,
                &ContentHash::new(lock.content_hash.clone())
                    .map_err(|_| ExportError::Mismatch(lock.media_path.clone()))?,
                &folder.join(&file),
            )?;
            spoken.push(json!({
                "line": line.line.line,
                "character": take.character,
                "text": take.text,
                "file": file,
                "artifact_id": take.artifact_id,
                "content_hash": take.content_hash,
                "duration_ms": take.duration_ms,
                "provider": take.provider,
                "model": take.model,
                "voice_id": take.voice.voice_id,
                "emotion": take.voice.emotion,
                "speed_permille": take.voice.speed_permille,
            }));
            timed.push(TimelineLine {
                line: line.line.line,
                character: take.character.clone(),
                file: file.clone(),
                artifact_id: take.artifact_id.clone(),
                content_hash: take.content_hash.clone(),
                duration_ms: take.duration_ms,
            });
            line_files.push(file);
            carried_lines += 1;
        }
        cut.push(TimelineShot {
            scene_id: key.scene().to_owned(),
            shot_id: key.shot().to_owned(),
            file: video_file.clone(),
            artifact_id: video.artifact_id.clone(),
            content_hash: video.content_hash.as_str().to_owned(),
            duration_ms,
            lines: timed,
        });
        let sidecar_file = format!("{stem}.json");
        let mut sidecar = sidecar(
            &shot,
            key,
            *order,
            &video,
            &video_file,
            first_frame_file.as_deref(),
        )?;
        sidecar["lines"] = Value::Array(spoken);
        bytes += write_json(&folder.join(&sidecar_file), &sidecar)?;
        runtime += u64::from(video.clip_seconds.unwrap_or(0));
        let entry = json!({
            "shot_id": key.shot(),
            "order": order,
            "video": video_file,
            "sidecar": sidecar_file,
            "first_frame": first_frame_file,
            "duration_seconds": video.clip_seconds,
            "lines": line_files,
        });
        match scenes.last_mut() {
            Some((scene, shots)) if scene == key.scene() => shots.push(entry),
            _ => scenes.push((key.scene().to_owned(), vec![entry])),
        }
    }
    let manifest = project.manifest();
    let placed = timeline::place(&cut);
    bytes += write_json(
        &folder.join(TIMELINE_FILE),
        &timeline::otio(&manifest.title, &placed),
    )?;
    let list = timeline::edl(&manifest.title, &placed).map_err(ExportError::Timeline)?;
    std::fs::write(folder.join(EDL_FILE), &list)?;
    bytes += list.len() as u64;
    let scenes = scenes
        .into_iter()
        .map(|(scene_id, shots)| {
            let intent = load_scene_intent(project, &scene_id)?;
            Ok(json!({
                "scene_id": scene_id,
                "location": intent.as_ref().and_then(|intent| intent.location.clone()),
                "time": intent.as_ref().and_then(|intent| intent.time.clone()),
                "shots": shots,
            }))
        })
        .collect::<Result<Vec<_>, ShotWorkflowError>>()?;
    let document = json!({
        "schema_version": EXPORT_SCHEMA_VERSION,
        "kind": "kineto.export",
        "project": {
            "project_id": manifest.project_id,
            "title": manifest.title,
        },
        "recipe": {
            "id": manifest.workflow.recipe_id,
            "version": manifest.workflow.recipe_version,
        },
        "exported_at": rfc3339(exported_at),
        "complete": readiness.missing.is_empty(),
        "scenes": scenes,
        "missing": readiness.missing.iter().map(|missing| json!({
            "scene_id": missing.shot.scene(),
            "shot_id": missing.shot.shot(),
            "order": missing.order,
            "reason": missing.reason,
        })).collect::<Vec<_>>(),
        "total_runtime_seconds": runtime,
        "timeline": {
            "otio": TIMELINE_FILE,
            "edl": EDL_FILE,
            "frame_rate": timeline::FRAME_RATE,
            "trimmed_lines": placed.trimmed.iter().map(|trimmed| json!({
                "shot_id": trimmed.shot_id,
                "line": trimmed.line,
                "cut_ms": trimmed.cut_ms,
            })).collect::<Vec<_>>(),
        },
    });
    bytes += write_json(&folder.join("manifest.json"), &document)?;
    Ok((bytes, carried_lines))
}

fn sidecar(
    shot: &ShotWorkflow,
    key: &ShotKey,
    order: usize,
    video: &LockedVideo,
    video_file: &str,
    first_frame_file: Option<&str>,
) -> Result<Value, ExportError> {
    let intent = shot.intent()?;
    Ok(json!({
        "schema_version": EXPORT_SCHEMA_VERSION,
        "kind": "kineto.export.shot",
        "scene_id": key.scene(),
        "shot_id": key.shot(),
        "order": order,
        "video": {
            "file": video_file,
            "artifact_id": video.artifact_id,
            "content_hash": video.content_hash.as_str(),
            "provider": video.provider,
            "model": video.model,
            "clip_seconds": video.clip_seconds,
            "parameters": video.parameters,
        },
        "first_frame": {
            "artifact_id": video.first_frame.artifact_id.as_str(),
            "content_hash": video.first_frame.content_hash.as_str(),
            "file": first_frame_file,
        },
        "prompt": video.provenance.as_ref().map(|provenance| json!({
            "compiler_version": provenance.compiler_version,
            "compiled_prompt": provenance.compiled_prompt,
            "prompt_hash": provenance.prompt_hash,
            "applied_constraints": provenance.applied_constraints,
            "dropped_constraints": provenance.dropped_constraints,
        })),
        "intent": {
            "action": intent.as_ref().map(|intent| intent.action.clone()).unwrap_or_default(),
            "framing": intent.as_ref().and_then(|intent| intent.framing.clone()),
            "lens": intent.as_ref().and_then(|intent| intent.lens.clone()),
            "camera_motion": intent.as_ref().and_then(|intent| intent.camera_motion.clone()),
            "emotion": intent.as_ref().and_then(|intent| intent.emotion.clone()),
            "dialogue": intent.as_ref().and_then(|intent| intent.dialogue.clone()),
            "characters": intent.as_ref().map(|intent| intent.characters.clone()).unwrap_or_default(),
            "duration_target": intent.as_ref().and_then(|intent| intent.duration_target),
            "continuity": intent.as_ref().map(|intent| intent.continuity.clone()).unwrap_or_default(),
            "continuity_from": intent.as_ref().and_then(|intent| intent.continuity_from.clone()),
        },
        "notes": video.notes,
        "branched_from": video.branched_from,
    }))
}

fn dubbed_lines(project: &CanonicalProject) -> Result<Vec<DubbedLine>, ExportError> {
    dubbing::dubbed_lines(project).map_err(|error| ExportError::Lines(Box::new(error)))
}

/// A recorded media path, if it is valid and its file is there.
fn present(project: &CanonicalProject, path: &str) -> Option<ProjectRelativePath> {
    ProjectRelativePath::new(path)
        .ok()
        .filter(|path| project.root().resolve(path).is_file())
}

/// The lines of `scene_id` a shot's `dialogue` holds verbatim, in the order
/// it says them.
fn carried<'a>(
    lines: &'a [DubbedLine],
    scene_id: &str,
    dialogue: Option<&str>,
) -> Vec<&'a DubbedLine> {
    let Some(dialogue) = dialogue.filter(|text| !text.trim().is_empty()) else {
        return Vec::new();
    };
    let mut found: Vec<(usize, &DubbedLine)> = lines
        .iter()
        .filter(|line| line.line.scene_id == scene_id && !line.line.text.is_empty())
        .filter_map(|line| dialogue.find(&line.line.text).map(|at| (at, line)))
        .collect();
    found.sort_by_key(|(at, line)| (*at, line.line.line));
    found.into_iter().map(|(_, line)| line).collect()
}

/// A shot's files are named `<scene>_<shot>`, or by the shot ID alone when
/// it already begins with its scene's: shot IDs are unique in a project.
fn file_stem(key: &ShotKey) -> String {
    if key
        .shot()
        .strip_prefix(key.scene())
        .is_some_and(|rest| rest.starts_with('_'))
    {
        key.shot().to_owned()
    } else {
        format!("{}_{}", key.scene(), key.shot())
    }
}

/// Copy `source` to `target` after checking its bytes hash to `hash`; the
/// bytes copied.
fn copy_checked(
    project: &CanonicalProject,
    source: &kineto_project::fs::ProjectRelativePath,
    hash: &ContentHash,
    target: &Path,
) -> Result<u64, ExportError> {
    let bytes = project
        .root()
        .read(source)
        .map_err(|_| ExportError::Mismatch(source.as_path().display().to_string()))?;
    if ContentHash::of(&bytes) != *hash {
        return Err(ExportError::Mismatch(
            source.as_path().display().to_string(),
        ));
    }
    std::fs::write(target, &bytes)?;
    Ok(bytes.len() as u64)
}

fn write_json(path: &Path, value: &Value) -> Result<u64, ExportError> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| ExportError::Io(std::io::Error::other(error)))?;
    bytes.push(b'\n');
    std::fs::write(path, &bytes)?;
    Ok(bytes.len() as u64)
}

/// `time` as an RFC 3339 UTC timestamp to the second.
#[must_use]
pub fn rfc3339(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let rest = seconds % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_shot_named_after_its_scene_is_not_named_twice() {
        let named = |scene: &str, shot: &str| file_stem(&ShotKey::new(scene, shot).unwrap());
        assert_eq!(named("scene_001", "shot_001"), "scene_001_shot_001");
        assert_eq!(
            named("scene_the_wait", "scene_the_wait_03"),
            "scene_the_wait_03"
        );
        assert_eq!(named("office_night", "office_night_01"), "office_night_01");
        assert_eq!(named("office", "night_01"), "office_night_01");
    }

    #[test]
    fn timestamps_are_rfc3339_utc() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00Z"
        );
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(1_791_015_045)),
            "2026-10-03T08:10:45Z"
        );
    }
}
