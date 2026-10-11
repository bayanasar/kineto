//! The tools agents call: reading the project, and changing it through
//! Kineto's open session (ADR-0010, ADR-0011).

use std::{collections::BTreeSet, io::Cursor, path::Path, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kineto_core::{
    generation::Consent,
    session::{AgentCommand, DocumentCommand, ProductionCommand, SessionRemote, ShotCommand},
    style_prompt::PRESETS,
    workflow::{self, Stage, StepState},
};
use kineto_project::{
    ArtifactStatus,
    bible::{EntryEdit, EntryKind},
    fs::ProjectRelativePath,
    manifest::CanonicalProject,
    revisions::{self, DocumentKind, RevisionStatus},
    screenplay::{Block, SceneEdit},
    shot::{ShotDirection, ShotKey, ShotWorkflow, VideoState},
    shot_plan::{ScenePlan, shots_in_play},
    source::{self, SourceFormat},
};
use serde_json::{Value, json};

use crate::{
    PROTOCOL_VERSIONS,
    desk::{Charge, Desk, Work},
};

/// The most source text one answer carries, in characters.
const MAX_SOURCE_CHARS: usize = 200_000;
/// The longest side of an image an agent is shown, in pixels.
const IMAGE_SIDE: u32 = 1024;

/// What a tool answers.
enum Answer {
    Json(Value),
    /// A JPEG, and what it shows.
    Image(Vec<u8>, Value),
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// The tools, and the project they reach.
pub struct Tools {
    desk: Arc<Desk>,
}

impl Tools {
    #[must_use]
    pub const fn new(desk: Arc<Desk>) -> Self {
        Self { desk }
    }

    /// The answer to one JSON-RPC message from `agent`; `None` for a
    /// notification, or a response to a request this server never makes.
    #[must_use]
    pub fn respond(&self, message: &Value, agent: &str) -> Option<Value> {
        let Some(message) = message.as_object() else {
            return Some(error(
                &Value::Null,
                -32600,
                "one JSON-RPC message, not a batch",
            ));
        };
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // A response: this server sends no requests to answer.
            return id.map(|id| error(&id, -32600, "not a request"));
        };
        let id = id?;
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(error(&id, -32600, "not JSON-RPC 2.0"));
        }
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        Some(match method {
            "initialize" => {
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let version = PROTOCOL_VERSIONS
                    .iter()
                    .find(|version| **version == asked)
                    .unwrap_or(&PROTOCOL_VERSIONS[0]);
                result(
                    &id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {
                            "name": "kineto",
                            "title": "Kineto",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "instructions": INSTRUCTIONS,
                    }),
                )
            }
            "ping" => result(&id, json!({})),
            "tools/list" => result(&id, json!({"tools": tool_list()})),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                if !tool_list()
                    .iter()
                    .any(|tool| tool["name"].as_str() == Some(name))
                {
                    return Some(error(&id, -32602, &format!("no tool named {name:?}")));
                }
                let content = match self.call(name, &arguments, agent) {
                    Ok(Answer::Json(value)) => json!({
                        "content": [{"type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default()}],
                        "isError": false,
                    }),
                    Ok(Answer::Image(jpeg, about)) => json!({
                        "content": [
                            {"type": "image", "data": STANDARD.encode(jpeg), "mimeType": "image/jpeg"},
                            {"type": "text", "text": about.to_string()},
                        ],
                        "isError": false,
                    }),
                    Err(why) => {
                        json!({"content": [{"type": "text", "text": why}], "isError": true})
                    }
                };
                result(&id, content)
            }
            _ => error(&id, -32601, &format!("no method {method:?}")),
        })
    }

    fn open(&self) -> Result<CanonicalProject, String> {
        let path = self
            .desk
            .project()
            .ok_or_else(|| "No project is open in Kineto.".to_owned())?;
        CanonicalProject::open_read_only(&path)
            .map_err(|error| format!("The project cannot be read: {error}"))
    }

    fn call(&self, name: &str, arguments: &Value, agent: &str) -> Result<Answer, String> {
        match name {
            "get_project" => project_overview(&self.open()?).map(Answer::Json),
            "get_story" => story(&self.open()?).map(Answer::Json),
            "get_document" => document(&self.open()?, &text(arguments, "kind")?).map(Answer::Json),
            "get_shot" => shot(
                &self.open()?,
                &text(arguments, "scene")?,
                &text(arguments, "shot")?,
            )
            .map(Answer::Json),
            "get_production" => production(&self.desk.session()?).map(Answer::Json),
            "get_image" => self.image(arguments),
            "get_request" => {
                let id = arguments["request"]
                    .as_u64()
                    .ok_or("Give the request's number.")?;
                self.desk.request(id).map(Answer::Json)
            }
            _ => {
                let session = self.desk.session()?;
                let work = work(&session, name, arguments)?;
                self.desk.run(agent, work).map(Answer::Json)
            }
        }
    }

    fn image(&self, arguments: &Value) -> Result<Answer, String> {
        let (path, about) = if let Some(sample) = arguments["sample"].as_str() {
            let production = published_production(&self.desk.session()?)?;
            let path = production
                .samples
                .iter()
                .find(|(known, _)| known.artifact_id == sample)
                .map(|(_, path)| path.clone())
                .ok_or_else(|| {
                    format!("There is no style sample {sample:?}: get_production lists them.")
                })?;
            (path, json!({"sample": sample}))
        } else if let Some(portrait) = arguments["portrait"].as_str() {
            let production = published_production(&self.desk.session()?)?;
            let path = production
                .characters
                .iter()
                .flat_map(|character| &character.images)
                .find(|(image, _)| image.artifact_id == portrait)
                .map(|(_, path)| path.clone())
                .ok_or_else(|| {
                    format!("There is no portrait {portrait:?}: get_production lists them.")
                })?;
            (path, json!({"portrait": portrait}))
        } else {
            let project = self.open()?;
            let (scene, shot) = (text(arguments, "scene")?, text(arguments, "shot")?);
            let workflow = load_shot(&project, &scene, &shot)?;
            let snapshot = workflow.snapshot().map_err(|error| error.to_string())?;
            let candidate = match arguments["candidate"].as_u64() {
                Some(candidate) => usize::try_from(candidate).map_err(|_| "No such candidate.")?,
                None => snapshot
                    .selected_index
                    .map(usize::from)
                    .ok_or("No keyframe is chosen: give a candidate, as get_shot lists them.")?,
            };
            let media = workflow
                .candidate_media()
                .map_err(|error| error.to_string())?;
            let path = media
                .get(candidate)
                .cloned()
                .flatten()
                .ok_or("That candidate has no image: get_shot lists them.")?;
            (
                project.root().resolve(&path),
                json!({"scene": scene, "shot": shot, "candidate": candidate}),
            )
        };
        Ok(Answer::Image(shrunk(&path)?, about))
    }
}

const INSTRUCTIONS: &str = "Kineto turns a story into a film, step by step: the story, its analysis, a screenplay, a bible of how characters, locations and props look, a shot plan, a style, the cast, then each shot's keyframe and video. These tools read and change the project open in Kineto, through Kineto itself, as the director's own buttons do. Start with get_project. Edits, choices and locks happen at once. A request that pays a service per call (a metered model, images, video) waits until the director allows it in Kineto's window; such a tool answers with a request number, and get_request says how it ended. Locked work stays as it is until the director or a tool makes a new revision.";

/// The tools, as `tools/list` gives them.
fn tool_list() -> Vec<Value> {
    let read = json!({"readOnlyHint": true, "openWorldHint": false});
    let edit = json!({"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false});
    let make = json!({"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true});
    let shot = json!({"scene": {"type": "string"}, "shot": {"type": "string"}});
    let object = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    let with_shot = |extra: Value| {
        let mut properties = shot.clone();
        if let (Some(properties), Some(extra)) = (properties.as_object_mut(), extra.as_object()) {
            properties.extend(extra.clone());
        }
        properties
    };
    let tool = |name: &str, title: &str, description: &str, input: Value, annotations: &Value| json!({"name": name, "title": title, "description": description, "inputSchema": input, "annotations": annotations});
    let revision = json!({"revision": {"type": "integer", "minimum": 1, "description": "The revision to lock; the latest when not given."}});
    let direction =
        json!({"type": "string", "description": "What the new version should do differently."});
    vec![
        tool(
            "get_project",
            "Where the project stands",
            "The project open in Kineto: its title, each step of making the film and where it stands (waiting, ready, awaiting_approval, done, skipped), and each scene's shots with their keyframe and video stage (missing, made, locked, stale).",
            object(json!({}), &[]),
            &read,
        ),
        tool(
            "get_story",
            "The story",
            "The story's source text as the director gave it, and Kineto's analysis of it (characters, locations, events, candidate scenes) when there is one.",
            object(json!({}), &[]),
            &read,
        ),
        tool(
            "get_document",
            "A document",
            "The current revision of one of the project's documents: the locked one, or the latest candidate. screenplay: the scenes, action and dialogue. bible: characters, locations and props as they must look. shot_plan: each scene's shots. preferences: the director's standing preferences.",
            object(
                json!({"kind": {"type": "string", "enum": ["screenplay", "bible", "shot_plan", "preferences"]}}),
                &["kind"],
            ),
            &read,
        ),
        tool(
            "get_shot",
            "A shot",
            "One shot: what it is meant to show, its keyframe candidates and which is chosen or locked, and its video takes, with the files as paths on the director's computer (get_image shows a keyframe). Scene and shot IDs are those get_project lists.",
            object(shot.clone(), &["scene", "shot"]),
            &read,
        ),
        tool(
            "get_production",
            "Style and cast",
            "The style presets, the style samples made and which is locked, and each character to cast with their portrait candidates and which is locked.",
            object(json!({}), &[]),
            &read,
        ),
        tool(
            "get_image",
            "See an image",
            "An image as a JPEG at most 1024 pixels on its longest side: a shot's keyframe candidate (scene, shot and candidate as get_shot lists it; the chosen one when no candidate is given), a style sample (sample), or a portrait (portrait), as get_production lists them.",
            object(
                json!({"scene": {"type": "string"}, "shot": {"type": "string"}, "candidate": {"type": "integer", "minimum": 0}, "sample": {"type": "string"}, "portrait": {"type": "string"}}),
                &[],
            ),
            &read,
        ),
        tool(
            "get_request",
            "How a request ended",
            "Where a request a changing tool answered with stands: awaiting_director, declined, lapsed, running, done or failed (with why).",
            object(
                json!({"request": {"type": "integer", "minimum": 1}}),
                &["request"],
            ),
            &read,
        ),
        tool(
            "import_story",
            "Import the story",
            "Give the story as a new source revision. Work built on the old one becomes stale.",
            object(
                json!({"text": {"type": "string"}, "markdown": {"type": "boolean"}}),
                &["text"],
            ),
            &edit,
        ),
        tool(
            "analyze_story",
            "Analyse the story",
            "Have Kineto's document model analyse the story: characters, locations, events, candidate scenes.",
            object(json!({}), &[]),
            &make,
        ),
        tool(
            "write_screenplay",
            "Write the screenplay",
            "Have the document model write a screenplay from the analysis, as a new candidate revision.",
            object(json!({}), &[]),
            &make,
        ),
        tool(
            "rewrite_scene",
            "Rewrite a scene",
            "Have the document model write one scene of the latest screenplay again, optionally as direction says.",
            object(
                json!({"scene": {"type": "string"}, "direction": direction}),
                &["scene"],
            ),
            &make,
        ),
        tool(
            "edit_scene",
            "Edit a scene",
            "Change one scene of the latest screenplay as a new revision. Fields not given stay as they are. blocks are as get_document gives them.",
            object(
                json!({"scene": {"type": "string"}, "heading": {"type": "string"}, "time": {"type": "string"}, "dramatic_goal": {"type": "string"}, "blocks": {"type": "array", "items": {"type": "object"}}}),
                &["scene"],
            ),
            &edit,
        ),
        tool(
            "lock_screenplay",
            "Lock the screenplay",
            "Lock a screenplay revision, so the bible and the shot plan build on it.",
            object(revision.clone(), &[]),
            &edit,
        ),
        tool(
            "write_bible",
            "Write the bible",
            "Have the document model write the bible of the locked screenplay: how characters, locations and props look.",
            object(json!({}), &[]),
            &make,
        ),
        tool(
            "edit_bible_entry",
            "Edit a bible entry",
            "Change one character, location or prop of the latest bible as a new revision. Fields not given stay as they are; wardrobe is a character's only.",
            object(
                json!({"kind": {"type": "string", "enum": ["character", "location", "prop"]}, "id": {"type": "string"}, "description": {"type": "string"}, "appearance": {"type": "string"}, "wardrobe": {"type": "string"}, "forbidden_changes": {"type": "array", "items": {"type": "string"}}}),
                &["kind", "id"],
            ),
            &edit,
        ),
        tool(
            "lock_bible",
            "Lock the bible",
            "Lock a bible revision, so the shots build on it.",
            object(revision.clone(), &[]),
            &edit,
        ),
        tool(
            "plan_shots",
            "Plan the shots",
            "Have the document model plan the shots of the locked screenplay and bible.",
            object(json!({}), &[]),
            &make,
        ),
        tool(
            "replan_scene",
            "Plan a scene again",
            "Have the document model plan one scene's shots again, optionally as direction says.",
            object(
                json!({"scene": {"type": "string"}, "direction": direction}),
                &["scene"],
            ),
            &make,
        ),
        tool(
            "edit_scene_shots",
            "Edit a scene's shots",
            "Replace one scene's shots in the latest shot plan, as a new revision. plan is the scene as get_document's shot_plan gives it; shots with new IDs are added.",
            object(json!({"plan": {"type": "object"}}), &["plan"]),
            &edit,
        ),
        tool(
            "lock_shot_plan",
            "Lock the shot plan",
            "Lock a shot plan revision, which puts its shots in play.",
            object(revision, &[]),
            &edit,
        ),
        tool(
            "sample_style",
            "Make a style sample",
            "Make one sample image of a style preset (get_production lists them), optionally with a note on how it should differ.",
            object(
                json!({"preset": {"type": "string"}, "note": {"type": "string"}}),
                &["preset"],
            ),
            &make,
        ),
        tool(
            "choose_style",
            "Choose a style sample",
            "Choose a style sample as the one to lock.",
            object(json!({"sample": {"type": "string"}}), &["sample"]),
            &edit,
        ),
        tool(
            "lock_style",
            "Lock the style",
            "Lock a style sample as the film's style.",
            object(json!({"sample": {"type": "string"}}), &["sample"]),
            &edit,
        ),
        tool(
            "audition",
            "Audition a character",
            "Have portrait candidates of a character made, up to count in all.",
            object(
                json!({"character": {"type": "string"}, "count": {"type": "integer", "minimum": 1, "maximum": 8}}),
                &["character", "count"],
            ),
            &make,
        ),
        tool(
            "cast_portrait",
            "Cast a portrait",
            "Choose a character's portrait and lock it as how they look.",
            object(
                json!({"character": {"type": "string"}, "portrait": {"type": "string"}}),
                &["character", "portrait"],
            ),
            &edit,
        ),
        tool(
            "direct_shot",
            "Direct a shot",
            "Say what a shot's keyframe should favour: reaction, spatial_clarity, intimacy or tension.",
            object(
                with_shot(
                    json!({"direction": {"type": "string", "enum": ["reaction", "spatial_clarity", "intimacy", "tension"]}}),
                ),
                &["scene", "shot", "direction"],
            ),
            &edit,
        ),
        tool(
            "make_keyframes",
            "Make keyframes",
            "Make a shot's keyframe candidates, or make them again as note says.",
            object(
                with_shot(json!({"note": {"type": "string"}})),
                &["scene", "shot"],
            ),
            &make,
        ),
        tool(
            "choose_keyframe",
            "Choose a keyframe",
            "Choose one of a shot's keyframe candidates, by its place as get_shot lists it.",
            object(
                with_shot(json!({"candidate": {"type": "integer", "minimum": 0}})),
                &["scene", "shot", "candidate"],
            ),
            &edit,
        ),
        tool(
            "lock_keyframe",
            "Lock a keyframe",
            "Lock a shot's chosen keyframe, which its video starts from.",
            object(shot.clone(), &["scene", "shot"]),
            &edit,
        ),
        tool(
            "make_video",
            "Make a video",
            "Make a shot's video from its locked keyframe. It arrives in the background; get_shot shows it.",
            object(shot.clone(), &["scene", "shot"]),
            &make,
        ),
        tool(
            "choose_video",
            "Choose a video take",
            "Choose one of a shot's latest video takes, by its place as get_shot lists it.",
            object(
                with_shot(json!({"take": {"type": "integer", "minimum": 0}})),
                &["scene", "shot", "take"],
            ),
            &edit,
        ),
        tool(
            "lock_video",
            "Lock a video",
            "Lock a shot's chosen video take.",
            object(shot, &["scene", "shot"]),
            &edit,
        ),
    ]
}

fn text(arguments: &Value, field: &str) -> Result<String, String> {
    arguments[field]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("Give {field}."))
}

fn optional(arguments: &Value, field: &str) -> Option<String> {
    arguments[field]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
}

fn index(arguments: &Value, field: &str) -> Result<usize, String> {
    arguments[field]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("Give {field} as a number from 0."))
}

/// The command tool `name` asks for, with what it does and costs.
fn work(session: &SessionRemote, name: &str, arguments: &Value) -> Result<Work, String> {
    let free = |command: AgentCommand, what: String| Work {
        command,
        what,
        charge: Charge::Free,
    };
    let documents = || {
        session
            .documents()
            .map_err(|_| "Kineto has not finished opening the project.".to_owned())
    };
    let shot = || -> Result<(u32, String), String> {
        let (scene, shot) = (text(arguments, "scene")?, text(arguments, "shot")?);
        let missing = || {
            format!("There is no shot {shot:?} in scene {scene:?} in play: get_project lists them.")
        };
        let key = ShotKey::new(scene.as_str(), shot.as_str()).map_err(|_| missing())?;
        let index = session.shot_index(&key).ok_or_else(missing)?;
        Ok((index, format!("{scene}/{shot}")))
    };
    let document = AgentCommand::Document;
    let production = AgentCommand::Production;
    let on_shot = |shot_index: u32, command: ShotCommand| AgentCommand::Shot {
        shot_index,
        command,
    };
    Ok(match name {
        "import_story" => free(
            document(DocumentCommand::ImportSource {
                text: text(arguments, "text")?,
                format: if arguments["markdown"].as_bool() == Some(true) {
                    SourceFormat::Markdown
                } else {
                    SourceFormat::Text
                },
                original_name: None,
            }),
            "import a new version of the story".to_owned(),
        ),
        "analyze_story" => Work {
            command: document(DocumentCommand::AnalyzeStory),
            what: "analyse the story".to_owned(),
            charge: Desk::model_charge(session, documents()?.analysis_cost),
        },
        "write_screenplay" => Work {
            command: document(DocumentCommand::WriteScreenplay),
            what: "write the screenplay".to_owned(),
            charge: Desk::model_charge(session, documents()?.screenplay_cost),
        },
        "rewrite_scene" => {
            let scene = text(arguments, "scene")?;
            Work {
                what: format!("rewrite scene {scene}"),
                command: document(DocumentCommand::RewriteScene {
                    scene_id: scene,
                    direction: optional(arguments, "direction"),
                }),
                charge: Desk::model_charge(session, None),
            }
        }
        "edit_scene" => {
            let scene_id = text(arguments, "scene")?;
            let documents = documents()?;
            let (based_on, screenplay) = documents
                .screenplay
                .as_ref()
                .ok_or("There is no screenplay yet: write_screenplay makes one.")?;
            let scene = screenplay
                .scenes
                .iter()
                .find(|scene| scene.id == scene_id)
                .ok_or_else(|| format!("The screenplay has no scene {scene_id:?}."))?;
            let blocks = match arguments.get("blocks") {
                Some(blocks) => {
                    serde_json::from_value::<Vec<Block>>(blocks.clone()).map_err(|error| {
                        format!("blocks are not as get_document gives them: {error}")
                    })?
                }
                None => scene.blocks.clone(),
            };
            free(
                document(DocumentCommand::SaveScene {
                    based_on: *based_on,
                    edit: SceneEdit {
                        scene_id: scene_id.clone(),
                        heading: optional(arguments, "heading")
                            .unwrap_or_else(|| scene.heading.clone()),
                        time: optional(arguments, "time").unwrap_or_else(|| scene.time.clone()),
                        dramatic_goal: optional(arguments, "dramatic_goal")
                            .unwrap_or_else(|| scene.dramatic_goal.clone()),
                        blocks,
                    },
                }),
                format!("edit scene {scene_id}"),
            )
        }
        "lock_screenplay" => {
            let revision = latest(
                arguments,
                documents()?
                    .screenplay
                    .as_ref()
                    .map(|(revision, _)| *revision),
                "screenplay",
            )?;
            free(
                document(DocumentCommand::LockScreenplay { revision }),
                format!("lock screenplay revision {revision}"),
            )
        }
        "write_bible" => Work {
            command: document(DocumentCommand::WriteBible),
            what: "write the bible".to_owned(),
            charge: Desk::model_charge(session, documents()?.bible_cost),
        },
        "edit_bible_entry" => {
            let id = text(arguments, "id")?;
            let documents = documents()?;
            let (based_on, bible) = documents
                .bible
                .as_ref()
                .ok_or("There is no bible yet: write_bible makes one.")?;
            let missing = || format!("The bible has no such entry {id:?}.");
            let (kind, description, appearance, wardrobe, forbidden) =
                match text(arguments, "kind")?.as_str() {
                    "character" => {
                        let entry = bible
                            .characters
                            .iter()
                            .find(|entry| entry.id == id)
                            .ok_or_else(missing)?;
                        (
                            EntryKind::Character,
                            &entry.description,
                            &entry.appearance,
                            entry.wardrobe.clone(),
                            &entry.forbidden_changes,
                        )
                    }
                    "location" => {
                        let entry = bible
                            .locations
                            .iter()
                            .find(|entry| entry.id == id)
                            .ok_or_else(missing)?;
                        (
                            EntryKind::Location,
                            &entry.description,
                            &entry.appearance,
                            String::new(),
                            &entry.forbidden_changes,
                        )
                    }
                    "prop" => {
                        let entry = bible
                            .props
                            .iter()
                            .find(|entry| entry.id == id)
                            .ok_or_else(missing)?;
                        (
                            EntryKind::Prop,
                            &entry.description,
                            &entry.appearance,
                            String::new(),
                            &entry.forbidden_changes,
                        )
                    }
                    other => return Err(format!("{other:?} is not character, location or prop.")),
                };
            let forbidden_changes = match arguments.get("forbidden_changes") {
                Some(list) => serde_json::from_value::<Vec<String>>(list.clone())
                    .map_err(|_| "forbidden_changes is a list of text.".to_owned())?,
                None => forbidden.clone(),
            };
            free(
                document(DocumentCommand::SaveBibleEntry {
                    based_on: *based_on,
                    edit: EntryEdit {
                        kind,
                        id: id.clone(),
                        description: optional(arguments, "description")
                            .unwrap_or_else(|| description.clone()),
                        appearance: optional(arguments, "appearance")
                            .unwrap_or_else(|| appearance.clone()),
                        wardrobe: optional(arguments, "wardrobe").unwrap_or(wardrobe),
                        forbidden_changes,
                    },
                }),
                format!("edit the bible's {id}"),
            )
        }
        "lock_bible" => {
            let revision = latest(
                arguments,
                documents()?.bible.as_ref().map(|(revision, _)| *revision),
                "bible",
            )?;
            free(
                document(DocumentCommand::LockBible { revision }),
                format!("lock bible revision {revision}"),
            )
        }
        "plan_shots" => Work {
            command: document(DocumentCommand::PlanShots),
            what: "plan the shots".to_owned(),
            charge: Desk::model_charge(session, documents()?.shot_plan_cost),
        },
        "replan_scene" => {
            let scene = text(arguments, "scene")?;
            Work {
                what: format!("plan scene {scene} again"),
                command: document(DocumentCommand::ReplanScene {
                    scene_id: scene,
                    direction: optional(arguments, "direction"),
                }),
                charge: Desk::model_charge(session, None),
            }
        }
        "edit_scene_shots" => {
            let plan: ScenePlan =
                serde_json::from_value(arguments["plan"].clone()).map_err(|error| {
                    format!("plan is not a scene as get_document's shot_plan gives it: {error}")
                })?;
            let documents = documents()?;
            let (based_on, current) = documents
                .shot_plan
                .as_ref()
                .ok_or("There is no shot plan yet: plan_shots makes one.")?;
            let known: BTreeSet<&str> = current
                .scenes
                .iter()
                .flat_map(|scene| &scene.shots)
                .map(|shot| shot.id.as_str())
                .collect();
            let added = plan
                .shots
                .iter()
                .filter(|shot| !known.contains(shot.id.as_str()))
                .map(|shot| shot.id.clone())
                .collect();
            let what = format!("edit the shots of scene {}", plan.scene_id);
            free(
                document(DocumentCommand::SaveScenePlan {
                    based_on: *based_on,
                    scene: plan,
                    added,
                }),
                what,
            )
        }
        "lock_shot_plan" => {
            let revision = latest(
                arguments,
                documents()?
                    .shot_plan
                    .as_ref()
                    .map(|(revision, _)| *revision),
                "shot plan",
            )?;
            free(
                document(DocumentCommand::LockShotPlan { revision }),
                format!("lock shot plan revision {revision}"),
            )
        }
        "sample_style" => {
            let preset = text(arguments, "preset")?;
            if !PRESETS.iter().any(|known| known.id == preset) {
                return Err(format!(
                    "There is no style preset {preset:?}: get_production lists them."
                ));
            }
            Work {
                what: format!("make a {preset} style sample"),
                command: production(ProductionCommand::SampleStyle {
                    preset,
                    note: optional(arguments, "note"),
                }),
                charge: Desk::image_charge(session),
            }
        }
        "choose_style" => {
            let sample = text(arguments, "sample")?;
            free(
                production(ProductionCommand::SelectStyle(sample.clone())),
                format!("choose style sample {sample}"),
            )
        }
        "lock_style" => {
            let sample = text(arguments, "sample")?;
            free(
                production(ProductionCommand::LockStyle(sample.clone())),
                format!("lock style sample {sample}"),
            )
        }
        "audition" => {
            let character = text(arguments, "character")?;
            let count = arguments["count"]
                .as_u64()
                .and_then(|count| u32::try_from(count).ok())
                .filter(|count| (1..=8).contains(count))
                .ok_or("Give count, from 1 to 8.")?;
            Work {
                what: format!("audition {character}, {count} portraits in all"),
                command: production(ProductionCommand::Audition { character, count }),
                charge: Desk::image_charge(session),
            }
        }
        "cast_portrait" => {
            let (character, portrait) =
                (text(arguments, "character")?, text(arguments, "portrait")?);
            free(
                production(ProductionCommand::CastPortrait {
                    character: character.clone(),
                    portrait,
                }),
                format!("cast {character}"),
            )
        }
        "direct_shot" => {
            let (index, label) = shot()?;
            let direction: ShotDirection = serde_json::from_value(arguments["direction"].clone())
                .map_err(|_| {
                "direction is reaction, spatial_clarity, intimacy or tension.".to_owned()
            })?;
            free(
                on_shot(index, ShotCommand::SetDirection(direction)),
                format!("direct shot {label}"),
            )
        }
        "make_keyframes" => {
            let (index, label) = shot()?;
            let (command, what) = match optional(arguments, "note") {
                Some(note) => (
                    ShotCommand::Refine {
                        note,
                        consent: Consent::default(),
                    },
                    format!("make keyframes of {label} again, as noted"),
                ),
                None => (ShotCommand::Generate, format!("make keyframes of {label}")),
            };
            Work {
                command: on_shot(index, command),
                what,
                charge: Desk::image_charge(session),
            }
        }
        "choose_keyframe" => {
            let (index, label) = shot()?;
            let candidate = index_of(arguments, "candidate")?;
            free(
                on_shot(index, ShotCommand::Select(candidate)),
                format!("choose keyframe {candidate} of {label}"),
            )
        }
        "lock_keyframe" => {
            let (index, label) = shot()?;
            free(
                on_shot(index, ShotCommand::Lock),
                format!("lock the keyframe of {label}"),
            )
        }
        "make_video" => {
            let (index, label) = shot()?;
            Work {
                command: on_shot(index, ShotCommand::GenerateVideo),
                what: format!("make a video of {label}"),
                charge: Desk::video_charge(session, index),
            }
        }
        "choose_video" => {
            let (index, label) = shot()?;
            let take = index_of(arguments, "take")?;
            free(
                on_shot(index, ShotCommand::SelectVideo(take)),
                format!("choose video take {take} of {label}"),
            )
        }
        "lock_video" => {
            let (index, label) = shot()?;
            free(
                on_shot(index, ShotCommand::LockVideo),
                format!("lock the video of {label}"),
            )
        }
        other => return Err(format!("no tool named {other:?}")),
    })
}

fn index_of(arguments: &Value, field: &str) -> Result<usize, String> {
    index(arguments, field)
}

/// The revision `arguments` names, else the latest, `current`.
fn latest(arguments: &Value, current: Option<u32>, document: &str) -> Result<u32, String> {
    match arguments.get("revision").and_then(Value::as_u64) {
        Some(revision) => u32::try_from(revision).map_err(|_| "No such revision.".to_owned()),
        None => current.ok_or_else(|| format!("There is no {document} yet.")),
    }
}

/// `path`'s image as a JPEG at most [`IMAGE_SIDE`] on its longest side.
fn shrunk(path: &Path) -> Result<Vec<u8>, String> {
    let image = image::open(path).map_err(|_| "The image cannot be read.".to_owned())?;
    let image = if image.width().max(image.height()) > IMAGE_SIDE {
        image.resize(
            IMAGE_SIDE,
            IMAGE_SIDE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image
    };
    let mut jpeg = Cursor::new(Vec::new());
    image
        .to_rgb8()
        .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut jpeg, 85,
        ))
        .map_err(|_| "The image cannot be shown.".to_owned())?;
    Ok(jpeg.into_inner())
}

fn published_production(
    session: &SessionRemote,
) -> Result<Arc<kineto_core::session::PublishedProduction>, String> {
    session
        .production()
        .map_err(|_| "Kineto has not finished opening the project.".to_owned())
}

fn production(session: &SessionRemote) -> Result<Value, String> {
    let production = published_production(session)?;
    let path = |path: &Path| path.to_string_lossy().into_owned();
    Ok(json!({
        "style": {
            "presets": PRESETS.iter().map(|preset| json!({"preset": preset.id, "name": preset.name})).collect::<Vec<_>>(),
            "samples": production.samples.iter().map(|(sample, file)| json!({
                "sample": sample.artifact_id,
                "preset": sample.preset,
                "note": sample.note,
                "status": sample.status,
                "file": path(file),
            })).collect::<Vec<_>>(),
            "locked": production.style.as_ref().map(|style| &style.artifact_id),
        },
        "characters": production.characters.iter().map(|character| json!({
            "character": character.id,
            "name": character.name,
            "portraits": character.images.iter().map(|(image, file)| json!({
                "portrait": image.artifact_id,
                "kind": image.kind,
                "status": image.status,
                "file": path(file),
            })).collect::<Vec<_>>(),
            "locked": character.lock.as_ref().map(|lock| &lock.portrait.artifact_id),
        })).collect::<Vec<_>>(),
        "problem": production.damaged,
    }))
}

const fn step_state(state: StepState) -> &'static str {
    match state {
        StepState::Waiting => "waiting",
        StepState::Ready => "ready",
        StepState::AwaitingApproval => "awaiting_approval",
        StepState::Done => "done",
        StepState::Skipped => "skipped",
    }
}

const fn stage(stage: Stage) -> &'static str {
    match stage {
        Stage::Missing => "missing",
        Stage::Made => "made",
        Stage::Locked => "locked",
        Stage::Stale => "stale",
    }
}

const fn revision_status(status: RevisionStatus) -> &'static str {
    match status {
        RevisionStatus::Candidate => "candidate",
        RevisionStatus::Locked => "locked",
        RevisionStatus::Superseded => "superseded",
    }
}

const fn artifact_status(status: ArtifactStatus) -> &'static str {
    match status {
        ArtifactStatus::Draft => "draft",
        ArtifactStatus::Candidate => "candidate",
        ArtifactStatus::Selected => "chosen",
        ArtifactStatus::Locked => "locked",
        ArtifactStatus::Superseded => "superseded",
    }
}

fn project_overview(project: &CanonicalProject) -> Result<Value, String> {
    let steps = workflow::status(project).map_err(|error| error.to_string())?;
    let scenes = workflow::scenes(project).map_err(|error| error.to_string())?;
    Ok(json!({
        "title": project.manifest().title,
        "steps": steps.iter().map(|step| json!({
            "id": step.id,
            "description": step.description,
            "state": step_state(step.state),
            "needs_approval": step.approval,
            "waiting_on": step.waiting_on,
            "changed_since": step.stale,
        })).collect::<Vec<_>>(),
        "scenes": scenes.iter().map(|scene| json!({
            "scene": scene.scene_id,
            "complete": scene.complete(),
            "shots": scene.shots.iter().map(|shot| json!({
                "shot": shot.shot_id,
                "keyframe": stage(shot.keyframe),
                "video": stage(shot.video),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    }))
}

/// The current revision of `kind`, as stored.
fn current(project: &CanonicalProject, kind: DocumentKind) -> Result<Value, String> {
    let Some(record) = revisions::current(project, kind).map_err(|error| error.to_string())? else {
        return Ok(Value::Null);
    };
    let path = ProjectRelativePath::new(&record.path).map_err(|error| error.to_string())?;
    let bytes = project
        .root()
        .read(&path)
        .map_err(|error| error.to_string())?;
    let document: Value =
        serde_json::from_slice(&bytes).map_err(|_| "a document is not JSON".to_owned())?;
    Ok(json!({
        "revision": record.revision,
        "status": revision_status(record.status),
        "document": document,
    }))
}

fn story(project: &CanonicalProject) -> Result<Value, String> {
    let revision = source::current(project).map_err(|error| error.to_string())?;
    let text = source::read(project, &revision).map_err(|error| error.to_string())?;
    let truncated = text.chars().count() > MAX_SOURCE_CHARS;
    Ok(json!({
        "source": {
            "revision": revision.revision,
            "text": text.chars().take(MAX_SOURCE_CHARS).collect::<String>(),
            "truncated": truncated,
        },
        "analysis": current(project, DocumentKind::StoryAnalysis)?,
    }))
}

fn document(project: &CanonicalProject, kind: &str) -> Result<Value, String> {
    let document = match kind {
        "screenplay" => DocumentKind::Screenplay,
        "bible" => DocumentKind::Bible,
        "shot_plan" => DocumentKind::ShotPlan,
        "preferences" => DocumentKind::Preferences,
        other => {
            return Err(format!(
                "{other:?} is not a document: screenplay, bible, shot_plan or preferences"
            ));
        }
    };
    let current = current(project, document)?;
    Ok(if current.is_null() {
        json!({"kind": kind, "revision": null, "note": "There is none yet."})
    } else {
        let mut current = current;
        current["kind"] = json!(kind);
        current
    })
}

fn load_shot(project: &CanonicalProject, scene: &str, shot: &str) -> Result<ShotWorkflow, String> {
    let missing =
        || format!("There is no shot {shot:?} in scene {scene:?}: get_project lists them.");
    let key = ShotKey::new(scene, shot).map_err(|_| missing())?;
    if !shots_in_play(project)
        .map_err(|error| error.to_string())?
        .contains(&key)
    {
        return Err(missing());
    }
    ShotWorkflow::load(project, &key).map_err(|error| error.to_string())
}

fn shot(project: &CanonicalProject, scene: &str, shot: &str) -> Result<Value, String> {
    let workflow = load_shot(project, scene, shot)?;
    let snapshot = workflow.snapshot().map_err(|error| error.to_string())?;
    let absolute =
        |path: &ProjectRelativePath| project.root().resolve(path).to_string_lossy().into_owned();
    let candidates = workflow
        .candidate_media()
        .map_err(|error| error.to_string())?
        .iter()
        .map(|path| path.as_ref().map(absolute))
        .collect::<Vec<_>>();
    let video = workflow.video_media().map_err(|error| error.to_string())?;
    let history = workflow
        .video_history()
        .map_err(|error| error.to_string())?;
    let takes = workflow
        .current_video_ids()
        .iter()
        .enumerate()
        .filter_map(|(take, id)| {
            let record = history.iter().find(|record| &record.artifact_id == id)?;
            Some(json!({
                "take": take,
                "status": artifact_status(record.status),
                "file": record.media_path.as_ref().map(absolute),
            }))
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "scene": scene,
        "shot": shot,
        "intent": workflow.intent_record().map_err(|error| error.to_string())?,
        "keyframes": {
            "generated": snapshot.generated,
            "chosen": snapshot.selected_index,
            "locked": snapshot.locked,
            "stale": snapshot.stale,
            "candidates": candidates,
        },
        "video": {
            "state": match snapshot.video {
                VideoState::None => "none",
                VideoState::Ready => "made",
                VideoState::Locked => "locked",
                VideoState::Stale => "stale",
            },
            "file": video.as_ref().map(absolute),
            "takes": takes,
        },
    }))
}
