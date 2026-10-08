//! A clip closes on the locked keyframe of the shot that continues from it,
//! where the provider takes a last frame and that image: it is sent, the
//! video depends on it, and a later change to that keyframe makes the video
//! stale and unlockable. Where the provider cannot, the prompt says why. A
//! provider whose tasks would not survive a restart is refused before
//! anything is written.
//!
//! The `planned` fixture's `office_night_02` continues from
//! `office_night_01`. As planned, it cuts from a static wide shot on 28mm
//! to a medium close-up on 50mm; most tests first make `office_night_01` a
//! dolly in on 50mm, which carries the framing on to it. Both shots'
//! keyframes are drawn by a fake MiniMax as real JPEGs, alike unless a
//! test draws them otherwise: a clip closes only on a keyframe that looks
//! like its own.

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

use base64::Engine as _;
use kineto_core::{
    generation::{self, GenerationError, NoHooks},
    minimax::MinimaxKeyframes,
    video::{self, DEMO_CLIP, LocalDemoVideos, TaskState, VideoGenerator, VideoSubmission},
};
use kineto_jobs::{ErrorClass, ProviderJobId};
use kineto_project::{
    ContentHash,
    manifest::CanonicalProject,
    shot::{ShotKey, ShotWorkflow, ShotWorkflowError, VideoState},
};
use kineto_provider_minimax::{HttpResponse, MinimaxClient, Transport, TransportFailure};
use kineto_providers::{ImageFormat, Price, VideoCapabilities, VideoClip, secret::SecretString};

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

/// A 160x90 JPEG of a place (`place` seeds its texture) with a lit
/// figure at `x`, as wide as `size`; `nudge` changes one pixel, so every
/// keyframe's bytes differ.
fn picture((place, x, size): (u32, u32, u32), nudge: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(160, 90, |px, py| {
        if (px, py) == (0, 0) {
            image::Rgb([u8::try_from(nudge % 256).unwrap(), 0, 0])
        } else if px.abs_diff(x) < size / 2 && py.abs_diff(45) < size * 3 / 4 {
            image::Rgb([200, 170, 150])
        } else {
            let grain = (px / 4).wrapping_mul(73_856_093)
                ^ (py / 4).wrapping_mul(19_349_663)
                ^ place.wrapping_mul(83_492_791);
            let level = u8::try_from(30 + grain % 90).unwrap();
            image::Rgb([level, level, level.saturating_add(10)])
        }
    });
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
    bytes
}

/// MiniMax `image-01`, offline, drawing `figure` for every keyframe.
struct Drawing {
    figure: (u32, u32, u32),
    calls: AtomicU64,
}

impl Transport for Drawing {
    fn post_json(
        &self,
        _url: &str,
        _api_key: &SecretString,
        _body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        let call = u32::try_from(self.calls.fetch_add(1, Ordering::SeqCst)).unwrap();
        Ok(HttpResponse {
            status: 200,
            body: format!(
                r#"{{"id":"k{call}","data":{{"image_base64":["{}"]}},"base_resp":{{"status_code":0}}}}"#,
                base64::engine::general_purpose::STANDARD.encode(picture(self.figure, call))
            )
            .into_bytes(),
        })
    }

    fn get(&self, url: &str, _api_key: &SecretString) -> Result<HttpResponse, TransportFailure> {
        unreachable!("keyframes make no GET request: {url}")
    }

    fn download(&self, url: &str, _max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
        unreachable!("keyframes download nothing: {url}")
    }
}

/// Lock `key`'s first keyframe candidate, drawn as `figure` says.
fn draw(project: &CanonicalProject, key: &ShotKey, figure: (u32, u32, u32)) {
    let client = MinimaxClient::with_transport(
        Drawing {
            figure,
            calls: AtomicU64::new(u64::from(figure.1)),
        },
        SecretString::new("sk-test".to_owned()).unwrap(),
        "https://example.test",
    );
    let mut shot = ShotWorkflow::load(project, key).unwrap();
    generation::generate_with(
        project,
        &mut shot,
        MinimaxKeyframes::new(project, Some(&client)),
        &mut NoHooks,
    )
    .unwrap();
    shot.select(project, 0).unwrap();
    shot.lock(project).unwrap();
}

/// The place, where the figure stands and how wide it is, in a keyframe.
const FIGURE: (u32, u32, u32) = (1, 80, 30);

/// The planned film with `office_night_01` a slow dolly in on 50mm that
/// `office_night_02` carries on, both keyframes locked and alike.
fn film(target: &Path) -> CanonicalProject {
    filmed(target, Some(("50mm", "slow_dolly_in")), FIGURE)
}

/// [`film`], with `office_night_01` on `setup`'s lens and camera motion,
/// or as planned, and `office_night_02`'s keyframe drawn with `next`'s
/// figure.
fn filmed(target: &Path, setup: Option<(&str, &str)>, next: (u32, u32, u32)) -> CanonicalProject {
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/planned"),
        target,
    );
    if let Some((lens, motion)) = setup {
        let path = target.join("scenes/office_night/shots/office_night_01/shot.json");
        let mut record: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        record["lens"] = lens.into();
        record["camera_motion"] = motion.into();
        fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    }
    let project = CanonicalProject::open(target).unwrap();
    draw(&project, &opening(), FIGURE);
    draw(&project, &following(), next);
    project
}

/// A provider that takes a last frame in `formats`, recording what it was
/// sent. Its tasks finish at once with the demo clip.
#[derive(Clone)]
struct Closing {
    formats: Vec<ImageFormat>,
    resumable: bool,
    /// The clip lengths it closes on a last frame at; any when empty.
    closes_at: Vec<u8>,
    sent: Arc<Mutex<Vec<VideoSubmission>>>,
}

impl Closing {
    fn new(formats: &[ImageFormat]) -> Self {
        Self {
            formats: formats.to_vec(),
            resumable: true,
            closes_at: Vec::new(),
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
            end_frame_seconds: self.closes_at.iter().copied().collect(),
            ..video::demo_video_capabilities()
        }
    }

    fn needs_frame_image(&self) -> bool {
        false
    }

    fn canonical_params(&self, request: &VideoSubmission) -> String {
        format!(
            "duration={};lines={}",
            request.clip_seconds,
            request.lines.unwrap_or(0)
        )
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
    let project = film(&target);
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
    let project = film(&temp.0.join("film"));
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
    let project = film(&temp.0.join("film"));
    // The demo takes no last frame.
    make(&project, &LocalDemoVideos);
    let (_, _, dropped) = compiled(&project);
    assert!(dropped.contains(&(
        "continuity.end_frame".to_owned(),
        "office_night_02 continues from this shot, but the provider takes no last frame".to_owned()
    )));

    // One that takes only WebP cannot take the next shot's JPEG.
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"));
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

    // One that closes on a last frame only in 8 s clips, for a 2 s clip.
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"));
    let long = Closing {
        closes_at: vec![8],
        ..Closing::new(&[ImageFormat::Jpeg])
    };
    make(&project, &long);
    assert!(long.sent.lock().unwrap()[0].last_frame.is_none());
    let (_, _, dropped) = compiled(&project);
    assert!(dropped.contains(&(
        "continuity.end_frame".to_owned(),
        "office_night_02 continues from this shot, but the provider closes on a last frame only \
         in a clip of 8 s, and this clip is 2 s"
            .to_owned()
    )));
}

#[test]
fn a_next_shot_that_cuts_away_is_not_closed_on() {
    let temp = TempDir::new();
    // As planned: a static wide shot on 28mm, then a close one on 50mm.
    let project = filmed(&temp.0.join("film"), None, FIGURE);
    let closing = Closing::new(&[ImageFormat::Jpeg]);
    let shot = make(&project, &closing);
    assert!(closing.sent.lock().unwrap()[0].last_frame.is_none());
    let (prompt, applied, dropped) = compiled(&project);
    assert!(!prompt.contains("End on the opening frame"), "{prompt}");
    assert!(!applied.contains(&"continuity.end_frame".to_owned()));
    assert!(
        dropped.contains(&(
            "continuity.end_frame".to_owned(),
            "office_night_02 continues from this shot, but it is a cut, not a continuation: the \
         lens changes from 28mm to 50mm"
                .to_owned()
        ))
    );
    // Nothing closes on the next shot, so nothing goes stale with it.
    assert!(shot.locked_video().unwrap().is_none());
    assert_eq!(shot.video_staleness().unwrap(), Vec::<String>::new());

    // On one lens, a static camera still does not explain the change of
    // framing.
    let temp = TempDir::new();
    let project = filmed(&temp.0.join("film"), Some(("50mm", "static")), FIGURE);
    let closing = Closing::new(&[ImageFormat::Jpeg]);
    make(&project, &closing);
    assert!(closing.sent.lock().unwrap()[0].last_frame.is_none());
    let (_, _, dropped) = compiled(&project);
    assert!(
        dropped.contains(&(
            "continuity.end_frame".to_owned(),
            "office_night_02 continues from this shot, but it is a cut, not a continuation: the \
         framing changes from wide to medium close up, which a static camera does not explain"
                .to_owned()
        ))
    );
}

#[test]
fn a_next_keyframe_framed_unlike_this_ones_is_not_closed_on() {
    let temp = TempDir::new();
    // Planned as one setup, but the next keyframe shows another place,
    // its figure elsewhere.
    let project = filmed(
        &temp.0.join("film"),
        Some(("50mm", "slow_dolly_in")),
        (2, 130, 60),
    );
    let closing = Closing::new(&[ImageFormat::Jpeg]);
    make(&project, &closing);
    assert!(closing.sent.lock().unwrap()[0].last_frame.is_none());
    let (_, applied, dropped) = compiled(&project);
    assert!(!applied.contains(&"continuity.end_frame".to_owned()));
    let (_, why) = dropped
        .iter()
        .find(|(id, _)| id == "continuity.end_frame")
        .unwrap();
    assert!(
        why.starts_with(
            "office_night_02 continues from this shot, but it is a cut, not a continuation: its \
             keyframe is framed unlike this shot's (likeness 0."
        ) && why.ends_with(", below 0.50)"),
        "{why}"
    );
}

#[test]
fn a_provider_whose_tasks_do_not_survive_a_restart_is_refused_before_anything_is_written() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target);
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
