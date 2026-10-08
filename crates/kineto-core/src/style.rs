//! Style exploration: paid sample images of style directions.
//!
//! Each sample is one paid image (see [`crate::paid_image`]) in its
//! provider's style partition (`minimax-style` for MiniMax), recorded in
//! `style/samples.json` with what its plan kept: preset, note, scene and
//! dependencies.
//!
//! The director can also import an image they already have, a mood board or
//! a still, as a sample ([`import_sample`]), with their own description of
//! its style. Selected and locked like any sample, its description is what
//! prompts follow, and the image itself goes with keyframe requests as a
//! style reference to a provider that takes one.

use std::sync::Arc;

use kineto_project::{
    DependencyImpact, InputHash, InputHashMaterial,
    bible::{self, Bible},
    manifest::CanonicalProject,
    revisions::{self, DependencyRecord, DocumentKind},
    style::{self as document, SampleStatus, StyleSample},
};
use kineto_providers::{ImageCapabilities, ImageFormat, Price, ReferenceRole};
use serde::{Deserialize, Serialize};

pub use crate::paid_image::{ImageRecovery as StyleRecovery, Submission};
use crate::{
    documents::document_error,
    generation::{
        CandidateGenerator, CandidateOwner, CandidateRequest, GeneratedCandidate, GenerationError,
        GenerationHooks, ImageProvider, KEYFRAME_ASPECT, Prerequisite, require_frame,
        require_synchronous,
    },
    minimax::MinimaxKeyframes,
    paid_image::{self, Jobs, Ledger},
    plugin::PluginImages,
    style_prompt::{self, StylePreset},
};

/// The job partition MiniMax's style samples are recorded under.
pub const STYLE_PROVIDER: &str = "minimax-style";

/// The provider an imported sample records: the director brought it.
pub const IMPORT_PROVIDER: &str = "import";

/// The preset an imported sample records: none, its look is its image and
/// the director's description.
pub const IMPORT_PRESET: &str = "imported";

/// What an imported sample's provenance names in place of a compiler.
pub const IMPORT_VERSION: &str = "import/1";

/// The largest image the director may import as a style, in bytes.
pub const MAX_IMPORT_BYTES: u64 = 20 * 1024 * 1024;

/// The longest description of an imported style, in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 2_000;

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
        media_state: None,
    })
}

struct Samples;

impl Ledger for Samples {
    type Plan = SamplePlan;

    const KIND: &'static str = "style";

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

/// Complete style samples an interrupted session left with MiniMax, as a
/// project opens. Never calls MiniMax.
pub fn recover(project: &CanonicalProject) -> Result<StyleRecovery, GenerationError> {
    paid_image::recover::<Samples>(project)
}

/// [`recover`], and the samples `provider` left when it is a plugin. Makes
/// no new call; the plugin may be asked whether it kept a sent call's
/// answer.
pub fn recover_for(
    project: &CanonicalProject,
    provider: &ImageProvider,
) -> Result<StyleRecovery, GenerationError> {
    let mut recovery = recover(project)?;
    if let ImageProvider::Plugin(plugin) = provider {
        recovery.extend(paid_image::recover_with::<Samples, _>(
            project,
            PluginImages::new(project, plugin),
        )?);
    }
    Ok(recovery)
}

/// What one sample, one call, costs with `provider`, as it declares; `None`
/// when it cannot sample or does not say.
#[must_use]
pub fn sample_cost(provider: &ImageProvider) -> Option<Price> {
    match provider {
        ImageProvider::Minimax(Some(_)) => provider.capabilities().price_per_image,
        ImageProvider::Plugin(plugin) => plugin
            .image_capabilities()
            .and_then(|capabilities| capabilities.price_per_image),
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
        ImageProvider::Plugin(plugin) => sample_with(
            project,
            PluginImages::new(project, plugin),
            request,
            submission,
            hooks,
        ),
    }
}

/// [`sample`] with an explicit image generator.
pub fn sample_with<G: CandidateGenerator>(
    project: &CanonicalProject,
    adapter: G,
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
        ImageProvider::Plugin(plugin) => refine_with(
            project,
            PluginImages::new(project, plugin),
            artifact_id,
            note,
            submission,
            hooks,
        ),
    }
}

/// [`refine`] with an explicit image generator.
pub fn refine_with<G: CandidateGenerator>(
    project: &CanonicalProject,
    adapter: G,
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
fn ask<G: CandidateGenerator>(
    project: &CanonicalProject,
    adapter: G,
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
    require_frame(&adapter.capabilities(), KEYFRAME_ASPECT)?;
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
        provider: adapter.provider().as_str(),
        model: &adapter.model(),
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
        style: None,
        prompt: Some(Arc::new(prompt)),
    };
    jobs.generate(&plan, &call, 0, hooks)?;
    document::samples(project)
        .map_err(document_error)?
        .into_iter()
        .find(|sample| sample.artifact_id == artifact_id.as_str())
        .ok_or(GenerationError::Unexpected("a recorded sample is missing"))
}

/// Import `bytes`, an image the director chose, as a style sample described
/// by `description`: copied into the project by its hash and recorded as a
/// candidate, to be selected and locked like any sample. Free, and needing
/// nothing locked first. The image must be a JPEG, PNG or WebP of at most
/// 20 MB, and a format and size `capabilities` takes as a style reference
/// when it takes one; the description must say something. Importing the
/// same image with the same description again records nothing new.
pub fn import_sample(
    project: &CanonicalProject,
    capabilities: &ImageCapabilities,
    bytes: &[u8],
    description: &str,
) -> Result<StyleSample, GenerationError> {
    let description = description.trim();
    if description.is_empty() || description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(GenerationError::Prerequisite(
            Prerequisite::StyleDescription,
        ));
    }
    let format = kineto_provider_minimax::sniff(bytes);
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let takes_style = capabilities.reference_limit(ReferenceRole::Style) > 0;
    let fits = format
        .is_some_and(|format| !takes_style || capabilities.reference_formats.contains(&format))
        && size <= MAX_IMPORT_BYTES
        && (!takes_style
            || capabilities
                .max_reference_bytes
                .is_none_or(|max| size <= max));
    if !fits {
        return Err(GenerationError::Prerequisite(Prerequisite::StyleReference));
    }
    let content_hash = kineto_project::ContentHash::of(bytes);
    if let Some(sample) = document::samples(project)
        .map_err(document_error)?
        .into_iter()
        .find(|sample| {
            sample.provider == IMPORT_PROVIDER
                && sample.content_hash == content_hash.as_str()
                && sample.directives == description
        })
    {
        return Ok(sample);
    }
    if project.is_read_only() {
        return Err(document_error(document::StyleError::Manifest(
            kineto_project::manifest::ProjectManifestError::ReadOnlyFormatVersion(
                project.manifest().format_version,
            ),
        )));
    }
    let artifact_id = document::next_artifact_id(project).map_err(document_error)?;
    let extension = match format {
        Some(ImageFormat::Png) => "png",
        Some(ImageFormat::Webp) => "webp",
        _ => "jpg",
    };
    let media = document::sample_media_path(&artifact_id, extension).map_err(document_error)?;
    project
        .root()
        .write_atomic(&media, bytes)
        .map_err(|error| document_error(document::StyleError::Fs(error)))?;
    let sample = StyleSample {
        artifact_id: artifact_id.as_str().to_owned(),
        preset: IMPORT_PRESET.to_owned(),
        preset_version: 1,
        directives: description.to_owned(),
        note: None,
        refines: None,
        scene_id: String::new(),
        status: SampleStatus::Candidate,
        content_hash: content_hash.as_str().to_owned(),
        input_hash: content_hash.as_str().to_owned(),
        provider: IMPORT_PROVIDER.to_owned(),
        model: IMPORT_PROVIDER.to_owned(),
        media_path: media.as_path().to_string_lossy().replace('\\', "/"),
        provenance: kineto_project::shot::PromptProvenance {
            compiler_version: IMPORT_VERSION.to_owned(),
            compiled_prompt: "Imported by the director.".to_owned(),
            prompt_hash: content_hash.as_str().to_owned(),
            applied_constraints: Vec::new(),
            dropped_constraints: Vec::new(),
        },
        dependencies: Vec::new(),
        media_state: None,
    };
    document::attach(project, sample.clone()).map_err(document_error)?;
    Ok(sample)
}
