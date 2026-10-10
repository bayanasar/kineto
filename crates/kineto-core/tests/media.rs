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
    assert_eq!(by_kind[&MediaKind::Cast].len(), 3);
    assert_eq!(
        count(MediaKind::Cast, Standing::Locked),
        1,
        "the filmmaker's portrait"
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

fn record(
    owner: MediaOwner,
    id: &str,
    kind: MediaKind,
    standing: Standing,
    dependencies: &[&str],
) -> media::MediaRecord {
    media::MediaRecord {
        owner,
        artifact_id: id.to_owned(),
        kind,
        path: kineto_project::fs::ProjectRelativePath::new(format!("media/{id}.bin")).unwrap(),
        content_hash: kineto_project::ContentHash::of(id.as_bytes()),
        state: MediaState::Present,
        standing,
        dependencies: dependencies.iter().map(|id| (*id).to_owned()).collect(),
    }
}

#[test]
fn locked_work_and_everything_it_was_made_from_is_never_collectable() {
    use media::RetentionClass::{Archived, Collectable, Pinned, Recent};
    let shot = || MediaOwner::Shot(ShotKey::new("scene_a", "shot_a").unwrap());
    let records = vec![
        // A style sample turned down after a portrait was made from it.
        record(
            MediaOwner::Style,
            "style_s0001",
            MediaKind::StyleSample,
            Standing::Passed,
            &[],
        ),
        record(
            MediaOwner::Style,
            "style_s0002",
            MediaKind::StyleSample,
            Standing::Passed,
            &[],
        ),
        record(
            MediaOwner::Character("filmmaker".to_owned()),
            "cast.filmmaker.0001",
            MediaKind::Cast,
            Standing::Passed,
            &["style_s0001"],
        ),
        // Keyframes: an old superseded generation the locked video still
        // starts from, and two more superseded takes.
        record(
            shot(),
            "shot_a_g0001_candidate_a",
            MediaKind::Keyframe,
            Standing::Passed,
            &["cast.filmmaker.0001"],
        ),
        record(
            shot(),
            "shot_a_g0001_candidate_b",
            MediaKind::Keyframe,
            Standing::Passed,
            &[],
        ),
        record(
            shot(),
            "shot_a_g0002_candidate_a",
            MediaKind::Keyframe,
            Standing::Passed,
            &[],
        ),
        record(
            shot(),
            "shot_a_g0003_candidate_a",
            MediaKind::Keyframe,
            Standing::Candidate,
            &[],
        ),
        record(
            shot(),
            "shot_a_v0001_candidate_a",
            MediaKind::Video,
            Standing::Locked,
            &["shot_a_g0001_candidate_a"],
        ),
        media::MediaRecord {
            state: MediaState::Evicted,
            ..record(
                shot(),
                "shot_a_v0000_candidate_a",
                MediaKind::Video,
                Standing::Passed,
                &[],
            )
        },
    ];
    let classes: Vec<_> = media::retention(&records, 1)
        .into_iter()
        .map(|retention| (retention.record.artifact_id, retention.class))
        .collect();
    let class = |id: &str| classes.iter().find(|(found, _)| found == id).unwrap().1;
    // Locked, and its ancestry three steps back.
    assert_eq!(class("shot_a_v0001_candidate_a"), Pinned);
    assert_eq!(class("shot_a_g0001_candidate_a"), Pinned);
    assert_eq!(class("cast.filmmaker.0001"), Pinned);
    assert_eq!(class("style_s0001"), Pinned);
    // The latest style sample is the one recent take kept.
    assert_eq!(class("style_s0002"), Recent);
    // Undecided takes are kept whatever their age; older passed ones go.
    assert_eq!(class("shot_a_g0003_candidate_a"), Recent);
    assert_eq!(class("shot_a_g0002_candidate_a"), Collectable);
    assert_eq!(class("shot_a_g0001_candidate_b"), Collectable);
    assert_eq!(class("shot_a_v0000_candidate_a"), Archived);
    // A wider window keeps more.
    assert!(
        media::retention(&records, 6)
            .iter()
            .all(|retention| retention.class != Collectable)
    );
}

#[test]
fn a_dry_run_says_what_collecting_frees_and_collecting_does_exactly_that() {
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
    for _ in 0..4 {
        demo_video(&project, &mut shot);
    }
    // The last take is locked; the three before it were superseded.
    shot.lock_video(&project).unwrap();
    let records = media::catalog(&project).unwrap();
    // The two latest takes are kept: the locked one and the one before it.
    let plan = media::plan(&project, &records, 2);
    let planned: Vec<_> = plan
        .items
        .iter()
        .map(|item| item.record.artifact_id.as_str())
        .collect();
    assert_eq!(
        planned,
        ["shot_001_v0001_candidate_a", "shot_001_v0002_candidate_a"]
    );
    assert!(
        plan.items
            .iter()
            .all(|item| item.reason.contains("latest 2"))
    );
    assert_eq!(plan.bytes, 2 * DEMO_CLIP.len() as u64);

    // Once the identical clips are one file, the locked take holds it, so
    // collecting frees nothing.
    media::dedupe(&project, &records).unwrap();
    assert_eq!(media::plan(&project, &records, 2).bytes, 0);
    let usage = media::usage(&project, &records);
    assert_eq!(usage.bytes, DEMO_CLIP.len() as u64);
    assert_eq!(usage.by_owner.len(), 1);

    // A wider window than the dry run's keeps everything it listed.
    let skipped = media::collect(&project, &plan, 10).unwrap();
    assert!(skipped.evicted.is_empty());
    assert_eq!(skipped.skipped, planned);
    assert_eq!(media::catalog(&project).unwrap(), records);

    let done = media::collect(&project, &plan, 2).unwrap();
    assert_eq!(done.evicted, planned);
    let after = media::catalog(&project).unwrap();
    for record in &after {
        let file = project.root().resolve(&record.path);
        if planned.contains(&record.artifact_id.as_str()) {
            assert_eq!(record.state, MediaState::Evicted);
            assert!(!file.exists());
        } else {
            assert_eq!(record.state, MediaState::Present);
            assert_eq!(fs::read(file).unwrap(), DEMO_CLIP);
        }
    }
    // The records, their prompts and order stay; their media is gone.
    let shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
    let history = shot.video_history().unwrap();
    assert_eq!(history.len(), 4);
    assert!(history[0].media_path.is_none() && history[1].media_path.is_none());
    assert!(history[3].media_path.is_some());
    assert!(
        media::retention(&after, 2)
            .iter()
            .filter(|retention| retention.class == media::RetentionClass::Archived)
            .count()
            == 2
    );
    // Nothing is left to collect, and the classes come from the records
    // alone.
    fs::remove_dir_all(target.join(".kineto")).ok();
    assert!(media::plan(&project, &after, 2).items.is_empty());
}

#[test]
fn an_interrupted_collection_is_finished_when_reconciled() {
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
    demo_video(&project, &mut shot);
    demo_video(&project, &mut shot);
    // Stopped after the record said evicted, before the file was removed.
    shot.evict_media(&project, &["shot_001_v0001_candidate_a".to_owned()])
        .unwrap();
    let records = media::catalog(&project).unwrap();
    let evicted = records
        .iter()
        .find(|record| record.state == MediaState::Evicted)
        .unwrap();
    assert!(project.root().resolve(&evicted.path).is_file());
    // A file the records call evicted is no one's to report as missing.
    assert!(media::check(&project, &records, true).unwrap().is_clean());
    assert_eq!(media::reconcile(&project, &records).unwrap(), 1);
    assert!(!project.root().resolve(&evicted.path).exists());
    assert_eq!(media::reconcile(&project, &records).unwrap(), 0);
    assert_eq!(
        fs::read(project.root().resolve(&records.last().unwrap().path)).unwrap(),
        DEMO_CLIP
    );
}

#[test]
fn bytes_changed_since_the_dry_run_are_never_let_go() {
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
    for _ in 0..4 {
        demo_video(&project, &mut shot);
    }
    shot.lock_video(&project).unwrap();
    let records = media::catalog(&project).unwrap();
    let plan = media::plan(&project, &records, 2);
    let [first, second] = plan.items.as_slice() else {
        panic!("two takes are collectable: {:?}", plan.items);
    };
    // Someone edits a file the director agreed to let go.
    let edited = project.root().resolve(&first.record.path);
    fs::write(&edited, b"an edit made after the dry run").unwrap();

    let done = media::collect(&project, &plan, 2).unwrap();
    assert_eq!(
        done.evicted,
        std::slice::from_ref(&second.record.artifact_id)
    );
    assert_eq!(
        done.skipped,
        std::slice::from_ref(&first.record.artifact_id)
    );
    assert_eq!(
        fs::read(&edited).unwrap(),
        b"an edit made after the dry run"
    );
    let after = media::catalog(&project).unwrap();
    let kept = after
        .iter()
        .find(|record| record.artifact_id == first.record.artifact_id)
        .unwrap();
    assert_eq!(kept.state, MediaState::Present, "its record is untouched");

    // A file put where an evicted one was is not removed by reconciling.
    let gone = project.root().resolve(&second.record.path);
    assert!(!gone.exists());
    fs::write(&gone, b"a new file in the old place").unwrap();
    assert_eq!(media::reconcile(&project, &after).unwrap(), 0);
    assert_eq!(fs::read(&gone).unwrap(), b"a new file in the old place");
}
