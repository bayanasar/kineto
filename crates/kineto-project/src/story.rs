//! The story analysis: what a source story contains, as structured state.
//!
//! `schemas/story-analysis.schema.json` is the contract, and these types
//! enforce it on the way in: unknown fields are refused, identifiers are
//! portable and unique within their list, and every reference names
//! something the analysis defines. A model's answer becomes a
//! [`StoryAnalysis`] only if all of that holds.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// The committed schema, as the model is shown it.
pub const STORY_ANALYSIS_SCHEMA: &str = include_str!("../../../schemas/story-analysis.schema.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CharacterRole {
    Protagonist,
    Antagonist,
    Supporting,
    Minor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Character {
    pub id: String,
    pub name: String,
    pub description: String,
    pub role: CharacterRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoryEvent {
    pub id: String,
    pub summary: String,
    pub characters: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relationship {
    pub from: String,
    pub to: String,
    pub nature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateScene {
    pub id: String,
    pub summary: String,
    pub location: String,
    pub characters: Vec<String>,
    pub events: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStoryAnalysis {
    logline: String,
    time_period: String,
    tone: String,
    themes: Vec<String>,
    characters: Vec<Character>,
    locations: Vec<Location>,
    events: Vec<StoryEvent>,
    relationships: Vec<Relationship>,
    candidate_scenes: Vec<CandidateScene>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawStoryAnalysis")]
pub struct StoryAnalysis {
    pub logline: String,
    pub time_period: String,
    pub tone: String,
    pub themes: Vec<String>,
    pub characters: Vec<Character>,
    pub locations: Vec<Location>,
    pub events: Vec<StoryEvent>,
    pub relationships: Vec<Relationship>,
    pub candidate_scenes: Vec<CandidateScene>,
}

/// An identifier as the schema allows it.
#[must_use]
pub fn is_portable_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn ids<'a>(list: impl Iterator<Item = &'a str>, what: &str) -> Result<BTreeSet<&'a str>, String> {
    let mut seen = BTreeSet::new();
    for id in list {
        if !is_portable_id(id) {
            return Err(format!("{what} id {id:?} is not a portable identifier"));
        }
        if !seen.insert(id) {
            return Err(format!("{what} id {id:?} appears twice"));
        }
    }
    Ok(seen)
}

fn known(set: &BTreeSet<&str>, id: &str, what: &str, owner: &str) -> Result<(), String> {
    if set.contains(id) {
        Ok(())
    } else {
        Err(format!("{owner} names {what} {id:?}, which is not defined"))
    }
}

fn filled(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} is empty"))
    } else {
        Ok(())
    }
}

impl TryFrom<RawStoryAnalysis> for StoryAnalysis {
    type Error = String;

    fn try_from(raw: RawStoryAnalysis) -> Result<Self, String> {
        filled(&raw.logline, "logline")?;
        filled(&raw.time_period, "time_period")?;
        filled(&raw.tone, "tone")?;
        if raw.characters.is_empty() {
            return Err("there are no characters".to_owned());
        }
        if raw.candidate_scenes.is_empty() {
            return Err("there are no candidate scenes".to_owned());
        }
        let characters = ids(raw.characters.iter().map(|c| c.id.as_str()), "character")?;
        let locations = ids(raw.locations.iter().map(|l| l.id.as_str()), "location")?;
        let events = ids(raw.events.iter().map(|e| e.id.as_str()), "event")?;
        ids(raw.candidate_scenes.iter().map(|s| s.id.as_str()), "scene")?;
        for character in &raw.characters {
            filled(&character.name, "character name")?;
            filled(&character.description, "character description")?;
        }
        for location in &raw.locations {
            filled(&location.name, "location name")?;
        }
        for event in &raw.events {
            filled(&event.summary, "event summary")?;
            for character in &event.characters {
                known(
                    &characters,
                    character,
                    "character",
                    &format!("event {:?}", event.id),
                )?;
            }
            if let Some(location) = &event.location {
                known(
                    &locations,
                    location,
                    "location",
                    &format!("event {:?}", event.id),
                )?;
            }
        }
        for relationship in &raw.relationships {
            known(
                &characters,
                &relationship.from,
                "character",
                "a relationship",
            )?;
            known(&characters, &relationship.to, "character", "a relationship")?;
        }
        for scene in &raw.candidate_scenes {
            let owner = format!("scene {:?}", scene.id);
            filled(&scene.summary, "scene summary")?;
            known(&locations, &scene.location, "location", &owner)?;
            for character in &scene.characters {
                known(&characters, character, "character", &owner)?;
            }
            for event in &scene.events {
                known(&events, event, "event", &owner)?;
            }
        }
        Ok(Self {
            logline: raw.logline,
            time_period: raw.time_period,
            tone: raw.tone,
            themes: raw.themes,
            characters: raw.characters,
            locations: raw.locations,
            events: raw.events,
            relationships: raw.relationships,
            candidate_scenes: raw.candidate_scenes,
        })
    }
}

/// A small, valid analysis, for tests across the workspace.
#[doc(hidden)]
pub const SAMPLE_ANALYSIS_JSON: &str = r#"{
  "logline": "A filmmaker hears the last line of an interview and leaves before he can answer.",
  "time_period": "The present day",
  "tone": "Quiet, withheld, tense",
  "themes": ["what goes unsaid"],
  "characters": [
    {"id": "filmmaker", "name": "The Filmmaker", "description": "A tired man in his forties in a dark coat.", "role": "protagonist"},
    {"id": "interviewer", "name": "The Interviewer", "description": "A calm woman with a notebook.", "role": "supporting"}
  ],
  "locations": [
    {"id": "small_office", "name": "A small office", "description": "One table, two chairs, a window at night."}
  ],
  "events": [
    {"id": "last_line", "summary": "The interviewer delivers the final line.", "characters": ["filmmaker", "interviewer"], "location": "small_office"},
    {"id": "leaving", "summary": "The filmmaker stands and leaves.", "characters": ["filmmaker"], "location": "small_office"}
  ],
  "relationships": [
    {"from": "filmmaker", "to": "interviewer", "nature": "subject and questioner"}
  ],
  "candidate_scenes": [
    {"id": "the_last_line", "summary": "The final line lands and the filmmaker leaves.", "location": "small_office", "characters": ["filmmaker", "interviewer"], "events": ["last_line", "leaving"]}
  ]
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> serde_json::Value {
        serde_json::from_str(SAMPLE_ANALYSIS_JSON).unwrap()
    }

    fn parse(value: &serde_json::Value) -> Result<StoryAnalysis, String> {
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())
    }

    #[test]
    fn a_consistent_analysis_round_trips() {
        let analysis = parse(&sample()).unwrap();
        assert_eq!(analysis.characters.len(), 2);
        let encoded = serde_json::to_value(&analysis).unwrap();
        assert_eq!(parse(&encoded).unwrap(), analysis);
    }

    #[test]
    fn inconsistent_analyses_are_refused_with_a_reason() {
        let cases: [(fn(&mut serde_json::Value), &str); 7] = [
            (
                |v| v["events"][0]["characters"][0] = "ghost".into(),
                "not defined",
            ),
            (
                |v| v["candidate_scenes"][0]["location"] = "moon".into(),
                "location \"moon\"",
            ),
            (
                |v| v["characters"][1]["id"] = "filmmaker".into(),
                "appears twice",
            ),
            (
                |v| v["characters"][0]["id"] = "has space".into(),
                "portable",
            ),
            (|v| v["characters"] = serde_json::json!([]), "no characters"),
            (|v| v["tone"] = " ".into(), "tone is empty"),
            (|v| v["extra"] = 1.into(), "unknown field"),
        ];
        for (edit, expected) in cases {
            let mut value = sample();
            edit(&mut value);
            let error = parse(&value).unwrap_err();
            assert!(error.contains(expected), "{expected}: {error}");
        }
    }
}
