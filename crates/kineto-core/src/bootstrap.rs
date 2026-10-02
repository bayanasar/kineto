//! The bootstrap scene every project created by the desktop app starts with.
//!
//! The app works one fixed scene of [`SHOT_COUNT`](crate::session::SHOT_COUNT)
//! shots until screenplay and shot planning exist. Its intent is written into
//! the project when the project is created, so generation compiles from
//! canonical files rather than from text held in the application.

use kineto_project::fs::ProjectRelativePath;
use serde_json::{Value, json};

use crate::session::SHOT_COUNT;

const SCENE_PATH: &str = "scenes/scene_001/scene.json";

fn scene() -> Value {
    json!({
        "scene_id": "scene_001",
        "schema_version": 1,
        "location": "small_office",
        "time": "night",
        "characters": ["filmmaker"],
        "dramatic_goal": "The final line of the conversation lands and the filmmaker leaves",
        "shots": ["shot_001", "shot_002"],
    })
}

fn shots() -> [Value; SHOT_COUNT] {
    [
        json!({
            "shot_id": "shot_001",
            "schema_version": 1,
            "direction": "reaction",
            "generation_revision": 0,
            "duration_target": 4.0,
            "characters": ["filmmaker"],
            "framing": "medium_close_up",
            "lens": "50mm",
            "camera_motion": "slow_dolly_in",
            "action": "A filmmaker sits across a table and hears the final line of the conversation",
            "dialogue": "",
            "emotion": "a quiet, withheld reaction that matters more than the words",
        }),
        json!({
            "shot_id": "shot_002",
            "schema_version": 1,
            "direction": "reaction",
            "generation_revision": 0,
            "duration_target": 3.5,
            "characters": ["filmmaker"],
            "framing": "medium_wide",
            "lens": "40mm",
            "camera_motion": "restrained_pan",
            "action": "The filmmaker stands and walks toward the doorway to leave the office",
            "dialogue": "",
            "emotion": "tension carried out through the doorway",
        }),
    ]
}

/// The canonical files of the bootstrap scene, ready for
/// `CanonicalProject::create_with_files`.
#[must_use]
pub fn scene_files() -> Vec<(ProjectRelativePath, Vec<u8>)> {
    let encode = |value: &Value| {
        let mut bytes = serde_json::to_vec_pretty(value).expect("static JSON encodes");
        bytes.push(b'\n');
        bytes
    };
    let mut files = vec![(
        ProjectRelativePath::new(SCENE_PATH).expect("static scene path is valid"),
        encode(&scene()),
    )];
    for (index, shot) in shots().iter().enumerate() {
        files.push((
            ProjectRelativePath::new(format!(
                "scenes/scene_001/shots/shot_{:03}/shot.json",
                index + 1
            ))
            .expect("static shot path is valid"),
            encode(shot),
        ));
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyframe_prompt::{self, Locks};
    use kineto_project::{
        manifest::{
            CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest,
            ProjectSource, ProjectWorkflow,
        },
        shot::{ShotWorkflow, load_scene_intent},
    };
    use std::collections::BTreeMap;

    #[test]
    fn every_bootstrap_shot_has_an_intent_that_compiles() {
        let temp = std::env::temp_dir().join(format!(
            "kineto-bootstrap-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let project = CanonicalProject::create_with_files(
            temp.join("film"),
            ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                project_id: "bootstrap_test".to_owned(),
                title: "Bootstrap Test".to_owned(),
                created_at: "2026-09-30T00:00:00Z".to_owned(),
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
            &scene_files(),
        )
        .unwrap();

        let scene = load_scene_intent(&project).unwrap().unwrap();
        for shot_number in 1..=u32::try_from(SHOT_COUNT).unwrap() {
            let shot = ShotWorkflow::load(&project, shot_number).unwrap();
            let intent = shot.intent().unwrap().expect("bootstrap shot has intent");
            let compiled = keyframe_prompt::compile(
                Some(&scene),
                Some(&intent),
                shot.direction(),
                Locks::default(),
                &kineto_provider_minimax::image_capabilities(),
            )
            .unwrap();
            assert!(compiled.compiled.prompt().contains("filmmaker"));
            assert!(!shot.snapshot().unwrap().generated);
        }
        drop(project);
        std::fs::remove_dir_all(&temp).unwrap();
    }
}
