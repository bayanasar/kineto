//! Style exploration: paid sample images of style directions.
//!
//! Each sample is one paid image (see [`crate::paid_image`]) in the
//! `minimax-style` partition, recorded in `style/samples.json` with what its
//! plan kept: preset, note, scene and dependencies.

use std::sync::Arc;

use kineto_project::{
    DependencyImpact, InputHash, InputHashMaterial,
    bible::{self, Bible},
    manifest::CanonicalProject,
    revisions::{self, DependencyRecord, DocumentKind},
    style::{self as document, SampleStatus, StyleSample},
};
use kineto_provider_minimax::{IMAGE_MODEL, PROVIDER, Transport};
use kineto_providers::Price;
use serde::{Deserialize, Serialize};

pub use crate::paid_image::{ImageRecovery as StyleRecovery, Submission};
use crate::{
    documents::document_error,
    generation::{
        CandidateGenerator, CandidateOwner, CandidateRequest, GeneratedCandidate, GenerationError,
        GenerationHooks, ImageProvider, KEYFRAME_ASPECT, Prerequisite, require_synchronous,
    },
    minimax::MinimaxKeyframes,
    paid_image::{self, Jobs, Ledger},
    style_prompt::{self, StylePreset},
};

/// The job partition style samples are recorded under.
pub const STYLE_PROVIDER: &str = "minimax-style";

/// The aspect ratio samples are generated at, as keyframes are.
const SAMPLE_PARAMS: &str = "aspect_ratio=16:9";

const PLAN_SCHEMA_VERSION: u32 = 1;

/// What a sample record needs beyond its receipt, kept before the call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SamplePlan {
    schema_version: u32,
    artifact_id: String,
    input_hash: String,
    preset: String,
    preset_version: u32,
    directives: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refines: Option<String>,
    scene_id: String,
    dependencies: Vec<DependencyRecord>,
}

/// What to sample.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SampleRequest<'a> {
    /// A [`style_prompt::PRESETS`] ID.
    pub preset: &'a str,
    pub note: Option<&'a str>,
    /// The scene to show; the representative scene when `None`.
    pub scene_id: Option<&'a str>,
}

/// The record of a sample from its plan and the generated image.
fn sample_of(
    plan: &SamplePlan,
    generated: GeneratedCandidate,
) -> Result<StyleSample, GenerationError> {
    let output = generated.output;
    if generated.input_hash.as_str() != plan.input_hash
        || output.artifact_id.as_str() != plan.artifact_id
        || plan.schema_version != PLAN_SCHEMA_VERSION
    {
        return Err(GenerationError::Unexpected(
            "a style sample's result belongs to another plan",
        ));
    }
    let media_path = output
        .media_path
        .ok_or(GenerationError::Unexpected("a style sample has no image"))?;
    let provenance = output.provenance.ok_or(GenerationError::Unexpected(
        "a style sample has no prompt provenance",
    ))?;
    Ok(StyleSample {
        artifact_id: plan.artifact_id.clone(),
        preset: plan.preset.clone(),
        preset_version: plan.preset_version,
        directives: plan.directives.clone(),
        note: plan.note.clone(),
        refines: plan.refines.clone(),
        scene_id: plan.scene_id.clone(),
        status: SampleStatus::Candidate,
        content_hash: output.content_hash.as_str().to_owned(),
        input_hash: plan.input_hash.clone(),
        provider: output.provider,
        model: output.model,
        media_path: media_path.as_path().to_string_lossy().replace('\\', "/"),
        provenance,
        dependencies: plan.dependencies.clone(),
    })
}

struct Samples;

impl Ledger for Samples {
    type Plan = SamplePlan;

    const PARTITION: &'static str = STYLE_PROVIDER;
    const PLAN: &'static str = "style";

    fn artifact_id(plan: &SamplePlan) -> &str {
        &plan.artifact_id
    }

    fn input_hash(plan: &SamplePlan) -> &str {
        &plan.input_hash
    }

    fn attach(
        project: &CanonicalProject,
        plan: &SamplePlan,
        generated: GeneratedCandidate,
    ) -> Result<(), GenerationError> {
        document::attach(project, sample_of(plan, generated)?)
            .map(|_| ())
            .map_err(document_error)
    }
}

/// Complete style samples an interrupted session left, as a project opens.
/// Never calls MiniMax.
pub fn recover(project: &CanonicalProject) -> Result<StyleRecovery, GenerationError> {
    paid_image::recover::<Samples>(project)
}

/// What one sample, one call, costs with `provider`, as it declares; `None`
/// when it cannot sample or does not say.
#[must_use]
pub fn sample_cost(provider: &ImageProvider) -> Option<Price> {
    match provider {
        ImageProvider::Minimax(Some(_)) => provider.capabilities().price_per_image,
        ImageProvider::Minimax(None) | ImageProvider::LocalDemo => None,
    }
}

/// Generate a sample of a style direction with the session's image
/// provider and record it as a candidate.
pub fn sample(
    project: &CanonicalProject,
    provider: &ImageProvider,
    request: SampleRequest<'_>,
    submission: Submission,
    hooks: &mut dyn GenerationHooks,
) -> Result<StyleSample, GenerationError> {
    match provider {
        ImageProvider::LocalDemo => Err(GenerationError::Prerequisite(
            Prerequisite::RealImageProvider,
        )),
        ImageProvider::Minimax(client) => sample_with(
            project,
            MinimaxKeyframes::new(project, client.as_ref()),
            request,
            submission,
            hooks,
        ),
    }
}

/// [`sample`] with an explicit MiniMax adapter.
pub fn sample_with<T: Transport + Sync>(
    project: &CanonicalProject,
    adapter: MinimaxKeyframes<'_, T>,
    request: SampleRequest<'_>,
    submission: Submission,
    hooks: &mut dyn GenerationHooks,
) -> Result<StyleSample, GenerationError> {
    let preset = style_prompt::preset(request.preset)
        .ok_or(GenerationError::Prerequisite(Prerequisite::StylePreset))?;
    ask(
        project,
        adapter,
        preset,
        request.note,
        None,
        request.scene_id,
        submission,
        hooks,
    )
}

/// Sample the direction of `artifact_id` again, as `note` asks, on the same
/// scene. The new sample records the one it refines.
pub fn refine(
    project: &CanonicalProject,
    provider: &ImageProvider,
    artifact_id: &str,
    note: &str,
    submission: Submission,
    hooks: &mut dyn GenerationHooks,
) -> Result<StyleSample, GenerationError> {
    match provider {
        ImageProvider::LocalDemo => Err(GenerationError::Prerequisite(
            Prerequisite::RealImageProvider,
        )),
        ImageProvider::Minimax(client) => refine_with(
            project,
            MinimaxKeyframes::new(project, client.as_ref()),
            artifact_id,
            note,
            submission,
            hooks,
        ),
    }
}

/// [`refine`] with an explicit MiniMax adapter.
pub fn refine_with<T: Transport + Sync>(
    project: &CanonicalProject,
    adapter: MinimaxKeyframes<'_, T>,
    artifact_id: &str,
    note: &str,
    submission: Submission,
    hooks: &mut dyn GenerationHooks,
) -> Result<StyleSample, GenerationError> {
    let earlier = document::samples(project)
        .map_err(document_error)?
        .into_iter()
        .find(|sample| sample.artifact_id == artifact_id)
        .ok_or(GenerationError::Prerequisite(Prerequisite::StyleSample))?;
    let preset = style_prompt::preset(&earlier.preset)
        .ok_or(GenerationError::Prerequisite(Prerequisite::StylePreset))?;
    ask(
        project,
        adapter,
        preset,
        Some(note),
        Some(earlier.artifact_id.as_str()),
        Some(earlier.scene_id.as_str()),
        submission,
        hooks,
    )
}

#[allow(clippy::too_many_arguments)]
fn ask<T: Transport + Sync>(
    project: &CanonicalProject,
    adapter: MinimaxKeyframes<'_, T>,
    preset: &StylePreset,
    note: Option<&str>,
    refines: Option<&str>,
    scene_id: Option<&str>,
    submission: Submission,
    hooks: &mut dyn GenerationHooks,
) -> Result<StyleSample, GenerationError> {
    let (_, screenplay) = bible::locked_screenplay(project)
        .map_err(document_error)?
        .ok_or(GenerationError::Prerequisite(
            Prerequisite::LockedScreenplay,
        ))?;
    let scene = match scene_id {
        Some(id) => screenplay
            .scene(id)
            .ok_or(GenerationError::Prerequisite(Prerequisite::Scene))?,
        None => style_prompt::representative_scene(&screenplay)
            .ok_or(GenerationError::Prerequisite(Prerequisite::Scene))?,
    };
    let bible_record = revisions::current(project, DocumentKind::Bible).map_err(document_error)?;
    let bible: Option<Bible> = bible_record
        .as_ref()
        .map(|record| revisions::read(project, record))
        .transpose()
        .map_err(document_error)?;
    // Refuse before writing anything.
    if !adapter.can_invoke() {
        return Err(GenerationError::NotConfigured);
    }
    require_synchronous(&adapter.capabilities())?;
    let note = note.map(str::trim).filter(|note| !note.is_empty());
    let prompt = style_prompt::compile(
        preset,
        note,
        scene,
        &screenplay,
        bible.as_ref(),
        &adapter.capabilities(),
    )
    .map_err(GenerationError::Prompt)?;
    let mut dependencies = vec![scene.dependency().map_err(document_error)?];
    if let Some(record) = &bible_record {
        dependencies.push(
            revisions::dependency_on(record, DependencyImpact::Semantic).map_err(document_error)?,
        );
    }
    let input_hash = InputHash::derive(InputHashMaterial {
        dependencies: &dependencies,
        canonical_intent: &style_prompt::canonical_intent(preset, note, scene, bible.as_ref()),
        provider: PROVIDER,
        model: IMAGE_MODEL,
        canonical_params: SAMPLE_PARAMS.as_bytes(),
        compiler_version: style_prompt::STYLE_COMPILER_VERSION,
    });

    // A sample an interruption left is recorded first, so it keeps its ID.
    // Every sample shares one numbering.
    let mut jobs = Jobs::<Samples, _>::open(project, adapter, |_| true, submission)?;
    let artifact_id = document::next_artifact_id(project).map_err(document_error)?;
    let plan = SamplePlan {
        schema_version: PLAN_SCHEMA_VERSION,
        artifact_id: artifact_id.as_str().to_owned(),
        input_hash: input_hash.as_str().to_owned(),
        preset: preset.id.to_owned(),
        preset_version: preset.version,
        directives: preset.directives.to_owned(),
        note: note.map(str::to_owned),
        refines: refines.map(str::to_owned),
        scene_id: scene.id.clone(),
        dependencies: dependencies.iter().map(DependencyRecord::from).collect(),
    };
    let call = CandidateRequest {
        artifact_id: artifact_id.clone(),
        input_hash,
        owner: CandidateOwner::Style,
        aspect_ratio: KEYFRAME_ASPECT,
        identities: Vec::new(),
        composition: None,
        prompt: Some(Arc::new(prompt)),
    };
    jobs.generate(&plan, &call, 0, hooks)?;
    document::samples(project)
        .map_err(document_error)?
        .into_iter()
        .find(|sample| sample.artifact_id == artifact_id.as_str())
        .ok_or(GenerationError::Unexpected("a recorded sample is missing"))
}
