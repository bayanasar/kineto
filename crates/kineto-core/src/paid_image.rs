//! Paid images kept as the provider answers them, as durable write-ahead
//! jobs: style samples and casting images.
//!
//! Each image is one provider call run exactly as a keyframe candidate is,
//! by the same image generators: the intent is written before the call,
//! the image and a receipt before the intent is marked invoked, and a
//! refusal record when the provider certainly did not accept the call.
//! Each kind of work keeps each provider's jobs in its own partition (a job
//! runtime provider key such as `minimax-style` or `plugin.studio-cast`)
//! and writes what its canonical record needs beyond the receipt as a plan
//! beside it, before the intent is prepared, so an interrupted image is
//! recorded from disk when the project opens or before the next image of
//! its kind, never by calling again.
//!
//! ```text
//! plan -> prepare -> provider -> image + receipt -> invoked -> attach -> acknowledge
//! ```
//!
//! An image whose call may have been accepted without its result being kept
//! is never paid for again unasked: no new image is made in its numbering
//! (every style sample, or one character's casting images) until the
//! director chooses to risk paying twice there. That choice retires only
//! those images; another numbering's stay guarded.

use std::marker::PhantomData;

use kineto_job_runtime::{
    DispatchOutcome, InvokePreparedError, JobRuntime, ProviderKey, StartupOutcome,
};
use kineto_jobs::JobId;
use kineto_project::{ArtifactId, InputHash, fs::ProjectRelativePath, manifest::CanonicalProject};
use kineto_provider_minimax::MinimaxClient;
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    documents::document_error,
    generation::{
        CandidateGenerator, CandidateOwner, CandidateRequest, GENERATE_OPERATION,
        GeneratedCandidate, GenerationError, GenerationHooks, GenerationPoint, KEYFRAME_ASPECT,
        invoke_when_permitted,
    },
    minimax::MinimaxKeyframes,
};

/// Where one kind of paid image is recorded.
pub(crate) trait Ledger {
    /// What the canonical record needs beyond the receipt.
    type Plan: Serialize + DeserializeOwned;

    /// The kind of work: the plan file's suffix, as in
    /// `<artifact_id>.<kind>.json`, and after the provider its job
    /// partition, apart from keyframes and every other kind.
    const KIND: &'static str;

    fn artifact_id(plan: &Self::Plan) -> &str;

    fn input_hash(plan: &Self::Plan) -> &str;

    /// Record the generated image in canonical state. Idempotent.
    fn attach(
        project: &CanonicalProject,
        plan: &Self::Plan,
        generated: GeneratedCandidate,
    ) -> Result<(), GenerationError>;
}

/// What finishing interrupted images found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImageRecovery {
    /// Images recorded from their receipts.
    pub attached: Vec<String>,
    /// Images whose call may have been accepted without their result being
    /// kept, by ID.
    pub unresolved: Vec<String>,
    /// Records this build cannot complete; they need a person.
    pub needs_attention: usize,
}

impl ImageRecovery {
    /// Add what another partition's recovery found.
    pub fn extend(&mut self, other: Self) {
        self.attached.extend(other.attached);
        self.unresolved.extend(other.unresolved);
        self.needs_attention += other.needs_attention;
    }
}

/// Whether the director accepted the risk of paying twice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Submission {
    /// Refused when an earlier image may have been charged without its
    /// result being kept.
    #[default]
    Once,
    /// The director's explicit decision to ask again anyway.
    RiskPayingTwice,
}

fn plan_path<L: Ledger>(artifact_id: &str) -> Result<ProjectRelativePath, GenerationError> {
    ProjectRelativePath::new(format!(".kineto/generation/{artifact_id}.{}.json", L::KIND))
        .map_err(|_| GenerationError::Unexpected("an image ID is not a valid file name"))
}

fn write_plan<L: Ledger>(
    project: &CanonicalProject,
    plan: &L::Plan,
) -> Result<(), GenerationError> {
    let mut bytes = serde_json::to_vec_pretty(plan)
        .map_err(|_| GenerationError::Unexpected("an image plan did not encode"))?;
    bytes.push(b'\n');
    project
        .root()
        .write_atomic(&plan_path::<L>(L::artifact_id(plan))?, &bytes)
        .map_err(document_error)
}

fn read_plan<L: Ledger>(project: &CanonicalProject, artifact_id: &str) -> Option<L::Plan> {
    let bytes = project
        .root()
        .read(&plan_path::<L>(artifact_id).ok()?)
        .ok()?;
    serde_json::from_slice::<L::Plan>(&bytes)
        .ok()
        .filter(|plan| L::artifact_id(plan) == artifact_id)
}

/// The plan is runtime state; a plan left behind is overwritten by the next
/// image with its ID.
fn remove_plan<L: Ledger>(project: &CanonicalProject, artifact_id: &str) {
    if let Ok(path) = plan_path::<L>(artifact_id) {
        let _ = std::fs::remove_file(project.root().resolve(&path));
    }
}

/// What the adapter keeps for `artifact_id` is released through a request
/// naming it; where its image goes no longer matters.
fn keyed(artifact_id: ArtifactId, input_hash: InputHash) -> CandidateRequest {
    CandidateRequest {
        artifact_id,
        input_hash,
        owner: CandidateOwner::Style,
        aspect_ratio: KEYFRAME_ASPECT,
        identities: Vec::new(),
        composition: None,
        style: None,
        prompt: None,
    }
}

/// L's partition of `provider`'s jobs.
pub(crate) fn partition<L: Ledger>(provider: &ProviderKey) -> ProviderKey {
    ProviderKey::new(format!("{}-{}", provider.as_str(), L::KIND))
        .expect("a provider key and a static kind make a valid key")
}

/// L's partition of one generator's jobs, opened for new images.
pub(crate) struct Jobs<'p, L, G> {
    project: &'p CanonicalProject,
    runtime: JobRuntime<'p>,
    adapter: G,
    ledger: PhantomData<L>,
}

/// Record images whose results arrived before an interruption, and sort
/// out the intents that never finished: those never called are returned,
/// as are those whose call may have been charged without a result.
fn finish<L: Ledger, G: CandidateGenerator>(
    project: &CanonicalProject,
    runtime: &JobRuntime<'_>,
    adapter: &mut G,
) -> Result<(ImageRecovery, Vec<JobId>), GenerationError> {
    let mut recovery = ImageRecovery::default();
    let mut waiting = Vec::new();
    for outcome in runtime.reconcile_startup(adapter)? {
        match outcome {
            StartupOutcome::ResultReady { job_id, output } => {
                let attached = read_plan::<L>(project, job_id.as_str())
                    .ok_or(GenerationError::Unexpected("a finished image has no plan"))
                    .and_then(|plan| {
                        L::attach(project, &plan, output.clone())?;
                        let _ = runtime.acknowledge_result(&job_id)?;
                        Ok(plan)
                    });
                match attached {
                    Ok(plan) => {
                        adapter.release(&keyed(
                            output.output.artifact_id.clone(),
                            output.input_hash.clone(),
                        ));
                        remove_plan::<L>(project, L::artifact_id(&plan));
                        recovery.attached.push(job_id.as_str().to_owned());
                    }
                    Err(_) => recovery.needs_attention += 1,
                }
            }
            StartupOutcome::RetryPrepared { job_id, .. } => {
                if let Some(record) = runtime.load(&job_id)?
                    && record.attempts() > 0
                    && let Ok(artifact_id) = ArtifactId::new(job_id.as_str())
                    && !adapter
                        .may_call_again(&keyed(artifact_id, record.intent().input_hash().clone()))
                {
                    recovery.unresolved.push(job_id.as_str().to_owned());
                }
                waiting.push(job_id);
            }
            StartupOutcome::StillPending { .. }
            | StartupOutcome::NeedsAttention { .. }
            | StartupOutcome::UnreadableIntent { .. }
            | StartupOutcome::ProviderError { .. } => recovery.needs_attention += 1,
        }
    }
    Ok((recovery, waiting))
}

/// Record images of L an interrupted session left with MiniMax, as a
/// project opens. Never calls MiniMax.
pub(crate) fn recover<L: Ledger>(
    project: &CanonicalProject,
) -> Result<ImageRecovery, GenerationError> {
    recover_with::<L, _>(
        project,
        MinimaxKeyframes::new(project, None::<&MinimaxClient>),
    )
}

/// Record images of L an interrupted session left with `adapter`'s
/// provider. Makes no new call; a plugin may be asked whether it kept the
/// answer to a call that was sent.
pub(crate) fn recover_with<L: Ledger, G: CandidateGenerator>(
    project: &CanonicalProject,
    mut adapter: G,
) -> Result<ImageRecovery, GenerationError> {
    let runtime = JobRuntime::new(
        project,
        partition::<L>(&adapter.provider()),
        adapter.policy(),
    );
    finish::<L, _>(project, &runtime, &mut adapter).map(|(recovery, _)| recovery)
}

impl<'p, L: Ledger, G: CandidateGenerator> Jobs<'p, L, G> {
    /// Open L's partition for new images numbered where `numbering` says
    /// an image ID belongs: record images an interruption left, retire
    /// intents never called or certainly refused, and refuse while an image
    /// of `numbering` may have been charged without its result, unless the
    /// director risks paying twice, which retires those images. An
    /// unresolved image outside `numbering` is left as it is: the director
    /// accepted no risk for it.
    pub(crate) fn open(
        project: &'p CanonicalProject,
        mut adapter: G,
        numbering: impl Fn(&str) -> bool,
        submission: Submission,
    ) -> Result<Self, GenerationError> {
        let runtime = JobRuntime::new(
            project,
            partition::<L>(&adapter.provider()),
            adapter.policy(),
        );
        let (recovery, waiting) = finish::<L, _>(project, &runtime, &mut adapter)?;
        if submission == Submission::Once && recovery.unresolved.iter().any(|id| numbering(id)) {
            return Err(GenerationError::UnresolvedSubmission);
        }
        for job_id in waiting {
            if recovery.unresolved.iter().any(|id| id == job_id.as_str())
                && !numbering(job_id.as_str())
            {
                continue;
            }
            let Some(record) = runtime.load(&job_id)? else {
                continue;
            };
            if record.attempts() == 0 {
                runtime.retire_unattempted(&job_id)?;
            } else {
                runtime.retire_unresolved(&job_id)?;
            }
            if let Ok(artifact_id) = ArtifactId::new(job_id.as_str()) {
                adapter.release(&keyed(artifact_id, record.intent().input_hash().clone()));
            }
            remove_plan::<L>(project, job_id.as_str());
        }
        Ok(Self {
            project,
            runtime,
            adapter,
            ledger: PhantomData,
        })
    }

    /// Generate the image `call` asks for and record it as `plan` says.
    pub(crate) fn generate(
        &mut self,
        plan: &L::Plan,
        call: &CandidateRequest,
        index: usize,
        hooks: &mut dyn GenerationHooks,
    ) -> Result<(), GenerationError> {
        let artifact_id = L::artifact_id(plan);
        if call.artifact_id.as_str() != artifact_id
            || call.input_hash.as_str() != L::input_hash(plan)
        {
            return Err(GenerationError::Unexpected(
                "an image call does not match its plan",
            ));
        }
        let job_id = JobId::new(artifact_id)?;
        write_plan::<L>(self.project, plan)?;
        self.runtime.prepare(
            job_id.clone(),
            GENERATE_OPERATION,
            call.input_hash.clone(),
            call.artifact_id.clone(),
        )?;
        hooks.reached(GenerationPoint::Prepared { index });
        let generated = match invoke_when_permitted(&self.runtime, &job_id, &mut self.adapter, call)
        {
            Ok(DispatchOutcome::Completed { output, .. }) => output,
            Ok(DispatchOutcome::Pending { .. }) => {
                return Err(GenerationError::Unexpected("an image was left pending"));
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
        L::attach(self.project, plan, generated)?;
        hooks.reached(GenerationPoint::Attached);
        let _ = self.runtime.acknowledge_result(&job_id)?;
        self.adapter.release(call);
        remove_plan::<L>(self.project, artifact_id);
        Ok(())
    }
}
