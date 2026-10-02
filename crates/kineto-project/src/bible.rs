//! The world and character bible: the production memory later visual work
//! follows, as structured state.
//!
//! `schemas/bible.schema.json` is the contract, and these types enforce it
//! on the way in, as the story analysis does. A bible is written from the
//! locked screenplay: [`Bible::check_against`] holds a model's answer to
//! that screenplay's cast and scenes and to the locations of the story
//! analysis it was adapted from, and [`staleness`] says what has changed
//! since.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    ArtifactDependency, ArtifactId, ContentHash, DependencyImpact,
    manifest::CanonicalProject,
    revisions::{self, DocumentKind, RevisionError, RevisionRecord, RevisionStatus},
    screenplay::{SCENE_ARTIFACT_PREFIX, SceneFreshness, Screenplay},
    story::{StoryAnalysis, is_portable_id},
};

/// The committed schema, as the model is shown it.
pub const BIBLE_SCHEMA: &str = include_str!("../../../schemas/bible.schema.json");

/// Entry IDs leave room for the prefix that names an entry as an artifact,
/// as in `bible_character.<id>`.
const MAX_ENTRY_ID: usize = 112;

/// Scene IDs as the screenplay allows them.
const MAX_SCENE_ID: usize = 64;

/// The artifact ID prefix a character's entry is depended on by, as in
/// `bible_character.mira`.
pub const CHARACTER_ARTIFACT_PREFIX: &str = "bible_character.";

/// The artifact ID the world's entry is depended on by.
pub const WORLD_ARTIFACT_ID: &str = "bible_world";

/// A dependency on one entry: it stays fresh across bible revisions for as
/// long as the entry says the same.
fn entry_dependency<T: Serialize>(artifact_id: &str, entry: &T) -> ArtifactDependency {
    ArtifactDependency {
        artifact_id: ArtifactId::new(artifact_id).expect("entry IDs leave room for the prefix"),
        content_hash: ContentHash::of(&serde_json::to_vec(entry).expect("an entry always encodes")),
        impact: DependencyImpact::Semantic,
        field: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct World {
    pub setting: String,
    pub period: String,
    pub visual_rules: Vec<String>,
    pub continuity_rules: Vec<String>,
}

/// What a character wears in some scenes instead of their usual wardrobe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WardrobeChange {
    pub scenes: Vec<String>,
    pub wardrobe: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Character {
    pub id: String,
    pub name: String,
    pub description: String,
    pub appearance: String,
    pub wardrobe: String,
    pub wardrobe_changes: Vec<WardrobeChange>,
    pub forbidden_changes: Vec<String>,
    pub continuity_notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub id: String,
    pub name: String,
    pub description: String,
    pub appearance: String,
    pub forbidden_changes: Vec<String>,
    pub continuity_notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prop {
    pub id: String,
    pub name: String,
    pub description: String,
    pub appearance: String,
    pub characters: Vec<String>,
    pub scenes: Vec<String>,
    pub forbidden_changes: Vec<String>,
    pub continuity_notes: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBible {
    world: World,
    characters: Vec<Character>,
    locations: Vec<Location>,
    props: Vec<Prop>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawBible")]
pub struct Bible {
    pub world: World,
    pub characters: Vec<Character>,
    pub locations: Vec<Location>,
    pub props: Vec<Prop>,
}

fn filled(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} is empty"))
    } else {
        Ok(())
    }
}

fn every(values: &[String], field: &str) -> Result<(), String> {
    values.iter().try_for_each(|value| filled(value, field))
}

fn entry_ids<'a>(
    list: impl Iterator<Item = &'a str>,
    what: &str,
) -> Result<BTreeSet<&'a str>, String> {
    let mut seen = BTreeSet::new();
    for id in list {
        if !is_portable_id(id) || id.len() > MAX_ENTRY_ID {
            return Err(format!(
                "{what} id {id:?} is not a portable identifier of at most {MAX_ENTRY_ID} characters"
            ));
        }
        if !seen.insert(id) {
            return Err(format!("{what} id {id:?} appears twice"));
        }
    }
    Ok(seen)
}

fn scene_ids<'a>(scenes: &'a [String], owner: &str) -> Result<BTreeSet<&'a str>, String> {
    let mut seen = BTreeSet::new();
    for scene in scenes {
        if !is_portable_id(scene) || scene.len() > MAX_SCENE_ID {
            return Err(format!(
                "{owner} names scene {scene:?}, which is not a scene id"
            ));
        }
        if !seen.insert(scene.as_str()) {
            return Err(format!("{owner} names scene {scene:?} twice"));
        }
    }
    Ok(seen)
}

impl Bible {
    fn validate(&self) -> Result<(), String> {
        filled(&self.world.setting, "world setting")?;
        filled(&self.world.period, "world period")?;
        every(&self.world.visual_rules, "a world visual rule")?;
        every(&self.world.continuity_rules, "a world continuity rule")?;
        if self.characters.is_empty() {
            return Err("there are no characters".to_owned());
        }
        let characters = entry_ids(self.characters.iter().map(|c| c.id.as_str()), "character")?;
        entry_ids(self.locations.iter().map(|l| l.id.as_str()), "location")?;
        entry_ids(self.props.iter().map(|p| p.id.as_str()), "prop")?;
        for character in &self.characters {
            let owner = format!("character {:?}", character.id);
            filled(&character.name, &format!("{owner} name"))?;
            filled(&character.description, &format!("{owner} description"))?;
            filled(&character.appearance, &format!("{owner} appearance"))?;
            filled(&character.wardrobe, &format!("{owner} wardrobe"))?;
            let mut dressed = BTreeSet::new();
            for change in &character.wardrobe_changes {
                let what = format!("a wardrobe change of {owner}");
                filled(&change.wardrobe, &what)?;
                if change.scenes.is_empty() {
                    return Err(format!("{what} names no scene"));
                }
                for scene in scene_ids(&change.scenes, &what)? {
                    if !dressed.insert(scene) {
                        return Err(format!(
                            "{owner} has two wardrobe changes in scene {scene:?}"
                        ));
                    }
                }
            }
            every(
                &character.forbidden_changes,
                &format!("a forbidden change of {owner}"),
            )?;
            every(
                &character.continuity_notes,
                &format!("a continuity note of {owner}"),
            )?;
        }
        for location in &self.locations {
            let owner = format!("location {:?}", location.id);
            filled(&location.name, &format!("{owner} name"))?;
            filled(&location.description, &format!("{owner} description"))?;
            filled(&location.appearance, &format!("{owner} appearance"))?;
            every(
                &location.forbidden_changes,
                &format!("a forbidden change of {owner}"),
            )?;
            every(
                &location.continuity_notes,
                &format!("a continuity note of {owner}"),
            )?;
        }
        for prop in &self.props {
            let owner = format!("prop {:?}", prop.id);
            filled(&prop.name, &format!("{owner} name"))?;
            filled(&prop.description, &format!("{owner} description"))?;
            filled(&prop.appearance, &format!("{owner} appearance"))?;
            let mut handlers = BTreeSet::new();
            for character in &prop.characters {
                if !characters.contains(character.as_str()) {
                    return Err(format!(
                        "{owner} names character {character:?}, who has no entry"
                    ));
                }
                if !handlers.insert(character.as_str()) {
                    return Err(format!("{owner} names character {character:?} twice"));
                }
            }
            scene_ids(&prop.scenes, &owner)?;
            every(
                &prop.forbidden_changes,
                &format!("a forbidden change of {owner}"),
            )?;
            every(
                &prop.continuity_notes,
                &format!("a continuity note of {owner}"),
            )?;
        }
        Ok(())
    }

    /// Check that the bible describes what `screenplay` needs and nothing
    /// it lacks: an entry for every cast member who appears in a scene and
    /// for every location a scene is set in, characters only from the cast,
    /// and locations only from `analysis`, the story analysis the screenplay
    /// was adapted from, since a place can be seen without a scene set in
    /// it.
    pub fn check_against(
        &self,
        screenplay: &Screenplay,
        analysis: &StoryAnalysis,
    ) -> Result<(), String> {
        let scenes: BTreeSet<&str> = screenplay.scenes.iter().map(|s| s.id.as_str()).collect();
        let appearing: BTreeSet<&str> = screenplay
            .scenes
            .iter()
            .flat_map(|scene| scene.characters.iter().map(String::as_str))
            .collect();
        let settings: BTreeSet<&str> = screenplay
            .scenes
            .iter()
            .map(|scene| scene.location.as_str())
            .collect();
        for character in &self.characters {
            if !screenplay
                .cast
                .iter()
                .any(|member| member.id == character.id)
            {
                return Err(format!(
                    "character {:?} is not in the screenplay's cast",
                    character.id
                ));
            }
        }
        for id in appearing {
            if !self.characters.iter().any(|character| character.id == id) {
                return Err(format!(
                    "cast member {id:?} appears in a scene but has no character entry"
                ));
            }
        }
        for location in &self.locations {
            if !analysis
                .locations
                .iter()
                .any(|known| known.id == location.id)
            {
                return Err(format!(
                    "location {:?} is not a location of the story analysis",
                    location.id
                ));
            }
        }
        for id in settings {
            if !self.locations.iter().any(|location| location.id == id) {
                return Err(format!(
                    "scenes are set in {id:?}, which has no location entry"
                ));
            }
        }
        let named = self
            .characters
            .iter()
            .flat_map(|character| {
                character.wardrobe_changes.iter().flat_map(move |change| {
                    change
                        .scenes
                        .iter()
                        .map(move |scene| (format!("character {:?}", character.id), scene))
                })
            })
            .chain(self.props.iter().flat_map(|prop| {
                prop.scenes
                    .iter()
                    .map(move |scene| (format!("prop {:?}", prop.id), scene))
            }));
        for (owner, scene) in named {
            if !scenes.contains(scene.as_str()) {
                return Err(format!(
                    "{owner} names scene {scene:?}, which is not in the screenplay"
                ));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn character(&self, id: &str) -> Option<&Character> {
        self.characters.iter().find(|character| character.id == id)
    }

    #[must_use]
    pub fn location(&self, id: &str) -> Option<&Location> {
        self.locations.iter().find(|location| location.id == id)
    }

    /// The dependency work on character `id` records on its entry.
    #[must_use]
    pub fn character_dependency(&self, id: &str) -> Option<ArtifactDependency> {
        self.character(id).map(|character| {
            entry_dependency(
                &format!("{CHARACTER_ARTIFACT_PREFIX}{}", character.id),
                character,
            )
        })
    }

    /// The dependency work that follows the world's rules records on them.
    #[must_use]
    pub fn world_dependency(&self) -> ArtifactDependency {
        entry_dependency(WORLD_ARTIFACT_ID, &self.world)
    }
}

impl TryFrom<RawBible> for Bible {
    type Error = String;

    fn try_from(raw: RawBible) -> Result<Self, String> {
        let bible = Self {
            world: raw.world,
            characters: raw.characters,
            locations: raw.locations,
            props: raw.props,
        };
        bible.validate()?;
        Ok(bible)
    }
}

/// The locked screenplay and its revision, or `None` when none is locked.
pub fn locked_screenplay(
    project: &CanonicalProject,
) -> Result<Option<(RevisionRecord, Screenplay)>, RevisionError> {
    let Some(record) = revisions::revisions(project, DocumentKind::Screenplay)?
        .into_iter()
        .find(|record| record.status == RevisionStatus::Locked)
    else {
        return Ok(None);
    };
    let screenplay = revisions::read(project, &record)?;
    Ok(Some((record, screenplay)))
}

/// What a bible describes and may name: the locked screenplay, with its
/// revision, and the story analysis it was adapted from, with its revision;
/// `None` when no screenplay is locked.
pub fn material(
    project: &CanonicalProject,
) -> Result<Option<(RevisionRecord, Screenplay, RevisionRecord, StoryAnalysis)>, RevisionError> {
    let Some((locked, screenplay)) = locked_screenplay(project)? else {
        return Ok(None);
    };
    let analysis_prefix = format!("{}_r", DocumentKind::StoryAnalysis.id_prefix());
    let missing = || {
        RevisionError::InvalidDocument(
            "the story analysis the locked screenplay was written from is missing".to_owned(),
        )
    };
    let written_from = locked
        .dependencies
        .iter()
        .find(|dependency| dependency.artifact_id.starts_with(&analysis_prefix))
        .ok_or_else(missing)?;
    let analysis_record = revisions::revisions(project, DocumentKind::StoryAnalysis)?
        .into_iter()
        .find(|record| {
            record.artifact_id == written_from.artifact_id
                && record.content_hash == written_from.content_hash
        })
        .ok_or_else(missing)?;
    let analysis = revisions::read(project, &analysis_record)?;
    Ok(Some((locked, screenplay, analysis_record, analysis)))
}

/// Check `bible` against the locked screenplay and its analysis, as a
/// model's bible is before it is kept.
fn check_references(project: &CanonicalProject, bible: &Bible) -> Result<(), RevisionError> {
    let (_, screenplay, _, analysis) = material(project)?
        .ok_or_else(|| RevisionError::InvalidDocument("no screenplay is locked".to_owned()))?;
    bible
        .check_against(&screenplay, &analysis)
        .map_err(RevisionError::InvalidDocument)
}

/// What a bible written from the screenplay `locked` depends on: the
/// material that screenplay was made from (its story analysis and source),
/// each of its scenes, and the revision itself, which is cosmetic: it names
/// where the scenes were read, while the scenes decide freshness.
pub fn dependencies(
    locked: &RevisionRecord,
    screenplay: &Screenplay,
) -> Result<Vec<ArtifactDependency>, RevisionError> {
    let own_revisions = format!("{}_r", DocumentKind::Screenplay.id_prefix());
    let mut dependencies = Vec::new();
    for material in locked.dependencies.iter().filter(|dependency| {
        dependency.impact == DependencyImpact::Semantic
            && !dependency.artifact_id.starts_with(&own_revisions)
            && !dependency.artifact_id.starts_with(SCENE_ARTIFACT_PREFIX)
    }) {
        dependencies.push(ArtifactDependency {
            artifact_id: ArtifactId::new(material.artifact_id.clone())?,
            content_hash: ContentHash::new(material.content_hash.clone())?,
            impact: DependencyImpact::Semantic,
            field: None,
        });
    }
    dependencies.push(revisions::dependency_on(
        locked,
        DependencyImpact::Cosmetic,
    )?);
    for scene in &screenplay.scenes {
        dependencies.push(scene.dependency()?);
    }
    Ok(dependencies)
}

/// Why a bible no longer describes the screenplay production follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleCause {
    /// No screenplay is locked.
    NoLockedScreenplay,
    /// The locked screenplay is not made from this story analysis or source
    /// revision, which the bible read.
    Material(String),
    SceneChanged(String),
    SceneRemoved(String),
    /// The locked screenplay has a scene the bible never read.
    SceneAdded(String),
}

/// What makes the bible revision `record` stale against the locked
/// screenplay; empty when it is fresh.
pub fn staleness(
    project: &CanonicalProject,
    record: &RevisionRecord,
) -> Result<Vec<StaleCause>, RevisionError> {
    let Some((locked, screenplay)) = locked_screenplay(project)? else {
        return Ok(vec![StaleCause::NoLockedScreenplay]);
    };
    let material = dependencies(&locked, &screenplay)?;
    let mut causes = Vec::new();
    let mut read = BTreeSet::new();
    for dependency in record
        .dependencies
        .iter()
        .filter(|dependency| dependency.impact == DependencyImpact::Semantic)
    {
        if let Some(scene) = dependency.artifact_id.strip_prefix(SCENE_ARTIFACT_PREFIX) {
            read.insert(scene);
            match screenplay.freshness(&dependency.artifact_id, &dependency.content_hash) {
                Some(SceneFreshness::Changed) => {
                    causes.push(StaleCause::SceneChanged(scene.to_owned()));
                }
                Some(SceneFreshness::Removed) => {
                    causes.push(StaleCause::SceneRemoved(scene.to_owned()));
                }
                Some(SceneFreshness::Fresh) | None => {}
            }
        } else if !material.iter().any(|current| {
            current.artifact_id.as_str() == dependency.artifact_id
                && current.content_hash.as_str() == dependency.content_hash
        }) {
            causes.push(StaleCause::Material(dependency.artifact_id.clone()));
        }
    }
    for scene in &screenplay.scenes {
        if !read.contains(scene.id.as_str()) {
            causes.push(StaleCause::SceneAdded(scene.id.clone()));
        }
    }
    Ok(causes)
}

/// Save the director's edit of revision `based_on` as the next revision.
/// `based_on` must be the latest revision, so an edit never silently
/// replaces one the director has not seen. An edit that changes nothing
/// returns the revision it started from. The edit is made from what the
/// edited revision was made from, and is held to the locked screenplay and
/// its analysis as a model's bible is.
pub fn edit(
    project: &CanonicalProject,
    based_on: u32,
    bible: &Bible,
) -> Result<RevisionRecord, RevisionError> {
    bible.validate().map_err(RevisionError::InvalidDocument)?;
    check_references(project, bible)?;
    revisions::save_edit(
        project,
        DocumentKind::Bible,
        based_on,
        &revisions::encode(bible)?,
    )
}

/// Lock `revision` as the bible production follows. It must name only what
/// the locked screenplay and its analysis define. A stale bible that does
/// can be locked: the director decides whether it still holds, and
/// [`staleness`] keeps saying why it may not.
pub fn lock(project: &CanonicalProject, revision: u32) -> Result<(), RevisionError> {
    let record = revisions::revisions(project, DocumentKind::Bible)?
        .into_iter()
        .find(|record| record.revision == revision)
        .ok_or(RevisionError::NoSuchRevision(revision))?;
    let bible: Bible = revisions::read(project, &record)?;
    check_references(project, &bible)?;
    revisions::lock(project, DocumentKind::Bible, revision)
}

/// A small, valid bible of [`crate::screenplay::SAMPLE_SCREENPLAY_JSON`],
/// for tests across the workspace.
#[doc(hidden)]
pub const SAMPLE_BIBLE_JSON: &str = r#"{
  "world": {
    "setting": "A city at night in the rain, seen through small interiors.",
    "period": "The present day; phones exist but nobody looks at one.",
    "visual_rules": ["Practical light only: desk lamps, street light through windows."],
    "continuity_rules": ["It rains through the whole film."]
  },
  "characters": [
    {
      "id": "filmmaker",
      "name": "The Filmmaker",
      "description": "A director at the end of a long career, guarded and tired.",
      "appearance": "A man in his late forties, lean, unshaven, grey at the temples, deep-set eyes.",
      "wardrobe": "A dark wool coat over a black sweater.",
      "wardrobe_changes": [],
      "forbidden_changes": ["Grey at the temples.", "Never without the coat."],
      "continuity_notes": ["His coat is wet from the rain."]
    },
    {
      "id": "interviewer",
      "name": "The Interviewer",
      "description": "A patient journalist who has saved her hardest question for last.",
      "appearance": "A woman in her thirties, short black hair, round glasses.",
      "wardrobe": "A grey blazer, a notebook always in hand.",
      "wardrobe_changes": [],
      "forbidden_changes": ["Round glasses."],
      "continuity_notes": []
    }
  ],
  "locations": [
    {
      "id": "small_office",
      "name": "A small office",
      "description": "Where the interview takes place.",
      "appearance": "One table, two chairs, a rain-streaked window, a single desk lamp.",
      "forbidden_changes": ["The window is behind the interviewer."],
      "continuity_notes": []
    }
  ],
  "props": [
    {
      "id": "notebook",
      "name": "The notebook",
      "description": "Where the interviewer keeps her questions.",
      "appearance": "A small black notebook with a red elastic band.",
      "characters": ["interviewer"],
      "scenes": ["office_night"],
      "forbidden_changes": [],
      "continuity_notes": []
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
        revisions::{DependencyRecord, Origin},
        screenplay::{self, SAMPLE_SCREENPLAY_JSON},
        story::SAMPLE_ANALYSIS_JSON,
    };
    use std::{collections::BTreeMap, fs, path::PathBuf};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "kineto-bible-doc-test-{}-{}",
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
                project_id: "bible_doc_test".to_owned(),
                title: "Bible Test".to_owned(),
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

    fn made_from(artifact_id: &str) -> DependencyRecord {
        DependencyRecord {
            artifact_id: artifact_id.to_owned(),
            content_hash: ContentHash::of(artifact_id.as_bytes()).as_str().to_owned(),
            impact: DependencyImpact::Semantic,
            field: None,
        }
    }

    /// Record `bytes` as the next revision of `kind`, made from
    /// `dependencies`.
    fn seed(
        project: &CanonicalProject,
        kind: DocumentKind,
        bytes: &[u8],
        dependencies: Vec<DependencyRecord>,
    ) -> RevisionRecord {
        let next = revisions::next(project, kind).unwrap();
        let content_hash = revisions::write_document(project, &next, bytes).unwrap();
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
            dependencies,
        };
        revisions::attach(project, kind, record.clone()).unwrap();
        record
    }

    /// Record the sample analysis, as the screenplay's material.
    fn analysed(project: &CanonicalProject) -> DependencyRecord {
        let record = seed(
            project,
            DocumentKind::StoryAnalysis,
            &revisions::encode(&analysis_sample()).unwrap(),
            vec![made_from("source_r0001")],
        );
        DependencyRecord::from(
            &revisions::dependency_on(&record, DependencyImpact::Semantic).unwrap(),
        )
    }

    fn analysis_sample() -> StoryAnalysis {
        serde_json::from_str(SAMPLE_ANALYSIS_JSON).unwrap()
    }

    fn screenplay_sample() -> Screenplay {
        serde_json::from_str(SAMPLE_SCREENPLAY_JSON).unwrap()
    }

    fn sample() -> serde_json::Value {
        serde_json::from_str(SAMPLE_BIBLE_JSON).unwrap()
    }

    fn parse(value: &serde_json::Value) -> Result<Bible, String> {
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())
    }

    /// A project with the sample screenplay locked and a bible written from
    /// it.
    fn with_bible(target: &std::path::Path) -> (CanonicalProject, RevisionRecord) {
        let project = project(target);
        let analysis = analysed(&project);
        seed(
            &project,
            DocumentKind::Screenplay,
            &revisions::encode(&screenplay_sample()).unwrap(),
            vec![analysis, made_from("source_r0001")],
        );
        screenplay::lock(&project, 1).unwrap();
        let (locked, screenplay) = locked_screenplay(&project).unwrap().unwrap();
        let bible = seed(
            &project,
            DocumentKind::Bible,
            &revisions::encode(&parse(&sample()).unwrap()).unwrap(),
            dependencies(&locked, &screenplay)
                .unwrap()
                .iter()
                .map(DependencyRecord::from)
                .collect(),
        );
        (project, bible)
    }

    #[test]
    fn an_entry_is_depended_on_by_what_it_says() {
        let bible = parse(&sample()).unwrap();
        let filmmaker = bible.character_dependency("filmmaker").unwrap();
        assert_eq!(filmmaker.artifact_id.as_str(), "bible_character.filmmaker");
        assert!(bible.character_dependency("waiter").is_none());
        let mut edited = bible.clone();
        edited.characters[1].wardrobe = "A raincoat.".to_owned();
        assert_eq!(
            edited.character_dependency("filmmaker"),
            Some(filmmaker),
            "another character's edit leaves this one fresh"
        );
        assert_ne!(
            edited.character_dependency("interviewer"),
            bible.character_dependency("interviewer")
        );
        assert_eq!(bible.world_dependency().artifact_id.as_str(), "bible_world");
        edited.world.continuity_rules.clear();
        assert_ne!(edited.world_dependency(), bible.world_dependency());
    }

    #[test]
    fn a_consistent_bible_round_trips_and_fits_its_screenplay() {
        let bible = parse(&sample()).unwrap();
        bible
            .check_against(&screenplay_sample(), &analysis_sample())
            .unwrap();
        assert_eq!(
            bible.character("filmmaker").unwrap().wardrobe,
            "A dark wool coat over a black sweater."
        );
        assert_eq!(
            bible.location("small_office").unwrap().name,
            "A small office"
        );
        let encoded = serde_json::to_value(&bible).unwrap();
        assert_eq!(parse(&encoded).unwrap(), bible);
    }

    #[test]
    fn inconsistent_bibles_are_refused_with_a_reason() {
        type Edit = fn(&mut serde_json::Value);
        let cases: [(Edit, &str); 10] = [
            (|v| v["characters"] = serde_json::json!([]), "no characters"),
            (
                |v| v["characters"][1]["id"] = "filmmaker".into(),
                "appears twice",
            ),
            (
                |v| v["characters"][0]["id"] = "c".repeat(113).into(),
                "at most 112",
            ),
            (
                |v| v["characters"][0]["appearance"] = " ".into(),
                "appearance is empty",
            ),
            (
                |v| v["props"][0]["characters"][0] = "ghost".into(),
                "\"ghost\", who has no entry",
            ),
            (
                |v| v["props"][0]["scenes"] = serde_json::json!(["office_night", "office_night"]),
                "twice",
            ),
            (
                |v| {
                    v["characters"][0]["wardrobe_changes"] = serde_json::json!([
                        {"scenes": ["office_night"], "wardrobe": "Shirtsleeves."},
                        {"scenes": ["office_night"], "wardrobe": "A raincoat."}
                    ]);
                },
                "two wardrobe changes",
            ),
            (
                |v| {
                    v["characters"][0]["wardrobe_changes"] =
                        serde_json::json!([{"scenes": [], "wardrobe": "Shirtsleeves."}]);
                },
                "names no scene",
            ),
            (
                |v| v["world"]["period"] = "".into(),
                "world period is empty",
            ),
            (|v| v["characters"][0]["age"] = 40.into(), "unknown field"),
        ];
        for (edit, expected) in cases {
            let mut value = sample();
            edit(&mut value);
            let error = parse(&value).unwrap_err();
            assert!(error.contains(expected), "{error:?} lacks {expected:?}");
        }
    }

    #[test]
    fn a_bible_describes_exactly_what_the_screenplay_needs() {
        type Edit = fn(&mut serde_json::Value);
        let cases: [(Edit, &str); 5] = [
            (
                |v| {
                    v["characters"][1]["id"] = "waiter".into();
                    v["props"][0]["characters"][0] = "waiter".into();
                },
                "\"waiter\" is not in the screenplay's cast",
            ),
            (
                |v| {
                    v["characters"].as_array_mut().unwrap().pop();
                    v["props"][0]["characters"] = serde_json::json!([]);
                },
                "cast member \"interviewer\" appears in a scene but has no character entry",
            ),
            (
                |v| {
                    let mut rooftop = v["locations"][0].clone();
                    rooftop["id"] = "rooftop".into();
                    v["locations"].as_array_mut().unwrap().push(rooftop);
                },
                "location \"rooftop\" is not a location of the story analysis",
            ),
            (
                |v| v["locations"] = serde_json::json!([]),
                "set in \"small_office\", which has no location entry",
            ),
            (
                |v| v["props"][0]["scenes"][0] = "office_dawn".into(),
                "prop \"notebook\" names scene \"office_dawn\", which is not in the screenplay",
            ),
        ];
        for (edit, expected) in cases {
            let mut value = sample();
            edit(&mut value);
            let error = parse(&value)
                .unwrap()
                .check_against(&screenplay_sample(), &analysis_sample())
                .unwrap_err();
            assert!(error.contains(expected), "{error:?} lacks {expected:?}");
        }
    }

    #[test]
    fn a_place_of_the_story_no_scene_is_set_in_can_be_described() {
        let mut analysis = serde_json::to_value(analysis_sample()).unwrap();
        analysis["locations"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "id": "rain_street",
                "name": "The street below",
                "description": "Seen through the office window."
            }));
        let analysis: StoryAnalysis = serde_json::from_value(analysis).unwrap();
        let mut value = sample();
        let mut street = value["locations"][0].clone();
        street["id"] = "rain_street".into();
        value["locations"].as_array_mut().unwrap().push(street);
        parse(&value)
            .unwrap()
            .check_against(&screenplay_sample(), &analysis)
            .unwrap();
    }

    #[test]
    fn edits_form_a_chain_and_the_lock_survives_losing_the_cache() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let (project, first) = with_bible(&target);
        assert_eq!(first.path, "bible/bible_r0001.json");

        let mut edited = parse(&sample()).unwrap();
        edited.characters[0].appearance = "A man of fifty, lean, clean-shaven.".to_owned();
        let second = edit(&project, 1, &edited).unwrap();
        assert_eq!(second.artifact_id, "bible_r0002");
        assert_eq!(second.origin, Origin::Edited);
        assert_eq!(second.based_on, Some(1));
        assert_eq!(second.dependencies, first.dependencies);
        assert!(matches!(
            edit(&project, 1, &parse(&sample()).unwrap()),
            Err(RevisionError::NotLatest {
                based_on: 1,
                latest: 2
            })
        ));
        assert_eq!(
            edit(&project, 2, &edited).unwrap(),
            second,
            "nothing to save"
        );
        let mut broken = edited.clone();
        broken.props[0].characters = vec!["ghost".to_owned()];
        assert!(matches!(
            edit(&project, 2, &broken),
            Err(RevisionError::InvalidDocument(_))
        ));

        lock(&project, 2).unwrap();
        let log = revisions::revisions(&project, DocumentKind::Bible).unwrap();
        assert_eq!(
            log.iter().map(|record| record.status).collect::<Vec<_>>(),
            [RevisionStatus::Superseded, RevisionStatus::Locked]
        );
        drop(project);

        fs::remove_dir_all(target.join(".kineto")).unwrap();
        let project = CanonicalProject::open(&target).unwrap();
        assert_eq!(
            revisions::revisions(&project, DocumentKind::Bible).unwrap(),
            log
        );
        let locked: Bible = revisions::read(
            &project,
            &revisions::current(&project, DocumentKind::Bible)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(locked, edited);
    }

    #[test]
    fn an_edit_or_a_lock_names_only_what_the_screenplay_and_its_analysis_define() {
        let temp = TempDir::new();
        let (project, _) = with_bible(&temp.0.join("film"));
        type Edit = fn(&mut Bible);
        let cases: [(Edit, &str); 3] = [
            (
                |bible| {
                    bible.characters[1].id = "waiter".to_owned();
                    bible.props[0].characters = vec!["waiter".to_owned()];
                },
                "character \"waiter\" is not in the screenplay's cast",
            ),
            (
                |bible| {
                    let mut rooftop = bible.locations[0].clone();
                    rooftop.id = "rooftop".to_owned();
                    bible.locations.push(rooftop);
                },
                "location \"rooftop\" is not a location of the story analysis",
            ),
            (
                |bible| bible.props[0].scenes = vec!["office_dawn".to_owned()],
                "names scene \"office_dawn\", which is not in the screenplay",
            ),
        ];
        for (change, expected) in cases {
            let mut edited = parse(&sample()).unwrap();
            change(&mut edited);
            let Err(RevisionError::InvalidDocument(reason)) = edit(&project, 1, &edited) else {
                panic!("an edit naming what the screenplay lacks was saved");
            };
            assert!(reason.contains(expected), "{reason:?} lacks {expected:?}");
        }
        assert_eq!(
            revisions::revisions(&project, DocumentKind::Bible)
                .unwrap()
                .len(),
            1,
            "no refused edit became a revision"
        );

        // A revision that got past the edit path anyway is not locked.
        let mut stranger = parse(&sample()).unwrap();
        stranger.props[0].scenes = vec!["office_dawn".to_owned()];
        let record = seed(
            &project,
            DocumentKind::Bible,
            &revisions::encode(&stranger).unwrap(),
            Vec::new(),
        );
        assert!(matches!(
            lock(&project, record.revision),
            Err(RevisionError::InvalidDocument(_))
        ));
        assert!(
            revisions::revisions(&project, DocumentKind::Bible)
                .unwrap()
                .iter()
                .all(|record| record.status != RevisionStatus::Locked)
        );
    }

    #[test]
    fn staleness_names_what_changed_in_the_locked_screenplay() {
        let temp = TempDir::new();
        let (project, bible) = with_bible(&temp.0.join("film"));
        assert_eq!(staleness(&project, &bible).unwrap(), []);
        let depends_on: Vec<_> = bible
            .dependencies
            .iter()
            .map(|dependency| (dependency.artifact_id.as_str(), dependency.impact))
            .collect();
        assert_eq!(
            depends_on,
            [
                ("story_analysis_r0001", DependencyImpact::Semantic),
                ("source_r0001", DependencyImpact::Semantic),
                ("screenplay_r0001", DependencyImpact::Cosmetic),
                ("screenplay_scene.office_night", DependencyImpact::Semantic),
            ]
        );

        // Only whitespace changes in the newly locked screenplay: still fresh.
        let mut layout = screenplay_sample();
        layout.scenes[0].blocks[0].text =
            format!("  {}   \n\n", screenplay_sample().scenes[0].blocks[0].text);
        screenplay::edit(&project, 1, &layout).unwrap();
        screenplay::lock(&project, 2).unwrap();
        assert_eq!(staleness(&project, &bible).unwrap(), []);

        // A changed scene, a new scene, and other material.
        let mut changed = layout.clone();
        changed.scenes[0].blocks[1].text = "Was any of it worth it?".to_owned();
        let mut dawn = changed.scenes[0].clone();
        dawn.id = "office_dawn".to_owned();
        changed.scenes.push(dawn);
        screenplay::edit(&project, 2, &changed).unwrap();
        screenplay::lock(&project, 3).unwrap();
        assert_eq!(
            staleness(&project, &bible).unwrap(),
            [
                StaleCause::SceneChanged("office_night".to_owned()),
                StaleCause::SceneAdded("office_dawn".to_owned()),
            ]
        );

        seed(
            &project,
            DocumentKind::Screenplay,
            &revisions::encode(&screenplay_sample()).unwrap(),
            vec![made_from("story_analysis_r0002"), made_from("source_r0001")],
        );
        screenplay::lock(&project, 4).unwrap();
        assert_eq!(
            staleness(&project, &bible).unwrap(),
            [StaleCause::Material("story_analysis_r0001".to_owned())]
        );
        let mut removed = screenplay_sample();
        removed.scenes[0].id = "office_dawn".to_owned();
        screenplay::edit(&project, 4, &removed).unwrap();
        screenplay::lock(&project, 5).unwrap();
        assert_eq!(
            staleness(&project, &bible).unwrap(),
            [
                StaleCause::Material("story_analysis_r0001".to_owned()),
                StaleCause::SceneRemoved("office_night".to_owned()),
                StaleCause::SceneAdded("office_dawn".to_owned()),
            ]
        );
    }

    #[test]
    fn without_a_locked_screenplay_a_bible_is_stale() {
        let temp = TempDir::new();
        let project = project(&temp.0.join("film"));
        let record = seed(
            &project,
            DocumentKind::Bible,
            &revisions::encode(&parse(&sample()).unwrap()).unwrap(),
            Vec::new(),
        );
        assert_eq!(
            staleness(&project, &record).unwrap(),
            [StaleCause::NoLockedScreenplay]
        );
        assert!(locked_screenplay(&project).unwrap().is_none());
    }
}
