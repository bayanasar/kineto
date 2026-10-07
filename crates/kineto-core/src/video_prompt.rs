//! Compile a shot's planned intent into the prompt for its video, which
//! starts from the locked keyframe.
//!
//! The first frame carries framing, lens, setting, the characters' look and
//! the shot it continues from, so those are applied through it rather than
//! restated. What moves is said, each as the provider declares it can:
//!
//! - Camera motion in the provider's own command where its control is
//!   parametric and it has one for the motion, in words where it takes
//!   words, and reported as dropped where it takes neither.
//! - Clip length as the declared length nearest the plan's, reported when
//!   they differ.
//! - Dialogue is reported as dropped: lines are dubbed apart from the
//!   clip. A provider that makes its own sound is asked for none of it to
//!   be speech.
//! - Mood, the shot's direction and its continuity notes while the prompt
//!   has room, cut by rank and reported when it has not.
//! - A last frame: when the next shot continues from this one in the same
//!   setup, its locked keyframe closes this clip where the provider takes
//!   it; otherwise the compiler reports why it was not used.
//!
//! The first frame alone does not hold a model to it: in the P4 live run,
//! Hailuo started from a black-and-white noir keyframe and cut within a
//! second to a colour scene the action's words described. Version 3 leads
//! with the locked style, to be kept throughout the clip, and asks for one
//! continuous shot.
//!
//! Continuing from a shot is not the same as staying in its setup. Given
//! a wide first frame and an over-the-shoulder last one, Veo made seven
//! seconds of action and dissolved into the close shot in the last: a
//! transition, which is the editor's. Version 7 sends a last frame only when
//! the next shot keeps the framing and lens, or this shot's dolly explains
//! the change, and in a clip length the provider closes on one at.

use kineto_project::shot::{ShotDirection, ShotIntent};
use kineto_prompts::{
    CompiledPrompt, ConstraintId, ConstraintIdError, DegradationLevel, DroppedConstraint,
    PromptHash,
};
use kineto_providers::{CameraControl, VideoCapabilities};
use sha2::{Digest, Sha256};

use crate::{
    keyframe_prompt::KeyframePromptError,
    style_prompt::{Part, fit, sentence},
};

/// Version recorded with every video prompt compiled by this module.
pub const VIDEO_COMPILER_VERSION: &str = "video-prompt/7";

const INTENT_DOMAIN: &[u8] = b"kineto.video-intent.v7";

/// Mood is cut last of what is optional, the shot's direction before it,
/// its continuity notes first: the first frame already holds what carries
/// over.
const RANK_CONTINUITY: u8 = 0;
const RANK_DIRECTION: u8 = 1;
const RANK_MOOD: u8 = 2;

/// A compiled video prompt, its hash, and the clip length it asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPrompt {
    pub compiled: CompiledPrompt,
    pub prompt_hash: PromptHash,
    pub duration_seconds: u8,
}

/// What became of the frame a clip could close on: the locked keyframe of
/// the shot that continues from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndFrameUse<'a> {
    /// Sent: the clip closes on it.
    Sent,
    /// The provider takes no last frame.
    NotTaken,
    /// The next shot's keyframe is not locked yet.
    NotLocked,
    /// The next shot's keyframe has no image to send.
    NoImage,
    /// Its image, or this clip's length, is not one the provider takes a
    /// last frame in; why.
    Unfit(&'a str),
    /// The next shot cuts away rather than carrying the setup on; why.
    Cut(&'a str),
}

impl EndFrameUse<'_> {
    const fn code(self) -> u8 {
        match self {
            Self::Sent => 1,
            Self::NotTaken => 2,
            Self::NotLocked => 3,
            Self::NoImage => 4,
            Self::Unfit(_) => 5,
            Self::Cut(_) => 6,
        }
    }
}

/// The shot that continues from the one being compiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Successor<'a> {
    pub shot_id: &'a str,
    pub end_frame: EndFrameUse<'a>,
}

/// Everything a video prompt is compiled from.
#[derive(Debug, Clone, Copy)]
pub struct VideoPromptInput<'a> {
    pub shot: Option<&'a ShotIntent>,
    pub direction: ShotDirection,
    /// The content hash of the locked keyframe the clip starts from.
    pub keyframe_hash: &'a str,
    pub capabilities: &'a VideoCapabilities,
    /// The provider's own command for the shot's camera motion, if it has
    /// one.
    pub camera_command: Option<&'a str>,
    pub successor: Option<Successor<'a>>,
    /// The director's notes for this generation, oldest first.
    pub notes: &'a [String],
    /// The locked style's directives, kept throughout the clip.
    pub style: Option<&'a str>,
    /// How long the shot's dialogue takes to say, which the clip should
    /// hold.
    pub dialogue_ms: Option<u32>,
    /// The director's preferences in force, worded for a clip.
    pub preferences: Option<&'a str>,
}

/// Compile the video prompt for `input`.
pub fn compile(input: VideoPromptInput<'_>) -> Result<VideoPrompt, KeyframePromptError> {
    let shot = input.shot.ok_or(KeyframePromptError::MissingIntent)?;
    let capabilities = input.capabilities;
    let mut required = Vec::new();
    let mut optional = Vec::new();
    let mut dropped = Vec::new();

    if let Some(style) = input.style.map(str::trim).filter(|style| !style.is_empty()) {
        needed(
            &mut required,
            "style.locked",
            &format!(
                "{}, throughout the clip",
                style.trim_end_matches(['.', ' '])
            ),
        )?;
    }
    needed(&mut required, "shot.action", &sentence_case(&shot.action))?;
    if let Some(motion) = &shot.camera_motion {
        let words = motion.replace('_', " ");
        match (capabilities.camera_control, input.camera_command) {
            (CameraControl::None, _) => dropped.push(drop(
                "shot.camera_motion",
                &format!("the provider cannot direct the camera; {words} was asked for"),
            )?),
            (CameraControl::Parametric, Some(command)) => needed(
                &mut required,
                "shot.camera_motion",
                &format!("Camera: {words} [{command}]"),
            )?,
            (CameraControl::Parametric | CameraControl::Textual, _) => {
                needed(
                    &mut required,
                    "shot.camera_motion",
                    &format!("Camera: {words}"),
                )?;
            }
        }
    }
    if !input.notes.is_empty() {
        needed(
            &mut required,
            "director.note",
            &format!(
                "Director's note: {}",
                input
                    .notes
                    .iter()
                    .map(|note| note.trim().trim_end_matches(['.', ' ']))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        )?;
    }
    needed(
        &mut required,
        "keyframe.first_frame",
        "One continuous shot without cuts. Keep the subject, framing and light of the first \
         frame; natural, restrained motion",
    )?;
    // Words in a prompt can come back as words in the picture.
    needed(
        &mut required,
        "frame.no_text",
        "No on-screen text, titles or captions",
    )?;
    if capabilities.audio {
        needed(
            &mut required,
            "audio.no_speech",
            "No speech or dialogue; only the sound of the scene",
        )?;
    }
    // Carried by the first frame rather than restated.
    let mut carried: Vec<String> = Vec::new();
    if shot.framing.is_some() {
        carried.push("shot.framing".to_owned());
    }
    if shot.lens.is_some() {
        carried.push("shot.lens".to_owned());
    }
    if shot.continuity_from.is_some() {
        carried.push("continuity.from".to_owned());
    }
    carried.extend(
        shot.characters
            .iter()
            .map(|character| format!("identity.{character}")),
    );
    for constraint in carried {
        needed(&mut required, &constraint, "")?;
    }

    if let Some(emotion) = &shot.emotion {
        optional.push(part(
            "shot.emotion",
            &format!("Mood: {emotion}"),
            RANK_MOOD,
        )?);
    }
    optional.push(part(
        "shot.direction",
        emphasis(input.direction),
        RANK_DIRECTION,
    )?);
    if !shot.continuity.is_empty() {
        optional.push(part(
            "shot.continuity",
            &format!(
                "Keep: {}",
                shot.continuity
                    .iter()
                    .map(|note| note.trim().trim_end_matches(['.', ' ']))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            RANK_CONTINUITY,
        )?);
    }
    // The director's preferences give way first, and to the locked style.
    if let Some(preferences) = input
        .preferences
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        optional.push(part("director.preferences", preferences, RANK_CONTINUITY)?);
    }

    if shot.dialogue.is_some() {
        dropped.push(drop(
            "shot.dialogue",
            if capabilities.audio {
                "the line is dubbed apart from the clip"
            } else {
                "the video model generates no speech"
            },
        )?);
    }
    let duration_seconds = clip_seconds(capabilities, shot.duration_target, input.dialogue_ms)
        .ok_or(KeyframePromptError::MissingIntent)?;
    if let Some(dialogue_ms) = input.dialogue_ms
        && u32::from(duration_seconds) * 1000 < dialogue_ms
    {
        dropped.push(drop(
            "shot.dialogue_duration",
            &format!(
                "the dialogue takes {:.1} s; the provider's longest clip is {duration_seconds} s",
                f64::from(dialogue_ms) / 1000.0
            ),
        )?);
    }
    if let Some(target) = shot.duration_target {
        if (f64::from(duration_seconds) - target).abs() > f64::EPSILON {
            dropped.push(drop(
                "shot.duration_target",
                &format!("{target} s was asked for; the provider renders {duration_seconds} s"),
            )?);
        } else {
            needed(&mut required, "shot.duration_target", "")?;
        }
    }
    if let Some(successor) = input.successor {
        let next = successor.shot_id;
        let unsent = |why: &str| format!("{next} continues from this shot, but {why}");
        match successor.end_frame {
            EndFrameUse::Sent => needed(
                &mut required,
                "continuity.end_frame",
                &format!("End on the opening frame of {next}, which continues from this shot"),
            )?,
            EndFrameUse::NotTaken => dropped.push(drop(
                "continuity.end_frame",
                &unsent("the provider takes no last frame"),
            )?),
            EndFrameUse::NotLocked => dropped.push(drop(
                "continuity.end_frame",
                &unsent("its keyframe is not locked yet"),
            )?),
            EndFrameUse::NoImage => dropped.push(drop(
                "continuity.end_frame",
                &unsent("its keyframe has no image to send"),
            )?),
            EndFrameUse::Unfit(why) => {
                dropped.push(drop("continuity.end_frame", &unsent(why))?);
            }
            EndFrameUse::Cut(why) => dropped.push(drop(
                "continuity.end_frame",
                &unsent(&format!("it is a cut, not a continuation: {why}")),
            )?),
        }
    }

    let prompt = fit(
        &required,
        &optional,
        dropped,
        capabilities.max_prompt_chars,
        VIDEO_COMPILER_VERSION,
        &canonical_intent(&input, shot, duration_seconds),
    )?;
    Ok(VideoPrompt {
        compiled: prompt.compiled,
        prompt_hash: prompt.prompt_hash,
        duration_seconds,
    })
}

/// The framings a dolly moves between, widest first.
const SCALE: [&str; 8] = [
    "extreme_wide",
    "wide",
    "full",
    "medium_wide",
    "medium",
    "medium_close_up",
    "close_up",
    "extreme_close_up",
];

/// Why `next`, which continues from `shot`, cuts away rather than carrying
/// its setup on, so that a clip closed on its keyframe would dissolve into
/// it. `None` when the camera stays: the same framing and lens, or
/// `shot`'s dolly carries the framing to `next`'s.
#[must_use]
pub fn cut_to(shot: &ShotIntent, next: &ShotIntent) -> Option<String> {
    let (Some(from), Some(to)) = (shot.framing.as_deref(), next.framing.as_deref()) else {
        return Some("the plan does not say how both shots are framed".to_owned());
    };
    if let (Some(from), Some(to)) = (shot.lens.as_deref(), next.lens.as_deref())
        && from != to
    {
        return Some(format!("the lens changes from {from} to {to}"));
    }
    if from == to {
        return None;
    }
    let place = |framing: &str| SCALE.iter().position(|step| *step == framing);
    let motion = shot.camera_motion.as_deref().unwrap_or("static");
    let carried = match (place(from), place(to), motion) {
        (Some(from), Some(to), "dolly_in" | "slow_dolly_in") => to > from,
        (Some(from), Some(to), "dolly_out" | "slow_dolly_out") => to < from,
        _ => false,
    };
    (!carried).then(|| {
        format!(
            "the framing changes from {} to {}, which a {} camera does not explain",
            from.replace('_', " "),
            to.replace('_', " "),
            motion.replace('_', " ")
        )
    })
}

/// The clip length a shot of `target` seconds is made at: the declared
/// length nearest it, or the shortest without a target. A clip that would
/// cut off the shot's dialogue (`dialogue_ms`) is made at the shortest
/// length that holds it, or the longest there is. `None` when the provider
/// declares no lengths.
#[must_use]
pub fn clip_seconds(
    capabilities: &VideoCapabilities,
    target: Option<f64>,
    dialogue_ms: Option<u32>,
) -> Option<u8> {
    let shortest = capabilities.nearest_clip_seconds(0.0)?;
    let wanted = target.map_or(shortest, |target| {
        capabilities
            .nearest_clip_seconds(target)
            .unwrap_or(shortest)
    });
    let Some(dialogue_ms) = dialogue_ms.filter(|ms| u32::from(wanted) * 1000 < *ms) else {
        return Some(wanted);
    };
    let needed = f64::from(dialogue_ms) / 1000.0;
    let holding = if capabilities.clip_seconds.is_empty() {
        let longest = capabilities.max_duration_ms.map(|ms| ms / 1000)?;
        // Within a declared range, any whole second can be asked for.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let seconds = needed.ceil() as u64;
        (seconds <= longest)
            .then(|| u8::try_from(seconds).ok())
            .flatten()
    } else {
        capabilities
            .clip_seconds
            .iter()
            .copied()
            .filter(|seconds| f64::from(*seconds) >= needed)
            .min()
    };
    holding.or_else(|| {
        capabilities.clip_seconds.iter().copied().max().or_else(|| {
            capabilities
                .max_duration_ms
                .and_then(|ms| u8::try_from(ms / 1000).ok())
        })
    })
}

/// A part the prompt cannot do without; an empty text is said by another
/// part or by the first frame.
fn needed(parts: &mut Vec<Part>, constraint: &str, text: &str) -> Result<(), KeyframePromptError> {
    ConstraintId::new(constraint)?;
    parts.push(Part {
        constraint: constraint.to_owned(),
        text: if text.is_empty() {
            String::new()
        } else {
            sentence(text)
        },
        rank: u8::MAX,
        degradation: DegradationLevel::Cosmetic,
    });
    Ok(())
}

fn part(constraint: &str, text: &str, rank: u8) -> Result<Part, KeyframePromptError> {
    ConstraintId::new(constraint)?;
    Ok(Part {
        constraint: constraint.to_owned(),
        text: sentence(text),
        rank,
        degradation: DegradationLevel::Cosmetic,
    })
}

fn drop(id: &str, reason: &str) -> Result<DroppedConstraint, ConstraintIdError> {
    Ok(DroppedConstraint {
        constraint: ConstraintId::new(id)?,
        reason: reason.to_owned(),
        degradation: DegradationLevel::Cosmetic,
    })
}

fn emphasis(direction: ShotDirection) -> &'static str {
    match direction {
        ShotDirection::Reaction => "Let the listener's reaction carry the moment",
        ShotDirection::SpatialClarity => {
            "Keep where each person stands readable as the camera moves"
        }
        ShotDirection::Intimacy => "Stay close and intimate with the subject",
        ShotDirection::Tension => "Let the tension build through the clip",
    }
}

fn sentence_case(value: &str) -> String {
    let mut chars = value.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A deterministic, length-framed encoding of everything the compiler read.
fn canonical_intent(input: &VideoPromptInput<'_>, shot: &ShotIntent, duration: u8) -> Vec<u8> {
    let mut hasher = Sha256::new();
    let mut field = |bytes: &[u8]| {
        hasher.update(
            u64::try_from(bytes.len())
                .expect("length fits in u64")
                .to_be_bytes(),
        );
        hasher.update(bytes);
    };
    let optional = |value: Option<&str>| match value {
        None => vec![0],
        Some(value) => [&[1], value.as_bytes()].concat(),
    };
    field(INTENT_DOMAIN);
    field(input.keyframe_hash.as_bytes());
    field(shot.action.as_bytes());
    field(&optional(shot.framing.as_deref()));
    field(&optional(shot.lens.as_deref()));
    field(&optional(shot.camera_motion.as_deref()));
    field(&optional(input.camera_command));
    field(&optional(shot.emotion.as_deref()));
    field(&optional(shot.dialogue.as_deref()));
    field(&(shot.characters.len() as u64).to_be_bytes());
    for character in &shot.characters {
        field(character.as_bytes());
    }
    field(&(shot.continuity.len() as u64).to_be_bytes());
    for note in &shot.continuity {
        field(note.as_bytes());
    }
    field(&optional(shot.continuity_from.as_deref()));
    field(
        &shot
            .duration_target
            .map_or_else(Vec::new, |seconds| seconds.to_bits().to_be_bytes().to_vec()),
    );
    field(&optional(
        input.successor.map(|successor| successor.shot_id),
    ));
    field(&(input.notes.len() as u64).to_be_bytes());
    for note in input.notes {
        field(note.as_bytes());
    }
    field(&optional(input.style));
    field(&optional(input.preferences));
    field(&[
        input.direction.code(),
        duration,
        input
            .successor
            .map_or(0, |successor| successor.end_frame.code()),
        input.capabilities.camera_control as u8,
        u8::from(input.capabilities.end_frame),
        u8::from(input.capabilities.audio),
    ]);
    if let Some(Successor {
        end_frame: EndFrameUse::Cut(why) | EndFrameUse::Unfit(why),
        ..
    }) = input.successor
    {
        field(why.as_bytes());
    }
    hasher.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kineto_providers::VideoMode;

    const KEYFRAME: &str =
        "sha256:1111111111111111111111111111111111111111111111111111111111111111";

    fn shot() -> ShotIntent {
        ShotIntent {
            action: "A filmmaker hears the final line across the table".to_owned(),
            framing: Some("medium_close_up".to_owned()),
            lens: Some("50mm".to_owned()),
            camera_motion: Some("slow_dolly_in".to_owned()),
            emotion: Some("a withheld reaction".to_owned()),
            dialogue: None,
            characters: vec!["filmmaker".to_owned()],
            duration_target: Some(4.0),
            continuity: Vec::new(),
            continuity_from: None,
        }
    }

    fn capabilities(control: CameraControl) -> VideoCapabilities {
        VideoCapabilities {
            modes: [VideoMode::ImageToVideo].into_iter().collect(),
            start_frame: true,
            camera_control: control,
            clip_seconds: [6, 10].into_iter().collect(),
            max_prompt_chars: Some(2000),
            ..VideoCapabilities::default()
        }
    }

    fn input<'a>(
        shot: &'a ShotIntent,
        capabilities: &'a VideoCapabilities,
        command: Option<&'a str>,
    ) -> VideoPromptInput<'a> {
        VideoPromptInput {
            shot: Some(shot),
            direction: ShotDirection::Reaction,
            keyframe_hash: KEYFRAME,
            capabilities,
            camera_command: command,
            successor: None,
            notes: &[],
            style: None,
            dialogue_ms: None,
            preferences: None,
        }
    }

    #[test]
    fn a_next_shot_continues_the_setup_only_in_its_framing_and_lens_or_by_a_dolly() {
        let framed = |framing: &str, lens: Option<&str>, motion: &str| ShotIntent {
            framing: Some(framing.to_owned()),
            lens: lens.map(str::to_owned),
            camera_motion: Some(motion.to_owned()),
            ..shot()
        };
        let wide = framed("wide", Some("28mm"), "static");
        assert_eq!(
            cut_to(&wide, &framed("wide", Some("28mm"), "pan_left")),
            None
        );
        assert_eq!(cut_to(&wide, &framed("wide", None, "static")), None);
        assert_eq!(
            cut_to(&wide, &framed("wide", Some("50mm"), "static")).as_deref(),
            Some("the lens changes from 28mm to 50mm")
        );
        // The trial's pair: wide to over the shoulder dissolved.
        assert_eq!(
            cut_to(&wide, &framed("over_the_shoulder", Some("28mm"), "static")).as_deref(),
            Some(
                "the framing changes from wide to over the shoulder, which a static camera \
                 does not explain"
            )
        );
        let pushing = framed("wide", Some("50mm"), "slow_dolly_in");
        assert_eq!(
            cut_to(&pushing, &framed("close_up", Some("50mm"), "static")),
            None
        );
        assert!(cut_to(&pushing, &framed("extreme_wide", Some("50mm"), "static")).is_some());
        let pulling = framed("close_up", None, "dolly_out");
        assert_eq!(cut_to(&pulling, &framed("medium", None, "static")), None);
        assert!(cut_to(&pulling, &framed("insert", None, "static")).is_some());
        let unframed = ShotIntent {
            framing: None,
            ..wide.clone()
        };
        assert_eq!(
            cut_to(&unframed, &wide).as_deref(),
            Some("the plan does not say how both shots are framed")
        );
    }

    #[test]
    fn a_cut_or_a_clip_too_short_for_a_last_frame_is_reported_and_hashed() {
        let shot = shot();
        let mut capabilities = capabilities(CameraControl::Textual);
        capabilities.end_frame = true;
        let closing = |end_frame| {
            compile(VideoPromptInput {
                successor: Some(Successor {
                    shot_id: "office_night_02",
                    end_frame,
                }),
                ..input(&shot, &capabilities, None)
            })
            .unwrap()
        };
        let cut = closing(EndFrameUse::Cut("the lens changes from 28mm to 50mm"));
        assert!(
            dropped(&cut).contains(&(
                "continuity.end_frame",
                "office_night_02 continues from this shot, but it is a cut, not a continuation: \
                 the lens changes from 28mm to 50mm"
            )),
            "{:?}",
            dropped(&cut)
        );
        let short = closing(EndFrameUse::Unfit(
            "the provider closes on a last frame only in a clip of 8 s, and this clip is 6 s",
        ));
        assert_ne!(cut.prompt_hash, short.prompt_hash);
        assert_ne!(
            short.prompt_hash,
            closing(EndFrameUse::Unfit("its keyframe is too large")).prompt_hash
        );
    }

    #[test]
    fn a_provider_that_makes_its_own_sound_is_asked_for_no_speech() {
        let mut talking = shot();
        talking.dialogue = Some("Was it worth it?".to_owned());
        let silent = capabilities(CameraControl::Textual);
        let quiet = compile(input(&talking, &silent, None)).unwrap();
        assert!(!applied(&quiet).contains(&"audio.no_speech"));
        assert!(
            dropped(&quiet).contains(&("shot.dialogue", "the video model generates no speech"))
        );

        let sounding = VideoCapabilities {
            audio: true,
            ..silent
        };
        let asked = compile(input(&talking, &sounding, None)).unwrap();
        assert!(applied(&asked).contains(&"audio.no_speech"));
        assert!(
            asked
                .compiled
                .prompt()
                .contains("No speech or dialogue; only the sound of the scene."),
            "{}",
            asked.compiled.prompt()
        );
        assert!(
            dropped(&asked).contains(&("shot.dialogue", "the line is dubbed apart from the clip"))
        );
        assert_ne!(quiet.prompt_hash, asked.prompt_hash);
    }

    fn applied(prompt: &VideoPrompt) -> Vec<&str> {
        prompt
            .compiled
            .applied_constraints()
            .iter()
            .map(ConstraintId::as_str)
            .collect()
    }

    fn dropped(prompt: &VideoPrompt) -> Vec<(&str, &str)> {
        prompt
            .compiled
            .dropped_constraints()
            .iter()
            .map(|dropped| (dropped.constraint.as_str(), dropped.reason.as_str()))
            .collect()
    }

    #[test]
    fn motion_is_applied_and_an_unrenderable_length_is_reported() {
        let parametric = capabilities(CameraControl::Parametric);
        let shot = shot();
        let prompt = compile(input(&shot, &parametric, Some("Push in"))).unwrap();
        assert_eq!(
            prompt.compiled.prompt(),
            "A filmmaker hears the final line across the table. Camera: slow dolly in \
             [Push in]. One continuous shot without cuts. Keep the subject, framing and light \
             of the first frame; natural, restrained motion. No on-screen text, titles or \
             captions. Mood: a withheld reaction. Let the listener's reaction carry the moment."
        );
        assert_eq!(prompt.duration_seconds, 6);
        assert_eq!(
            dropped(&prompt),
            [(
                "shot.duration_target",
                "4 s was asked for; the provider renders 6 s"
            )]
        );
        assert_eq!(prompt.compiled.degradation(), DegradationLevel::Cosmetic);
        for constraint in [
            "shot.camera_motion",
            "keyframe.first_frame",
            "shot.framing",
            "shot.lens",
            "identity.filmmaker",
            "shot.emotion",
            "shot.direction",
        ] {
            assert!(applied(&prompt).contains(&constraint), "{constraint}");
        }
    }

    #[test]
    fn camera_motion_follows_what_the_provider_can_take() {
        let shot = shot();
        let textual = capabilities(CameraControl::Textual);
        let words = compile(input(&shot, &textual, Some("Push in"))).unwrap();
        assert!(words.compiled.prompt().contains("Camera: slow dolly in."));
        assert!(!words.compiled.prompt().contains('['));

        let parametric = capabilities(CameraControl::Parametric);
        let no_command = compile(input(&shot, &parametric, None)).unwrap();
        assert!(
            no_command
                .compiled
                .prompt()
                .contains("Camera: slow dolly in.")
        );

        let none = capabilities(CameraControl::None);
        let still = compile(input(&shot, &none, None)).unwrap();
        assert!(!still.compiled.prompt().contains("Camera"));
        assert!(dropped(&still).contains(&(
            "shot.camera_motion",
            "the provider cannot direct the camera; slow dolly in was asked for"
        )));
        assert!(!applied(&still).contains(&"shot.camera_motion"));
    }

    #[test]
    fn dialogue_continuity_and_the_next_shot_are_said_or_reported() {
        let mut shot = shot();
        shot.dialogue = Some("Was it worth it?".to_owned());
        shot.continuity = vec![
            "Rain on the window.".to_owned(),
            "The lamp stays on".to_owned(),
        ];
        shot.continuity_from = Some("office_night_01".to_owned());
        shot.duration_target = Some(6.0);
        let parametric = capabilities(CameraControl::Parametric);
        let prompt = compile(VideoPromptInput {
            successor: Some(Successor {
                shot_id: "office_night_03",
                end_frame: EndFrameUse::NotTaken,
            }),
            ..input(&shot, &parametric, Some("Push in"))
        })
        .unwrap();
        assert!(
            prompt
                .compiled
                .prompt()
                .ends_with("Keep: Rain on the window; The lamp stays on.")
        );
        assert!(applied(&prompt).contains(&"continuity.from"));
        assert!(applied(&prompt).contains(&"shot.duration_target"));
        assert_eq!(
            dropped(&prompt),
            [
                ("shot.dialogue", "the video model generates no speech"),
                (
                    "continuity.end_frame",
                    "office_night_03 continues from this shot, but the provider takes no last \
                     frame"
                ),
            ]
        );

        let mut takes_last_frame = parametric.clone();
        takes_last_frame.end_frame = true;
        let waiting = compile(VideoPromptInput {
            successor: Some(Successor {
                shot_id: "office_night_03",
                end_frame: EndFrameUse::NotLocked,
            }),
            ..input(&shot, &takes_last_frame, Some("Push in"))
        })
        .unwrap();
        assert!(dropped(&waiting).contains(&(
            "continuity.end_frame",
            "office_night_03 continues from this shot, but its keyframe is not locked yet"
        )));
        let unfit = compile(VideoPromptInput {
            successor: Some(Successor {
                shot_id: "office_night_03",
                end_frame: EndFrameUse::Unfit("its keyframe is larger than the provider takes"),
            }),
            ..input(&shot, &takes_last_frame, Some("Push in"))
        })
        .unwrap();
        assert!(dropped(&unfit).contains(&(
            "continuity.end_frame",
            "office_night_03 continues from this shot, but its keyframe is larger than the \
             provider takes"
        )));

        // Sent, the clip is told to close on it, and the hash says so.
        let closing = compile(VideoPromptInput {
            successor: Some(Successor {
                shot_id: "office_night_03",
                end_frame: EndFrameUse::Sent,
            }),
            ..input(&shot, &takes_last_frame, Some("Push in"))
        })
        .unwrap();
        assert!(applied(&closing).contains(&"continuity.end_frame"));
        assert!(closing.compiled.prompt().contains(
            "End on the opening frame of office_night_03, which continues from this shot."
        ));
        assert!(
            !dropped(&closing)
                .iter()
                .any(|(constraint, _)| *constraint == "continuity.end_frame")
        );
        assert_ne!(closing.prompt_hash, waiting.prompt_hash);
    }

    #[test]
    fn the_directors_notes_lead_what_moves_and_change_the_hash() {
        let shot = shot();
        let parametric = capabilities(CameraControl::Parametric);
        let notes = ["Hold on his eyes.".to_owned(), "Slower".to_owned()];
        let noted = compile(VideoPromptInput {
            notes: &notes,
            ..input(&shot, &parametric, Some("Push in"))
        })
        .unwrap();
        assert!(
            noted.compiled.prompt().contains(
                "[Push in]. Director's note: Hold on his eyes; Slower. One continuous shot"
            )
        );
        assert!(applied(&noted).contains(&"director.note"));
        let plain = compile(input(&shot, &parametric, Some("Push in"))).unwrap();
        assert!(!applied(&plain).contains(&"director.note"));
        assert_ne!(noted.prompt_hash, plain.prompt_hash);
    }

    #[test]
    fn the_locked_style_leads_and_holds_through_the_clip() {
        let shot = shot();
        let parametric = capabilities(CameraControl::Parametric);
        let noir = "Black-and-white film noir still: hard low-key light.";
        let styled = compile(VideoPromptInput {
            style: Some(noir),
            ..input(&shot, &parametric, Some("Push in"))
        })
        .unwrap();
        assert!(styled.compiled.prompt().starts_with(
            "Black-and-white film noir still: hard low-key light, throughout the clip. A filmmaker"
        ));
        assert!(applied(&styled).contains(&"style.locked"));
        let plain = compile(input(&shot, &parametric, Some("Push in"))).unwrap();
        assert!(!applied(&plain).contains(&"style.locked"));
        assert_ne!(styled.prompt_hash, plain.prompt_hash);
    }

    #[test]
    fn the_directors_preferences_join_the_clip_and_give_way_first() {
        let shot = shot();
        let parametric = capabilities(CameraControl::Parametric);
        let wanted = "The director's preferences, where the locked style allows: prefer slow \
                      dolly camera motion";
        let preferred = compile(VideoPromptInput {
            preferences: Some(wanted),
            ..input(&shot, &parametric, Some("Push in"))
        })
        .unwrap();
        assert!(
            preferred
                .compiled
                .prompt()
                .contains("prefer slow dolly camera motion")
        );
        assert!(applied(&preferred).contains(&"director.preferences"));
        let plain = compile(input(&shot, &parametric, Some("Push in"))).unwrap();
        assert_ne!(preferred.prompt_hash, plain.prompt_hash);

        // Short of room, the preferences go before anything else.
        let mut short = parametric.clone();
        short.max_prompt_chars = Some(preferred.compiled.prompt().chars().count() - 5);
        let cut = compile(VideoPromptInput {
            preferences: Some(wanted),
            ..input(&shot, &short, Some("Push in"))
        })
        .unwrap();
        assert_eq!(dropped(&cut).last().unwrap().0, "director.preferences");
        assert!(cut.compiled.prompt().contains("Mood:"));
    }

    #[test]
    fn the_clip_holds_the_shots_dialogue_or_says_it_cannot() {
        let mut shot = shot();
        shot.dialogue = Some("Was it worth it?".to_owned());
        let listed = capabilities(CameraControl::Parametric);
        let at = |capabilities: &VideoCapabilities, dialogue_ms| {
            compile(VideoPromptInput {
                dialogue_ms,
                ..input(&shot, capabilities, Some("Push in"))
            })
            .unwrap()
        };
        // Four seconds asked for: 6 s holds 3 s of dialogue, not 7 s.
        assert_eq!(at(&listed, Some(3_000)).duration_seconds, 6);
        assert_eq!(at(&listed, Some(7_000)).duration_seconds, 10);
        let long = at(&listed, Some(12_000));
        assert_eq!(long.duration_seconds, 10);
        assert!(
            dropped(&long)
                .iter()
                .any(|(constraint, _)| *constraint == "shot.dialogue_duration")
        );
        assert!(
            !dropped(&at(&listed, Some(7_000)))
                .iter()
                .any(|(constraint, _)| *constraint == "shot.dialogue_duration")
        );

        let ranged = VideoCapabilities {
            clip_seconds: std::collections::BTreeSet::new(),
            min_duration_ms: Some(1_000),
            max_duration_ms: Some(8_000),
            ..listed.clone()
        };
        assert_eq!(clip_seconds(&ranged, Some(4.0), None), Some(4));
        assert_eq!(clip_seconds(&ranged, Some(4.0), Some(5_200)), Some(6));
        assert_eq!(clip_seconds(&ranged, Some(4.0), Some(9_000)), Some(8));
        assert_eq!(clip_seconds(&listed, None, Some(7_000)), Some(10));
    }

    #[test]
    fn optional_parts_are_cut_by_rank_when_the_prompt_is_too_long() {
        let mut shot = shot();
        shot.continuity = vec!["The rain never stops".to_owned()];
        let mut short = capabilities(CameraControl::Parametric);
        let full = compile(input(&shot, &short, Some("Push in"))).unwrap();
        let length = full.compiled.prompt().chars().count();
        // Room for everything but the continuity notes.
        short.max_prompt_chars = Some(length - 5);
        let cut = compile(input(&shot, &short, Some("Push in"))).unwrap();
        assert!(!cut.compiled.prompt().contains("Keep: The rain"));
        assert!(cut.compiled.prompt().contains("Mood:"));
        assert_eq!(
            dropped(&cut).last().unwrap().0,
            "shot.continuity",
            "continuity is cut first"
        );
        short.max_prompt_chars = Some(40);
        assert!(matches!(
            compile(input(&shot, &short, Some("Push in"))),
            Err(KeyframePromptError::TooLong { .. })
        ));
    }

    #[test]
    fn the_hash_follows_the_keyframe_the_clip_and_the_provider() {
        let shot = shot();
        let parametric = capabilities(CameraControl::Parametric);
        let base = compile(input(&shot, &parametric, Some("Push in")))
            .unwrap()
            .prompt_hash;
        let other_keyframe = format!("sha256:{}", "2".repeat(64));
        assert_ne!(
            compile(VideoPromptInput {
                keyframe_hash: &other_keyframe,
                ..input(&shot, &parametric, Some("Push in"))
            })
            .unwrap()
            .prompt_hash,
            base
        );
        let textual = capabilities(CameraControl::Textual);
        assert_ne!(
            compile(input(&shot, &textual, Some("Push in")))
                .unwrap()
                .prompt_hash,
            base
        );
        let mut longer = shot.clone();
        longer.duration_target = Some(10.0);
        let ten = compile(input(&longer, &parametric, Some("Push in"))).unwrap();
        assert_eq!(ten.duration_seconds, 10);
        assert!(ten.compiled.dropped_constraints().is_empty());
        assert_ne!(ten.prompt_hash, base);
        assert_eq!(
            compile(VideoPromptInput {
                shot: None,
                ..input(&shot, &parametric, None)
            }),
            Err(KeyframePromptError::MissingIntent)
        );
    }
}
