//! How long each line of dialogue takes to say, and whether the shots that
//! carry it are long enough.
//!
//! Before a line is voiced, its length is estimated at its speaker's locked
//! voice rate, or the normal rate while the speaker has no voice
//! ([`speech::estimate_duration_ms`]), and marked as an estimate. Shot
//! planning is told every line's length and asked to give a shot that
//! carries dialogue at least as long. A planned shot shorter than its
//! dialogue is reported, never refused: the director decides.

use kineto_project::{
    manifest::CanonicalProject,
    revisions::{self, DocumentKind},
    screenplay::{BlockKind, Screenplay},
    shot_plan::ShotPlan,
    voice as voice_document,
};

use crate::{
    documents::document_error,
    generation::GenerationError,
    speech::{self, NORMAL_SPEED},
};

/// The breath between two lines one shot carries.
const PAUSE_MS: u32 = 250;

/// Where a line's length comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationSource {
    /// Worked out from its words and its speaker's rate, before it is
    /// voiced.
    Estimated,
    /// The length of its locked voicing.
    Measured,
}

/// One line of the screenplay and how long it takes to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineDuration {
    pub scene_id: String,
    /// Its place among the scene's lines, from 1.
    pub line: u32,
    /// Who says it.
    pub character: Option<String>,
    pub text: String,
    pub duration_ms: u32,
    pub source: DurationSource,
}

/// Every line of `screenplay`, in order, with how long it takes to say.
pub fn line_durations(
    project: &CanonicalProject,
    screenplay: &Screenplay,
) -> Result<Vec<LineDuration>, GenerationError> {
    let mut lines = Vec::new();
    for scene in &screenplay.scenes {
        for (line, block) in (1..).zip(
            scene
                .blocks
                .iter()
                .filter(|block| block.kind == BlockKind::Dialogue),
        ) {
            let speed = match &block.character {
                Some(character) => voice_document::locked(project, character)
                    .map_err(document_error)?
                    .map_or(NORMAL_SPEED, |lock| lock.voice.speed_permille),
                None => NORMAL_SPEED,
            };
            lines.push(LineDuration {
                scene_id: scene.id.clone(),
                line,
                character: block.character.clone(),
                text: block.text.trim().to_owned(),
                duration_ms: speech::estimate_duration_ms(&block.text, speed),
                source: DurationSource::Estimated,
            });
        }
    }
    Ok(lines)
}

/// How long the dialogue a shot of `scene_id` carries takes to say: each of
/// the scene's lines it holds verbatim, at that line's length, with a
/// breath between them; any other words at the normal rate. A speaker's
/// name before a colon is not said.
#[must_use]
pub fn dialogue_ms(lines: &[LineDuration], scene_id: &str, dialogue: &str) -> u32 {
    let mut rest = dialogue.to_owned();
    let mut spoken: Vec<u32> = Vec::new();
    for line in lines
        .iter()
        .filter(|line| line.scene_id == scene_id && !line.text.is_empty())
    {
        if let Some(at) = rest.find(&line.text) {
            rest.replace_range(at..at + line.text.len(), " ");
            spoken.push(line.duration_ms);
        }
    }
    let unmatched: String = rest
        .split_whitespace()
        .filter(|word| !word.ends_with(':'))
        .collect::<Vec<_>>()
        .join(" ");
    let other = speech::estimate_duration_ms(&unmatched, NORMAL_SPEED);
    if other > 0 && unmatched.chars().any(char::is_alphanumeric) {
        spoken.push(other);
    }
    let pauses = u32::try_from(spoken.len().saturating_sub(1)).unwrap_or(u32::MAX);
    spoken.iter().sum::<u32>() + pauses * PAUSE_MS
}

/// How long `dialogue`, a shot of `scene_id` carries, takes to say, with
/// the lines of the screenplay work builds on; `None` when it carries none.
pub fn shot_dialogue_ms(
    project: &CanonicalProject,
    scene_id: &str,
    dialogue: Option<&str>,
) -> Result<Option<u32>, GenerationError> {
    let Some(dialogue) = dialogue.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    let lines =
        match revisions::current(project, DocumentKind::Screenplay).map_err(document_error)? {
            Some(record) => line_durations(
                project,
                &revisions::read::<Screenplay>(project, &record).map_err(document_error)?,
            )?,
            None => Vec::new(),
        };
    Ok(Some(dialogue_ms(&lines, scene_id, dialogue)).filter(|ms| *ms > 0))
}

/// A planned shot shorter than the dialogue it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortShot {
    pub scene_id: String,
    pub shot_id: String,
    /// The shot's target length.
    pub planned_ms: u32,
    /// How long its dialogue takes to say.
    pub dialogue_ms: u32,
}

/// The shots of `plan` too short for their dialogue, in plan order.
#[must_use]
pub fn short_shots(plan: &ShotPlan, lines: &[LineDuration]) -> Vec<ShortShot> {
    plan.scenes
        .iter()
        .flat_map(|scene| {
            scene.shots.iter().filter_map(|shot| {
                if shot.dialogue.trim().is_empty() {
                    return None;
                }
                let dialogue_ms = dialogue_ms(lines, &scene.scene_id, &shot.dialogue);
                let planned_ms = seconds_to_ms(shot.duration_target);
                (planned_ms < dialogue_ms).then(|| ShortShot {
                    scene_id: scene.scene_id.clone(),
                    shot_id: shot.id.clone(),
                    planned_ms,
                    dialogue_ms,
                })
            })
        })
        .collect()
}

fn seconds_to_ms(seconds: f64) -> u32 {
    if seconds.is_finite() && seconds > 0.0 {
        // Plans keep 1 to 30 seconds, so the product fits.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ms = (seconds * 1000.0).round().min(f64::from(u32::MAX)) as u32;
        ms
    } else {
        0
    }
}

/// Every line's length, as shot planning is told it, between
/// `<line_durations>` and `</line_durations>`; empty when no one speaks.
#[must_use]
pub fn planning_note(lines: &[LineDuration]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut note = String::from("<line_durations>\n");
    for line in lines {
        note.push_str(&format!(
            "{} line {} ({}): {:.1} s: {}\n",
            line.scene_id,
            line.line,
            line.character.as_deref().unwrap_or("unattributed"),
            f64::from(line.duration_ms) / 1000.0,
            line.text
        ));
    }
    note.push_str("</line_durations>");
    note
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(scene_id: &str, number: u32, text: &str, duration_ms: u32) -> LineDuration {
        LineDuration {
            scene_id: scene_id.to_owned(),
            line: number,
            character: Some("interviewer".to_owned()),
            text: text.to_owned(),
            duration_ms,
            source: DurationSource::Estimated,
        }
    }

    #[test]
    fn a_shots_dialogue_lasts_as_long_as_the_lines_it_holds() {
        let lines = [
            line("office", 1, "Was it worth it?", 1520),
            line("office", 2, "Yes.", 380),
            line("street", 1, "Was it worth it?", 9_000),
        ];
        assert_eq!(dialogue_ms(&lines, "office", "Was it worth it?"), 1520);
        // Two lines, a breath between; a speaker's name is not said.
        assert_eq!(
            dialogue_ms(
                &lines,
                "office",
                "INTERVIEWER: Was it worth it? FILMMAKER: Yes."
            ),
            1520 + PAUSE_MS + 380
        );
        // Words the screenplay does not have are estimated.
        assert_eq!(
            dialogue_ms(&lines, "office", "Was it worth it? Truly"),
            1520 + PAUSE_MS + 380
        );
        assert_eq!(dialogue_ms(&lines, "office", "  "), 0);
    }

    #[test]
    fn shots_shorter_than_their_dialogue_are_reported() {
        let plan: ShotPlan = serde_json::from_value(serde_json::json!({
            "scenes": [{
                "scene_id": "office",
                "shots": [
                    shot("office_01", "Was it worth it?", 1.0),
                    shot("office_02", "Was it worth it?", 2.0),
                    shot("office_03", "", 1.0)
                ]
            }]
        }))
        .unwrap();
        let lines = [line("office", 1, "Was it worth it?", 1520)];
        assert_eq!(
            short_shots(&plan, &lines),
            [ShortShot {
                scene_id: "office".to_owned(),
                shot_id: "office_01".to_owned(),
                planned_ms: 1000,
                dialogue_ms: 1520,
            }]
        );
        assert_eq!(
            planning_note(&lines),
            "<line_durations>\noffice line 1 (interviewer): 1.5 s: Was it worth it?\n</line_durations>"
        );
        assert_eq!(planning_note(&[]), "");
    }

    fn shot(id: &str, dialogue: &str, seconds: f64) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "purpose": "Hold on the question.",
            "direction": "tension",
            "framing": "close_up",
            "lens": "50mm",
            "camera_motion": "static",
            "action": "The interviewer waits.",
            "dialogue": dialogue,
            "emotion": "quiet",
            "duration_target": seconds,
            "characters": [],
            "continuity": []
        })
    }
}
