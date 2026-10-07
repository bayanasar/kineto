//! The director's preferences as later proposals read them.
//!
//! Shot planning, keyframes and videos each read the dimensions of the
//! profile that apply to them, worded as one sentence the compiler adds as
//! its lowest-ranked part. It is the first cut when the prompt runs out of
//! room, and it defers to the locked style. Work records the profile it was
//! made with as a cosmetic dependency: a change of preferences never makes
//! it stale, but shows on what has not been chosen yet.

use kineto_project::{
    manifest::CanonicalProject,
    preferences::{self as document, Dimension, Preferences, Stance},
    revisions::RevisionRecord,
};

use crate::{documents::document_error, generation::GenerationError};

/// What a keyframe can show of the director's preferences: a still has no
/// motion or pace.
pub const KEYFRAME_DIMENSIONS: [Dimension; 6] = [
    Dimension::Lighting,
    Dimension::Framing,
    Dimension::Lens,
    Dimension::Color,
    Dimension::Composition,
    Dimension::Performance,
];

/// What a clip can show: everything but the lens and framing, which its
/// first frame already holds.
pub const VIDEO_DIMENSIONS: [Dimension; 5] = [
    Dimension::CameraMotion,
    Dimension::Lighting,
    Dimension::Color,
    Dimension::Performance,
    Dimension::Pacing,
];

/// The profile in force, and the revision that holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InForce {
    pub record: RevisionRecord,
    pub preferences: Preferences,
}

impl InForce {
    /// The profile's `dimensions`, worded for a prompt; `None` when it says
    /// nothing about them.
    #[must_use]
    pub fn sentence(&self, dimensions: &[Dimension]) -> Option<String> {
        sentence(&self.preferences, dimensions)
    }
}

/// The profile in force; `None` before the director writes one.
pub fn in_force(project: &CanonicalProject) -> Result<Option<InForce>, GenerationError> {
    Ok(document::current(project)
        .map_err(document_error)?
        .map(|(record, preferences)| InForce {
            record,
            preferences,
        }))
}

/// `preferences` about `dimensions`, as one sentence a model reads after
/// the locked style.
#[must_use]
pub fn sentence(preferences: &Preferences, dimensions: &[Dimension]) -> Option<String> {
    let entries = preferences.about(dimensions);
    let said = |stance: Stance| -> Vec<String> {
        entries
            .iter()
            .filter(|entry| entry.stance == stance)
            .map(|entry| {
                let value = entry.value.trim().trim_end_matches(['.', ' ']);
                format!("{value} {}", entry.dimension.label())
            })
            .collect()
    };
    let (prefer, avoid) = (said(Stance::Prefer), said(Stance::Avoid));
    if prefer.is_empty() && avoid.is_empty() {
        return None;
    }
    let mut clauses = Vec::new();
    if !prefer.is_empty() {
        clauses.push(format!("prefer {}", prefer.join(", ")));
    }
    if !avoid.is_empty() {
        clauses.push(format!("avoid {}", avoid.join(", ")));
    }
    Some(format!(
        "The director's preferences, where the locked style allows: {}",
        clauses.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kineto_project::preferences::{Preference, Source};

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
    fn each_kind_of_work_reads_the_preferences_it_can_show() {
        let preferences = Preferences {
            entries: vec![
                entry(Dimension::CameraMotion, Stance::Avoid, "handheld"),
                entry(Dimension::CameraMotion, Stance::Prefer, "slow dolly"),
                entry(Dimension::Lighting, Stance::Prefer, "low-key."),
                entry(Dimension::Lens, Stance::Prefer, "long"),
            ],
        };
        assert_eq!(
            sentence(&preferences, &KEYFRAME_DIMENSIONS).as_deref(),
            Some(
                "The director's preferences, where the locked style allows: prefer low-key lighting, long lens"
            )
        );
        assert_eq!(
            sentence(&preferences, &VIDEO_DIMENSIONS).as_deref(),
            Some(
                "The director's preferences, where the locked style allows: prefer slow dolly \
                 camera motion, low-key lighting; avoid handheld camera motion"
            )
        );
        assert_eq!(sentence(&preferences, &[Dimension::Pacing]), None);
        assert_eq!(sentence(&Preferences::default(), &Dimension::ALL), None);
    }
}
