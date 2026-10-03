//! Video providers are chosen by what they declare: one that would make a
//! shot's video from words alone is refused before anything is written, and
//! the free local demo runs through the same write-ahead job as a paid
//! provider, so a restart between submission and attach finishes it.

use std::{
    collections::BTreeMap,
    convert::Infallible,
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use kineto_core::{
    bootstrap,
    generation::GenerationError,
    video::{
        self, DEMO_CLIP, DEMO_VIDEO_PROVIDER, LocalDemoVideos, TaskState, VideoGenerator,
        VideoSubmission,
    },
};
use kineto_jobs::{ErrorClass, ProviderJobId};
use kineto_project::{
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    shot::{ShotKey, ShotWorkflow, VideoState},
};
use kineto_providers::{VideoCapabilities, VideoContainer, VideoMode};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = format!("{nonce}-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let path = env::temp_dir().join(format!(
            "kineto-video-providers-{}-{nonce}",
            std::process::id()
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

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "video_providers".to_owned(),
        title: "Video Providers".to_owned(),
        created_at: "2026-10-03T00:00:00Z".to_owned(),
        source: ProjectSource {
            kind: "text".to_owned(),
            path: "source/story.txt".to_owned(),
            extra: BTreeMap::new(),
        },
        defaults: ProjectDefaults {
            language: "en".to_owned(),
            extra: BTreeMap::new(),
        },
        workflow: ProjectWorkflow {
            recipe_id: "default-film".to_owned(),
            recipe_version: 1,
            extra: BTreeMap::new(),
        },
        extra: BTreeMap::new(),
    }
}

/// The bootstrap film with shot 1's demo keyframe locked.
fn film(target: &Path) -> (CanonicalProject, ShotWorkflow) {
    let project = CanonicalProject::create_with_files(
        target,
        manifest(),
        b"A filmmaker waits.\n",
        &bootstrap::scene_files(),
    )
    .unwrap();
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
    shot.generate(&project).unwrap();
    shot.select(&project, 0).unwrap();
    shot.lock(&project).unwrap();
    (project, shot)
}

/// A provider that makes video from words alone, and counts what it is
/// asked.
#[derive(Default)]
struct TextOnly {
    submits: AtomicU32,
}

impl VideoGenerator for TextOnly {
    type Error = Infallible;

    fn provider(&self) -> &'static str {
        "text-only-video"
    }

    fn model(&self) -> &'static str {
        "words"
    }

    fn capabilities(&self) -> VideoCapabilities {
        VideoCapabilities {
            modes: [VideoMode::TextToVideo].into_iter().collect(),
            min_duration_ms: Some(1_000),
            max_duration_ms: Some(8_000),
            async_jobs: true,
            output_containers: [VideoContainer::Mp4].into_iter().collect(),
            ..VideoCapabilities::default()
        }
    }

    fn needs_frame_image(&self) -> bool {
        false
    }

    fn canonical_params(&self, clip_seconds: u8) -> String {
        format!("duration={clip_seconds}")
    }

    fn submit(&self, _request: &VideoSubmission) -> Result<String, Infallible> {
        self.submits.fetch_add(1, Ordering::SeqCst);
        Ok("task".to_owned())
    }

    fn query(&self, _task: &ProviderJobId) -> Result<TaskState, Infallible> {
        Ok(TaskState::Pending)
    }

    fn fetch(&self, _file: &str) -> Result<Vec<u8>, Infallible> {
        Ok(Vec::new())
    }

    fn class(&self, error: &Infallible) -> ErrorClass {
        match *error {}
    }

    fn certainly_not_accepted(&self, error: &Infallible) -> bool {
        match *error {}
    }
}

#[test]
fn a_provider_that_would_not_start_from_the_keyframe_is_refused_before_anything_is_written() {
    let temp = TempDir::new();
    let (project, shot) = film(&temp.0.join("film"));
    let text_only = TextOnly::default();

    let Err(GenerationError::Incapable(missing)) = video::start(&project, &shot, &text_only) else {
        panic!("a text-to-video provider must be refused");
    };
    assert_eq!(missing, ["video.mode", "video.start_frame"]);
    assert_eq!(text_only.submits.load(Ordering::SeqCst), 0);
    assert!(
        video::in_flight(&project, "text-only-video")
            .unwrap()
            .jobs
            .is_empty()
    );
    assert_eq!(shot.snapshot().unwrap().video, VideoState::None);
}

#[test]
fn a_demo_video_left_between_submission_and_attach_is_finished_after_a_restart() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let (project, shot) = film(&target);
    let job = video::start(&project, &shot, &LocalDemoVideos).unwrap();
    assert_eq!(job.provider, DEMO_VIDEO_PROVIDER);
    // Asking again follows the same job rather than starting another.
    assert_eq!(
        video::start(&project, &shot, &LocalDemoVideos).unwrap(),
        job
    );
    drop((shot, project));

    let project = CanonicalProject::open(&target).unwrap();
    let mut found = video::in_flight(&project, DEMO_VIDEO_PROVIDER).unwrap();
    assert_eq!(found.jobs, [job]);
    assert!(found.unresolved.is_empty());
    assert_eq!(found.needs_attention, 0);

    let done = video::watch(
        &LocalDemoVideos,
        project.root(),
        found.jobs.remove(0),
        &AtomicBool::new(false),
        Duration::ZERO,
    )
    .unwrap();
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
    video::finish(&project, &mut shot, done).unwrap();
    assert_eq!(shot.snapshot().unwrap().video, VideoState::Ready);
    let media = shot.video_media().unwrap().unwrap();
    assert_eq!(project.root().read(&media).unwrap(), DEMO_CLIP);
    assert!(
        video::in_flight(&project, DEMO_VIDEO_PROVIDER)
            .unwrap()
            .jobs
            .is_empty()
    );
    shot.lock_video(&project).unwrap();
    assert_eq!(shot.snapshot().unwrap().video, VideoState::Locked);
}

fn edit_intent(target: &Path, shot: &str, field: &str, value: serde_json::Value) {
    let path = target.join(format!("scenes/scene_001/shots/{shot}/shot.json"));
    let mut intent: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    intent[field] = value;
    fs::write(&path, serde_json::to_vec_pretty(&intent).unwrap()).unwrap();
}

fn make_demo_video(project: &CanonicalProject, shot: &mut ShotWorkflow) {
    let job = video::start(project, shot, &LocalDemoVideos).unwrap();
    let done = video::watch(
        &LocalDemoVideos,
        project.root(),
        job,
        &AtomicBool::new(false),
        Duration::ZERO,
    )
    .unwrap();
    video::finish(project, shot, done).unwrap();
}

#[test]
fn a_video_says_what_its_provider_could_not_do_and_goes_stale_when_the_plan_moves() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let (project, shot) = film(&target);
    drop(shot);
    // The second shot continues from the first.
    edit_intent(&target, "shot_002", "continuity_from", "shot_001".into());
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
    make_demo_video(&project, &mut shot);

    let artifacts: serde_json::Value = serde_json::from_slice(
        &fs::read(target.join("scenes/scene_001/shots/shot_001/artifacts.json")).unwrap(),
    )
    .unwrap();
    let record = artifacts
        .as_array()
        .unwrap()
        .iter()
        .find(|artifact| artifact["type"] == "shot_video")
        .unwrap();
    assert_eq!(record["compiler_version"], "video-prompt/4");
    let dropped: Vec<(&str, &str)> = record["dropped_constraints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|dropped| {
            (
                dropped["constraint"].as_str().unwrap(),
                dropped["reason"].as_str().unwrap(),
            )
        })
        .collect();
    // The demo cannot move its camera and takes no last frame.
    assert!(
        dropped
            .iter()
            .any(|(constraint, reason)| *constraint == "shot.camera_motion"
                && reason.starts_with("the provider cannot direct the camera"))
    );
    assert!(dropped.contains(&(
        "continuity.end_frame",
        "shot_002 continues from this shot, but the provider takes no last frame"
    )));
    assert_eq!(record["intent"], shot.intent_record().unwrap());
    assert!(shot.video_staleness().unwrap().is_empty());

    // The plan's mood for the shot changes: the video no longer follows it,
    // says why, and is not locked.
    edit_intent(&target, "shot_001", "emotion", "Relief, at last.".into());
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
    assert_eq!(shot.video_staleness().unwrap(), ["intent.emotion"]);
    assert_eq!(shot.snapshot().unwrap().video, VideoState::Stale);
    assert!(shot.lock_video(&project).is_err());

    // Made again for the new mood, it is locked.
    make_demo_video(&project, &mut shot);
    assert!(shot.video_staleness().unwrap().is_empty());
    shot.lock_video(&project).unwrap();
    assert_eq!(shot.snapshot().unwrap().video, VideoState::Locked);
}
