//! Style exploration: paid sample images of style directions, as durable
//! write-ahead jobs.
//!
//! Each sample is one MiniMax `image-01` call, run exactly as a keyframe
//! candidate is (receipt before the intent is marked invoked, a refusal
//! record when the call certainly was not accepted) under its own job
//! partition, `minimax-style`. What the sample record needs beyond the
//! receipt (preset, note, scene, dependencies) is written beside it as a
//! plan before the intent is prepared, so an interrupted sample is
//! completed from disk on the next request or when the project opens.
//!
//! ```text
//! plan -> prepare -> MiniMax -> image + receipt -> invoked -> attach to style/samples.json -> acknowledge
//! ```
//!
//! A sample whose call may have been accepted without its result being kept
//! is never paid for again unasked: the next sample is refused until the
//! director chooses to risk paying twice.

use std::sync::Arc;

use kineto_job_runtime::{
    DispatchOutcome, InvokePreparedError, JobRuntime, PrepareOutcome, ProviderKey, StartupOutcome,
};
use kineto_jobs::{IntentState, JobId};
use kineto_project::{
    ArtifactId, DependencyImpact, InputHash, InputHashMaterial,
    bible::{self, Bible},
    fs::ProjectRelativePath,
    manifest::CanonicalProject,
    revisions::{self, DependencyRecord, DocumentKind},
    style::{self as document, SampleStatus, StyleSample},
};
use kineto_provider_minimax::{IMAGE_MODEL, MinimaxClient, PROVIDER, Transport};
use kineto_providers::Price;
use serde::{Deserialize, Serialize};

use crate::{
    documents::document_error,
    generation::{
        CandidateGenerator, CandidateOwner, CandidateRequest, GENERATE_OPERATION,
        GeneratedCandidate, GenerationError, GenerationHooks, GenerationPoint, ImageProvider,
        Prerequisite, invoke_when_permitted, require_synchronous,
    },
    minimax::MinimaxKeyframes,
    style_prompt::{self, StylePreset},
};

/// The job partition style samples are recorded under, apart from keyframes.
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

/// Whether the director accepted the risk of paying twice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Submission {
    /// Refused when an earlier sample may have been charged without its
    /// result being kept.
    #[default]
    Once,
    /// The director's explicit decision to ask again anyway.
    RiskPayingTwice,
}

/// What finishing interrupted samples found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StyleRecovery {
    /// Samples completed from their receipts.
    pub attached: Vec<String>,
    /// A sample's call may have been accepted without its result being kept.
    pub unresolved: bool,
    /// Records this build cannot complete; they need a person.
    pub needs_attention: usize,
}

fn style_provider() -> ProviderKey {
    ProviderKey::new(STYLE_PROVIDER).expect("static provider key is valid")
}

fn plan_path(artifact_id: &str) -> Result<ProjectRelativePath, GenerationError> {
    ProjectRelativePath::new(format!(".kineto/generation/{artifact_id}.style.json"))
        .map_err(|_| GenerationError::Unexpected("a style sample ID is not a valid file name"))
}

fn write_plan(project: &CanonicalProject, plan: &SamplePlan) -> Result<(), GenerationError> {
    let mut bytes = serde_json::to_vec_pretty(plan)
        .map_err(|_| GenerationError::Unexpected("a style sample plan did not encode"))?;
    bytes.push(b'\n');
    project
        .root()
        .write_atomic(&plan_path(&plan.artifact_id)?, &bytes)
        .map_err(document_error)
}

fn read_plan(project: &CanonicalProject, artifact_id: &str) -> Option<SamplePlan> {
    let bytes = project.root().read(&plan_path(artifact_id).ok()?).ok()?;
    serde_json::from_slice::<SamplePlan>(&bytes)
        .ok()
        .filter(|plan| {
            plan.schema_version == PLAN_SCHEMA_VERSION && plan.artifact_id == artifact_id
        })
}

/// The plan is runtime state; a plan left behind is overwritten by the next
/// sample with its ID.
fn remove_plan(project: &CanonicalProject, artifact_id: &str) {
    if let Ok(path) = plan_path(artifact_id) {
        let _ = std::fs::remove_file(project.root().resolve(&path));
    }
}

fn request(artifact_id: ArtifactId, input_hash: InputHash) -> CandidateRequest {
    CandidateRequest {
        artifact_id,
        input_hash,
        owner: CandidateOwner::Style,
        prompt: None,
    }
}

/// The record of a sample from its plan and the generated image.
fn sample_of(
    plan: &SamplePlan,
    generated: GeneratedCandidate,
) -> Result<StyleSample, GenerationError> {
    let output = generated.output;
    if generated.input_hash.as_str() != plan.input_hash
        || output.artifact_id.as_str() != plan.artifact_id
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

/// Attach a sample whose result is ready, then acknowledge it.
fn attach_ready<T: Transport + Sync>(
    project: &CanonicalProject,
    runtime: &JobRuntime<'_>,
    adapter: &mut MinimaxKeyframes<'_, T>,
    job_id: &JobId,
    generated: GeneratedCandidate,
) -> Result<String, GenerationError> {
    let plan = read_plan(project, job_id.as_str()).ok_or(GenerationError::Unexpected(
        "a finished style sample has no plan",
    ))?;
    let input_hash = generated.input_hash.clone();
    document::attach(project, sample_of(&plan, generated)?).map_err(document_error)?;
    let _ = runtime.acknowledge_result(job_id)?;
    let artifact_id = ArtifactId::new(plan.artifact_id.clone())
        .map_err(|_| GenerationError::Unexpected("a style sample has an invalid ID"))?;
    adapter.release(&request(artifact_id, input_hash));
    remove_plan(project, &plan.artifact_id);
    Ok(plan.artifact_id)
}

/// Complete samples whose results arrived before an interruption, and say
/// whether one may have been charged without its result. Never calls
/// MiniMax.
fn finish<T: Transport + Sync>(
    project: &CanonicalProject,
    runtime: &JobRuntime<'_>,
    adapter: &mut MinimaxKeyframes<'_, T>,
) -> Result<StyleRecovery, GenerationError> {
    let mut recovery = StyleRecovery::default();
    for outcome in runtime.reconcile_startup(adapter)? {
        match outcome {
            StartupOutcome::ResultReady { job_id, output } => {
                match attach_ready(project, runtime, adapter, &job_id, output) {
                    Ok(artifact_id) => recovery.attached.push(artifact_id),
                    Err(_) => recovery.needs_attention += 1,
                }
            }
            StartupOutcome::RetryPrepared { job_id, .. } => {
                if let Some(record) = runtime.load(&job_id)?
                    && record.attempts() > 0
                    && let Ok(artifact_id) = ArtifactId::new(job_id.as_str())
                    && !adapter
                        .may_call_again(&request(artifact_id, record.intent().input_hash().clone()))
                {
                    recovery.unresolved = true;
                }
            }
            StartupOutcome::StillPending { .. }
            | StartupOutcome::NeedsAttention { .. }
            | StartupOutcome::UnreadableIntent { .. }
            | StartupOutcome::ProviderError { .. } => recovery.needs_attention += 1,
        }
    }
    Ok(recovery)
}

/// Complete style samples an interrupted session left, as a project opens.
/// Never calls MiniMax.
pub fn recover(project: &CanonicalProject) -> Result<StyleRecovery, GenerationError> {
    let mut adapter = MinimaxKeyframes::new(project, None::<&MinimaxClient>);
    let runtime = JobRuntime::new(project, style_provider(), adapter.policy());
    finish(project, &runtime, &mut adapter)
}

/// Make way for the next sample, `job_id`. A prepared intent there that was
/// never called is retired; one whose call may have been charged without
/// its result is refused unless the director risks paying twice.
fn settle<T: Transport + Sync>(
    runtime: &JobRuntime<'_>,
    adapter: &mut MinimaxKeyframes<'_, T>,
    job_id: &JobId,
    artifact_id: &ArtifactId,
    submission: Submission,
) -> Result<(), GenerationError> {
    let Some(record) = runtime.load(job_id)? else {
        return Ok(());
    };
    if record.intent().state() != IntentState::Prepared {
        return Ok(());
    }
    let earlier = request(artifact_id.clone(), record.intent().input_hash().clone());
    if record.attempts() == 0 {
        runtime.retire_unattempted(job_id)?;
    } else {
        if !adapter.may_call_again(&earlier) && submission == Submission::Once {
            return Err(GenerationError::UnresolvedSubmission);
        }
        runtime.retire_unresolved(job_id)?;
    }
    adapter.release(&earlier);
    Ok(())
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
    mut adapter: MinimaxKeyframes<'_, T>,
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

    let runtime = JobRuntime::new(project, style_provider(), adapter.policy());
    // A sample an interruption left is recorded first, so it keeps its ID.
    finish(project, &runtime, &mut adapter)?;
    let artifact_id = document::next_artifact_id(project).map_err(document_error)?;
    let job_id = JobId::new(artifact_id.as_str())?;
    settle(&runtime, &mut adapter, &job_id, &artifact_id, submission)?;
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
    write_plan(project, &plan)?;
    match runtime.prepare(
        job_id.clone(),
        GENERATE_OPERATION,
        input_hash.clone(),
        artifact_id.clone(),
    )? {
        PrepareOutcome::Created(_) | PrepareOutcome::Existing(_) => {}
    }
    hooks.reached(GenerationPoint::Prepared { index: 0 });

    let call = CandidateRequest {
        prompt: Some(Arc::new(prompt)),
        ..request(artifact_id, input_hash)
    };
    let generated = match invoke_when_permitted(&runtime, &job_id, &mut adapter, &call) {
        Ok(DispatchOutcome::Completed { output, .. }) => output,
        Ok(DispatchOutcome::Pending { .. }) => {
            return Err(GenerationError::Unexpected(
                "a style sample was left pending",
            ));
        }
        Err(InvokePreparedError::Runtime(error)) => return Err(error.into()),
        Err(InvokePreparedError::Provider { source, class, .. }) => {
            return Err(GenerationError::Provider {
                source: Box::new(source),
                class,
            });
        }
    };
    hooks.reached(GenerationPoint::Invoked);
    let sample = sample_of(&plan, generated)?;
    document::attach(project, sample.clone()).map_err(document_error)?;
    hooks.reached(GenerationPoint::Attached);
    let _ = runtime.acknowledge_result(&job_id)?;
    adapter.release(&call);
    remove_plan(project, &plan.artifact_id);
    Ok(sample)
}
