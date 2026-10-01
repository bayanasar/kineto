//! The screenplay: scenes as structured state, and the Markdown they render
//! to.
//!
//! `schemas/screenplay.schema.json` is the contract. Each revision's JSON is
//! the authority; `screenplay/screenplay.md` is rendered from the current
//! revision and can be deleted and rebuilt byte for byte.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    ArtifactDependency, ArtifactId, ContentHash, DependencyImpact,
    fs::{ProjectFsError, ProjectRelativePath},
    manifest::CanonicalProject,
    revisions::{self, DocumentKind, Origin, RevisionError, RevisionRecord, RevisionStatus},
    story::{StoryAnalysis, is_portable_id},
};

/// The committed schema, as the model is shown it.
pub const SCREENPLAY_SCHEMA: &str = include_str!("../../../schemas/screenplay.schema.json");

/// The schema of one scene, for regenerating a single scene.
pub const SCENE_SCHEMA: &str = include_str!("../../../schemas/screenplay-scene.schema.json");

/// Where the rendered screenplay is written.
pub const MARKDOWN_PATH: &str = "screenplay/screenplay.md";

/// Scene IDs are short enough to name a scene as an artifact.
const MAX_SCENE_ID: usize = 64;

/// The artifact ID prefix a scene is depended on by, as in
/// `screenplay_scene.office_night`.
pub const SCENE_ARTIFACT_PREFIX: &str = "screenplay_scene.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Setting {
    Interior,
    Exterior,
    InteriorExterior,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CastMember {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    Action,
    Dialogue,
    Transition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub kind: BlockKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parenthetical: Option<String>,
    pub text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScene {
    id: String,
    heading: String,
    setting: Setting,
    location: String,
    time: String,
    characters: Vec<String>,
    dramatic_goal: String,
    blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawScene")]
pub struct Scene {
    pub id: String,
    pub heading: String,
    pub setting: Setting,
    pub location: String,
    pub time: String,
    pub characters: Vec<String>,
    pub dramatic_goal: String,
    pub blocks: Vec<Block>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScreenplay {
    title: String,
    cast: Vec<CastMember>,
    scenes: Vec<Scene>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawScreenplay")]
pub struct Screenplay {
    pub title: String,
    pub cast: Vec<CastMember>,
    pub scenes: Vec<Scene>,
}

fn filled(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} is empty"))
    } else {
        Ok(())
    }
}

fn portable(id: &str, what: &str) -> Result<(), String> {
    if is_portable_id(id) {
        Ok(())
    } else {
        Err(format!("{what} id {id:?} is not a portable identifier"))
    }
}

impl Scene {
    fn validate(&self) -> Result<(), String> {
        portable(&self.id, "scene")?;
        if self.id.len() > MAX_SCENE_ID {
            return Err(format!(
                "scene id {:?} is longer than {MAX_SCENE_ID} characters",
                self.id
            ));
        }
        portable(&self.location, "location")?;
        let owner = format!("scene {:?}", self.id);
        filled(&self.heading, &format!("{owner} heading"))?;
        filled(&self.time, &format!("{owner} time"))?;
        filled(&self.dramatic_goal, &format!("{owner} dramatic goal"))?;
        if self.blocks.is_empty() {
            return Err(format!("{owner} has no blocks"));
        }
        let mut present = BTreeSet::new();
        for character in &self.characters {
            portable(character, "character")?;
            if !present.insert(character.as_str()) {
                return Err(format!("{owner} lists character {character:?} twice"));
            }
        }
        for block in &self.blocks {
            filled(&block.text, &format!("a block of {owner}"))?;
            match (block.kind, &block.character) {
                (BlockKind::Dialogue, None) => {
                    return Err(format!("dialogue in {owner} has no character"));
                }
                (BlockKind::Dialogue, Some(character)) if !present.contains(character.as_str()) => {
                    return Err(format!(
                        "{character:?} speaks in {owner} but is not among its characters"
                    ));
                }
                (BlockKind::Action | BlockKind::Transition, Some(_)) => {
                    return Err(format!("only dialogue in {owner} names a character"));
                }
                _ => {}
            }
            if block.kind != BlockKind::Dialogue && block.parenthetical.is_some() {
                return Err(format!("only dialogue in {owner} has a parenthetical"));
            }
            if let Some(parenthetical) = &block.parenthetical {
                filled(parenthetical, &format!("a parenthetical in {owner}"))?;
            }
        }
        Ok(())
    }

    /// The scene's own hash: what it says, independent of layout. How the
    /// file that holds it is formatted, where in the screenplay it sits, and
    /// whitespace at line ends or between paragraphs do not change it.
    #[must_use]
    pub fn content_hash(&self) -> ContentHash {
        let mut scene = self.clone();
        for text in [
            &mut scene.heading,
            &mut scene.time,
            &mut scene.dramatic_goal,
        ] {
            *text = tidy(text);
        }
        for block in &mut scene.blocks {
            block.text = tidy(&block.text);
            if let Some(parenthetical) = &mut block.parenthetical {
                *parenthetical = tidy(parenthetical);
            }
        }
        ContentHash::of(&serde_json::to_vec(&scene).expect("a scene always encodes"))
    }

    /// The dependency downstream work records on this scene. It stays fresh
    /// across revisions for as long as the scene's content does.
    pub fn dependency(&self) -> Result<ArtifactDependency, RevisionError> {
        Ok(ArtifactDependency {
            artifact_id: ArtifactId::new(format!("{SCENE_ARTIFACT_PREFIX}{}", self.id))
                .map_err(|_| RevisionError::InvalidDocument("scene id is too long".to_owned()))?,
            content_hash: self.content_hash(),
            impact: DependencyImpact::Semantic,
            field: None,
        })
    }
}

/// `text` with every line trimmed and runs of blank lines folded to one.
fn tidy(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in text.lines().map(str::trim) {
        if !(line.is_empty() && out.last().is_none_or(|last| last.is_empty())) {
            out.push(line);
        }
    }
    while out.last().is_some_and(|last| last.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// Where downstream work that depends on a scene stands against a
/// screenplay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneFreshness {
    /// The scene says what it said when the work was made.
    Fresh,
    /// The scene has changed; the work is stale.
    Changed,
    /// The scene is gone; the work is stale.
    Removed,
}

impl TryFrom<RawScene> for Scene {
    type Error = String;

    fn try_from(raw: RawScene) -> Result<Self, String> {
        let scene = Self {
            id: raw.id,
            heading: raw.heading,
            setting: raw.setting,
            location: raw.location,
            time: raw.time,
            characters: raw.characters,
            dramatic_goal: raw.dramatic_goal,
            blocks: raw.blocks,
        };
        scene.validate()?;
        Ok(scene)
    }
}

impl Screenplay {
    fn validate(&self) -> Result<(), String> {
        filled(&self.title, "title")?;
        if self.cast.is_empty() {
            return Err("the cast is empty".to_owned());
        }
        if self.scenes.is_empty() {
            return Err("there are no scenes".to_owned());
        }
        let mut cast = BTreeSet::new();
        for member in &self.cast {
            portable(&member.id, "cast")?;
            filled(&member.name, &format!("cast member {:?} name", member.id))?;
            if !cast.insert(member.id.as_str()) {
                return Err(format!("cast id {:?} appears twice", member.id));
            }
        }
        let mut scenes = BTreeSet::new();
        for scene in &self.scenes {
            scene.validate()?;
            if !scenes.insert(scene.id.as_str()) {
                return Err(format!("scene id {:?} appears twice", scene.id));
            }
            if let Some(stranger) = scene
                .characters
                .iter()
                .find(|character| !cast.contains(character.as_str()))
            {
                return Err(format!(
                    "scene {:?} names character {stranger:?}, who is not in the cast",
                    scene.id
                ));
            }
        }
        Ok(())
    }

    /// Check that every cast member and location is one `analysis` defines.
    pub fn check_against(&self, analysis: &StoryAnalysis) -> Result<(), String> {
        for member in &self.cast {
            if !analysis
                .characters
                .iter()
                .any(|character| character.id == member.id)
            {
                return Err(format!(
                    "cast member {:?} is not a character of the story analysis",
                    member.id
                ));
            }
        }
        for scene in &self.scenes {
            if !analysis
                .locations
                .iter()
                .any(|location| location.id == scene.location)
            {
                return Err(format!(
                    "scene {:?} is set in {:?}, which is not a location of the story analysis",
                    scene.id, scene.location
                ));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn scene(&self, id: &str) -> Option<&Scene> {
        self.scenes.iter().find(|scene| scene.id == id)
    }

    /// Where work that recorded a dependency on a scene stands against this
    /// screenplay, or `None` when the dependency is not on a scene.
    #[must_use]
    pub fn freshness(&self, artifact_id: &str, content_hash: &str) -> Option<SceneFreshness> {
        let id = artifact_id.strip_prefix(SCENE_ARTIFACT_PREFIX)?;
        Some(match self.scene(id) {
            None => SceneFreshness::Removed,
            Some(scene) if scene.content_hash().as_str() == content_hash => SceneFreshness::Fresh,
            Some(_) => SceneFreshness::Changed,
        })
    }

    /// This screenplay with `scene` in place of the scene that has its ID.
    pub fn with_scene(&self, scene: Scene) -> Result<Self, String> {
        let mut next = self.clone();
        let slot = next
            .scenes
            .iter_mut()
            .find(|existing| existing.id == scene.id)
            .ok_or_else(|| format!("there is no scene {:?}", scene.id))?;
        *slot = scene;
        next.validate()?;
        Ok(next)
    }

    /// The screenplay as Markdown. Deterministic: the same screenplay always
    /// renders to the same bytes.
    #[must_use]
    pub fn render_markdown(&self, artifact_id: &str) -> String {
        let mut out = format!(
            "<!-- Rendered by Kineto from {artifact_id}. Edit the screenplay in Kineto; this file \
             is rebuilt from it. -->\n\n# {}\n",
            escape_line(&self.title)
        );
        for (number, scene) in self.scenes.iter().enumerate() {
            out.push_str(&format!(
                "\n## {}. {}\n\n> {}\n",
                number + 1,
                escape_line(&scene.heading),
                lines(&scene.dramatic_goal).join("\\\n> ")
            ));
            for block in &scene.blocks {
                out.push('\n');
                match block.kind {
                    BlockKind::Action => out.push_str(&paragraphs(&block.text)),
                    BlockKind::Dialogue => {
                        let speaker = block.character.as_deref().unwrap_or_default();
                        let name = self
                            .cast
                            .iter()
                            .find(|member| member.id == speaker)
                            .map_or(speaker, |member| member.name.as_str());
                        let mut speech = vec![format!("**{}**", escape_line(&name.to_uppercase()))];
                        if let Some(parenthetical) = &block.parenthetical {
                            speech.push(format!("*({})*", lines(parenthetical).join(" ")));
                        }
                        speech.extend(lines(&block.text));
                        out.push_str(&speech.join("\\\n"));
                    }
                    BlockKind::Transition => {
                        out.push_str(&format!("*{}*", lines(&block.text).join(" ")));
                    }
                }
                out.push('\n');
            }
        }
        out
    }
}

impl TryFrom<RawScreenplay> for Screenplay {
    type Error = String;

    fn try_from(raw: RawScreenplay) -> Result<Self, String> {
        let screenplay = Self {
            title: raw.title,
            cast: raw.cast,
            scenes: raw.scenes,
        };
        screenplay.validate()?;
        Ok(screenplay)
    }
}

/// Escape what Markdown would read as markup, so text renders as written.
fn escape_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for character in line.chars() {
        if matches!(
            character,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '|' | '~'
        ) {
            out.push('\\');
        }
        out.push(character);
    }
    // Line starts that open a heading, list, quote or rule.
    let starts_block = out.starts_with(['#', '-', '+', '='])
        || out
            .find(|character: char| !character.is_ascii_digit())
            .is_some_and(|end| end > 0 && out[end..].starts_with(['.', ')']));
    if starts_block {
        out.insert(0, '\\');
    }
    out
}

/// The non-blank lines of `text`, trimmed and escaped.
fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(escape_line)
        .collect()
}

/// `text` as Markdown paragraphs: blank lines separate them, and line
/// breaks within one are kept.
fn paragraphs(text: &str) -> String {
    let mut out = Vec::new();
    let mut current = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current).join("\\\n"));
            }
        } else {
            current.push(escape_line(line));
        }
    }
    if !current.is_empty() {
        out.push(current.join("\\\n"));
    }
    out.join("\n\n")
}

fn markdown_path() -> ProjectRelativePath {
    ProjectRelativePath::new(MARKDOWN_PATH).expect("static screenplay path is valid")
}

/// Write `screenplay/screenplay.md` from the current revision, or remove it
/// when there is none. Returns the revision rendered.
pub fn render(project: &CanonicalProject) -> Result<Option<RevisionRecord>, RevisionError> {
    let Some(record) = revisions::current(project, DocumentKind::Screenplay)? else {
        match std::fs::remove_file(project.root().resolve(&markdown_path())) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(RevisionError::Fs(ProjectFsError::Io(error))),
        }
        return Ok(None);
    };
    let screenplay: Screenplay = revisions::read(project, &record)?;
    project
        .validate_for_write()
        .map_err(RevisionError::Manifest)?;
    project
        .root()
        .write_atomic(
            &markdown_path(),
            screenplay.render_markdown(&record.artifact_id).as_bytes(),
        )
        .map_err(RevisionError::Fs)?;
    Ok(Some(record))
}

/// Save the director's edit of revision `based_on` as the next revision.
/// `based_on` must be the latest revision, so an edit never silently
/// replaces one the director has not seen. An edit that changes nothing
/// returns the revision it started from.
pub fn edit(
    project: &CanonicalProject,
    based_on: u32,
    screenplay: &Screenplay,
) -> Result<RevisionRecord, RevisionError> {
    screenplay
        .validate()
        .map_err(RevisionError::InvalidDocument)?;
    let latest = revisions::latest(project, DocumentKind::Screenplay)?
        .ok_or(RevisionError::NoSuchRevision(based_on))?;
    if latest.revision != based_on {
        return Err(RevisionError::NotLatest {
            based_on,
            latest: latest.revision,
        });
    }
    let bytes = revisions::encode(screenplay)?;
    if ContentHash::of(&bytes).as_str() == latest.content_hash {
        return Ok(latest);
    }
    let next = revisions::next(project, DocumentKind::Screenplay)?;
    let content_hash = revisions::write_document(project, &next, &bytes)?;
    let own_revisions = format!("{}_r", DocumentKind::Screenplay.id_prefix());
    let record = RevisionRecord {
        revision: next.revision,
        artifact_id: next.artifact_id.as_str().to_owned(),
        path: next.path.as_path().to_string_lossy().replace('\\', "/"),
        status: RevisionStatus::Candidate,
        origin: Origin::Edited,
        content_hash: content_hash.as_str().to_owned(),
        input_hash: None,
        provider: None,
        model: None,
        provenance: None,
        based_on: Some(based_on),
        // Still made from the analysis and source the edited revision was;
        // `based_on` records the lineage.
        dependencies: latest
            .dependencies
            .into_iter()
            .filter(|dependency| !dependency.artifact_id.starts_with(&own_revisions))
            .collect(),
    };
    revisions::attach(project, DocumentKind::Screenplay, record.clone())?;
    rerender(project);
    Ok(record)
}

/// Lock `revision` as the screenplay production follows.
pub fn lock(project: &CanonicalProject, revision: u32) -> Result<(), RevisionError> {
    revisions::lock(project, DocumentKind::Screenplay, revision)?;
    rerender(project);
    Ok(())
}

/// Render after a change. `screenplay.md` is derived: failing to write it
/// does not undo the change, and the next render rebuilds it.
pub fn rerender(project: &CanonicalProject) {
    let _ = render(project);
}

/// Where work that depends on `artifact_id` at `content_hash` stands against
/// the current screenplay, or `None` when the dependency is not on a scene.
pub fn scene_freshness(
    project: &CanonicalProject,
    artifact_id: &str,
    content_hash: &str,
) -> Result<Option<SceneFreshness>, RevisionError> {
    if !artifact_id.starts_with(SCENE_ARTIFACT_PREFIX) {
        return Ok(None);
    }
    let Some(record) = revisions::current(project, DocumentKind::Screenplay)? else {
        return Ok(Some(SceneFreshness::Removed));
    };
    let screenplay: Screenplay = revisions::read(project, &record)?;
    Ok(screenplay.freshness(artifact_id, content_hash))
}

/// A small, valid screenplay of [`crate::story::SAMPLE_ANALYSIS_JSON`], for
/// tests across the workspace.
#[doc(hidden)]
pub const SAMPLE_SCREENPLAY_JSON: &str = r#"{
  "title": "The Last Line",
  "cast": [
    {"id": "filmmaker", "name": "Filmmaker"},
    {"id": "interviewer", "name": "Interviewer"}
  ],
  "scenes": [
    {
      "id": "office_night",
      "heading": "INT. SMALL OFFICE - NIGHT",
      "setting": "interior",
      "location": "small_office",
      "time": "night",
      "characters": ["filmmaker", "interviewer"],
      "dramatic_goal": "The last question lands and he cannot stay to answer it.",
      "blocks": [
        {"kind": "action", "text": "One table, two chairs. Rain on the window."},
        {"kind": "dialogue", "character": "interviewer", "parenthetical": "quietly", "text": "Was it worth it?"},
        {"kind": "action", "text": "The filmmaker stands.\nHe leaves the door open."},
        {"kind": "transition", "text": "CUT TO BLACK."}
      ]
    }
  ]
}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        manifest::{
            PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
            ProjectWorkflow,
        },
        revisions::DependencyRecord,
        story::SAMPLE_ANALYSIS_JSON,
    };
    use std::{collections::BTreeMap, fs, path::PathBuf};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "kineto-screenplay-doc-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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

    fn project(target: &std::path::Path) -> CanonicalProject {
        CanonicalProject::create(
            target,
            ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                project_id: "screenplay_doc_test".to_owned(),
                title: "Screenplay Test".to_owned(),
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
            b"A filmmaker hears the last line and leaves.\n",
        )
        .unwrap()
    }

    /// Record `screenplay` as a generated first revision, as a model would.
    fn seed(project: &CanonicalProject, screenplay: &Screenplay) -> RevisionRecord {
        let next = revisions::next(project, DocumentKind::Screenplay).unwrap();
        let bytes = revisions::encode(screenplay).unwrap();
        let content_hash = revisions::write_document(project, &next, &bytes).unwrap();
        let made_from = |artifact_id: &str| DependencyRecord {
            artifact_id: artifact_id.to_owned(),
            content_hash: ContentHash::of(artifact_id.as_bytes()).as_str().to_owned(),
            impact: DependencyImpact::Semantic,
            field: None,
        };
        let record = RevisionRecord {
            revision: next.revision,
            artifact_id: next.artifact_id.as_str().to_owned(),
            path: next.path.as_path().to_string_lossy().into_owned(),
            status: RevisionStatus::Candidate,
            origin: Origin::Generated,
            content_hash: content_hash.as_str().to_owned(),
            input_hash: None,
            provider: Some("fake-llm".to_owned()),
            model: Some("fake".to_owned()),
            provenance: None,
            based_on: None,
            dependencies: vec![made_from("story_analysis_r0001"), made_from("source_r0001")],
        };
        revisions::attach(project, DocumentKind::Screenplay, record.clone()).unwrap();
        record
    }

    /// The sample with a second scene.
    fn two_scenes() -> Screenplay {
        let mut screenplay = parse(&sample()).unwrap();
        let mut second = screenplay.scenes[0].clone();
        second.id = "office_dawn".to_owned();
        second.heading = "INT. SMALL OFFICE - DAWN".to_owned();
        screenplay.scenes.push(second);
        screenplay
    }

    fn sample() -> serde_json::Value {
        serde_json::from_str(SAMPLE_SCREENPLAY_JSON).unwrap()
    }

    fn parse(value: &serde_json::Value) -> Result<Screenplay, String> {
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())
    }

    #[test]
    fn a_consistent_screenplay_round_trips_and_fits_its_analysis() {
        let screenplay = parse(&sample()).unwrap();
        let analysis: StoryAnalysis = serde_json::from_str(SAMPLE_ANALYSIS_JSON).unwrap();
        screenplay.check_against(&analysis).unwrap();
        let encoded = serde_json::to_value(&screenplay).unwrap();
        assert_eq!(parse(&encoded).unwrap(), screenplay);

        let mut stranger = screenplay.clone();
        stranger.cast[0].id = "ghost".to_owned();
        assert!(
            stranger
                .check_against(&analysis)
                .unwrap_err()
                .contains("\"ghost\"")
        );
        let mut elsewhere = screenplay;
        elsewhere.scenes[0].location = "moon".to_owned();
        assert!(
            elsewhere
                .check_against(&analysis)
                .unwrap_err()
                .contains("\"moon\"")
        );
    }

    #[test]
    fn inconsistent_screenplays_are_refused_with_a_reason() {
        type Edit = fn(&mut serde_json::Value);
        let cases: [(Edit, &str); 9] = [
            (
                |v| v["scenes"][0]["blocks"][1]["character"] = "filmmaker2".into(),
                "not among its characters",
            ),
            (
                |v| {
                    v["scenes"][0]["blocks"][1]
                        .as_object_mut()
                        .unwrap()
                        .remove("character");
                },
                "has no character",
            ),
            (
                |v| v["scenes"][0]["blocks"][0]["character"] = "filmmaker".into(),
                "only dialogue",
            ),
            (
                |v| v["scenes"][0]["blocks"][0]["parenthetical"] = "softly".into(),
                "parenthetical",
            ),
            (
                |v| v["scenes"][0]["characters"][0] = "nobody".into(),
                "not in the cast",
            ),
            (|v| v["cast"][1]["id"] = "filmmaker".into(), "appears twice"),
            (
                |v| v["scenes"][0]["blocks"] = serde_json::json!([]),
                "no blocks",
            ),
            (|v| v["scenes"][0]["time"] = " ".into(), "time is empty"),
            (|v| v["scenes"][0]["mood"] = "dark".into(), "unknown field"),
        ];
        for (edit, expected) in cases {
            let mut value = sample();
            edit(&mut value);
            let error = parse(&value).unwrap_err();
            assert!(error.contains(expected), "{expected}: {error}");
        }
    }

    #[test]
    fn markdown_is_deterministic_and_escapes_markup() {
        let screenplay = parse(&sample()).unwrap();
        let markdown = screenplay.render_markdown("screenplay_r0001");
        assert_eq!(
            markdown,
            "<!-- Rendered by Kineto from screenplay_r0001. Edit the screenplay in Kineto; this \
             file is rebuilt from it. -->\n\n\
             # The Last Line\n\n\
             ## 1. INT. SMALL OFFICE - NIGHT\n\n\
             > The last question lands and he cannot stay to answer it.\n\n\
             One table, two chairs. Rain on the window.\n\n\
             **INTERVIEWER**\\\n*(quietly)*\\\nWas it worth it?\n\n\
             The filmmaker stands.\\\nHe leaves the door open.\n\n\
             *CUT TO BLACK.*\n"
        );
        assert_eq!(screenplay.render_markdown("screenplay_r0001"), markdown);

        assert_eq!(escape_line("# not a heading"), "\\# not a heading");
        assert_eq!(escape_line("1. not a list"), "\\1. not a list");
        assert_eq!(escape_line("a *b* <c>"), "a \\*b\\* \\<c\\>");
        assert_eq!(escape_line("1999 was long"), "1999 was long");
    }

    #[test]
    fn a_scene_is_replaced_by_id_and_hashed_by_content() {
        let screenplay = parse(&sample()).unwrap();
        let mut scene = screenplay.scenes[0].clone();
        let before = scene.content_hash();
        scene.blocks[0].text = "Two chairs, one empty.".to_owned();
        assert_ne!(scene.content_hash(), before);
        let next = screenplay.with_scene(scene.clone()).unwrap();
        assert_eq!(next.scenes[0], scene);

        scene.id = "elsewhere".to_owned();
        assert!(screenplay.with_scene(scene).is_err());
        // Layout is not content: reformatted JSON is the same scene.
        let compact: Screenplay = serde_json::from_str(&sample().to_string()).unwrap();
        assert_eq!(compact.scenes[0].content_hash(), before);
    }

    #[test]
    fn edits_form_a_chain_and_never_replace_an_unseen_revision() {
        let temp = TempDir::new();
        let project = project(&temp.0.join("film"));
        let first = seed(&project, &two_scenes());

        let mut edited = two_scenes();
        edited.title = "The Very Last Line".to_owned();
        let second = edit(&project, 1, &edited).unwrap();
        assert_eq!(second.artifact_id, "screenplay_r0002");
        assert_eq!(second.origin, Origin::Edited);
        assert_eq!(second.based_on, Some(1));
        assert_eq!(second.provider, None);
        assert_eq!(second.dependencies, first.dependencies);

        // Revision 2 is newer than what this edit started from.
        assert!(matches!(
            edit(&project, 1, &two_scenes()),
            Err(RevisionError::NotLatest {
                based_on: 1,
                latest: 2
            })
        ));
        // Saving without a change makes no revision.
        assert_eq!(edit(&project, 2, &edited).unwrap(), second);
        let mut broken = edited.clone();
        broken.scenes[0].blocks[1].character = None;
        assert!(matches!(
            edit(&project, 2, &broken),
            Err(RevisionError::InvalidDocument(_))
        ));

        let all = revisions::revisions(&project, DocumentKind::Screenplay).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].status, RevisionStatus::Superseded);
        let kept: Screenplay = revisions::read(&project, &all[0]).unwrap();
        assert_eq!(kept, two_scenes(), "the earlier revision is kept as it was");
    }

    #[test]
    fn the_lock_and_the_revisions_survive_losing_the_cache() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        seed(&project, &two_scenes());
        let mut edited = two_scenes();
        edited.title = "Locked Title".to_owned();
        edit(&project, 1, &edited).unwrap();
        lock(&project, 2).unwrap();
        edited.title = "Working Title".to_owned();
        edit(&project, 2, &edited).unwrap();

        // Production follows the lock while the director works on revision 3.
        let current = revisions::current(&project, DocumentKind::Screenplay)
            .unwrap()
            .unwrap();
        assert_eq!(current.revision, 2);
        let markdown = fs::read_to_string(target.join(MARKDOWN_PATH)).unwrap();
        assert!(markdown.contains("from screenplay_r0002"));
        assert!(markdown.contains("# Locked Title"));
        let log = revisions::revisions(&project, DocumentKind::Screenplay).unwrap();
        drop(project);

        fs::remove_dir_all(target.join(".kineto")).unwrap();
        fs::remove_file(target.join(MARKDOWN_PATH)).unwrap();
        let project = CanonicalProject::open(&target).unwrap();
        assert_eq!(
            revisions::revisions(&project, DocumentKind::Screenplay).unwrap(),
            log
        );
        assert_eq!(
            revisions::latest(&project, DocumentKind::Screenplay)
                .unwrap()
                .unwrap()
                .revision,
            3
        );
        render(&project).unwrap();
        assert_eq!(
            fs::read_to_string(target.join(MARKDOWN_PATH)).unwrap(),
            markdown
        );
    }

    #[test]
    fn scene_staleness_follows_content_not_layout() {
        let temp = TempDir::new();
        let project = project(&temp.0.join("film"));
        let first = two_scenes();
        seed(&project, &first);
        lock(&project, 1).unwrap();
        let night = first.scene("office_night").unwrap().dependency().unwrap();
        let dawn = first.scene("office_dawn").unwrap().dependency().unwrap();
        assert_eq!(night.artifact_id.as_str(), "screenplay_scene.office_night");
        let freshness = |project: &CanonicalProject, dependency: &ArtifactDependency| {
            scene_freshness(
                project,
                dependency.artifact_id.as_str(),
                dependency.content_hash.as_str(),
            )
            .unwrap()
        };

        // Reorder the scenes and touch only whitespace in one; rewrite a line
        // of dialogue in the other.
        let mut second = first.clone();
        second.scenes.reverse();
        let night_scene = second
            .scenes
            .iter_mut()
            .find(|scene| scene.id == "office_night")
            .unwrap();
        night_scene.blocks[2].text =
            "  The filmmaker stands.   \nHe leaves the door open.\t\n\n\n".to_owned();
        let dawn_scene = second
            .scenes
            .iter_mut()
            .find(|scene| scene.id == "office_dawn")
            .unwrap();
        dawn_scene.blocks[1].text = "Was any of it worth it?".to_owned();
        edit(&project, 1, &second).unwrap();
        // A candidate is not production yet.
        assert_eq!(freshness(&project, &dawn), Some(SceneFreshness::Fresh));
        lock(&project, 2).unwrap();
        assert_eq!(freshness(&project, &night), Some(SceneFreshness::Fresh));
        assert_eq!(freshness(&project, &dawn), Some(SceneFreshness::Changed));

        let mut third = second.clone();
        third.scenes.retain(|scene| scene.id != "office_dawn");
        edit(&project, 2, &third).unwrap();
        lock(&project, 3).unwrap();
        assert_eq!(freshness(&project, &night), Some(SceneFreshness::Fresh));
        assert_eq!(freshness(&project, &dawn), Some(SceneFreshness::Removed));
        assert_eq!(
            scene_freshness(
                &project,
                "story_analysis_r0001",
                night.content_hash.as_str()
            )
            .unwrap(),
            None
        );
    }
}
