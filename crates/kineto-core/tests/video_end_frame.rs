//! A clip closes on the locked keyframe of the shot that continues from it,
//! where the provider takes a last frame and that image: it is sent, the
//! video depends on it, and a later change to that keyframe makes the video
//! stale and unlockable. Where the provider cannot, the prompt says why. A
//! provider whose tasks would not survive a restart is refused before
//! anything is written.
//!
//! The `planned` fixture's `office_night_02` continues from
//! `office_night_01` and has keyframe candidates with images.

use std::{
    convert::Infallible,
    env, fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use kineto_core::{
    generation::GenerationError,
    video::{self, DEMO_CLIP, LocalDemoVideos, TaskState, VideoGenerator, VideoSubmission},
};
use kineto_jobs::{ErrorClass, ProviderJobId};
use kineto_project::{
    ContentHash,
    manifest::CanonicalProject,
    shot::{ShotKey, ShotWorkflow, ShotWorkflowError, VideoState},
};
use kineto_providers::{ImageFormat, Price, VideoCapabilities, VideoClip};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "kineto-video-end-frame-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn opening() -> ShotKey {
    ShotKey::new("office_night", "office_night_01").unwrap()
}

fn following() -> ShotKey {
    ShotKey::new("office_night", "office_night_02").unwrap()
}

/// The planned film with `office_night_01`'s keyframe locked (a demo one,
/// without an image) and `office_night_02`'s locked at candidate `which`.
fn film(target: &Path, which: usize) -> CanonicalProject {
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/planned"),
        target,
    );
    let project = CanonicalProject::open(target).unwrap();
    let mut first = ShotWorkflow::load(&project, &opening()).unwrap();
    first.generate(&project).unwrap();
    first.select(&project, 0).unwrap();
    first.lock(&project).unwrap();
    let mut next = ShotWorkflow::load(&project, &following()).unwrap();
    next.select(&project, which).unwrap();
    next.lock(&project).unwrap();
    project
}

/// A provider that takes a last frame in `formats`, recording what it was
/// sent. Its tasks finish at once with the demo clip.
#[derive(Clone)]
struct Closing {
    formats: Vec<ImageFormat>,
    resumable: bool,
    sent: Arc<Mutex<Vec<VideoSubmission>>>,
}

impl Closing {
    fn new(formats: &[ImageFormat]) -> Self {
        Self {
            formats: formats.to_vec(),
            resumable: true,
            sent: Arc::default(),
        }
    }
}

impl VideoGenerator for Closing {
    type Error = Infallible;

    fn provider(&self) -> &'static str {
        "closing-video"
    }

    fn model(&self) -> &'static str {
        "closing-1"
    }

    fn capabilities(&self) -> VideoCapabilities {
        VideoCapabilities {
            end_frame: true,
            end_frame_formats: self.formats.iter().copied().collect(),
            max_end_frame_bytes: Some(20 * 1024 * 1024),
            resolutions: [360, 720].into_iter().collect(),
            clip_prices: [(
                VideoClip {
                    seconds: 2,
                    lines: 720,
                },
                Price::usd(0),
            )]
            .into_iter()
            .collect(),
            resumable_remote_handle: self.resumable,
            ..video::demo_video_capabilities()
        }
    }

    fn needs_frame_image(&self) -> bool {
        false
    }

    fn canonical_params(&self, clip_seconds: u8, lines: Option<u16>) -> String {
        format!("duration={clip_seconds};lines={}", lines.unwrap_or(0))
    }

    fn submit(&self, request: &VideoSubmission) -> Result<String, Infallible> {
        let mut sent = self.sent.lock().unwrap();
        sent.push(request.clone());
        Ok(format!("closing-{}", sent.len()))
    }

    fn query(&self, task: &ProviderJobId) -> Result<TaskState, Infallible> {
        Ok(TaskState::Succeeded {
            file: task.as_str().to_owned(),
        })
    }

    fn fetch(&self, _file: &str) -> Result<Vec<u8>, Infallible> {
        Ok(DEMO_CLIP.to_vec())
    }

    fn class(&self, error: &Infallible) -> ErrorClass {
        match *error {}
    }

    fn certainly_not_accepted(&self, error: &Infallible) -> bool {
        match *error {}
    }
}

fn make<G: VideoGenerator>(project: &CanonicalProject, generator: &G) -> ShotWorkflow {
    let mut shot = ShotWorkflow::load(project, &opening()).unwrap();
    let job = video::start(project, &shot, generator).unwrap();
    let done = video::watch(
        generator,
        project.root(),
        job,
        &AtomicBool::new(false),
        Duration::ZERO,
    )
    .unwrap();
    video::finish(project, &mut shot, done).unwrap();
    shot
}

/// The latest video's compiled prompt, what it applied and what it
/// dropped, as its record keeps them.
fn compiled(project: &CanonicalProject) -> (String, Vec<String>, Vec<(String, String)>) {
    let artifacts: serde_json::Value = serde_json::from_slice(
        &fs::read(
            project.root().resolve(
                &kineto_project::fs::ProjectRelativePath::new(
                    "scenes/office_night/shots/office_night_01/artifacts.json",
                )
                .unwrap(),
            ),
        )
        .unwrap(),
    )
    .unwrap();
    let video = artifacts
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|artifact| artifact["type"] == "shot_video")
        .unwrap();
    let texts = |field: &str| -> Vec<String> {
        video[field]
            .as_array()
            .unwrap()
            .iter()
            .map(|text| text.as_str().unwrap().to_owned())
            .collect()
    };
    (
        video["compiled_prompt"].as_str().unwrap().to_owned(),
        texts("applied_constraints"),
        video["dropped_constraints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|dropped| {
                (
                    dropped["constraint"].as_str().unwrap().to_owned(),
                    dropped["reason"].as_str().unwrap().to_owned(),
                )
            })
            .collect(),
    )
}

#[test]
fn a_clip_closes_on_the_next_shots_keyframe_and_goes_stale_when_it_changes() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, 0);
    let closing = Closing::new(&[ImageFormat::Jpeg, ImageFormat::Png]);
    let mut shot = make(&project, &closing);

    let next = ShotWorkflow::load(&project, &following()).unwrap();
    let (keyframe, media) = next.first_frame().unwrap().unwrap();
    let expected = fs::read(project.root().resolve(&media.unwrap())).unwrap();
    {
        let sent = closing.sent.lock().unwrap();
        let [request] = sent.as_slice() else {
            panic!("one submission: {sent:?}");
        };
        let (bytes, format) = request.last_frame.as_ref().expect("the clip closes on it");
        assert_eq!(*bytes, expected);
        assert_eq!(*format, ImageFormat::Jpeg);
        // The lowest production height it renders.
        assert_eq!(request.lines, Some(720));
    }
    let (prompt, applied, dropped) = compiled(&project);
    assert!(
        prompt.contains(
            "End on the opening frame of office_night_02, which continues from this shot."
        ),
        "{prompt}"
    );
    assert!(applied.contains(&"continuity.end_frame".to_owned()));
    assert!(!dropped.iter().any(|(id, _)| id == "continuity.end_frame"));
    assert_eq!(shot.video_staleness().unwrap(), Vec::<String>::new());
    assert_eq!(shot.snapshot().unwrap().video, VideoState::Ready);

    // Locked, it says what it closes on.
    shot.lock_video(&project).unwrap();
    let end = shot.locked_video().unwrap().unwrap().end_frame.unwrap();
    assert_eq!(end.shot, following());
    assert_eq!(end.keyframe, ContentHashed::of(&keyframe, &expected));

    // The next shot lets its keyframe go: the clip no longer meets it.
    let mut next = ShotWorkflow::load(&project, &following()).unwrap();
    next.reset(&project).unwrap();
    let reloaded = ShotWorkflow::load(&project, &opening()).unwrap();
    assert_eq!(reloaded.video_staleness().unwrap(), ["end_frame"]);
    assert_eq!(reloaded.snapshot().unwrap().video, VideoState::Stale);
}

/// The dependency a video closing on `keyframe`, of `bytes`, records.
struct ContentHashed;

impl ContentHashed {
    fn of(
        keyframe: &kineto_project::ArtifactDependency,
        bytes: &[u8],
    ) -> kineto_project::ArtifactDependency {
        assert_eq!(keyframe.content_hash, ContentHash::of(bytes));
        kineto_project::ArtifactDependency {
            field: Some("end_frame".to_owned()),
            ..keyframe.clone()
        }
    }
}

#[test]
fn a_clip_whose_closing_frame_changed_is_not_locked() {
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"), 0);
    let mut shot = make(&project, &Closing::new(&[ImageFormat::Jpeg]));
    // The next shot no longer continues from this one.
    let path = project.root().resolve(
        &kineto_project::fs::ProjectRelativePath::new(
            "scenes/office_night/shots/office_night_02/shot.json",
        )
        .unwrap(),
    );
    let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["continuity_from"] = serde_json::Value::Null;
    fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    // Judged as the shots are now, not as they were loaded.
    assert!(matches!(
        shot.lock_video(&project),
        Err(ShotWorkflowError::StaleVideo)
    ));
    assert_eq!(shot.video_staleness().unwrap(), ["end_frame"]);
}

#[test]
fn where_no_last_frame_can_be_sent_the_prompt_says_why() {
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"), 0);
    // The demo takes no last frame.
    make(&project, &LocalDemoVideos);
    let (_, _, dropped) = compiled(&project);
    assert!(dropped.contains(&(
        "continuity.end_frame".to_owned(),
        "office_night_02 continues from this shot, but the provider takes no last frame".to_owned()
    )));

    // One that takes only WebP cannot take the next shot's JPEG.
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"), 0);
    let webp = Closing::new(&[ImageFormat::Webp]);
    let shot = make(&project, &webp);
    assert!(webp.sent.lock().unwrap()[0].last_frame.is_none());
    let (_, _, dropped) = compiled(&project);
    assert!(
        dropped.contains(&(
            "continuity.end_frame".to_owned(),
            "office_night_02 continues from this shot, but its keyframe is not an image the \
         provider takes as a last frame"
                .to_owned()
        ))
    );
    assert_eq!(shot.video_staleness().unwrap(), Vec::<String>::new());
}

#[test]
fn a_provider_whose_tasks_do_not_survive_a_restart_is_refused_before_anything_is_written() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, 0);
    let forgetful = Closing {
        resumable: false,
        ..Closing::new(&[ImageFormat::Jpeg])
    };
    let shot = ShotWorkflow::load(&project, &opening()).unwrap();
    let Err(GenerationError::Incapable(missing)) = video::start(&project, &shot, &forgetful) else {
        panic!("refused");
    };
    assert_eq!(missing, ["video.resumable_remote_handle"]);
    assert!(forgetful.sent.lock().unwrap().is_empty());
    assert!(
        !target.join(".kineto/jobs/intents").exists() || {
            fs::read_dir(target.join(".kineto/jobs/intents"))
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().contains("_v0001"))
        }
    );
}
