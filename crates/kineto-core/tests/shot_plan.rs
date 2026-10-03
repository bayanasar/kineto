//! Shot plans from a copy of `fixtures/projects/production`: planned by a
//! scripted model from the locked screenplay and bible, edited, locked, and
//! stale when what they read changes.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use kineto_core::{
    generation::{GenerationError, Prerequisite},
    shot_plan,
    workflow::{self, StepState},
};
use kineto_llm::fake::FakeLlm;
use kineto_project::{
    bible::{self, Bible, EntryEdit, EntryKind},
    manifest::CanonicalProject,
    revisions::{self, DocumentKind, Origin, RevisionError},
    shot_plan::{self as document, SAMPLE_SHOT_PLAN_JSON, ShotPlan, StaleCause},
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kineto-shot-plan-{}-{}",
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

fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

/// The production fixture: screenplay locked, bible written but not locked.
fn film(temp: &TempDir) -> CanonicalProject {
    let target = temp.0.join("film");
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/production"),
        &target,
    );
    CanonicalProject::open(&target).unwrap()
}

fn sample() -> serde_json::Value {
    serde_json::from_str(SAMPLE_SHOT_PLAN_JSON).unwrap()
}

fn latest(project: &CanonicalProject) -> (u32, ShotPlan) {
    let record = revisions::latest(project, DocumentKind::ShotPlan)
        .unwrap()
        .unwrap();
    (record.revision, revisions::read(project, &record).unwrap())
}

#[test]
fn shots_are_planned_from_the_locked_screenplay_and_bible() {
    let temp = TempDir::new();
    let project = film(&temp);
    assert!(matches!(
        shot_plan::write(&project, &FakeLlm::answering([SAMPLE_SHOT_PLAN_JSON])),
        Err(GenerationError::Prerequisite(Prerequisite::LockedBible))
    ));
    bible::lock(&project, 1).unwrap();

    // An answer that leaves a scene's cast for a stranger is asked for again.
    let mut stranger = sample();
    stranger["scenes"][0]["shots"][1]["characters"] = serde_json::json!(["waiter"]);
    let model = FakeLlm::answering([stranger.to_string(), SAMPLE_SHOT_PLAN_JSON.to_owned()]);
    let record = shot_plan::write(&project, &model).unwrap();
    assert_eq!(record.revision, 1);
    assert_eq!(record.origin, Origin::Generated);
    let depends_on: Vec<_> = record
        .dependencies
        .iter()
        .map(|dependency| dependency.artifact_id.as_str())
        .collect();
    assert_eq!(
        depends_on,
        [
            "screenplay_r0001",
            "bible_r0001",
            "screenplay_scene.office_night",
            "bible_character.filmmaker",
            "bible_character.interviewer",
            "bible_location.small_office",
        ]
    );
    let (_, plan) = latest(&project);
    assert_eq!(plan.scenes[0].shots.len(), 2);
    assert!(document::staleness(&project, &record).unwrap().is_empty());

    // One scene again, as the director asks; the rest carried over.
    let mut tighter = sample()["scenes"][0].clone();
    tighter["shots"].as_array_mut().unwrap().truncate(1);
    let record = shot_plan::replan_scene(
        &project,
        &FakeLlm::answering([tighter.to_string()]),
        "office_night",
        Some("One wide take."),
    )
    .unwrap();
    assert_eq!(record.revision, 2);
    assert_eq!(record.based_on, Some(1));
    assert_eq!(latest(&project).1.scenes[0].shots.len(), 1);
    assert!(matches!(
        shot_plan::replan_scene(
            &project,
            &FakeLlm::answering(Vec::<String>::new()),
            "dawn",
            None
        ),
        Err(GenerationError::Prerequisite(Prerequisite::Scene))
    ));

    // The director's edit of a scene is a new revision, made only from the
    // latest one.
    let mut scene = plan.scenes[0].clone();
    scene.shots[1].duration_target = 6.0;
    assert!(matches!(
        document::edit_scene(&project, 1, scene.clone()),
        Err(RevisionError::NotLatest {
            based_on: 1,
            latest: 2
        })
    ));
    let edited = document::edit_scene(&project, 2, scene).unwrap();
    assert_eq!(edited.origin, Origin::Edited);
    let mut stranger = latest(&project).1.scenes[0].clone();
    stranger.shots[0].characters = vec!["waiter".to_owned()];
    assert!(matches!(
        document::edit_scene(&project, 3, stranger),
        Err(RevisionError::InvalidDocument(_))
    ));
    document::lock(&project, 3).unwrap();
    let statuses = workflow::status(&project).unwrap();
    let step = statuses
        .iter()
        .find(|status| status.id == "shot_plan")
        .unwrap();
    assert_eq!(step.state, StepState::Done);
    assert!(step.stale.is_empty());

    // The plan read the filmmaker's entry, which then changes.
    let record = revisions::latest(&project, DocumentKind::Bible)
        .unwrap()
        .unwrap();
    let current: Bible = revisions::read(&project, &record).unwrap();
    let changed = current
        .with_entry_edit(EntryEdit {
            kind: EntryKind::Character,
            id: "filmmaker".to_owned(),
            description: "A documentary filmmaker.".to_owned(),
            appearance: "Clean-shaven now.".to_owned(),
            wardrobe: "A worn grey overcoat.".to_owned(),
            forbidden_changes: Vec::new(),
        })
        .unwrap();
    bible::edit(&project, 1, &changed).unwrap();
    bible::lock(&project, 2).unwrap();
    let locked = document::locked_plan(&project).unwrap().unwrap().0;
    assert_eq!(
        document::staleness(&project, &locked).unwrap(),
        [StaleCause::Entry("bible_character.filmmaker".to_owned())]
    );
    let statuses = workflow::status(&project).unwrap();
    let step = statuses
        .iter()
        .find(|status| status.id == "shot_plan")
        .unwrap();
    assert_eq!(step.stale, ["bible_character.filmmaker"]);
}
