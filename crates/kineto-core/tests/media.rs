//! The project's media as its records name it: every file is listed with
//! its owner and standing, a file that is gone or whose bytes changed is
//! reported rather than rewritten, and identical bytes are stored once.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use kineto_core::{
    bootstrap,
    video::{self, DEMO_CLIP, LocalDemoVideos},
};
use kineto_project::{
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    media::{self, MediaKind, MediaOwner, MediaState, Standing},
    shot::{ShotKey, ShotWorkflow},
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = format!("{nonce}-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let path = env::temp_dir().join(format!("kineto-media-{}-{nonce}", std::process::id()));
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

/// A copy of `fixtures/projects/planned`: a locked style, a cast with its
/// pack, and two generations of keyframes for one planned shot.
fn planned(target: &Path) -> CanonicalProject {
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/planned"),
        target,
    );
    CanonicalProject::open(target).unwrap()
}

#[test]
fn the_catalog_names_every_kept_file_and_reports_what_is_gone_or_changed() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = planned(&target);
    let records = media::catalog(&project).unwrap();

    let mut by_kind: BTreeMap<MediaKind, Vec<Standing>> = BTreeMap::new();
    for record in &records {
        assert_eq!(record.state, MediaState::Present);
        assert!(
            project.root().resolve(&record.path).is_file(),
            "{}",
            record.artifact_id
        );
        by_kind
            .entry(record.kind)
            .or_default()
            .push(record.standing);
    }
    let count = |kind, standing| {
        by_kind[&kind]
            .iter()
            .filter(|found| **found == standing)
            .count()
    };
    assert_eq!(by_kind[&MediaKind::Keyframe].len(), 6);
    assert_eq!(
        count(MediaKind::Keyframe, Standing::Passed),
        3,
        "generation 1"
    );
    assert_eq!(
        count(MediaKind::Keyframe, Standing::Candidate),
        3,
        "generation 2"
    );
    assert_eq!(by_kind[&MediaKind::StyleSample].len(), 2);
    assert_eq!(count(MediaKind::StyleSample, Standing::Locked), 1);
    assert_eq!(by_kind[&MediaKind::Cast].len(), 10);
    assert_eq!(
        count(MediaKind::Cast, Standing::Locked),
        8,
        "the filmmaker's portrait and pack"
    );
    assert!(!by_kind.contains_key(&MediaKind::Video));
    assert!(records.iter().any(|record| record.owner
        == MediaOwner::Shot(ShotKey::new("office_night", "office_night_02").unwrap())));
    assert!(
        records
            .iter()
            .any(|record| record.owner == MediaOwner::Character("interviewer".to_owned()))
    );
    assert!(media::check(&project, &records, true).unwrap().is_clean());

    // One keyframe is deleted, another's bytes change under it.
    let gone = records
        .iter()
        .find(|record| record.artifact_id == "office_night_02_g0001_candidate_a")
        .unwrap();
    fs::remove_file(project.root().resolve(&gone.path)).unwrap();
    let changed = records
        .iter()
        .find(|record| record.artifact_id == "style_s0001")
        .unwrap();
    fs::write(project.root().resolve(&changed.path), b"not the sample").unwrap();

    let quick = media::check(&project, &records, false).unwrap();
    assert_eq!(quick.missing, std::slice::from_ref(gone));
    assert!(
        quick.mismatched.is_empty(),
        "bytes are read only when asked"
    );
    let thorough = media::check(&project, &records, true).unwrap();
    assert_eq!(thorough.missing, std::slice::from_ref(gone));
    assert_eq!(thorough.mismatched, std::slice::from_ref(changed));
    // Nothing was rewritten: the records still say what they said.
    assert_eq!(media::catalog(&project).unwrap(), records);
}

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "media".to_owned(),
        title: "Media".to_owned(),
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

fn demo_video(project: &CanonicalProject, shot: &mut ShotWorkflow) {
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
fn identical_media_is_stored_once_and_changed_media_is_left_alone() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = CanonicalProject::create_with_files(
        &target,
        manifest(),
        b"A filmmaker waits.\n",
        &bootstrap::scene_files(),
    )
    .unwrap();
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
    shot.generate(&project).unwrap();
    shot.select(&project, 0).unwrap();
    shot.lock(&project).unwrap();
    // The demo's clip is the same bytes every time.
    for _ in 0..3 {
        demo_video(&project, &mut shot);
    }
    let records = media::catalog(&project).unwrap();
    let videos: Vec<_> = records
        .iter()
        .filter(|record| record.kind == MediaKind::Video)
        .collect();
    assert_eq!(videos.len(), 3);
    // The third take's file is changed by hand: its bytes no longer match
    // its record, so it is not someone else's to share.
    let changed = project.root().resolve(&videos[2].path);
    fs::write(&changed, b"\0\0\0\x18ftypisom-edited").unwrap();

    let done = media::dedupe(&project, &records).unwrap();
    assert_eq!(done.linked, 1);
    assert_eq!(done.bytes, DEMO_CLIP.len() as u64);
    for record in &videos[..2] {
        assert_eq!(
            fs::read(project.root().resolve(&record.path)).unwrap(),
            DEMO_CLIP
        );
    }
    assert_eq!(fs::read(&changed).unwrap(), b"\0\0\0\x18ftypisom-edited");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let inode = |record: &media::MediaRecord| {
            fs::metadata(project.root().resolve(&record.path))
                .unwrap()
                .ino()
        };
        assert_eq!(inode(videos[0]), inode(videos[1]));
        assert_ne!(inode(videos[0]), inode(videos[2]));
    }
    // Every record keeps its own path, and a second pass finds nothing to do.
    assert_eq!(media::catalog(&project).unwrap(), records);
    assert_eq!(media::dedupe(&project, &records).unwrap().linked, 0);
    assert!(
        media::check(&project, &records, false)
            .unwrap()
            .missing
            .is_empty()
    );
}
