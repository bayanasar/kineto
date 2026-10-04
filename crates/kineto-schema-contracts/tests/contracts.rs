use std::{
    collections::BTreeMap,
    fs,
    num::NonZeroU16,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use kineto_core::generation::{self, ImageProvider, NoHooks};
use kineto_job_runtime::{JobRuntime, JobRuntimePolicy, ProviderKey};
use kineto_jobs::{BackoffPolicy, FallbackPolicy, JobId, PaidFallback, ProviderExecutionPolicy};
use kineto_json_schema::{ensure_supported_schema, validate_value};
use kineto_project::{
    ArtifactId, InputHash,
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    shot::{ShotKey, ShotWorkflow},
};
use serde_json::Value;

const DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        // SystemTime is only microsecond-precise on macOS, so tests running in
        // parallel could otherwise share a directory name.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = format!(
            "{nonce}-{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let name = format!("kineto-schema-contracts-{}-{nonce}", std::process::id());
        Self(std::env::temp_dir().join(name))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn schema(name: &str) -> Value {
    let path = repo_root().join("schemas").join(name);
    let bytes = fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read schema {}: {error}", path.display()));
    let value: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("failed to parse schema {}: {error}", path.display()));
    ensure_supported_schema(&value, "$")
        .unwrap_or_else(|error| panic!("unsupported schema {}: {error}", path.display()));
    assert_eq!(
        value.get("$schema").and_then(Value::as_str),
        Some(DRAFT_2020_12),
        "{} must declare JSON Schema draft 2020-12",
        path.display()
    );
    value
}

fn json_file(path: &Path) -> Value {
    let bytes =
        fs::read(path).unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn toml_file_as_json(path: &Path) -> Value {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let value: toml::Value = toml::from_str(&text)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
    serde_json::to_value(value)
        .unwrap_or_else(|error| panic!("failed to convert {} to JSON: {error}", path.display()))
}

fn validate(schema: &Value, instance: &Value, label: &str) {
    validate_value(schema, instance, "$")
        .unwrap_or_else(|error| panic!("{label} violates committed schema: {error}"));
}

fn validate_artifact_array(path: &Path, artifact_schema: &Value) {
    let value = json_file(path);
    let records = value
        .as_array()
        .unwrap_or_else(|| panic!("{} must contain an artifact array", path.display()));
    for (index, record) in records.iter().enumerate() {
        let label = format!("{}[{index}]", path.display());
        validate(artifact_schema, record, &label);
    }
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<String>) {
    let mut entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("failed to enumerate {}: {error}", directory.display()));
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let metadata = entry
            .metadata()
            .unwrap_or_else(|error| panic!("failed to stat {}: {error}", path.display()));
        if metadata.is_dir() {
            collect_files(root, &path, files);
        } else if metadata.is_file() {
            files.push(
                path.strip_prefix(root)
                    .expect("golden path is below golden root")
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        } else {
            panic!(
                "golden project contains unsupported entry {}",
                path.display()
            );
        }
    }
}

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "schema_contract_project".to_owned(),
        title: "Schema Contract Project".to_owned(),
        created_at: "2026-09-20T00:00:00Z".to_owned(),
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

fn runtime_policy() -> JobRuntimePolicy {
    JobRuntimePolicy {
        execution: ProviderExecutionPolicy::new(
            NonZeroU16::new(1).expect("one is non-zero"),
            BackoffPolicy::new(100, 1_000, 3).expect("test backoff is valid"),
        ),
        fallback_policy: FallbackPolicy {
            paid_fallback: PaidFallback::Allowed,
            max_paid_amount_micros: Some(1_000_000),
            require_user_approval: true,
        },
    }
}

#[test]
fn committed_schema_inventory_is_explicit_and_supported() {
    let root = repo_root().join("schemas");
    let mut files = Vec::new();
    collect_files(&root, &root, &mut files);
    assert_eq!(
        files,
        [
            "artifact.schema.json",
            "bible.schema.json",
            "casting.schema.json",
            "character.schema.json",
            "export-manifest.schema.json",
            "export-shot.schema.json",
            "job.schema.json",
            "lines.schema.json",
            "preference-suggestions.schema.json",
            "preferences.schema.json",
            "project.schema.json",
            "recipe.schema.json",
            "references.schema.json",
            "revisions.schema.json",
            "scene.schema.json",
            "screenplay-scene.schema.json",
            "screenplay.schema.json",
            "selection.schema.json",
            "shot-plan-scene.schema.json",
            "shot-plan.schema.json",
            "shot.schema.json",
            "source.schema.json",
            "story-analysis.schema.json",
            "style-samples.schema.json",
            "style.schema.json",
            "voice-auditions.schema.json",
            "voice-recommendations.schema.json",
            "voice.schema.json",
        ]
    );

    for file in files {
        let _ = schema(&file);
    }
}

#[test]
fn every_shipped_recipe_matches_the_committed_schema() {
    let recipe_schema = schema("recipe.schema.json");
    let root = repo_root().join("recipes");
    let mut files = Vec::new();
    collect_files(&root, &root, &mut files);
    assert_eq!(files, ["default-film-1.json", "default-film-2.json"]);
    for file in files {
        validate(&recipe_schema, &json_file(&root.join(&file)), &file);
    }
    let mut drifted = json_file(&root.join("default-film-1.json"));
    drifted["steps"][0]["produces"] = "music".into();
    assert!(validate_value(&recipe_schema, &drifted, "$").is_err());
}

#[test]
fn minimal_golden_project_is_fully_schema_accounted() {
    let project_schema = schema("project.schema.json");
    let artifact_schema = schema("artifact.schema.json");
    let selection_schema = schema("selection.schema.json");
    let scene_schema = schema("scene.schema.json");
    let shot_schema = schema("shot.schema.json");
    let root = repo_root().join("fixtures/projects/minimal");

    let mut files = Vec::new();
    collect_files(&root, &root, &mut files);
    assert_eq!(
        files,
        [
            "characters/alice/artifacts.json",
            "characters/alice/selection.json",
            "project.toml",
            "scenes/scene_001/scene.json",
            "scenes/scene_001/shots/shot_001/artifacts.json",
            "scenes/scene_001/shots/shot_001/selection.json",
            "scenes/scene_001/shots/shot_001/shot.json",
            "source/story.txt",
        ]
    );

    let project = toml_file_as_json(&root.join("project.toml"));
    validate(&project_schema, &project, "golden project.toml");
    validate_artifact_array(
        &root.join("characters/alice/artifacts.json"),
        &artifact_schema,
    );
    validate(
        &selection_schema,
        &json_file(&root.join("characters/alice/selection.json")),
        "golden character selection",
    );
    validate(
        &scene_schema,
        &json_file(&root.join("scenes/scene_001/scene.json")),
        "golden scene",
    );
    validate_artifact_array(
        &root.join("scenes/scene_001/shots/shot_001/artifacts.json"),
        &artifact_schema,
    );
    validate(
        &selection_schema,
        &json_file(&root.join("scenes/scene_001/shots/shot_001/selection.json")),
        "golden shot selection",
    );
    validate(
        &shot_schema,
        &json_file(&root.join("scenes/scene_001/shots/shot_001/shot.json")),
        "golden shot",
    );

    let source_path = project["source"]["path"]
        .as_str()
        .expect("golden project source.path must be a string");
    let source = fs::read_to_string(root.join(source_path))
        .expect("declared golden source must be valid UTF-8 text");
    assert!(!source.trim().is_empty(), "golden source must not be empty");
}

#[test]
fn real_project_and_shot_persistence_match_committed_schemas() {
    let project_schema = schema("project.schema.json");
    let artifact_schema = schema("artifact.schema.json");
    let selection_schema = schema("selection.schema.json");
    let shot_schema = schema("shot.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");

    validate(
        &project_schema,
        &toml_file_as_json(&temp.0.join("project.toml")),
        "production project.toml",
    );

    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).expect("shot should load");
    shot.generate(&project)
        .expect("shot generation should persist");
    let shot_root = temp.0.join("scenes/scene_001/shots/shot_001");

    validate(
        &shot_schema,
        &json_file(&shot_root.join("shot.json")),
        "production shot.json",
    );
    validate_artifact_array(&shot_root.join("artifacts.json"), &artifact_schema);
    validate(
        &selection_schema,
        &json_file(&shot_root.join("selection.json")),
        "production selection.json",
    );
}

#[test]
fn bootstrap_scene_written_at_creation_matches_committed_schemas() {
    let scene_schema = schema("scene.schema.json");
    let shot_schema = schema("shot.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create_with_files(
        &temp.0,
        manifest(),
        b"schema contract story\n",
        &kineto_core::bootstrap::scene_files(),
    )
    .expect("test project should be creatable with the bootstrap scene");

    validate(
        &scene_schema,
        &json_file(&temp.0.join("scenes/scene_001/scene.json")),
        "bootstrap scene.json",
    );
    for shot_number in 1..=2 {
        let shot_json = temp.0.join(format!(
            "scenes/scene_001/shots/shot_{shot_number:03}/shot.json"
        ));
        validate(&shot_schema, &json_file(&shot_json), "bootstrap shot.json");

        // A canonical write keeps the intent and still matches the schema.
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(shot_number))
            .expect("shot should load");
        shot.generate(&project)
            .expect("bootstrap shot should accept a generation");
        let written = json_file(&shot_json);
        validate(&shot_schema, &written, "bootstrap shot.json after a write");
        assert!(
            written["action"]
                .as_str()
                .is_some_and(|action| !action.is_empty())
        );
    }
}

#[test]
fn shot_video_records_match_committed_schemas() {
    let artifact_schema = schema("artifact.schema.json");
    let shot_schema = schema("shot.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).expect("shot should load");
    shot.generate(&project)
        .expect("shot generation should persist");
    shot.select(&project, 0)
        .expect("a candidate should be selectable");
    shot.lock(&project).expect("the selection should lock");

    let plan = shot
        .plan_video(
            &project,
            kineto_project::shot::VideoGenerationSpec {
                provider: "schema-contract-video",
                model: "clip-1",
                canonical_params: b"duration=6",
                compiler_version: "video-prompt/1",
                notes: &[],
                branched_from: None,
            },
        )
        .expect("a video should be plannable from the locked keyframe");
    let media_path =
        kineto_project::shot::video_media_path(&ShotKey::bootstrap(1), plan.artifact_id())
            .expect("video path should be valid");
    let bytes = b"\0\0\0\x18ftypisomschema";
    project
        .root()
        .write_atomic(&media_path, bytes)
        .expect("video bytes should be written");
    shot.attach_video(
        &project,
        &plan,
        &kineto_project::shot::VideoOutput {
            artifact_id: plan.artifact_id().clone(),
            content_hash: kineto_project::ContentHash::of(bytes),
            provider: "schema-contract-video".to_owned(),
            model: "clip-1".to_owned(),
            media_path,
            provenance: None,
            keyframe: plan.keyframe().expect("plan names its keyframe").0.clone(),
            intent: Some(shot.intent_record().expect("the shot has an intent")),
            notes: vec!["Slower.".to_owned()],
            branched_from: Some("shot_001_v0001_candidate_b".to_owned()),
            clip_seconds: Some(6),
            parameters: Some("duration=6".to_owned()),
        },
    )
    .expect("the video should attach");
    shot.lock_video(&project).expect("the video should lock");

    let shot_root = temp.0.join("scenes/scene_001/shots/shot_001");
    validate(
        &shot_schema,
        &json_file(&shot_root.join("shot.json")),
        "shot.json with a video revision",
    );
    validate_artifact_array(&shot_root.join("artifacts.json"), &artifact_schema);
}

#[test]
fn the_source_schema_rejects_an_empty_revision_list() {
    let source_schema = schema("source.schema.json");
    let empty = serde_json::json!({"schema_version": 1, "revisions": []});
    assert!(validate_value(&source_schema, &empty, "$").is_err());
}

#[test]
fn imported_source_revisions_match_committed_schema() {
    let source_schema = schema("source.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    kineto_project::source::import(
        &project,
        b"# Imported\r\n\r\nA second draft.\r\n",
        kineto_project::source::SourceFormat::Markdown,
        Some("draft.md"),
    )
    .expect("the source should import");
    validate(
        &source_schema,
        &json_file(&temp.0.join("source/source.json")),
        "source/source.json",
    );
}

#[test]
fn generated_story_analysis_and_its_revision_log_match_committed_schemas() {
    let analysis_schema = schema("story-analysis.schema.json");
    let revisions_schema = schema("revisions.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    validate(
        &analysis_schema,
        &serde_json::from_str(kineto_project::story::SAMPLE_ANALYSIS_JSON)
            .expect("the sample is JSON"),
        "sample story analysis",
    );
    let record = kineto_core::story::analyze(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::story::SAMPLE_ANALYSIS_JSON]),
    )
    .expect("the analysis should be generated");
    validate(
        &analysis_schema,
        &json_file(&temp.0.join(&record.path)),
        "generated story analysis",
    );
    validate(
        &revisions_schema,
        &json_file(&temp.0.join("story/analysis/revisions.json")),
        "story analysis revisions",
    );
}

#[test]
fn the_story_analysis_schema_rejects_unknown_fields() {
    let analysis_schema = schema("story-analysis.schema.json");
    let mut sample: Value = serde_json::from_str(kineto_project::story::SAMPLE_ANALYSIS_JSON)
        .expect("the sample is JSON");
    sample["characters"][0]["age"] = 40.into();
    assert!(validate_value(&analysis_schema, &sample, "$").is_err());
}

#[test]
fn generated_screenplays_and_their_revision_log_match_committed_schemas() {
    let screenplay_schema = schema("screenplay.schema.json");
    let scene_schema = schema("screenplay-scene.schema.json");
    let revisions_schema = schema("revisions.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    kineto_core::story::analyze(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::story::SAMPLE_ANALYSIS_JSON]),
    )
    .expect("the analysis should be generated");
    let written = kineto_core::screenplay::write(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::screenplay::SAMPLE_SCREENPLAY_JSON]),
    )
    .expect("the screenplay should be written");
    let sample: Value = serde_json::from_str(kineto_project::screenplay::SAMPLE_SCREENPLAY_JSON)
        .expect("the sample is JSON");
    validate(&scene_schema, &sample["scenes"][0], "sample scene");
    let mut scene = sample["scenes"][0].clone();
    scene["blocks"][0]["text"] = "Two chairs, one empty.".into();
    let rewritten = kineto_core::screenplay::rewrite_scene(
        &project,
        &kineto_llm::fake::FakeLlm::answering([scene.to_string()]),
        "office_night",
        None,
    )
    .expect("the scene should be rewritten");
    for record in [&written, &rewritten] {
        validate(
            &screenplay_schema,
            &json_file(&temp.0.join(&record.path)),
            &record.path,
        );
    }
    validate(
        &revisions_schema,
        &json_file(&temp.0.join("screenplay/revisions.json")),
        "screenplay revisions",
    );
}

#[test]
fn a_screenplay_scene_is_the_same_shape_alone_and_in_a_screenplay() {
    let strip = |mut value: Value| {
        let object = value.as_object_mut().expect("schemas are objects");
        for key in ["$schema", "$id", "title", "description"] {
            object.remove(key);
        }
        value
    };
    let screenplay_schema = schema("screenplay.schema.json");
    assert_eq!(
        strip(schema("screenplay-scene.schema.json")),
        strip(screenplay_schema["properties"]["scenes"]["items"].clone()),
        "a scene rewritten on its own must fit back into the screenplay"
    );
}

#[test]
fn the_screenplay_schema_rejects_drift() {
    let screenplay_schema = schema("screenplay.schema.json");
    let sample: Value = serde_json::from_str(kineto_project::screenplay::SAMPLE_SCREENPLAY_JSON)
        .expect("the sample is JSON");
    type Edit = fn(&mut Value);
    let drift: [Edit; 5] = [
        |value| value["scenes"][0]["mood"] = "dark".into(),
        |value| value["scenes"][0]["blocks"][0]["kind"] = "montage".into(),
        |value| value["scenes"][0]["setting"] = "indoors".into(),
        |value| value["scenes"] = serde_json::json!([]),
        |value| value["scenes"][0]["id"] = "s".repeat(65).into(),
    ];
    for edit in drift {
        let mut value = sample.clone();
        edit(&mut value);
        assert!(validate_value(&screenplay_schema, &value, "$").is_err());
    }
}

#[test]
fn generated_and_edited_bibles_and_their_revision_log_match_committed_schemas() {
    let bible_schema = schema("bible.schema.json");
    let revisions_schema = schema("revisions.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    kineto_core::story::analyze(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::story::SAMPLE_ANALYSIS_JSON]),
    )
    .expect("the analysis should be generated");
    kineto_core::screenplay::write(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::screenplay::SAMPLE_SCREENPLAY_JSON]),
    )
    .expect("the screenplay should be written");
    kineto_project::screenplay::lock(&project, 1).expect("the screenplay should lock");
    validate(
        &bible_schema,
        &serde_json::from_str(kineto_project::bible::SAMPLE_BIBLE_JSON)
            .expect("the sample is JSON"),
        "sample bible",
    );
    let written = kineto_core::bible::write(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::bible::SAMPLE_BIBLE_JSON]),
    )
    .expect("the bible should be written");
    let mut bible: kineto_project::bible::Bible =
        kineto_project::revisions::read(&project, &written).expect("the bible is readable");
    bible.characters[0].wardrobe_changes = vec![kineto_project::bible::WardrobeChange {
        scenes: vec!["office_night".to_owned()],
        wardrobe: "Shirtsleeves, the coat over a chair.".to_owned(),
    }];
    let edited = kineto_project::bible::edit(&project, 1, &bible).expect("the edit is saved");
    kineto_project::bible::lock(&project, 2).expect("the bible should lock");
    for record in [&written, &edited] {
        validate(
            &bible_schema,
            &json_file(&temp.0.join(&record.path)),
            &record.path,
        );
    }
    validate(
        &revisions_schema,
        &json_file(&temp.0.join("bible/revisions.json")),
        "bible revisions",
    );
}

#[test]
fn generated_and_edited_shot_plans_and_their_revision_log_match_committed_schemas() {
    use kineto_llm::fake::FakeLlm;
    use kineto_project::shot_plan::{SAMPLE_SHOT_PLAN_JSON, ShotPlan};
    let plan_schema = schema("shot-plan.schema.json");
    let scene_schema = schema("shot-plan-scene.schema.json");
    let revisions_schema = schema("revisions.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    kineto_core::story::analyze(
        &project,
        &FakeLlm::answering([kineto_project::story::SAMPLE_ANALYSIS_JSON]),
    )
    .expect("the analysis should be generated");
    kineto_core::screenplay::write(
        &project,
        &FakeLlm::answering([kineto_project::screenplay::SAMPLE_SCREENPLAY_JSON]),
    )
    .expect("the screenplay should be written");
    kineto_project::screenplay::lock(&project, 1).expect("the screenplay should lock");
    kineto_core::bible::write(
        &project,
        &FakeLlm::answering([kineto_project::bible::SAMPLE_BIBLE_JSON]),
    )
    .expect("the bible should be written");
    kineto_project::bible::lock(&project, 1).expect("the bible should lock");
    let sample: Value = serde_json::from_str(SAMPLE_SHOT_PLAN_JSON).expect("the sample is JSON");
    validate(&plan_schema, &sample, "sample shot plan");
    validate(&scene_schema, &sample["scenes"][0], "sample scene plan");
    let written =
        kineto_core::shot_plan::write(&project, &FakeLlm::answering([SAMPLE_SHOT_PLAN_JSON]))
            .expect("the plan should be written");
    let plan: ShotPlan =
        kineto_project::revisions::read(&project, &written).expect("the plan is readable");
    let mut scene = plan.scenes[0].clone();
    scene.shots[0].continuity_from = None;
    scene.shots[1].dialogue = String::new();
    let edited = kineto_project::shot_plan::edit_scene(&project, 1, scene, &Default::default())
        .expect("the edit is saved");
    kineto_project::shot_plan::lock(&project, 2).expect("the plan should lock");
    for record in [&written, &edited] {
        validate(
            &plan_schema,
            &json_file(&temp.0.join(&record.path)),
            &record.path,
        );
    }
    validate(
        &revisions_schema,
        &json_file(&temp.0.join("shot_plan/revisions.json")),
        "shot plan revisions",
    );
    let mut drifted = sample;
    drifted["scenes"][0]["shots"][0]["camera_motion"] = "whip_pan".into();
    assert!(validate_value(&plan_schema, &drifted, "$").is_err());
}

#[test]
fn the_bible_schema_rejects_drift() {
    let bible_schema = schema("bible.schema.json");
    let sample: Value =
        serde_json::from_str(kineto_project::bible::SAMPLE_BIBLE_JSON).expect("the sample is JSON");
    type Edit = fn(&mut Value);
    let drift: [Edit; 6] = [
        |value| value["characters"][0]["age"] = 40.into(),
        |value| value["characters"] = serde_json::json!([]),
        |value| value["characters"][0]["id"] = "c".repeat(113).into(),
        |value| {
            value["characters"][0]["wardrobe_changes"] =
                serde_json::json!([{"scenes": [], "wardrobe": "A raincoat."}]);
        },
        |value| value["props"][0]["scenes"][0] = "s".repeat(65).into(),
        |value| {
            value["world"]
                .as_object_mut()
                .expect("the world is an object")
                .remove("continuity_rules");
        },
    ];
    for edit in drift {
        let mut value = sample.clone();
        edit(&mut value);
        assert!(validate_value(&bible_schema, &value, "$").is_err());
    }
}

#[test]
fn references_match_their_committed_schema() {
    let references_schema = schema("references.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    let arguments = serde_json::from_str(r#"{"query":"the filmmaker"}"#).unwrap();
    let add = |text: &str, attached_to: &[&str]| {
        kineto_project::references::add(
            &project,
            kineto_project::references::NewReference {
                server: "studio-archive".to_owned(),
                tool: "search_wardrobe".to_owned(),
                arguments: serde_json::Map::clone(&arguments),
                text: text.to_owned(),
                attached_to: attached_to
                    .iter()
                    .map(|target| (*target).to_owned())
                    .collect(),
            },
        )
        .expect("the reference should be kept")
    };
    add(
        "A grey wool overcoat.",
        &["bible_world", "bible_character.filmmaker"],
    );
    add("Rain on one window.", &[]);
    kineto_project::references::remove(&project, "reference.0002").expect("remove");
    let written = json_file(&temp.0.join("references/references.json"));
    validate(&references_schema, &written, "references");
    assert_eq!(written["next_number"], 3);

    let mut drifted = written.clone();
    drifted["references"][0]["attached_to"][0] = "the moon".into();
    assert!(validate_value(&references_schema, &drifted, "$").is_err());
    let mut drifted = written;
    drifted["references"][0]["recorded_at"] = "2026-10-03T00:00:00Z".into();
    assert!(
        validate_value(&references_schema, &drifted, "$").is_err(),
        "no time is recorded"
    );
}

#[test]
fn style_samples_and_the_locked_style_match_committed_schemas() {
    let samples_schema = schema("style-samples.schema.json");
    let lock_schema = schema("style.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    kineto_core::story::analyze(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::story::SAMPLE_ANALYSIS_JSON]),
    )
    .expect("the analysis should be generated");
    kineto_core::screenplay::write(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::screenplay::SAMPLE_SCREENPLAY_JSON]),
    )
    .expect("the screenplay should be written");
    kineto_project::screenplay::lock(&project, 1).expect("the screenplay should lock");
    kineto_core::bible::write(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::bible::SAMPLE_BIBLE_JSON]),
    )
    .expect("the bible should be written");
    let client = kineto_provider_minimax::MinimaxClient::with_transport(
        RecordedMinimax,
        kineto_providers::secret::SecretString::new("sk-contract".to_owned())
            .expect("test key is not blank"),
        "https://example.test",
    );
    let sample = |note| {
        kineto_core::style::sample_with(
            &project,
            kineto_core::minimax::MinimaxKeyframes::new(&project, Some(&client)),
            kineto_core::style::SampleRequest {
                preset: "film_noir",
                note,
                scene_id: None,
            },
            kineto_core::style::Submission::Once,
            &mut NoHooks,
        )
        .expect("the sample should be recorded")
    };
    sample(None);
    let refined = sample(Some("colder"));
    kineto_project::style::select(&project, &refined.artifact_id).expect("select");
    kineto_project::style::reject(&project, "style_s0001").expect("reject");
    kineto_project::style::lock(&project, &refined.artifact_id).expect("lock");
    validate(
        &samples_schema,
        &json_file(&temp.0.join("style/samples.json")),
        "style samples",
    );
    let lock = json_file(&temp.0.join("bible/style.json"));
    validate(&lock_schema, &lock, "locked style");

    let mut drifted = lock.clone();
    drifted["directives"] = "".into();
    assert!(validate_value(&lock_schema, &drifted, "$").is_err());
    let mut drifted = json_file(&temp.0.join("style/samples.json"));
    drifted["samples"][0]["status"] = "locked".into();
    assert!(validate_value(&samples_schema, &drifted, "$").is_err());
}

#[test]
fn casting_records_and_a_locked_character_match_committed_schemas() {
    let casting_schema = schema("casting.schema.json");
    let character_schema = schema("character.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    kineto_core::story::analyze(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::story::SAMPLE_ANALYSIS_JSON]),
    )
    .expect("the analysis should be generated");
    kineto_core::screenplay::write(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::screenplay::SAMPLE_SCREENPLAY_JSON]),
    )
    .expect("the screenplay should be written");
    kineto_project::screenplay::lock(&project, 1).expect("the screenplay should lock");
    kineto_core::bible::write(
        &project,
        &kineto_llm::fake::FakeLlm::answering([kineto_project::bible::SAMPLE_BIBLE_JSON]),
    )
    .expect("the bible should be written");
    let client = kineto_provider_minimax::MinimaxClient::with_transport(
        RecordedMinimax,
        kineto_providers::secret::SecretString::new("sk-contract".to_owned())
            .expect("test key is not blank"),
        "https://example.test",
    );
    let adapter = || kineto_core::minimax::MinimaxKeyframes::new(&project, Some(&client));
    let sample = kineto_core::style::sample_with(
        &project,
        adapter(),
        kineto_core::style::SampleRequest {
            preset: "film_noir",
            note: None,
            scene_id: None,
        },
        kineto_core::style::Submission::Once,
        &mut NoHooks,
    )
    .expect("the style sample should be recorded");
    kineto_project::style::select(&project, &sample.artifact_id).expect("select style");
    kineto_project::style::lock(&project, &sample.artifact_id).expect("lock style");
    kineto_core::cast::audition_with(
        &project,
        adapter(),
        "filmmaker",
        2,
        kineto_core::style::Submission::Once,
        &mut NoHooks,
    )
    .expect("portraits should be recorded");
    kineto_project::cast::select(&project, "filmmaker", "cast.filmmaker.0001").expect("select");
    kineto_project::cast::reject(&project, "filmmaker", "cast.filmmaker.0002").expect("reject");
    kineto_core::cast::make_pack_with(
        &project,
        adapter(),
        "filmmaker",
        kineto_core::style::Submission::Once,
        &mut NoHooks,
    )
    .expect("the pack should be recorded");
    let provider = ImageProvider::Minimax(Some(kineto_provider_minimax::MinimaxClient::shared(
        kineto_providers::secret::SecretString::new("sk-contract".to_owned())
            .expect("test key is not blank"),
    )));
    kineto_core::cast::lock(&project, &provider, "filmmaker").expect("the cast should lock");
    let casting = json_file(&temp.0.join("characters/filmmaker/casting.json"));
    validate(&casting_schema, &casting, "casting record");
    let character = json_file(&temp.0.join("characters/filmmaker/character.json"));
    validate(&character_schema, &character, "locked character");

    let mut drifted = casting.clone();
    drifted["images"][0]["kind"] = "headshot".into();
    assert!(validate_value(&casting_schema, &drifted, "$").is_err());
    let mut drifted = character.clone();
    drifted["provider_reference"]["role"] = "style".into();
    assert!(validate_value(&character_schema, &drifted, "$").is_err());

    // The cast character's voice, auditioned with the demo and locked.
    let auditions_schema = schema("voice-auditions.schema.json");
    let voice_schema = schema("voice.schema.json");
    let calm = kineto_core::speech::VoiceSettings {
        emotion: Some(kineto_providers::Emotion::Calm),
        ..kineto_core::speech::VoiceSettings::new("demo_low", "en")
    };
    kineto_core::voice::audition(
        &project,
        &kineto_core::speech::LocalDemoVoices,
        "filmmaker",
        &[
            kineto_core::voice::VoiceChoice {
                voice: calm,
                name: "Low".to_owned(),
            },
            kineto_core::voice::VoiceChoice {
                voice: kineto_core::speech::VoiceSettings::new("demo_high", "en"),
                name: "High".to_owned(),
            },
        ],
        None,
        kineto_core::style::Submission::Once,
        &mut NoHooks,
    )
    .expect("auditions should be recorded");
    kineto_project::voice::select(&project, "filmmaker", "voice.filmmaker.0001").expect("select");
    kineto_project::voice::reject(&project, "filmmaker", "voice.filmmaker.0002").expect("reject");
    kineto_core::voice::lock(&project, "filmmaker").expect("the voice should lock");
    let auditions = json_file(&temp.0.join("characters/filmmaker/voice_auditions.json"));
    validate(&auditions_schema, &auditions, "voice auditions");
    let voice = json_file(&temp.0.join("characters/filmmaker/voice.json"));
    validate(&voice_schema, &voice, "locked voice");

    let mut drifted = auditions.clone();
    drifted["auditions"][0]["voice"]["emotion"] = "wistful".into();
    assert!(validate_value(&auditions_schema, &drifted, "$").is_err());
    let mut drifted = voice.clone();
    drifted["artifact_id"] = "voice filmmaker".into();
    assert!(validate_value(&voice_schema, &drifted, "$").is_err());
}

#[test]
fn preference_revisions_match_committed_schemas() {
    use kineto_project::preferences::{self, Dimension, Preference, Preferences, Source, Stance};
    let preferences_schema = schema("preferences.schema.json");
    let revisions_schema = schema("revisions.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    let first = preferences::save(
        &project,
        None,
        &Preferences {
            entries: vec![Preference {
                dimension: Dimension::CameraMotion,
                stance: Stance::Prefer,
                value: "slow dolly".to_owned(),
                strength_permille: Some(700),
                source: Source::Director,
            }],
        },
    )
    .expect("the first profile is saved");
    preferences::save(
        &project,
        Some(first.revision),
        &Preferences {
            entries: vec![Preference {
                dimension: Dimension::Lighting,
                stance: Stance::Avoid,
                value: "flat".to_owned(),
                strength_permille: None,
                source: Source::Suggestion {
                    reason: "Every chosen take is lit from one side.".to_owned(),
                    evidence: vec!["office_night_02_g0001_candidate_b".to_owned()],
                },
            }],
        },
    )
    .expect("the edit is saved");
    for revision in ["preferences_r0001", "preferences_r0002"] {
        let document = json_file(&temp.0.join(format!("preferences/{revision}.json")));
        validate(&preferences_schema, &document, revision);
    }
    let log = json_file(&temp.0.join("preferences/revisions.json"));
    validate(&revisions_schema, &log, "preference revisions");
    assert_eq!(log["kind"], "preferences");

    let mut drifted = json_file(&temp.0.join("preferences/preferences_r0002.json"));
    drifted["entries"][0]["source"]["evidence"] = serde_json::json!([]);
    assert!(validate_value(&preferences_schema, &drifted, "$").is_err());
    let mut drifted = json_file(&temp.0.join("preferences/preferences_r0001.json"));
    drifted["entries"][0]["stance"] = "tolerate".into();
    assert!(validate_value(&preferences_schema, &drifted, "$").is_err());
}

#[test]
fn real_job_runtime_intent_matches_committed_schema() {
    let job_schema = schema("job.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    let runtime = JobRuntime::new(
        &project,
        ProviderKey::new("schema-contract-provider").expect("provider key should be valid"),
        runtime_policy(),
    );
    let job_id = JobId::new("schema_contract_job").expect("job id should be valid");

    runtime
        .prepare(
            job_id.clone(),
            "image.generate",
            InputHash::new(format!("sha256:{}", "a".repeat(64)))
                .expect("input hash should be valid"),
            ArtifactId::new("schema_contract_candidate").expect("artifact id should be valid"),
        )
        .expect("prepare should persist the write-ahead intent");

    validate(
        &job_schema,
        &json_file(
            &temp
                .0
                .join(".kineto/jobs/intents")
                .join(format!("{}.json", job_id.as_str())),
        ),
        "production job intent",
    );
}

#[test]
fn production_generation_jobs_and_candidates_match_committed_schemas() {
    let job_schema = schema("job.schema.json");
    let artifact_schema = schema("artifact.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create(&temp.0, manifest(), b"schema contract story\n")
        .expect("test project should be creatable");
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).expect("shot should load");

    generation::generate(&project, &mut shot, &ImageProvider::LocalDemo, &mut NoHooks)
        .expect("generation should run as write-ahead jobs");

    let mut intents = fs::read_dir(temp.0.join(".kineto/jobs/intents"))
        .expect("generation should record intents")
        .map(|entry| entry.expect("intent entry should be readable").path())
        .collect::<Vec<_>>();
    intents.sort();
    assert_eq!(intents.len(), 3, "one intent per candidate");
    for intent in &intents {
        let value = json_file(intent);
        validate(&job_schema, &value, "production generation job intent");
        assert_eq!(value["state"], "reconciled");
        assert_eq!(value["operation"], "image.generate");
    }

    let artifacts_path = temp
        .0
        .join("scenes/scene_001/shots/shot_001/artifacts.json");
    validate_artifact_array(&artifacts_path, &artifact_schema);
    for artifact in json_file(&artifacts_path)
        .as_array()
        .expect("artifacts.json is an array")
    {
        assert_eq!(artifact["provider"], "local-demo");
        assert_eq!(artifact["model"], "deterministic-demo-v1");
    }
}

/// Answers every MiniMax image request with a tiny JPEG; never touches the
/// network.
struct RecordedMinimax;

impl kineto_provider_minimax::Transport for RecordedMinimax {
    fn post_json(
        &self,
        _url: &str,
        _api_key: &kineto_providers::secret::SecretString,
        _body: &[u8],
    ) -> Result<kineto_provider_minimax::HttpResponse, kineto_provider_minimax::TransportFailure>
    {
        Ok(kineto_provider_minimax::HttpResponse {
            status: 200,
            // base64 of ff d8 ff e0 00 10
            body: br#"{"id":"r","data":{"image_base64":["/9j/4AAQ"]},"base_resp":{"status_code":0,"status_msg":"success"}}"#
                .to_vec(),
        })
    }

    fn get(
        &self,
        _url: &str,
        _api_key: &kineto_providers::secret::SecretString,
    ) -> Result<kineto_provider_minimax::HttpResponse, kineto_provider_minimax::TransportFailure>
    {
        unreachable!("image generation makes no GET request")
    }

    fn download(
        &self,
        _url: &str,
        _max_bytes: u64,
    ) -> Result<kineto_provider_minimax::HttpResponse, kineto_provider_minimax::TransportFailure>
    {
        unreachable!("image generation downloads nothing")
    }
}

#[test]
fn provider_generated_keyframe_candidates_match_committed_schemas() {
    let artifact_schema = schema("artifact.schema.json");
    let temp = TempDir::new();
    let project = CanonicalProject::create_with_files(
        &temp.0,
        manifest(),
        b"schema contract story\n",
        &kineto_core::bootstrap::scene_files(),
    )
    .expect("test project should be creatable with the bootstrap scene");
    let client = kineto_provider_minimax::MinimaxClient::with_transport(
        RecordedMinimax,
        kineto_providers::secret::SecretString::new("sk-contract".to_owned())
            .expect("test key is not blank"),
        "https://example.test",
    );
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).expect("shot should load");

    generation::generate_with(
        &project,
        &mut shot,
        kineto_core::minimax::MinimaxKeyframes::new(&project, Some(&client)),
        &mut NoHooks,
    )
    .expect("generation should attach provider keyframes");

    let artifacts_path = temp
        .0
        .join("scenes/scene_001/shots/shot_001/artifacts.json");
    validate_artifact_array(&artifacts_path, &artifact_schema);
    for artifact in json_file(&artifacts_path)
        .as_array()
        .expect("artifacts.json is an array")
    {
        for field in [
            "media_path",
            "media_state",
            "compiler_version",
            "compiled_prompt",
            "prompt_hash",
            "applied_constraints",
            "dropped_constraints",
            "degradation",
        ] {
            assert!(!artifact[field].is_null(), "{field} is recorded");
        }
    }
}

#[test]
fn contract_harness_fails_closed_on_unknown_schema_keywords_and_patterns() {
    let unsupported_keyword = serde_json::json!({
        "type": "object",
        "oneOf": [{"type": "object"}]
    });
    assert!(ensure_supported_schema(&unsupported_keyword, "$").is_err());

    let unsupported_pattern = serde_json::json!({
        "type": "string",
        "pattern": "^new-pattern$"
    });
    assert!(ensure_supported_schema(&unsupported_pattern, "$").is_err());
}

#[test]
fn committed_schemas_reject_representative_model_drift() {
    let project_schema = schema("project.schema.json");
    let mut project =
        toml_file_as_json(&repo_root().join("fixtures/projects/minimal/project.toml"));
    project
        .as_object_mut()
        .expect("project fixture is an object")
        .remove("title");
    assert!(validate_value(&project_schema, &project, "$").is_err());

    let mut invalid_timestamp =
        toml_file_as_json(&repo_root().join("fixtures/projects/minimal/project.toml"));
    invalid_timestamp["created_at"] = Value::String("2026-02-30T25:61:61+24:60".to_owned());
    assert!(validate_value(&project_schema, &invalid_timestamp, "$").is_err());

    let artifact_schema = schema("artifact.schema.json");
    let mut artifact =
        json_file(&repo_root().join("fixtures/projects/minimal/characters/alice/artifacts.json"))
            .as_array()
            .expect("artifact fixture is an array")[0]
            .clone();
    artifact["content_hash"] = Value::String("not-a-canonical-hash".to_owned());
    assert!(validate_value(&artifact_schema, &artifact, "$").is_err());

    let selection_schema = schema("selection.schema.json");
    let duplicate_selection = serde_json::json!({
        "selected_artifact_id": "candidate_a",
        "candidate_artifact_ids": ["candidate_a", "candidate_a"]
    });
    assert!(validate_value(&selection_schema, &duplicate_selection, "$").is_err());

    let job_schema = schema("job.schema.json");
    let invalid_job = serde_json::json!({
        "schema_version": 1,
        "provider": "provider with spaces",
        "job_id": "job_001",
        "operation": "image.generate",
        "state": "prepared",
        "idempotency_key": "image.generate:key",
        "input_hash": format!("sha256:{}", "a".repeat(64)),
        "attempts": 0
    });
    assert!(validate_value(&job_schema, &invalid_job, "$").is_err());
}
