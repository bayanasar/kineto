use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use kineto_project::{
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    shot::{ShotDirection, ShotKey, ShotWorkflow},
    shot_plan,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
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
        let path = std::env::temp_dir().join(format!(
            "kineto-locked-reset-{}-{nonce}",
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

fn project(target: &Path) -> CanonicalProject {
    CanonicalProject::create(
        target,
        ProjectManifest {
            format_version: PROJECT_FORMAT_VERSION,
            project_id: "locked_reset_test".to_owned(),
            title: "Locked Reset Test".to_owned(),
            created_at: "2026-09-13T00:00:00Z".to_owned(),
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
        },
        b"source\n",
    )
    .unwrap()
}

#[test]
fn reset_is_an_explicit_destructive_action_that_supersedes_a_lock() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = project(&target);
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();

    shot.set_direction(&project, ShotDirection::Tension)
        .unwrap();
    shot.generate(&project).unwrap();
    shot.select(&project, 2).unwrap();
    shot.lock(&project).unwrap();
    assert!(shot.snapshot().unwrap().locked);

    shot.reset(&project).unwrap();
    let discarded = shot.snapshot().unwrap();
    assert!(!discarded.generated);
    assert!(!discarded.locked);
    assert_eq!(discarded.selected_index, None);
    assert_eq!(discarded.direction, ShotDirection::Reaction);
    assert_eq!(discarded.generation_revision, 1);
    assert_eq!(discarded.superseded_count, 3);

    drop(project);
    let reopened = CanonicalProject::open(&target).unwrap();
    assert_eq!(
        ShotWorkflow::load(&reopened, &ShotKey::bootstrap(1))
            .unwrap()
            .snapshot()
            .unwrap(),
        discarded
    );
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

/// A planned shot's direction is the locked plan's. Reset keeps it, so
/// what is made and locked after a reset is still current when the project
/// opens again and the plan's intent is written back over `shot.json`.
#[test]
fn reset_keeps_the_planned_direction_and_what_follows_stays_current() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/planned"),
        &target,
    );
    let project = CanonicalProject::open(&target).unwrap();
    let key = ShotKey::new("office_night", "office_night_01").unwrap();
    assert_eq!(
        shot_plan::planned_direction(&project, &key).unwrap(),
        Some(ShotDirection::SpatialClarity)
    );
    let mut shot = ShotWorkflow::load(&project, &key).unwrap();
    shot.generate(&project).unwrap();
    shot.select(&project, 0).unwrap();
    shot.lock(&project).unwrap();

    shot.reset(&project).unwrap();
    assert_eq!(
        shot.snapshot().unwrap().direction,
        ShotDirection::SpatialClarity
    );
    shot.generate(&project).unwrap();
    shot.select(&project, 1).unwrap();
    shot.lock(&project).unwrap();
    let locked = shot.snapshot().unwrap();
    drop(project);

    // Opening the project writes the plan's intent back, as a session does.
    let reopened = CanonicalProject::open(&target).unwrap();
    shot_plan::materialize(&reopened).unwrap();
    let snapshot = ShotWorkflow::load(&reopened, &key)
        .unwrap()
        .snapshot()
        .unwrap();
    assert!(snapshot.locked);
    assert!(!snapshot.stale);
    assert_eq!(snapshot.direction, ShotDirection::SpatialClarity);
    assert_eq!(snapshot, locked);
}
