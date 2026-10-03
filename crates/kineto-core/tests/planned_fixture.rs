//! `fixtures/projects/planned` is the production fixture taken on to shot
//! planning: its bible locked, the sample shot plan written and locked so
//! its two shots are in play, and the second shot generated twice, the
//! second time refined by a note. A scripted model and an offline MiniMax
//! wrote it. Tests outside Rust open copies of it and make only the free
//! decisions. This
//! test rebuilds it and fails if the committed files differ;
//! `KINETO_UPDATE_FIXTURES=1` rewrites them instead.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use base64::Engine as _;
use kineto_core::{
    bible, bootstrap, cast,
    generation::{self, Consent, ImageProvider, NoHooks},
    minimax::MinimaxKeyframes,
    screenplay, shot_plan, story,
    style::{self, SampleRequest, Submission},
};
use kineto_llm::fake::FakeLlm;
use kineto_project::{
    bible::SAMPLE_BIBLE_JSON,
    cast as cast_document,
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    screenplay::SAMPLE_SCREENPLAY_JSON,
    shot::{ShotKey, ShotWorkflow},
    shot_plan::SAMPLE_SHOT_PLAN_JSON,
    story::SAMPLE_ANALYSIS_JSON,
    style as style_document,
};
use kineto_provider_minimax::{HttpResponse, MinimaxClient, Transport, TransportFailure};
use kineto_providers::secret::SecretString;

const STORY: &str = "The interview is over. A filmmaker hears the interviewer's last question, \
says nothing, and leaves before he can answer.\n";

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/planned")
}

/// MiniMax `image-01`, offline: an image derived from the request, so the
/// same request always answers the same however calls interleave (a
/// generation's candidates are asked for at once).
#[derive(Default)]
struct FakeMinimax;

impl Transport for FakeMinimax {
    fn post_json(
        &self,
        _url: &str,
        _api_key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        use sha2::Digest as _;
        let digest = sha2::Sha256::digest(body);
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0x50];
        jpeg.extend_from_slice(&digest[..8]);
        Ok(HttpResponse {
            status: 200,
            body: format!(
                r#"{{"id":"f","data":{{"image_base64":["{}"]}},"base_resp":{{"status_code":0}}}}"#,
                base64::engine::general_purpose::STANDARD.encode(jpeg)
            )
            .into_bytes(),
        })
    }

    fn get(&self, url: &str, _api_key: &SecretString) -> Result<HttpResponse, TransportFailure> {
        unreachable!("images make no GET request: {url}")
    }

    fn download(&self, url: &str, _max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
        unreachable!("images download nothing: {url}")
    }
}

fn build(target: &Path) {
    let project = CanonicalProject::create_with_files(
        target,
        ProjectManifest {
            format_version: PROJECT_FORMAT_VERSION,
            project_id: "planned_fixture".to_owned(),
            title: "The Last Line".to_owned(),
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
    screenplay::write(&project, &FakeLlm::answering([SAMPLE_SCREENPLAY_JSON]))
        .expect("the screenplay is written");
    kineto_project::screenplay::lock(&project, 1).expect("the screenplay is locked");
    bible::write(&project, &FakeLlm::answering([SAMPLE_BIBLE_JSON])).expect("the bible is written");

    let client = MinimaxClient::with_transport(
        FakeMinimax,
        SecretString::new("sk-fixture".to_owned()).expect("a key"),
        "https://example.test",
    );
    let adapter = || MinimaxKeyframes::new(&project, Some(&client));
    for preset in ["film_noir", "film_realism"] {
        style::sample_with(
            &project,
            adapter(),
            SampleRequest {
                preset,
                ..SampleRequest::default()
            },
            Submission::Once,
            &mut NoHooks,
        )
        .expect("a sample is made");
    }
    style_document::select(&project, "style_s0001").expect("the first sample is selected");
    style_document::lock(&project, "style_s0001").expect("the first sample is locked");
    cast::audition_with(
        &project,
        adapter(),
        "filmmaker",
        2,
        Submission::Once,
        &mut NoHooks,
    )
    .expect("portraits are made");
    cast_document::select(&project, "filmmaker", "cast.filmmaker.0002")
        .expect("a portrait is selected");
    cast::make_pack_with(
        &project,
        adapter(),
        "filmmaker",
        Submission::Once,
        &mut NoHooks,
    )
    .expect("the pack is made");
    cast::lock(&project, &ImageProvider::Minimax(None), "filmmaker")
        .expect("the filmmaker is cast");
    cast::audition_with(
        &project,
        adapter(),
        "interviewer",
        1,
        Submission::Once,
        &mut NoHooks,
    )
    .expect("a portrait is made");

    kineto_project::bible::lock(&project, 1).expect("the bible is locked");
    shot_plan::write(&project, &FakeLlm::answering([SAMPLE_SHOT_PLAN_JSON]))
        .expect("the shots are planned");
    kineto_project::shot_plan::lock(&project, 1).expect("the plan is locked");
    let key = ShotKey::new("office_night", "office_night_02").expect("a planned shot");
    let mut shot = ShotWorkflow::load(&project, &key).expect("the shot loads");
    generation::generate_noted(
        &project,
        &mut shot,
        adapter(),
        &mut NoHooks,
        Consent::default(),
        None,
    )
    .expect("keyframes are made");
    generation::generate_noted(
        &project,
        &mut shot,
        adapter(),
        &mut NoHooks,
        Consent::default(),
        Some("Tighter on his hands."),
    )
    .expect("keyframes are refined");
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
fn the_planned_fixture_is_what_the_engine_writes() {
    let temp = std::env::temp_dir().join(format!("kineto-planned-fixture-{}", std::process::id()));
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
