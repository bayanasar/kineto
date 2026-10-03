//! The director's preference profile: what the director favours and
//! avoids, dimension by dimension, in words a person can read and edit.
//!
//! It is kept as document revisions (`preferences/preferences_rNNNN.json`,
//! `preferences.schema.json`), so every change is a revision the director
//! can look back on. Nothing is locked: the latest revision is in force.
//! Each entry says where it came from: the director's own hand, or a
//! suggestion drawn from the director's choices, with the candidates that
//! showed it.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    ArtifactDependency, ArtifactId, ContentHash, DependencyImpact,
    manifest::CanonicalProject,
    revisions::{self, DocumentKind, Origin, RevisionError, RevisionRecord, RevisionStatus},
};

/// What a preference is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    CameraMotion,
    Lighting,
    Framing,
    Lens,
    Color,
    Composition,
    Performance,
    Pacing,
}

impl Dimension {
    pub const ALL: [Self; 8] = [
        Self::CameraMotion,
        Self::Lighting,
        Self::Framing,
        Self::Lens,
        Self::Color,
        Self::Composition,
        Self::Performance,
        Self::Pacing,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CameraMotion => "camera_motion",
            Self::Lighting => "lighting",
            Self::Framing => "framing",
            Self::Lens => "lens",
            Self::Color => "color",
            Self::Composition => "composition",
            Self::Performance => "performance",
            Self::Pacing => "pacing",
        }
    }

    /// The dimension in words, as a prompt or a person reads it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::CameraMotion => "camera motion",
            Self::Lighting => "lighting",
            Self::Framing => "framing",
            Self::Lens => "lens",
            Self::Color => "colour",
            Self::Composition => "composition",
            Self::Performance => "performance",
            Self::Pacing => "pacing",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|dimension| dimension.as_str() == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stance {
    Prefer,
    Avoid,
}

/// Where a preference came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// The director wrote it.
    Director,
    /// The director accepted it from a suggestion drawn from their choices.
    Suggestion {
        /// Why: what the chosen candidates share that the others lack.
        reason: String,
        /// The candidates that showed it, by artifact ID.
        evidence: Vec<String>,
    },
}

/// One thing the director favours or avoids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preference {
    pub dimension: Dimension,
    pub stance: Stance,
    /// What, in the director's words, such as `slow dolly` or `low-key`.
    pub value: String,
    /// How strongly, in thousandths, when the director says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength_permille: Option<u16>,
    pub source: Source,
}

/// The whole profile, as one revision holds it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub entries: Vec<Preference>,
}

impl Preferences {
    /// Every entry says something, at most once, and a strength is at most
    /// a whole.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = BTreeSet::new();
        for entry in &self.entries {
            let value = entry.value.trim();
            if value.is_empty() {
                return Err(format!(
                    "a {} preference says nothing",
                    entry.dimension.as_str()
                ));
            }
            if !seen.insert((entry.dimension, entry.stance, value.to_lowercase())) {
                return Err(format!(
                    "{} {:?} is said twice",
                    entry.dimension.as_str(),
                    value
                ));
            }
            if entry
                .strength_permille
                .is_some_and(|strength| strength > 1000)
            {
                return Err(format!("{value:?} is stronger than a whole"));
            }
            if let Source::Suggestion { reason, evidence } = &entry.source
                && (reason.trim().is_empty()
                    || evidence.is_empty()
                    || evidence
                        .iter()
                        .any(|id| ArtifactId::new(id.clone()).is_err()))
            {
                return Err(format!(
                    "{value:?} came from a suggestion without its reason or evidence"
                ));
            }
        }
        Ok(())
    }

    /// The entries about `dimensions`, preferred first.
    #[must_use]
    pub fn about(&self, dimensions: &[Dimension]) -> Vec<&Preference> {
        let mut entries: Vec<&Preference> = self
            .entries
            .iter()
            .filter(|entry| dimensions.contains(&entry.dimension))
            .collect();
        entries.sort_by_key(|entry| entry.stance);
        entries
    }
}

/// The profile in force: the latest revision, and what it says.
pub fn current(
    project: &CanonicalProject,
) -> Result<Option<(RevisionRecord, Preferences)>, RevisionError> {
    let Some(record) = revisions::latest(project, DocumentKind::Preferences)? else {
        return Ok(None);
    };
    let preferences: Preferences = revisions::read(project, &record)?;
    Ok(Some((record, preferences)))
}

/// Save `preferences` as the next revision, starting from `based_on`, which
/// must be the latest revision (`None` only for the first). A profile that
/// changes nothing returns the revision it started from.
pub fn save(
    project: &CanonicalProject,
    based_on: Option<u32>,
    preferences: &Preferences,
) -> Result<RevisionRecord, RevisionError> {
    preferences
        .validate()
        .map_err(RevisionError::InvalidDocument)?;
    let bytes = revisions::encode(preferences)?;
    match (
        based_on,
        revisions::latest(project, DocumentKind::Preferences)?,
    ) {
        (Some(based_on), _) => {
            revisions::save_edit(project, DocumentKind::Preferences, based_on, &bytes)
        }
        (None, Some(latest)) => Err(RevisionError::NotLatest {
            based_on: 0,
            latest: latest.revision,
        }),
        (None, None) => {
            let next = revisions::next(project, DocumentKind::Preferences)?;
            let content_hash = revisions::write_document(project, &next, &bytes)?;
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
                based_on: None,
                dependencies: Vec::new(),
            };
            revisions::attach(project, DocumentKind::Preferences, record.clone())?;
            Ok(record)
        }
    }
}

/// The dependency work made with the profile records on it: cosmetic, so
/// changing preferences never makes that work stale; it only shows that
/// the preferences it was made with have since changed.
#[must_use]
pub fn dependency(record: &RevisionRecord) -> Option<ArtifactDependency> {
    Some(ArtifactDependency {
        artifact_id: ArtifactId::new(record.artifact_id.clone()).ok()?,
        content_hash: ContentHash::new(record.content_hash.clone()).ok()?,
        impact: DependencyImpact::Cosmetic,
        field: Some("preferences".to_owned()),
    })
}

/// [`changed_since`] for a document revision's recorded dependencies.
#[must_use]
pub fn revision_changed_since(
    recorded: &[crate::revisions::DependencyRecord],
    current: Option<&RevisionRecord>,
) -> bool {
    let recorded: Vec<ArtifactDependency> = recorded
        .iter()
        .filter(|dependency| dependency.field.as_deref() == Some("preferences"))
        .filter_map(|dependency| {
            Some(ArtifactDependency {
                artifact_id: ArtifactId::new(dependency.artifact_id.clone()).ok()?,
                content_hash: ContentHash::new(dependency.content_hash.clone()).ok()?,
                impact: dependency.impact,
                field: dependency.field.clone(),
            })
        })
        .collect();
    changed_since(&recorded, current)
}

/// Whether work that recorded `recorded` was made with preferences other
/// than those in force now (`current`).
#[must_use]
pub fn changed_since(recorded: &[ArtifactDependency], current: Option<&RevisionRecord>) -> bool {
    let made_with = recorded
        .iter()
        .find(|dependency| dependency.field.as_deref() == Some("preferences"));
    match (made_with, current) {
        (None, None) => false,
        (Some(made_with), Some(current)) => {
            made_with.artifact_id.as_str() != current.artifact_id
                || made_with.content_hash.as_str() != current.content_hash
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource, ProjectWorkflow,
    };
    use std::{
        collections::BTreeMap,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(std::env::temp_dir().join(format!(
                "kineto-preferences-test-{}-{nonce}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn project(target: &std::path::Path) -> CanonicalProject {
        CanonicalProject::create(
            target,
            ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                project_id: "preferences_test".to_owned(),
                title: "Preferences".to_owned(),
                created_at: "2026-10-03T00:00:00Z".to_owned(),
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
                    recipe_version: 2,
                    extra: BTreeMap::new(),
                },
                extra: BTreeMap::new(),
            },
            b"source\n",
        )
        .unwrap()
    }

    fn entry(dimension: Dimension, stance: Stance, value: &str) -> Preference {
        Preference {
            dimension,
            stance,
            value: value.to_owned(),
            strength_permille: None,
            source: Source::Director,
        }
    }

    #[test]
    fn every_change_is_a_revision_and_the_latest_is_in_force() {
        let dir = TempDir::new();
        let project = project(&dir.0);
        assert_eq!(current(&project).unwrap(), None);
        let first = Preferences {
            entries: vec![entry(Dimension::CameraMotion, Stance::Prefer, "slow dolly")],
        };
        let one = save(&project, None, &first).unwrap();
        assert_eq!(one.artifact_id, "preferences_r0001");
        assert!(matches!(
            save(&project, None, &first),
            Err(RevisionError::NotLatest { latest: 1, .. })
        ));
        // Saving the same profile changes nothing.
        assert_eq!(save(&project, Some(1), &first).unwrap(), one);
        let mut second = first.clone();
        second.entries.push(Preference {
            strength_permille: Some(800),
            ..entry(Dimension::CameraMotion, Stance::Avoid, "handheld")
        });
        let two = save(&project, Some(1), &second).unwrap();
        assert_eq!(two.revision, 2);
        assert_eq!(two.based_on, Some(1));
        assert_eq!(
            current(&project).unwrap(),
            Some((two.clone(), second.clone()))
        );
        assert!(matches!(
            save(&project, Some(1), &first),
            Err(RevisionError::NotLatest {
                based_on: 1,
                latest: 2
            })
        ));
        assert_eq!(
            revisions::revisions(&project, DocumentKind::Preferences)
                .unwrap()
                .len(),
            2,
            "the earlier profile is kept"
        );

        // Work made with the first profile shows it changed; with the
        // second, it does not. Neither is stale: the dependency is cosmetic.
        let made_with = dependency(&one).unwrap();
        assert_eq!(made_with.impact, DependencyImpact::Cosmetic);
        assert!(changed_since(&[made_with], Some(&two)));
        assert!(!changed_since(&[dependency(&two).unwrap()], Some(&two)));
        assert!(!changed_since(&[], None));
        assert!(changed_since(&[], Some(&two)));
    }

    #[test]
    fn a_profile_that_says_nothing_twice_or_unexplained_is_refused() {
        let twice = Preferences {
            entries: vec![
                entry(Dimension::Lighting, Stance::Prefer, "low-key"),
                entry(Dimension::Lighting, Stance::Prefer, "Low-key"),
            ],
        };
        assert!(twice.validate().is_err());
        let empty = Preferences {
            entries: vec![entry(Dimension::Lighting, Stance::Prefer, " ")],
        };
        assert!(empty.validate().is_err());
        let too_strong = Preferences {
            entries: vec![Preference {
                strength_permille: Some(1200),
                ..entry(Dimension::Performance, Stance::Prefer, "restrained")
            }],
        };
        assert!(too_strong.validate().is_err());
        let unexplained = Preferences {
            entries: vec![Preference {
                source: Source::Suggestion {
                    reason: "Chosen takes are all still.".to_owned(),
                    evidence: Vec::new(),
                },
                ..entry(Dimension::CameraMotion, Stance::Prefer, "static")
            }],
        };
        assert!(unexplained.validate().is_err());
        let both = Preferences {
            entries: vec![
                entry(Dimension::Lighting, Stance::Avoid, "flat"),
                entry(Dimension::Lighting, Stance::Prefer, "low-key"),
                entry(Dimension::Pacing, Stance::Prefer, "unhurried"),
            ],
        };
        both.validate().unwrap();
        let lighting: Vec<_> = both
            .about(&[Dimension::Lighting])
            .iter()
            .map(|entry| entry.value.as_str())
            .collect();
        assert_eq!(lighting, ["low-key", "flat"], "preferred first");
        for dimension in Dimension::ALL {
            assert_eq!(Dimension::parse(dimension.as_str()), Some(dimension));
        }
    }
}
