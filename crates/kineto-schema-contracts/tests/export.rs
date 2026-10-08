//! A production package, as an editor without Kineto receives it: only
//! locked videos that still follow their shots, each checked against its
//! record, with sidecars and a manifest that match the committed schemas,
//! never half written, and the same bytes every time it is made again.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use kineto_core::{
    bootstrap,
    export::{self, ExportError, ExportRequest},
    video::{self, DEMO_CLIP, LocalDemoVideos},
};
use kineto_json_schema::{ensure_supported_schema, validate_value};
use kineto_project::{
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    shot::{ShotKey, ShotWorkflow},
};
use serde_json::Value;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = format!("{nonce}-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let path = env::temp_dir().join(format!("kineto-export-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn schema(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas")
        .join(name);
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    ensure_supported_schema(&value, "$").unwrap();
    value
}

fn json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "export".to_owned(),
        title: "The Last Question".to_owned(),
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

/// The bootstrap film's shot `number`, its demo keyframe locked and a demo
/// video made, locked when `lock`.
fn shoot(project: &CanonicalProject, number: u32, lock: bool) {
    let mut shot = ShotWorkflow::load(project, &ShotKey::bootstrap(number)).unwrap();
    shot.generate(project).unwrap();
    shot.select(project, 0).unwrap();
    shot.lock(project).unwrap();
    let job = video::start(project, &shot, &LocalDemoVideos).unwrap();
    let done = video::watch(
        &LocalDemoVideos,
        project.root(),
        job,
        &AtomicBool::new(false),
        Duration::ZERO,
    )
    .unwrap();
    video::finish(project, &mut shot, done).unwrap();
    if lock {
        shot.lock_video(project).unwrap();
    }
}

fn request(destination: &Path, name: &str, partial: bool, replace: bool) -> ExportRequest {
    ExportRequest {
        destination: destination.to_path_buf(),
        name: name.to_owned(),
        partial,
        replace,
        timeline: true,
    }
}

const AT: Duration = Duration::from_secs(1_791_015_045);

#[test]
fn a_package_holds_the_locked_shots_with_their_sidecars_and_a_manifest() {
    let temp = TempDir::new();
    let project = CanonicalProject::create_with_files(
        temp.0.join("film"),
        manifest(),
        b"A filmmaker waits.\n",
        &bootstrap::scene_files(),
    )
    .unwrap();
    let out = temp.0.join("out");
    fs::create_dir_all(&out).unwrap();
    shoot(&project, 1, true);
    shoot(&project, 2, false);

    // The second shot's video is not locked: nothing is written unless a
    // partial package is asked for.
    let readiness = export::readiness(&project).unwrap();
    assert_eq!(readiness.ready, [(ShotKey::bootstrap(1), 1)]);
    assert_eq!(readiness.missing.len(), 1);
    assert_eq!(readiness.missing[0].reason, "its video is not locked");
    let Err(ExportError::Incomplete(missing)) = export::export(
        &project,
        &request(&out, "dailies", false, false),
        UNIX_EPOCH + AT,
    ) else {
        panic!("an incomplete package needs the director's say");
    };
    assert_eq!(missing, readiness.missing);
    assert!(
        fs::read_dir(&out).unwrap().next().is_none(),
        "nothing is written"
    );

    let partial = export::export(
        &project,
        &request(&out, "dailies", true, false),
        UNIX_EPOCH + AT,
    )
    .unwrap();
    assert_eq!((partial.shots, partial.complete), (1, false));
    let package = out.join("dailies");
    let manifest = json(&package.join("manifest.json"));
    validate_value(&schema("export-manifest.schema.json"), &manifest, "$").unwrap();
    assert_eq!(manifest["complete"], false);
    assert_eq!(manifest["exported_at"], "2026-10-03T08:10:45Z");
    assert_eq!(manifest["missing"][0]["shot_id"], "shot_002");
    assert_eq!(manifest["missing"][0]["order"], 2);
    assert_eq!(manifest["total_runtime_seconds"], 2);
    let shot = &manifest["scenes"][0]["shots"][0];
    assert_eq!(shot["video"], "scene_001_shot_001.mp4");
    assert_eq!(
        shot["first_frame"],
        Value::Null,
        "a demo keyframe has no image"
    );
    assert_eq!(
        fs::read(package.join("scene_001_shot_001.mp4")).unwrap(),
        DEMO_CLIP
    );
    let sidecar = json(&package.join("scene_001_shot_001.json"));
    validate_value(&schema("export-shot.schema.json"), &sidecar, "$").unwrap();
    assert_eq!(
        sidecar["video"]["artifact_id"],
        "shot_001_v0001_candidate_a"
    );
    assert_eq!(sidecar["video"]["provider"], "local-demo-video");
    assert_eq!(sidecar["video"]["clip_seconds"], 2);
    assert!(
        sidecar["video"]["locked_at"].is_string(),
        "when it was locked"
    );
    assert_eq!(sidecar["prompt"]["compiler_version"], "video-prompt/7");
    assert!(sidecar["intent"]["action"].as_str().unwrap().len() > 10);
    assert_eq!(
        sidecar["first_frame"]["artifact_id"],
        "shot_001_g0001_candidate_a"
    );

    // Every shot locked: a complete package, scene by scene in shot order.
    let mut second = ShotWorkflow::load(&project, &ShotKey::bootstrap(2)).unwrap();
    second.lock_video_at(&project, UNIX_EPOCH + AT).unwrap();
    let Err(ExportError::Exists(_)) = export::export(
        &project,
        &request(&out, "dailies", false, false),
        UNIX_EPOCH + AT,
    ) else {
        panic!("a package is not replaced unasked");
    };
    let complete = export::export(
        &project,
        &request(&out, "dailies", false, true),
        UNIX_EPOCH + AT,
    )
    .unwrap();
    assert!(complete.complete);
    let manifest = json(&package.join("manifest.json"));
    validate_value(&schema("export-manifest.schema.json"), &manifest, "$").unwrap();
    let scene = &manifest["scenes"][0];
    assert_eq!(scene["scene_id"], "scene_001");
    assert_eq!(
        scene["shots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|shot| (
                shot["shot_id"].as_str().unwrap(),
                shot["order"].as_u64().unwrap()
            ))
            .collect::<Vec<_>>(),
        [("shot_001", 1), ("shot_002", 2)]
    );
    assert_eq!(manifest["total_runtime_seconds"], 4);
    assert!(manifest["missing"].as_array().unwrap().is_empty());
    assert_eq!(
        json(&package.join("scene_001_shot_002.json"))["video"]["locked_at"],
        export::rfc3339(UNIX_EPOCH + AT),
        "the lock time is the canonical record's"
    );

    // Made again, the package is the same, byte for byte.
    export::export(
        &project,
        &request(&out, "again", false, false),
        UNIX_EPOCH + AT,
    )
    .unwrap();
    let names = |folder: &Path| {
        let mut names: Vec<_> = fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    assert_eq!(names(&package), names(&out.join("again")));
    for name in names(&package) {
        assert_eq!(
            fs::read(package.join(&name)).unwrap(),
            fs::read(out.join("again").join(&name)).unwrap(),
            "{name:?}"
        );
    }
    assert_eq!(
        names(&out),
        ["again", "dailies"],
        "nothing is left beside them"
    );
}

#[test]
fn bytes_that_are_not_what_the_record_says_are_never_exported() {
    let temp = TempDir::new();
    let project = CanonicalProject::create_with_files(
        temp.0.join("film"),
        manifest(),
        b"A filmmaker waits.\n",
        &bootstrap::scene_files(),
    )
    .unwrap();
    let out = temp.0.join("out");
    fs::create_dir_all(&out).unwrap();
    shoot(&project, 1, true);
    shoot(&project, 2, true);
    // A package interrupted earlier left its staging folder.
    fs::create_dir_all(out.join(".dailies.partial")).unwrap();
    fs::write(out.join(".dailies.partial/stray.mp4"), b"stray").unwrap();

    let shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(2)).unwrap();
    let media = shot.locked_video().unwrap().unwrap().media_path;
    fs::write(
        project.root().resolve(&media),
        b"\0\0\0\x18ftypisom-changed",
    )
    .unwrap();
    let Err(ExportError::Mismatch(path)) = export::export(
        &project,
        &request(&out, "dailies", false, false),
        UNIX_EPOCH + AT,
    ) else {
        panic!("changed bytes are refused");
    };
    assert!(path.ends_with("shot_002_v0001_candidate_a.mp4"));
    assert!(
        fs::read_dir(&out).unwrap().next().is_none(),
        "no half package is left"
    );
    assert!(matches!(
        export::export(
            &project,
            &request(&out, "../escape", true, false),
            UNIX_EPOCH + AT
        ),
        Err(ExportError::InvalidName(_))
    ));
}
