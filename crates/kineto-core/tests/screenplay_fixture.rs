//! `fixtures/projects/screenplay` is a project as the desktop app leaves it
//! after a story analysis and a two-scene screenplay, written by a scripted
//! model. Tests outside Rust open copies of it. This test rebuilds it and
//! fails if the committed files differ; `KINETO_UPDATE_FIXTURES=1` rewrites
//! them instead.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use kineto_core::{bootstrap, screenplay, story};
use kineto_llm::fake::FakeLlm;
use kineto_project::{
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    screenplay::SAMPLE_SCREENPLAY_JSON,
    story::SAMPLE_ANALYSIS_JSON,
};

const STORY: &str = "The interview is over. A filmmaker hears the interviewer's last question, \
says nothing, and leaves before he can answer. At dawn he comes back to the empty office.\n";

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/screenplay")
}

/// The sample screenplay with a second scene, at dawn.
fn two_scenes() -> String {
    let mut value: serde_json::Value =
        serde_json::from_str(SAMPLE_SCREENPLAY_JSON).expect("the sample is JSON");
    let mut dawn = value["scenes"][0].clone();
    dawn["id"] = "office_dawn".into();
    dawn["heading"] = "INT. SMALL OFFICE - DAWN".into();
    dawn["time"] = "dawn".into();
    dawn["characters"] = serde_json::json!(["filmmaker"]);
    dawn["dramatic_goal"] = "He returns to the question he left unanswered.".into();
    dawn["blocks"] = serde_json::json!([
        {"kind": "action", "text": "Grey light. The chair is still pushed back."},
        {"kind": "dialogue", "character": "filmmaker", "text": "It was."}
    ]);
    value["scenes"]
        .as_array_mut()
        .expect("scenes is a list")
        .push(dawn);
    value.to_string()
}

fn build(target: &Path) {
    let project = CanonicalProject::create_with_files(
        target,
        ProjectManifest {
            format_version: PROJECT_FORMAT_VERSION,
            project_id: "screenplay_fixture".to_owned(),
            title: "The Last Line".to_owned(),
            created_at: "2026-10-01T00:00:00Z".to_owned(),
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
        STORY.as_bytes(),
        &bootstrap::scene_files(),
    )
    .expect("the fixture project is created");
    story::analyze(&project, &FakeLlm::answering([SAMPLE_ANALYSIS_JSON]))
        .expect("the analysis is written");
    screenplay::write(&project, &FakeLlm::answering([two_scenes()]))
        .expect("the screenplay is written");
}

/// Every file under `root` except runtime state, by relative path.
fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, directory: &Path, found: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(directory).expect("the directory is readable") {
            let path = entry.expect("the entry is readable").path();
            let relative = path
                .strip_prefix(root)
                .expect("under the root")
                .to_string_lossy()
                .replace('\\', "/");
            if relative == ".kineto" {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, found);
            } else {
                found.insert(relative, fs::read(&path).expect("the file is readable"));
            }
        }
    }
    let mut found = BTreeMap::new();
    walk(root, root, &mut found);
    found
}

#[test]
fn the_screenplay_fixture_is_what_the_engine_writes() {
    let temp =
        std::env::temp_dir().join(format!("kineto-screenplay-fixture-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    build(&temp);
    let built = files(&temp);
    let _ = fs::remove_dir_all(&temp);

    if std::env::var_os("KINETO_UPDATE_FIXTURES").is_some() {
        let _ = fs::remove_dir_all(fixture());
        for (path, bytes) in &built {
            let target = fixture().join(path);
            fs::create_dir_all(target.parent().expect("files have a parent"))
                .expect("the fixture directory is writable");
            fs::write(target, bytes).expect("the fixture is writable");
        }
    }

    let committed = files(&fixture());
    let names = |files: &BTreeMap<String, Vec<u8>>| files.keys().cloned().collect::<BTreeSet<_>>();
    assert_eq!(names(&committed), names(&built), "the fixture's files");
    for (path, bytes) in &built {
        assert!(
            committed[path] == *bytes,
            "{path} differs from what the engine writes; rerun with KINETO_UPDATE_FIXTURES=1 \
             and review the change"
        );
    }
}
