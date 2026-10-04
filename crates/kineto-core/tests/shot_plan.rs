//! Shot plans from a copy of `fixtures/projects/production`: planned by a
//! scripted model from the locked screenplay and bible, edited, locked, and
//! stale when what they read changes.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use kineto_core::{
    generation::{self, GenerationError, LocalDemoAdapter, NoHooks, Prerequisite},
    shot_plan,
    workflow::{self, StepState},
};
use kineto_llm::fake::FakeLlm;
use kineto_project::{
    bible::{self, Bible, EntryEdit, EntryKind},
    fs::ProjectRelativePath,
    manifest::CanonicalProject,
    revisions::{self, DocumentKind, Origin, RevisionError},
    shot::{ShotKey, ShotWorkflow},
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
    // latest one. It adds back the shot the model just took out, which then
    // takes a new ID: a shot added never takes one a shot had before.
    let mut scene = plan.scenes[0].clone();
    scene.shots[1].duration_target = 6.0;
    assert!(matches!(
        document::edit_scene(&project, 1, scene.clone(), &BTreeSet::new()),
        Err(RevisionError::NotLatest {
            based_on: 1,
            latest: 2
        })
    ));
    let edited = document::edit_scene(&project, 2, scene, &BTreeSet::new()).unwrap();
    assert_eq!(edited.origin, Origin::Edited);
    let added = &latest(&project).1.scenes[0].shots[1];
    assert_eq!(added.id, "office_night_03");
    assert_eq!(added.continuity_from.as_deref(), Some("office_night_01"));
    let mut stranger = latest(&project).1.scenes[0].clone();
    stranger.shots[0].characters = vec!["waiter".to_owned()];
    assert!(matches!(
        document::edit_scene(&project, 3, stranger, &BTreeSet::new()),
        Err(RevisionError::InvalidDocument(_))
    ));
    document::lock(&project, 3).unwrap();
    let shot_file = project.root().resolve(
        &ProjectRelativePath::new("scenes/office_night/shots/office_night_03/shot.json").unwrap(),
    );
    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(&shot_file).unwrap()).unwrap();
    assert_eq!(written["duration_target"], 6.0);
    assert_eq!(
        document::shots_in_play(&project).unwrap(),
        [
            ShotKey::new("office_night", "office_night_01").unwrap(),
            ShotKey::new("office_night", "office_night_03").unwrap(),
        ]
    );
    // Locking a changed plan again replaces the intent and keeps what the
    // shot has made.
    let mut generated = written;
    generated["generation_revision"] = 4.into();
    fs::write(&shot_file, serde_json::to_vec_pretty(&generated).unwrap()).unwrap();
    let mut scene = latest(&project).1.scenes[0].clone();
    scene.shots[1].emotion = "Nothing shows yet.".to_owned();
    document::edit_scene(&project, 3, scene, &BTreeSet::new()).unwrap();
    document::lock(&project, 4).unwrap();
    let relocked: serde_json::Value =
        serde_json::from_slice(&fs::read(&shot_file).unwrap()).unwrap();
    assert_eq!(relocked["emotion"], "Nothing shows yet.");
    assert_eq!(relocked["generation_revision"], 4);
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

/// Every file under `dir`, by path, with its bytes.
fn tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(tree(&path));
        } else {
            files.push((path.clone(), fs::read(&path).unwrap()));
        }
    }
    files.sort();
    files
}

#[test]
fn a_shot_added_after_one_was_removed_never_takes_its_id_or_history() {
    let temp = TempDir::new();
    let project = film(&temp);
    bible::lock(&project, 1).unwrap();
    shot_plan::write(&project, &FakeLlm::answering([SAMPLE_SHOT_PLAN_JSON])).unwrap();
    document::lock(&project, 1).unwrap();

    // The second shot is made: takes, one locked, and a video.
    let removed = ShotKey::new("office_night", "office_night_02").unwrap();
    let mut shot = ShotWorkflow::load(&project, &removed).unwrap();
    generation::generate_with(&project, &mut shot, LocalDemoAdapter, &mut NoHooks).unwrap();
    shot.select(&project, 1).unwrap();
    generation::lock(&project, &mut shot).unwrap();
    let removed_dir = temp
        .0
        .join("film/scenes/office_night/shots/office_night_02");
    let path = removed_dir.join("shot.json");
    let mut written: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    written["video_revision"] = 1.into();
    fs::write(&path, serde_json::to_vec_pretty(&written).unwrap()).unwrap();
    let history = tree(&removed_dir);

    // The director removes it, locks, then adds a shot after the first; the
    // editor offers the first free number in the scene, the removed one's.
    let (based_on, plan) = latest(&project);
    let mut scene = plan.scenes[0].clone();
    let mut new_shot = scene.shots.pop().unwrap();
    document::edit_scene(&project, based_on, scene.clone(), &BTreeSet::new()).unwrap();
    document::lock(&project, 2).unwrap();
    new_shot.purpose = "Hold on the interviewer.".to_owned();
    new_shot.characters = vec!["interviewer".to_owned()];
    new_shot.continuity_from = None;
    scene.shots.push(new_shot);
    document::edit_scene(&project, 2, scene, &BTreeSet::new()).unwrap();
    let (revision, plan) = latest(&project);
    let ids: Vec<_> = plan.shots().map(|(_, shot)| shot.id.as_str()).collect();
    assert_eq!(ids, ["office_night_01", "office_night_03"]);
    document::lock(&project, revision).unwrap();

    // The new shot starts with nothing; the removed one keeps all it had.
    let added = ShotKey::new("office_night", "office_night_03").unwrap();
    assert_eq!(
        document::shots_in_play(&project).unwrap(),
        [
            ShotKey::new("office_night", "office_night_01").unwrap(),
            added.clone()
        ]
    );
    let snapshot = ShotWorkflow::load(&project, &added)
        .unwrap()
        .snapshot()
        .unwrap();
    assert!(!snapshot.generated && !snapshot.locked);
    assert_eq!(snapshot.video, kineto_project::shot::VideoState::None);
    let intent: serde_json::Value = serde_json::from_slice(
        &fs::read(
            temp.0
                .join("film/scenes/office_night/shots/office_night_03/shot.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(intent["purpose"], "Hold on the interviewer.");
    assert!(intent.get("generation_revision").is_none() && intent.get("video_revision").is_none());
    assert_eq!(tree(&removed_dir), history);

    // A model planning the scene again cannot bring the removed ID back
    // either, and a new plan's shots are all new.
    let mut again = sample()["scenes"][0].clone();
    again["shots"][1]
        .as_object_mut()
        .unwrap()
        .remove("continuity_from");
    shot_plan::replan_scene(
        &project,
        &FakeLlm::answering([again.to_string()]),
        "office_night",
        None,
    )
    .unwrap();
    let ids: Vec<_> = latest(&project)
        .1
        .shots()
        .map(|(_, shot)| shot.id.clone())
        .collect();
    assert_eq!(ids, ["office_night_01", "office_night_04"]);
    shot_plan::write(&project, &FakeLlm::answering([SAMPLE_SHOT_PLAN_JSON])).unwrap();
    let plan = latest(&project).1;
    let shots: Vec<_> = plan
        .shots()
        .map(|(_, shot)| (shot.id.as_str(), shot.continuity_from.as_deref()))
        .collect();
    assert_eq!(
        shots,
        [
            ("office_night_05", None),
            ("office_night_06", Some("office_night_05"))
        ]
    );
    assert_eq!(tree(&removed_dir), history);
}

#[test]
fn a_shot_removed_and_added_back_in_one_edit_is_a_new_shot() {
    let temp = TempDir::new();
    let project = film(&temp);
    bible::lock(&project, 1).unwrap();
    shot_plan::write(&project, &FakeLlm::answering([SAMPLE_SHOT_PLAN_JSON])).unwrap();
    document::lock(&project, 1).unwrap();
    let removed = ShotKey::new("office_night", "office_night_02").unwrap();
    let mut shot = ShotWorkflow::load(&project, &removed).unwrap();
    generation::generate_with(&project, &mut shot, LocalDemoAdapter, &mut NoHooks).unwrap();
    shot.select(&project, 0).unwrap();
    generation::lock(&project, &mut shot).unwrap();
    let removed_dir = temp
        .0
        .join("film/scenes/office_night/shots/office_night_02");
    let history = tree(&removed_dir);

    // In one edit the director removes the second shot and adds a new one,
    // which the editor numbered as the removed one. Sent as added, it is a
    // new shot.
    let (based_on, plan) = latest(&project);
    let mut scene = plan.scenes[0].clone();
    let mut new_shot = scene.shots.pop().unwrap();
    new_shot.purpose = "Show the empty chair.".to_owned();
    new_shot.continuity_from = None;
    scene.shots.push(new_shot);
    let added: BTreeSet<String> = ["office_night_02".to_owned()].into();
    document::edit_scene(&project, based_on, scene, &added).unwrap();
    let (revision, plan) = latest(&project);
    let ids: Vec<_> = plan.shots().map(|(_, shot)| shot.id.as_str()).collect();
    assert_eq!(ids, ["office_night_01", "office_night_03"]);
    document::lock(&project, revision).unwrap();
    let snapshot = ShotWorkflow::load(
        &project,
        &ShotKey::new("office_night", "office_night_03").unwrap(),
    )
    .unwrap()
    .snapshot()
    .unwrap();
    assert!(!snapshot.generated && !snapshot.locked);
    assert_eq!(tree(&removed_dir), history);
}
