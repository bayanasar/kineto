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
    shot::{ShotDirection, ShotWorkflow},
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
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
    let mut shot = ShotWorkflow::load(&project, 1).unwrap();

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
        ShotWorkflow::load(&reopened, 1)
            .unwrap()
            .snapshot()
            .unwrap(),
        discarded
    );
}
