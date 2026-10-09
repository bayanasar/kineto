//! Casting: portrait candidates of a character, the reference pack made
//! from the one the director selects, and the lock.
//!
//! Portraits are written from the character's bible entry in the locked
//! style. The pack shows the selected portrait's person, sent as the
//! provider's identity reference, from the angles and in the expressions
//! later shots need; which kinds and frames the pack holds follows what the
//! provider declares, and what it cannot hold is reported. Every image is a
//! paid image (see [`crate::paid_image`]) in its provider's casting
//! partition (`minimax-cast` for MiniMax).

use std::sync::Arc;

use kineto_project::{
    ArtifactDependency, ArtifactId, DependencyImpact, InputHash, InputHashMaterial,
    bible::{self, Bible, Character},
    cast::{
        self as document, CastImage, CastImageKind, CharacterLock, LockRequest, PortraitStatus,
    },
    manifest::CanonicalProject,
    revisions::{self, DependencyRecord, DocumentKind},
    story::CharacterRole,
    style::{self as style_document, StyleLock},
};
use kineto_prompts::{ConstraintId, DegradationLevel, DroppedConstraint};
use kineto_providers::{AspectRatio, ImageCapabilities, Price, ReferenceRole};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use crate::paid_image::{ImageRecovery as CastRecovery, Submission};
use crate::{
    documents::document_error,
    generation::{
        CandidateGenerator, CandidateOwner, CandidateRequest, GeneratedCandidate, GenerationError,
        GenerationHooks, ImageProvider, Prerequisite, ReferenceImage, require_synchronous,
    },
    keyframe_prompt::{KeyframePrompt, KeyframePromptError},
    minimax::MinimaxKeyframes,
    paid_image::{self, Jobs, Ledger},
    plugin::PluginImages,
    style_prompt::{Part, fit, sentence},
};

/// The job partition MiniMax's casting images are recorded under.
pub const CAST_PROVIDER: &str = "minimax-cast";

/// Version of the portrait prompt.
pub const PORTRAIT_COMPILER_VERSION: &str = "cast-portrait/2";

/// Version of the reference prompt.
pub const REFERENCE_COMPILER_VERSION: &str = "cast-reference/1";

/// The provider an imported portrait records: the director brought it.
pub const IMPORT_PROVIDER: &str = "import";

/// What an imported portrait's provenance names in place of a compiler.
pub const IMPORT_VERSION: &str = "import/1";

/// The largest image the director may import, in bytes.
pub const MAX_IMPORT_BYTES: u64 = 20 * 1024 * 1024;

const INTENT_DOMAIN: &[u8] = b"kineto.cast-intent.v1";

const PLAN_SCHEMA_VERSION: u32 = 1;

const SQUARE: AspectRatio = AspectRatio {
    width: 1,
    height: 1,
};

const TALL: AspectRatio = AspectRatio {
    width: 9,
    height: 16,
};

/// A casting look: what sets one actor apart from another, in features a
/// character's description rarely fixes. Never the shape of the face, the
/// nose, colouring, hair, age or build, which the description and its
/// forbidden changes keep. Each portrait candidate is asked for its own, so
/// an audition is of different people to choose between, not one face drawn
/// several times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub id: &'static str,
    pub features: &'static str,
}

/// The looks an audition draws from, in the order a new character's
/// candidates take them.
pub const LOOKS: [Look; 12] = [
    Look {
        id: "hooded",
        features: "a guarded, brooding face, deep-set hooded eyes, heavy straight brows, a thin wide mouth and high, sharp cheekbones",
    },
    Look {
        id: "wide-set",
        features: "a boyish, open face, large wide-set eyes, fine arched brows, full lips and a soft, rounded chin",
    },
    Look {
        id: "downturned",
        features: "a melancholy, sensitive face, narrow downturned eyes, thick low brows, a small tight mouth and a pointed chin",
    },
    Look {
        id: "almond",
        features: "a sly, foxlike face, almond-shaped eyes, faint sparse brows, a wide mouth with a heavy lower lip and a strong, square chin",
    },
    Look {
        id: "heavy-lidded",
        features: "a languid, sardonic face, heavy-lidded eyes, straight brows set close together, thin lips and a cleft chin",
    },
    Look {
        id: "open",
        features: "an earnest, plain face, round open eyes, brows that lift at their inner ends, a short upper lip and prominent ears",
    },
    Look {
        id: "close-set",
        features: "a hard, watchful face, close-set eyes, bushy brows, a broad mouth, flat cheekbones and a long chin",
    },
    Look {
        id: "upturned",
        features: "a warm, mischievous face, upturned eyes creased at the corners, high curved brows, a bow-shaped mouth and a receding chin",
    },
    Look {
        id: "bright",
        features: "a nervous, alert face, small bright eyes under a high forehead, thin angled brows, a crooked mouth and a jutting jaw",
    },
    Look {
        id: "hollow",
        features: "a dreamy, distant face, wide sleepy eyes, a low straight brow line, a full wide mouth and hollow cheeks",
    },
    Look {
        id: "shadowed",
        features: "a tired, intense face, deep-set eyes shadowed beneath, arched expressive brows, narrow lips and a dimpled chin",
    },
    Look {
        id: "uneven",
        features: "a homely, likeable face, slightly uneven eyes, soft rounded brows, a small mouth, a broad forehead and ears set close",
    },
];

fn look_index(id: Option<&str>) -> Option<usize> {
    let id = id?;
    LOOKS.iter().position(|look| look.id == id)
}

/// One image of a reference pack, as the provider can make it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackShot {
    pub kind: CastImageKind,
    pub aspect_ratio: AspectRatio,
    /// Why it is made otherwise than it should be, when it is.
    pub degraded: Option<String>,
}

/// A kind of reference the provider cannot make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackDrop {
    pub kind: CastImageKind,
    pub reason: String,
    pub degradation: DegradationLevel,
}

/// What a reference pack holds with a provider.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PackPlan {
    pub shots: Vec<PackShot>,
    pub dropped: Vec<PackDrop>,
}

/// The frame each kind wants: tall for the whole figure, square otherwise.
const fn wanted_frame(kind: CastImageKind) -> AspectRatio {
    match kind {
        CastImageKind::FullBody | CastImageKind::Costume => TALL,
        _ => SQUARE,
    }
}

const fn ratio(frame: AspectRatio) -> (u16, u16) {
    (frame.width, frame.height)
}

/// What a reference pack holds with a provider of `capabilities`: every
/// kind, in the frame it wants or the nearest the provider takes; nothing
/// when the provider takes no identity reference, since a pack without one
/// would not show the same person.
#[must_use]
pub fn pack_plan(capabilities: &ImageCapabilities) -> PackPlan {
    if capabilities.reference_limit(ReferenceRole::Identity) == 0 {
        return PackPlan {
            shots: Vec::new(),
            dropped: CastImageKind::PACK
                .iter()
                .map(|kind| PackDrop {
                    kind: *kind,
                    reason: "the provider takes no identity reference, so it would not show the \
                             same person"
                        .to_owned(),
                    degradation: DegradationLevel::IdentityAffecting,
                })
                .collect(),
        };
    }
    let declared = &capabilities.aspect_ratios;
    let shots = CastImageKind::PACK
        .iter()
        .map(|kind| {
            let wanted = wanted_frame(*kind);
            if declared.is_empty() || declared.contains(&wanted) {
                return PackShot {
                    kind: *kind,
                    aspect_ratio: wanted,
                    degraded: None,
                };
            }
            let fallback = [SQUARE, TALL]
                .into_iter()
                .find(|frame| declared.contains(frame))
                .or_else(|| declared.iter().next().copied())
                .unwrap_or(wanted);
            PackShot {
                kind: *kind,
                aspect_ratio: fallback,
                degraded: Some(format!(
                    "the provider takes no {}:{} frame; made at {}:{}",
                    ratio(wanted).0,
                    ratio(wanted).1,
                    ratio(fallback).0,
                    ratio(fallback).1
                )),
            }
        })
        .collect();
    PackPlan {
        shots,
        dropped: Vec::new(),
    }
}

/// What a request for the next image of a character is made from.
struct Material {
    character: Character,
    style: StyleLock,
    dependencies: Vec<ArtifactDependency>,
}

impl Material {
    fn gather(project: &CanonicalProject, character_id: &str) -> Result<Self, GenerationError> {
        let record = revisions::current(project, DocumentKind::Bible)
            .map_err(document_error)?
            .ok_or(GenerationError::Prerequisite(Prerequisite::Bible))?;
        let bible: Bible = revisions::read(project, &record).map_err(document_error)?;
        let character = bible
            .character(character_id)
            .cloned()
            .ok_or(GenerationError::Prerequisite(Prerequisite::Character))?;
        let style = style_document::locked(project)
            .map_err(document_error)?
            .ok_or(GenerationError::Prerequisite(Prerequisite::LockedStyle))?;
        let dependencies = current_dependencies(&bible, character_id, &style)
            .ok_or(GenerationError::Prerequisite(Prerequisite::Character))?;
        Ok(Self {
            character,
            style,
            dependencies,
        })
    }
}

/// What casting `character_id` depends on now: its bible entry and the
/// locked style.
fn current_dependencies(
    bible: &Bible,
    character_id: &str,
    style: &StyleLock,
) -> Option<Vec<ArtifactDependency>> {
    Some(vec![
        bible.character_dependency(character_id)?,
        style_document::lock_dependency(style)?,
    ])
}

fn part(constraint: &str, text: String, rank: u8, degradation: DegradationLevel) -> Part {
    Part {
        constraint: constraint.to_owned(),
        text,
        rank,
        degradation,
    }
}

const REQUIRED: u8 = u8::MAX;

/// The style, as every casting image carries it.
fn style_parts(style: &StyleLock) -> Vec<Part> {
    let mut parts = vec![part(
        "style.locked",
        sentence(&style.directives),
        REQUIRED,
        DegradationLevel::Cosmetic,
    )];
    if let Some(note) = &style.note {
        parts.push(part(
            "style.note",
            sentence(&format!("Style note: {note}")),
            REQUIRED,
            DegradationLevel::Cosmetic,
        ));
    }
    parts
}

fn framing(kind: CastImageKind, name: &str) -> String {
    let (view, how) = match kind {
        CastImageKind::Portrait => (
            "Close-up casting portrait of",
            "head and shoulders only, the face filling the upper frame, facing the camera, \
             neutral expression, plain neutral background, soft even light",
        ),
        CastImageKind::Front => (
            "Front view of",
            "the person in the reference image, head and shoulders, looking straight at the \
             camera, neutral expression, plain background",
        ),
        CastImageKind::ProfileLeft => (
            "Left profile of",
            "the person in the reference image, head and shoulders, face turned fully to the \
             left, plain background",
        ),
        CastImageKind::ProfileRight => (
            "Right profile of",
            "the person in the reference image, head and shoulders, face turned fully to the \
             right, plain background",
        ),
        CastImageKind::FullBody => (
            "Full-length view of",
            "the person in the reference image, standing, head to toe in frame, facing the \
             camera, plain background",
        ),
        CastImageKind::Costume => (
            "Costume reference of",
            "the person in the reference image, full length, three-quarter view, the whole \
             costume clearly visible, plain background",
        ),
        CastImageKind::ExpressionSmile => (
            "Close-up of",
            "the person in the reference image, a warm, unguarded smile, plain background",
        ),
        CastImageKind::ExpressionTense => (
            "Close-up of",
            "the person in the reference image, tense and troubled, brows drawn, plain \
             background",
        ),
    };
    sentence(&format!("{view} {name}: {how}"))
}

/// Compile the prompt of a `kind` image of `character` in `style`.
/// `identity` says whether the image is sent with an identity reference;
/// `frame` is why the frame differs from the one wanted, when it does.
pub fn compile(
    character: &Character,
    kind: CastImageKind,
    style: &StyleLock,
    identity: bool,
    frame: Option<&str>,
    capabilities: &ImageCapabilities,
) -> Result<KeyframePrompt, KeyframePromptError> {
    compile_as(character, kind, None, style, identity, frame, capabilities)
}

/// [`compile`], asking for the actor's `look` where one is given.
pub fn compile_as(
    character: &Character,
    kind: CastImageKind,
    look: Option<&Look>,
    style: &StyleLock,
    identity: bool,
    frame: Option<&str>,
    capabilities: &ImageCapabilities,
) -> Result<KeyframePrompt, KeyframePromptError> {
    let portrait = kind == CastImageKind::Portrait;
    // The style first: a long description of a person otherwise reads as a
    // photograph, whatever the style says after it.
    let mut required = style_parts(style);
    required.push(part(
        &format!("cast.frame.{}", kind.as_str()),
        framing(kind, &character.name),
        REQUIRED,
        DegradationLevel::Cosmetic,
    ));
    // Who plays the part comes first after the frame: a provider weighs the
    // start of a prompt most, and without it the candidates are one face.
    if let Some(look) = look {
        required.push(part(
            &format!("cast.look.{}", look.id),
            sentence(&format!("Played by an actor with {}", look.features)),
            REQUIRED,
            DegradationLevel::Cosmetic,
        ));
    }
    if identity {
        required.push(part(
            &format!("identity.{}", character.id),
            sentence("Same face, build and colouring as the reference image"),
            REQUIRED,
            DegradationLevel::IdentityAffecting,
        ));
    }
    // A portrait has only the text to make the person from; a reference has
    // the portrait, and the text only helps.
    let appearance = part(
        "bible.appearance",
        sentence(&character.appearance),
        if portrait { REQUIRED } else { 2 },
        DegradationLevel::Cosmetic,
    );
    let costume = matches!(kind, CastImageKind::FullBody | CastImageKind::Costume);
    let clothes = part(
        "bible.wardrobe",
        sentence(&format!("Wardrobe: {}", character.wardrobe)),
        if costume { REQUIRED } else { 0 },
        DegradationLevel::Cosmetic,
    );
    let mut forbidden = (!character.forbidden_changes.is_empty()).then(|| {
        part(
            "bible.forbidden_changes",
            sentence(&format!(
                "Always: {}",
                character
                    .forbidden_changes
                    .iter()
                    .map(|change| change.trim_end_matches(['.', ' ']))
                    .collect::<Vec<_>>()
                    .join("; ")
            )),
            1,
            if portrait {
                DegradationLevel::IdentityAffecting
            } else {
                DegradationLevel::Cosmetic
            },
        )
    });
    let mut optional = Vec::new();
    // A portrait's look must not win over what the entry keeps: what may
    // never change follows the description at once, before the wardrobe.
    if portrait {
        optional.extend(forbidden.take());
    }
    for piece in [appearance, clothes] {
        if piece.rank == REQUIRED {
            required.push(piece);
        } else {
            optional.push(piece);
        }
    }
    optional.extend(forbidden);
    let mut dropped = Vec::new();
    if let Some(reason) = frame {
        dropped.push(DroppedConstraint {
            constraint: ConstraintId::new(format!("cast.aspect.{}", kind.as_str()))?,
            reason: reason.to_owned(),
            degradation: DegradationLevel::Cosmetic,
        });
    }
    fit(
        &required,
        &optional,
        dropped,
        capabilities.max_prompt_chars,
        if portrait {
            PORTRAIT_COMPILER_VERSION
        } else {
            REFERENCE_COMPILER_VERSION
        },
        &canonical_intent(character, kind, look, style, identity),
    )
}

fn canonical_intent(
    character: &Character,
    kind: CastImageKind,
    look: Option<&Look>,
    style: &StyleLock,
    identity: bool,
) -> Vec<u8> {
    let mut hasher = Sha256::new();
    let mut field = |bytes: &[u8]| {
        hasher.update(
            u64::try_from(bytes.len())
                .expect("length fits in u64")
                .to_be_bytes(),
        );
        hasher.update(bytes);
    };
    field(INTENT_DOMAIN);
    field(&serde_json::to_vec(character).expect("a character always encodes"));
    field(kind.as_str().as_bytes());
    field(&serde_json::to_vec(style).expect("a style lock always encodes"));
    field(&[u8::from(identity)]);
    // Only a portrait has a look, so a reference's intent is as it was.
    if let Some(look) = look {
        field(look.id.as_bytes());
        field(look.features.as_bytes());
    }
    hasher.finalize().to_vec()
}

/// What a casting record needs beyond its receipt, kept before the call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CastPlan {
    schema_version: u32,
    artifact_id: String,
    input_hash: String,
    character_id: String,
    kind: CastImageKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity_reference: Option<String>,
    dependencies: Vec<DependencyRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    look: Option<String>,
}

fn image_of(plan: &CastPlan, generated: GeneratedCandidate) -> Result<CastImage, GenerationError> {
    let output = generated.output;
    if generated.input_hash.as_str() != plan.input_hash
        || output.artifact_id.as_str() != plan.artifact_id
        || plan.schema_version != PLAN_SCHEMA_VERSION
    {
        return Err(GenerationError::Unexpected(
            "a casting image's result belongs to another plan",
        ));
    }
    let media_path = output
        .media_path
        .ok_or(GenerationError::Unexpected("a casting image has no file"))?;
    let provenance = output.provenance.ok_or(GenerationError::Unexpected(
        "a casting image has no prompt provenance",
    ))?;
    Ok(CastImage {
        artifact_id: plan.artifact_id.clone(),
        kind: plan.kind,
        status: (plan.kind == CastImageKind::Portrait).then_some(PortraitStatus::Candidate),
        identity_reference: plan.identity_reference.clone(),
        content_hash: output.content_hash.as_str().to_owned(),
        input_hash: plan.input_hash.clone(),
        provider: output.provider,
        model: output.model,
        media_path: media_path.as_path().to_string_lossy().replace('\\', "/"),
        provenance,
        dependencies: plan.dependencies.clone(),
        media_state: None,
        look: plan.look.clone(),
    })
}

struct Casting;

impl Ledger for Casting {
    type Plan = CastPlan;

    const KIND: &'static str = "cast";

    fn artifact_id(plan: &CastPlan) -> &str {
        &plan.artifact_id
    }

    fn input_hash(plan: &CastPlan) -> &str {
        &plan.input_hash
    }

    fn attach(
        project: &CanonicalProject,
        plan: &CastPlan,
        generated: GeneratedCandidate,
    ) -> Result<(), GenerationError> {
        document::attach(project, &plan.character_id, image_of(plan, generated)?)
            .map(|_| ())
            .map_err(document_error)
    }
}

/// Casting images are numbered per character, so the director's decision to
/// risk paying twice for one character's images retires none of another's.
fn numbered(character_id: &str) -> impl Fn(&str) -> bool + '_ {
    move |artifact_id| document::character_of(artifact_id) == Some(character_id)
}

/// Record casting images an interrupted session left with MiniMax, as a
/// project opens. Never calls MiniMax.
pub fn recover(project: &CanonicalProject) -> Result<CastRecovery, GenerationError> {
    paid_image::recover::<Casting>(project)
}

/// [`recover`], and the images `provider` left when it is a plugin. Makes
/// no new call; the plugin may be asked whether it kept a sent call's
/// answer.
pub fn recover_for(
    project: &CanonicalProject,
    provider: &ImageProvider,
) -> Result<CastRecovery, GenerationError> {
    let mut recovery = recover(project)?;
    if let ImageProvider::Plugin(plugin) = provider {
        recovery.extend(paid_image::recover_with::<Casting, _>(
            project,
            PluginImages::new(project, plugin),
        )?);
    }
    Ok(recovery)
}

/// One image to make: what it shows and in which frame.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shot {
    kind: CastImageKind,
    frame: AspectRatio,
    /// Why the frame differs from the one wanted, when it does.
    degraded: Option<String>,
    /// The actor a portrait candidate asks for.
    look: Option<&'static Look>,
}

/// An image compiled and hashed, before anything is written.
#[derive(Debug, Clone)]
struct Planned {
    prompt: KeyframePrompt,
    input_hash: InputHash,
    dependencies: Vec<ArtifactDependency>,
}

/// Compile `shot` of the character in `material` and derive its input
/// hash: what makes two images the same request.
fn plan_shot<G: CandidateGenerator>(
    material: &Material,
    shot: &Shot,
    identity: Option<&ReferenceImage>,
    generator: &G,
) -> Result<Planned, GenerationError> {
    let capabilities = &generator.capabilities();
    let prompt = compile_as(
        &material.character,
        shot.kind,
        shot.look,
        &material.style,
        identity.is_some(),
        shot.degraded.as_deref(),
        capabilities,
    )
    .map_err(GenerationError::Prompt)?;
    let mut dependencies = material.dependencies.clone();
    if let Some(identity) = identity {
        dependencies.push(ArtifactDependency {
            artifact_id: identity.artifact_id.clone(),
            content_hash: kineto_project::ContentHash::of(&identity.bytes),
            impact: DependencyImpact::Semantic,
            field: Some("identity".to_owned()),
        });
    }
    let params = format!("aspect_ratio={}:{}", shot.frame.width, shot.frame.height);
    let input_hash = InputHash::derive(InputHashMaterial {
        dependencies: &dependencies,
        canonical_intent: &canonical_intent(
            &material.character,
            shot.kind,
            shot.look,
            &material.style,
            identity.is_some(),
        ),
        provider: generator.provider().as_str(),
        model: &generator.model(),
        canonical_params: params.as_bytes(),
        compiler_version: prompt.compiled.compiler_version(),
    });
    Ok(Planned {
        prompt,
        input_hash,
        dependencies,
    })
}

/// Generate each of `images`, a shot as planned, at once, and record them
/// in order. Each is numbered before any call, so one whose call failed
/// leaves its number unused, and the others are still recorded, since they
/// are paid for; its error is returned.
fn make_all<G: CandidateGenerator>(
    project: &CanonicalProject,
    jobs: &mut Jobs<'_, Casting, G>,
    character_id: &str,
    images: Vec<(Shot, Planned)>,
    identity: Option<&ReferenceImage>,
    hooks: &mut dyn GenerationHooks,
) -> Result<Vec<CastImage>, GenerationError> {
    if images.is_empty() {
        return Ok(Vec::new());
    }
    let ids =
        document::next_artifact_ids(project, character_id, images.len()).map_err(document_error)?;
    let calls: Vec<(CastPlan, CandidateRequest)> = images
        .into_iter()
        .zip(ids)
        .map(|((shot, planned), artifact_id)| {
            let plan = CastPlan {
                schema_version: PLAN_SCHEMA_VERSION,
                artifact_id: artifact_id.as_str().to_owned(),
                input_hash: planned.input_hash.as_str().to_owned(),
                character_id: character_id.to_owned(),
                kind: shot.kind,
                identity_reference: identity
                    .map(|identity| identity.artifact_id.as_str().to_owned()),
                dependencies: planned
                    .dependencies
                    .iter()
                    .map(DependencyRecord::from)
                    .collect(),
                look: shot.look.map(|look| look.id.to_owned()),
            };
            let call = CandidateRequest {
                artifact_id,
                input_hash: planned.input_hash,
                owner: CandidateOwner::Character {
                    id: character_id.to_owned(),
                    kind: shot.kind,
                },
                aspect_ratio: shot.frame,
                identities: identity.cloned().into_iter().collect(),
                composition: None,
                style: None,
                prompt: Some(Arc::new(planned.prompt)),
            };
            (plan, call)
        })
        .collect();
    let batch: Vec<(&CastPlan, &CandidateRequest)> =
        calls.iter().map(|(plan, call)| (plan, call)).collect();
    let outcomes = jobs.generate_all(&batch, 0, hooks);
    let recorded = document::images(project, character_id).map_err(document_error)?;
    let mut made = Vec::with_capacity(calls.len());
    let mut failure = None;
    for ((plan, _), outcome) in calls.iter().zip(outcomes) {
        match outcome {
            Ok(()) => made.push(
                recorded
                    .iter()
                    .find(|image| image.artifact_id == plan.artifact_id)
                    .cloned()
                    .ok_or(GenerationError::Unexpected(
                        "a recorded casting image is missing",
                    ))?,
            ),
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
    failure.map_or(Ok(made), Err)
}

/// Have `count` portrait candidates of `character_id` that are not
/// rejected and were made from the character's current bible entry in the
/// locked style, making only those missing. `count` is how many to have,
/// not how many to add: asking again after an interruption pays only for
/// what the interruption left undone, and a director who wants more asks
/// for more. Returns those portraits, oldest first.
pub fn audition(
    project: &CanonicalProject,
    provider: &ImageProvider,
    character_id: &str,
    count: usize,
    submission: Submission,
    hooks: &mut dyn GenerationHooks,
) -> Result<Vec<CastImage>, GenerationError> {
    match provider {
        ImageProvider::LocalDemo => Err(GenerationError::Prerequisite(
            Prerequisite::RealImageProvider,
        )),
        ImageProvider::Minimax(client) => audition_with(
            project,
            MinimaxKeyframes::new(project, client.as_ref()),
            character_id,
            count,
            submission,
            hooks,
        ),
        ImageProvider::Plugin(plugin) => audition_with(
            project,
            PluginImages::new(project, plugin),
            character_id,
            count,
            submission,
            hooks,
        ),
    }
}

/// [`audition`] with an explicit image generator.
pub fn audition_with<G: CandidateGenerator>(
    project: &CanonicalProject,
    adapter: G,
    character_id: &str,
    count: usize,
    submission: Submission,
    hooks: &mut dyn GenerationHooks,
) -> Result<Vec<CastImage>, GenerationError> {
    let material = Material::gather(project, character_id)?;
    if !adapter.can_invoke() {
        return Err(GenerationError::NotConfigured);
    }
    let capabilities = adapter.capabilities();
    require_synchronous(&capabilities)?;
    let frame =
        if capabilities.aspect_ratios.is_empty() || capabilities.aspect_ratios.contains(&SQUARE) {
            SQUARE
        } else {
            *capabilities
                .aspect_ratios
                .iter()
                .next()
                .expect("a non-empty set has a first ratio")
        };
    let degraded = (frame != SQUARE).then(|| {
        format!(
            "the provider takes no 1:1 frame; made at {}:{}",
            frame.width, frame.height
        )
    });
    let shot = |look: &'static Look| Shot {
        kind: CastImageKind::Portrait,
        frame,
        degraded: degraded.clone(),
        look: Some(look),
    };
    // Every look's prompt is compiled, and refused if it cannot fit, before
    // anything is written.
    let planned = LOOKS
        .iter()
        .map(|look| plan_shot(&material, &shot(look), None, &adapter))
        .collect::<Result<Vec<_>, _>>()?;
    // Portraits an interruption left are recorded first, so they count.
    let mut jobs = Jobs::<Casting, _>::open(project, adapter, numbered(character_id), submission)?;
    let images: Vec<CastImage> = document::images(project, character_id)
        .map_err(document_error)?
        .into_iter()
        .filter(|image| image.kind == CastImageKind::Portrait)
        .collect();
    // Any unrejected portrait of this entry in this style counts, whoever
    // made it and however it was asked for: the director is shown that
    // count, and pays for what it lacks.
    let parents: Vec<DependencyRecord> = material
        .dependencies
        .iter()
        .map(DependencyRecord::from)
        .collect();
    let mut portraits: Vec<CastImage> = images
        .iter()
        .filter(|image| {
            image.status != Some(PortraitStatus::Rejected)
                && image.dependencies.len() == parents.len()
                && parents
                    .iter()
                    .all(|parent| image.dependencies.contains(parent))
        })
        .cloned()
        .collect();
    // Each new candidate is another actor: a look none of the portraits kept
    // has, the one this character was least often given first, so neither a
    // kept face nor a rejected one is asked for again while others are left.
    let mut uses = [0_usize; LOOKS.len()];
    for index in images
        .iter()
        .filter_map(|image| look_index(image.look.as_deref()))
    {
        uses[index] += 1;
    }
    let mut kept: Vec<usize> = portraits
        .iter()
        .filter_map(|image| look_index(image.look.as_deref()))
        .collect();
    let mut wanted = Vec::new();
    for _ in 0..count.saturating_sub(portraits.len()) {
        let next = (0..LOOKS.len())
            .min_by_key(|look| (kept.contains(look), uses[*look], *look))
            .expect("there are looks");
        uses[next] += 1;
        kept.push(next);
        wanted.push((shot(&LOOKS[next]), planned[next].clone()));
    }
    // The portraits are drawn at once, not one after another.
    portraits.extend(make_all(
        project,
        &mut jobs,
        character_id,
        wanted,
        None,
        hooks,
    )?);
    Ok(portraits)
}

/// Complete the reference pack of `character_id`'s selected portrait, as
/// [`pack_plan`] says the provider can make it: only the kinds the pack of
/// that portrait lacks for the current bible entry and style are made, so
/// asking again after an interruption pays only for what it left undone.
/// Returns the pack, in plan order.
pub fn make_pack(
    project: &CanonicalProject,
    provider: &ImageProvider,
    character_id: &str,
    submission: Submission,
    hooks: &mut dyn GenerationHooks,
) -> Result<Vec<CastImage>, GenerationError> {
    remake_pack(project, provider, character_id, &[], submission, hooks)
}

/// [`make_pack`] that also makes `remake` again, though the pack holds
/// them: the director's explicit request for other takes of those kinds.
pub fn remake_pack(
    project: &CanonicalProject,
    provider: &ImageProvider,
    character_id: &str,
    remake: &[CastImageKind],
    submission: Submission,
    hooks: &mut dyn GenerationHooks,
) -> Result<Vec<CastImage>, GenerationError> {
    match provider {
        ImageProvider::LocalDemo => Err(GenerationError::Prerequisite(
            Prerequisite::RealImageProvider,
        )),
        ImageProvider::Minimax(client) => remake_pack_with(
            project,
            MinimaxKeyframes::new(project, client.as_ref()),
            character_id,
            remake,
            submission,
            hooks,
        ),
        ImageProvider::Plugin(plugin) => remake_pack_with(
            project,
            PluginImages::new(project, plugin),
            character_id,
            remake,
            submission,
            hooks,
        ),
    }
}

/// [`make_pack`] with an explicit image generator.
pub fn make_pack_with<G: CandidateGenerator>(
    project: &CanonicalProject,
    adapter: G,
    character_id: &str,
    submission: Submission,
    hooks: &mut dyn GenerationHooks,
) -> Result<Vec<CastImage>, GenerationError> {
    remake_pack_with(project, adapter, character_id, &[], submission, hooks)
}

/// [`remake_pack`] with an explicit image generator.
pub fn remake_pack_with<G: CandidateGenerator>(
    project: &CanonicalProject,
    adapter: G,
    character_id: &str,
    remake: &[CastImageKind],
    submission: Submission,
    hooks: &mut dyn GenerationHooks,
) -> Result<Vec<CastImage>, GenerationError> {
    let material = Material::gather(project, character_id)?;
    let portrait = document::selected_portrait(project, character_id)
        .map_err(document_error)?
        .ok_or(GenerationError::Prerequisite(
            Prerequisite::SelectedPortrait,
        ))?;
    if !adapter.can_invoke() {
        return Err(GenerationError::NotConfigured);
    }
    let capabilities = adapter.capabilities();
    require_synchronous(&capabilities)?;
    let plan = pack_plan(&capabilities);
    if plan.shots.is_empty() {
        return Err(GenerationError::Prerequisite(
            Prerequisite::IdentityReference,
        ));
    }
    let bytes = document::read_image(project, &portrait).map_err(document_error)?;
    let format = kineto_provider_minimax::sniff(&bytes);
    let fits = format.is_some_and(|format| capabilities.reference_formats.contains(&format))
        && capabilities
            .max_reference_bytes
            .is_none_or(|max| u64::try_from(bytes.len()).is_ok_and(|len| len <= max));
    if !fits {
        return Err(GenerationError::Prerequisite(
            Prerequisite::IdentityReference,
        ));
    }
    let identity = ReferenceImage {
        artifact_id: ArtifactId::new(portrait.artifact_id.clone())
            .map_err(|_| GenerationError::Unexpected("a portrait has an invalid ID"))?,
        bytes: Arc::from(bytes),
    };
    // Every prompt is compiled, and refused if it cannot fit, before
    // anything is written.
    let shots = plan
        .shots
        .iter()
        .map(|pack_shot| {
            let shot = Shot {
                kind: pack_shot.kind,
                frame: pack_shot.aspect_ratio,
                degraded: pack_shot.degraded.clone(),
                look: None,
            };
            let planned = plan_shot(&material, &shot, Some(&identity), &adapter)?;
            Ok((shot, planned))
        })
        .collect::<Result<Vec<_>, GenerationError>>()?;
    // References an interruption left are recorded first, so they count.
    let mut jobs = Jobs::<Casting, _>::open(project, adapter, numbered(character_id), submission)?;
    let recorded = document::images(project, character_id).map_err(document_error)?;
    let mut pack: Vec<Option<CastImage>> = Vec::with_capacity(shots.len());
    let mut wanted = Vec::new();
    for (shot, planned) in shots {
        let kept = recorded.iter().rev().find(|image| {
            image.kind == shot.kind
                && image.identity_reference.as_deref() == Some(portrait.artifact_id.as_str())
                && image.input_hash == planned.input_hash.as_str()
        });
        match kept {
            Some(image) if !remake.contains(&shot.kind) => pack.push(Some(image.clone())),
            _ => {
                pack.push(None);
                wanted.push((shot, planned));
            }
        }
    }
    // What the pack lacks is drawn at once, in plan order.
    let mut made = make_all(
        project,
        &mut jobs,
        character_id,
        wanted,
        Some(&identity),
        hooks,
    )?
    .into_iter();
    let pack = pack
        .into_iter()
        .map(|image| image.or_else(|| made.next()))
        .collect::<Option<Vec<_>>>()
        .ok_or(GenerationError::Unexpected("a made reference is missing"))?;
    Ok(pack)
}

/// Lock `character_id` as cast with `provider`: its selected portrait and
/// that portrait's pack, which must hold every kind [`pack_plan`] says the
/// provider can make.
pub fn lock(
    project: &CanonicalProject,
    provider: &ImageProvider,
    character_id: &str,
) -> Result<CharacterLock, GenerationError> {
    let material = Material::gather(project, character_id)?;
    let portrait = document::selected_portrait(project, character_id)
        .map_err(document_error)?
        .ok_or(GenerationError::Prerequisite(
            Prerequisite::SelectedPortrait,
        ))?;
    let (provider_name, mechanism) =
        provider
            .identity_mechanism()
            .ok_or(GenerationError::Prerequisite(
                Prerequisite::IdentityReference,
            ))?;
    document::lock(
        project,
        character_id,
        LockRequest {
            name: material.character.name.clone(),
            forbidden_changes: material.character.forbidden_changes.clone(),
            pack: pack_plan(&provider.capabilities())
                .shots
                .iter()
                .map(|shot| shot.kind)
                .collect(),
            provider_reference: document::ProviderReference {
                provider: provider_name.to_owned(),
                mechanism: mechanism.to_owned(),
                role: "identity".to_owned(),
                artifact_id: portrait.artifact_id,
            },
        },
    )
    .map_err(document_error)
}

/// Import `bytes`, an image the director chose, as a portrait candidate of
/// `character_id`: copied into the project, recorded by its hash with the
/// bible entry and locked style it was brought in for, and selected,
/// packed and locked like a generated portrait. Free. The image must be a
/// format and size `capabilities` takes as an identity reference, when it
/// declares one; importing the same image again records nothing new.
pub fn import_portrait(
    project: &CanonicalProject,
    capabilities: &ImageCapabilities,
    character_id: &str,
    bytes: &[u8],
) -> Result<CastImage, GenerationError> {
    let material = Material::gather(project, character_id)?;
    let format = kineto_provider_minimax::sniff(bytes);
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let takes_identity = capabilities.reference_limit(ReferenceRole::Identity) > 0;
    let fits = format
        .is_some_and(|format| !takes_identity || capabilities.reference_formats.contains(&format))
        && size <= MAX_IMPORT_BYTES
        && (!takes_identity
            || capabilities
                .max_reference_bytes
                .is_none_or(|max| size <= max));
    if !fits {
        return Err(GenerationError::Prerequisite(
            Prerequisite::IdentityReference,
        ));
    }
    let content_hash = kineto_project::ContentHash::of(bytes);
    let recorded = document::images(project, character_id).map_err(document_error)?;
    if let Some(image) = recorded.iter().find(|image| {
        image.provider == IMPORT_PROVIDER && image.content_hash == content_hash.as_str()
    }) {
        return Ok(image.clone());
    }
    if project.is_read_only() {
        return Err(document_error(document::CastError::Manifest(
            kineto_project::manifest::ProjectManifestError::ReadOnlyFormatVersion(
                project.manifest().format_version,
            ),
        )));
    }
    let artifact_id = document::next_artifact_id(project, character_id).map_err(document_error)?;
    let extension = match format {
        Some(kineto_providers::ImageFormat::Png) => "png",
        Some(kineto_providers::ImageFormat::Webp) => "webp",
        _ => "jpg",
    };
    let media = document::media_path(
        character_id,
        &artifact_id,
        CastImageKind::Portrait,
        extension,
    )
    .map_err(document_error)?;
    project
        .root()
        .write_atomic(&media, bytes)
        .map_err(|error| document_error(document::CastError::Fs(error)))?;
    let image = CastImage {
        artifact_id: artifact_id.as_str().to_owned(),
        kind: CastImageKind::Portrait,
        status: Some(PortraitStatus::Candidate),
        identity_reference: None,
        content_hash: content_hash.as_str().to_owned(),
        input_hash: content_hash.as_str().to_owned(),
        provider: IMPORT_PROVIDER.to_owned(),
        model: IMPORT_PROVIDER.to_owned(),
        media_path: media.as_path().to_string_lossy().into_owned(),
        provenance: kineto_project::shot::PromptProvenance {
            compiler_version: IMPORT_VERSION.to_owned(),
            compiled_prompt: "Imported by the director.".to_owned(),
            prompt_hash: content_hash.as_str().to_owned(),
            applied_constraints: Vec::new(),
            dropped_constraints: Vec::new(),
        },
        dependencies: material
            .dependencies
            .iter()
            .map(DependencyRecord::from)
            .collect(),
        media_state: None,
        look: None,
    };
    document::attach(project, character_id, image.clone()).map_err(document_error)?;
    Ok(image)
}

/// What makes the cast of `character_id` stale: each parent of its locked
/// portrait (its bible entry, the locked style) that changed, went or came,
/// by artifact ID. Empty when it is fresh or not cast.
pub fn staleness(
    project: &CanonicalProject,
    character_id: &str,
) -> Result<Vec<String>, GenerationError> {
    let Some(lock) = document::locked(project, character_id).map_err(document_error)? else {
        return Ok(Vec::new());
    };
    let bible = revisions::current(project, DocumentKind::Bible)
        .map_err(document_error)?
        .map(|record| revisions::read::<Bible>(project, &record))
        .transpose()
        .map_err(document_error)?;
    let style = style_document::locked(project).map_err(document_error)?;
    let current = match (&bible, &style) {
        (Some(bible), Some(style)) => {
            current_dependencies(bible, character_id, style).unwrap_or_default()
        }
        _ => Vec::new(),
    };
    Ok(document::stale_parents(&lock.dependencies, &current))
}

/// The characters worth casting: those of the bible whose role in the story
/// analysis the locked screenplay was adapted from is not minor, in bible
/// order.
pub fn main_characters(project: &CanonicalProject) -> Result<Vec<String>, GenerationError> {
    let Some(record) = revisions::current(project, DocumentKind::Bible).map_err(document_error)?
    else {
        return Ok(Vec::new());
    };
    let bible: Bible = revisions::read(project, &record).map_err(document_error)?;
    let analysis = bible::material(project)
        .map_err(document_error)?
        .map(|(_, _, _, analysis)| analysis);
    Ok(bible
        .characters
        .iter()
        .filter(|character| {
            analysis.as_ref().is_none_or(|analysis| {
                analysis
                    .characters
                    .iter()
                    .find(|known| known.id == character.id)
                    .is_none_or(|known| known.role != CharacterRole::Minor)
            })
        })
        .map(|character| character.id.clone())
        .collect())
}

/// What `count` portraits cost with `provider`, as it declares; `None` when
/// it cannot cast or does not say.
#[must_use]
pub fn audition_cost(provider: &ImageProvider, count: usize) -> Option<Price> {
    price_of(provider, count)
}

/// How many images a reference pack takes with `provider` and what they
/// cost, as it declares.
#[must_use]
pub fn pack_cost(provider: &ImageProvider) -> Option<(usize, Price)> {
    let count = pack_plan(&provider.capabilities()).shots.len();
    price_of(provider, count).map(|price| (count, price))
}

fn price_of(provider: &ImageProvider, count: usize) -> Option<Price> {
    match provider {
        ImageProvider::Minimax(Some(_)) => {
            provider.capabilities().price_per_image.map(|price| Price {
                amount_micros: price.amount_micros * u64::try_from(count).unwrap_or(u64::MAX),
                ..price
            })
        }
        ImageProvider::Plugin(plugin) => plugin
            .image_capabilities()
            .and_then(|capabilities| capabilities.price_per_image)
            .map(|price| Price {
                amount_micros: price.amount_micros * u64::try_from(count).unwrap_or(u64::MAX),
                ..price
            }),
        ImageProvider::Minimax(None) | ImageProvider::LocalDemo => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kineto_project::bible::SAMPLE_BIBLE_JSON;

    fn filmmaker() -> Character {
        let bible: Bible = serde_json::from_str(SAMPLE_BIBLE_JSON).unwrap();
        bible.character("filmmaker").unwrap().clone()
    }

    fn noir() -> StyleLock {
        StyleLock {
            schema_version: 1,
            artifact_id: "style_s0002".to_owned(),
            content_hash: format!("sha256:{}", "c".repeat(64)),
            preset: "film_noir".to_owned(),
            preset_version: 1,
            directives: "Black-and-white film noir still".to_owned(),
            note: Some("colder".to_owned()),
        }
    }

    fn minimax() -> ImageCapabilities {
        kineto_provider_minimax::image_capabilities()
    }

    fn ids(prompt: &KeyframePrompt) -> Vec<&str> {
        prompt
            .compiled
            .applied_constraints()
            .iter()
            .map(ConstraintId::as_str)
            .collect()
    }

    #[test]
    fn a_portrait_is_made_from_the_entry_in_the_locked_style() {
        let prompt = compile(
            &filmmaker(),
            CastImageKind::Portrait,
            &noir(),
            false,
            None,
            &minimax(),
        )
        .unwrap();
        let text = prompt.compiled.prompt();
        assert!(text.starts_with(
            "Black-and-white film noir still. Style note: colder. Close-up casting portrait of The \
             Filmmaker: head and shoulders only"
        ));
        assert!(text.contains("A man in his late forties, lean, unshaven, grey at the temples"));
        assert!(text.contains("Wardrobe: A dark wool coat over a black sweater."));
        assert!(text.contains("Always: Grey at the temples; Never without the coat."));
        assert!(!text.contains("  "));
        assert_eq!(
            prompt.compiled.compiler_version(),
            PORTRAIT_COMPILER_VERSION
        );
        assert_eq!(
            ids(&prompt),
            [
                "style.locked",
                "style.note",
                "cast.frame.portrait",
                "bible.appearance",
                "bible.forbidden_changes",
                "bible.wardrobe"
            ]
        );

        let reference = compile(
            &filmmaker(),
            CastImageKind::ProfileLeft,
            &noir(),
            true,
            None,
            &minimax(),
        )
        .unwrap();
        assert!(
            reference.compiled.prompt().contains(
                "colder. Left profile of The Filmmaker: the person in the reference image"
            )
        );
        assert!(ids(&reference).contains(&"identity.filmmaker"));
        assert!(
            reference
                .compiled
                .prompt()
                .contains("Same face, build and colouring as the reference image.")
        );
        assert_eq!(
            reference.compiled.compiler_version(),
            REFERENCE_COMPILER_VERSION
        );
    }

    #[test]
    fn each_look_asks_for_another_actor_after_the_description() {
        let prompt = |look| {
            compile_as(
                &filmmaker(),
                CastImageKind::Portrait,
                Some(look),
                &noir(),
                false,
                None,
                &minimax(),
            )
            .unwrap()
        };
        let first = prompt(&LOOKS[0]);
        let text = first.compiled.prompt();
        assert!(text.contains(
            "plain neutral background, soft even light. Played by an actor with a guarded, \
             brooding face, deep-set hooded eyes, heavy straight brows, a thin wide mouth and \
             high, sharp cheekbones. A man in his late forties"
        ));
        assert!(ids(&first).contains(&"cast.look.hooded"));
        let mut seen = std::collections::BTreeSet::new();
        for look in &LOOKS {
            let made = prompt(look);
            assert!(
                seen.insert(made.compiled.prompt().to_owned()),
                "{}",
                look.id
            );
            assert!(seen.len() <= LOOKS.len());
        }
        let ids: std::collections::BTreeSet<_> = LOOKS.iter().map(|look| look.id).collect();
        assert_eq!(ids.len(), LOOKS.len(), "every look is told apart");
        // A look never names what a description keeps.
        for look in &LOOKS {
            for kept in [
                "nose", "hair", "skin", "oval", "age", "old", "young", "tall", "scar", "beard",
            ] {
                assert!(
                    !look.features.split([' ', ',']).any(|word| word == kept),
                    "{} names {kept}",
                    look.id
                );
            }
        }
    }

    #[test]
    fn what_does_not_fit_is_dropped_and_said() {
        let mut character = filmmaker();
        character.wardrobe = "A long coat. ".repeat(40);
        let tight = ImageCapabilities {
            max_prompt_chars: Some(400),
            ..minimax()
        };
        let prompt = compile(
            &character,
            CastImageKind::Front,
            &noir(),
            true,
            None,
            &tight,
        )
        .unwrap();
        let dropped: Vec<_> = prompt
            .compiled
            .dropped_constraints()
            .iter()
            .map(|dropped| (dropped.constraint.as_str(), dropped.degradation))
            .collect();
        assert_eq!(dropped[0], ("bible.wardrobe", DegradationLevel::Cosmetic));
        // The costume itself is never dropped from a costume reference.
        assert!(matches!(
            compile(
                &character,
                CastImageKind::Costume,
                &noir(),
                true,
                None,
                &tight
            ),
            Err(KeyframePromptError::TooLong { max: 400, .. })
        ));
    }

    #[test]
    fn the_pack_follows_what_the_provider_declares() {
        let plan = pack_plan(&minimax());
        assert_eq!(plan.shots.len(), 7);
        assert!(plan.dropped.is_empty());
        let full_body = plan
            .shots
            .iter()
            .find(|shot| shot.kind == CastImageKind::FullBody)
            .unwrap();
        assert_eq!(full_body.aspect_ratio, TALL);
        assert_eq!(full_body.degraded, None);

        let square_only = ImageCapabilities {
            aspect_ratios: [SQUARE].into_iter().collect(),
            ..minimax()
        };
        let plan = pack_plan(&square_only);
        let costume = plan
            .shots
            .iter()
            .find(|shot| shot.kind == CastImageKind::Costume)
            .unwrap();
        assert_eq!(costume.aspect_ratio, SQUARE);
        assert_eq!(
            costume.degraded.as_deref(),
            Some("the provider takes no 9:16 frame; made at 1:1")
        );

        let text_only = pack_plan(&ImageCapabilities::default());
        assert!(text_only.shots.is_empty());
        assert_eq!(text_only.dropped.len(), 7);
        assert!(
            text_only
                .dropped
                .iter()
                .all(|drop| drop.degradation == DegradationLevel::IdentityAffecting)
        );
    }
}
