//! Where a project stands in its recipe, computed from its files: a copy of
//! `fixtures/projects/production`, one whose story has no main character,
//! and a project just created.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use kineto_core::workflow::{self, StepState, StepStatus, WorkflowError};
use kineto_project::{
    ContentHash,
    bible::{self, Bible, EntryEdit, EntryKind},
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    recipe::{RecipeError, SkipCondition},
    revisions::{self, DocumentKind},
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kineto-workflow-{}-{}",
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

/// A copy of the production fixture: screenplay locked, bible written but
/// not locked, style locked, the filmmaker cast and the interviewer not.
fn production(temp: &TempDir) -> PathBuf {
    let target = temp.0.join("film");
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/production"),
        &target,
    );
    target
}

fn states(statuses: &[StepStatus]) -> Vec<(&str, StepState)> {
    statuses
        .iter()
        .map(|status| (status.id.as_str(), status.state))
        .collect()
}

fn step<'a>(statuses: &'a [StepStatus], id: &str) -> &'a StepStatus {
    statuses.iter().find(|status| status.id == id).unwrap()
}

#[test]
fn each_step_says_where_the_film_stands_and_what_it_waits_for() {
    let temp = TempDir::new();
    let target = production(&temp);
    let project = CanonicalProject::open(&target).unwrap();
    let statuses = workflow::status(&project).unwrap();
    assert_eq!(
        states(&statuses),
        [
            ("source", StepState::Done),
            ("story_analysis", StepState::Done),
            ("screenplay", StepState::Done),
            ("bible", StepState::AwaitingApproval),
            ("style", StepState::Done),
            ("casting", StepState::AwaitingApproval),
            ("shot_plan", StepState::Waiting),
            ("keyframes", StepState::Waiting),
            ("video", StepState::Waiting),
        ]
    );
    assert_eq!(
        step(&statuses, "shot_plan").waiting_on,
        ["bible", "casting"]
    );
    assert_eq!(step(&statuses, "casting").waiting_on, ["bible"]);
    assert!(statuses.iter().all(|status| status.stale.is_empty()));

    // Locking the bible moves only the bible.
    bible::lock(&project, 1).unwrap();
    let statuses = workflow::status(&project).unwrap();
    assert_eq!(step(&statuses, "bible").state, StepState::Done);
    assert!(step(&statuses, "casting").waiting_on.is_empty());
    assert_eq!(
        step(&statuses, "casting").state,
        StepState::AwaitingApproval
    );
    assert_eq!(step(&statuses, "shot_plan").waiting_on, ["casting"]);

    // The cast was made from the entry the director then changes.
    let record = revisions::latest(&project, DocumentKind::Bible)
        .unwrap()
        .unwrap();
    let current: Bible = revisions::read(&project, &record).unwrap();
    let edited = current
        .with_entry_edit(EntryEdit {
            kind: EntryKind::Character,
            id: "filmmaker".to_owned(),
            description: "A documentary filmmaker.".to_owned(),
            appearance: "Clean-shaven now.".to_owned(),
            wardrobe: "A worn grey overcoat.".to_owned(),
            forbidden_changes: vec!["Never without the coat.".to_owned()],
        })
        .unwrap();
    bible::edit(&project, 1, &edited).unwrap();
    bible::lock(&project, 2).unwrap();
    let statuses = workflow::status(&project).unwrap();
    assert_eq!(
        step(&statuses, "casting").stale,
        ["filmmaker: bible_character.filmmaker"]
    );
    let before = workflow::status(&project).unwrap();
    drop(project);

    // Nothing here lives in .kineto.
    fs::remove_dir_all(target.join(".kineto")).unwrap();
    let project = CanonicalProject::open(&target).unwrap();
    assert_eq!(workflow::status(&project).unwrap(), before);
}

#[test]
fn casting_is_skipped_once_the_locked_bible_has_no_main_character() {
    let temp = TempDir::new();
    let target = production(&temp);
    // Everyone in the story is minor: the film has no one to cast.
    let path = target.join("story/analysis/story_analysis_r0001.json");
    let mut analysis: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for character in analysis["characters"].as_array_mut().unwrap() {
        character["role"] = "minor".into();
    }
    let bytes = serde_json::to_vec_pretty(&analysis).unwrap();
    fs::write(&path, &bytes).unwrap();
    let hash = ContentHash::of(&bytes);
    // The analysis's record, and the locked screenplay written from it.
    for (index, at) in [
        ("story/analysis/revisions.json", "/revisions/0/content_hash"),
        (
            "screenplay/revisions.json",
            "/revisions/0/dependencies/0/content_hash",
        ),
    ] {
        let index = target.join(index);
        let mut revisions: serde_json::Value =
            serde_json::from_slice(&fs::read(&index).unwrap()).unwrap();
        *revisions.pointer_mut(at).unwrap() = hash.as_str().into();
        fs::write(&index, serde_json::to_vec_pretty(&revisions).unwrap()).unwrap();
    }
    let project = CanonicalProject::open(&target).unwrap();

    // An unlocked bible may yet gain a main character.
    let statuses = workflow::status(&project).unwrap();
    assert_eq!(step(&statuses, "casting").state, StepState::Waiting);
    assert_eq!(step(&statuses, "casting").waiting_on, ["bible"]);
    assert!(step(&statuses, "casting").skipped_when.is_empty());

    bible::lock(&project, 1).unwrap();
    let statuses = workflow::status(&project).unwrap();
    let casting = step(&statuses, "casting");
    assert_eq!(casting.state, StepState::Skipped);
    assert_eq!(casting.skipped_when, [SkipCondition::NoMainCharacters]);
    assert!(casting.waiting_on.is_empty() && casting.stale.is_empty());
    // It counts as done for what follows.
    assert!(step(&statuses, "shot_plan").waiting_on.is_empty());
    assert_eq!(step(&statuses, "shot_plan").state, StepState::Ready);
    assert_eq!(step(&statuses, "keyframes").waiting_on, ["shot_plan"]);
    // A step the recipe names no condition for is never skipped.
    assert!(
        statuses
            .iter()
            .filter(|status| status.id != "casting")
            .all(|status| status.state != StepState::Skipped)
    );

    let before = statuses;
    drop(project);
    fs::remove_dir_all(target.join(".kineto")).unwrap();
    let project = CanonicalProject::open(&target).unwrap();
    assert_eq!(workflow::status(&project).unwrap(), before);
}

fn manifest(recipe_version: u32) -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "workflow".to_owned(),
        title: "Workflow".to_owned(),
        created_at: "2026-10-02T00:00:00Z".to_owned(),
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
            recipe_version,
            extra: BTreeMap::new(),
        },
        extra: BTreeMap::new(),
    }
}

#[test]
fn a_new_film_is_ready_to_analyse_and_nothing_else() {
    let temp = TempDir::new();
    let project = CanonicalProject::create_with_files(
        temp.0.join("film"),
        manifest(1),
        b"A filmmaker hears the last line.\n",
        &kineto_core::bootstrap::scene_files(),
    )
    .unwrap();
    let statuses = workflow::status(&project).unwrap();
    assert_eq!(
        states(&statuses)[..4],
        [
            ("source", StepState::Done),
            ("story_analysis", StepState::Ready),
            ("screenplay", StepState::Waiting),
            ("bible", StepState::Waiting),
        ]
    );
    assert_eq!(step(&statuses, "screenplay").waiting_on, ["story_analysis"]);
}

#[test]
fn a_recipe_this_build_does_not_ship_is_refused_not_replaced() {
    let temp = TempDir::new();
    let project = CanonicalProject::create(
        temp.0.join("film"),
        manifest(99),
        b"A filmmaker hears the last line.\n",
    )
    .unwrap();
    assert!(matches!(
        workflow::status(&project),
        Err(WorkflowError::Recipe(RecipeError::Unknown {
            version: 99,
            ..
        }))
    ));
}
