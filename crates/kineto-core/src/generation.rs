//! Shot candidate generation as durable write-ahead jobs.
//!
//! Each planned candidate is one job: its artifact ID is the job ID and the
//! target artifact, the operation is `image.generate`, and the provider is the
//! deterministic local demo generator. A generation runs
//!
//! ```text
//! prepare (write-ahead intent) -> invoke -> attach all candidates -> acknowledge
//! ```
//!
//! and [`complete`] drives a plan to the end from whichever of those steps it
//! last reached durably, so a normal run and the recovery of an interrupted
//! one take the same path. Canonical state changes only in the attach step,
//! which is idempotent.

use std::{error::Error, fmt, num::NonZeroU16};

use kineto_job_runtime::{
    DispatchOutcome, Invocation, InvokePreparedError, JobRuntime, JobRuntimeError,
    JobRuntimePolicy, PaidJobAdapter, PrepareOutcome, ProviderKey, Reconciliation, StartupOutcome,
};
use kineto_jobs::{
    BackoffPolicy, CostEstimate, CurrencyCode, ErrorClass, FallbackPolicy, IdempotencyKey,
    IntentState, JobId, JobValueError, PaidFallback, ProviderExecutionPolicy, ProviderJobId,
};
use kineto_project::{
    ArtifactId, InputHash,
    manifest::CanonicalProject,
    shot::{
        AttachOutcome, CandidateOutput, DEMO_PROVIDER, GenerationPlan, ShotWorkflow,
        ShotWorkflowError, demo_candidate_output,
    },
};

/// The capability a shot candidate is generated with.
pub const GENERATE_OPERATION: &str = "image.generate";

/// One candidate to generate, as the adapter receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRequest {
    pub artifact_id: ArtifactId,
    pub input_hash: InputHash,
}

/// A generated candidate together with the input it was generated for, so an
/// interrupted generation can be regrouped into its plan after a restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedCandidate {
    pub output: CandidateOutput,
    pub input_hash: InputHash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDemoError(&'static str);

impl fmt::Display for LocalDemoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for LocalDemoError {}

/// The deterministic local demo generator.
///
/// It keeps no remote state: its result for an idempotency key is a pure
/// function of the candidate and input hash the key names. Reconciling by key
/// therefore recomputes the result instead of looking anything up, and never
/// reports a job as missing.
#[derive(Debug, Default)]
pub struct LocalDemoAdapter;

impl LocalDemoAdapter {
    fn generated(
        artifact_id: &ArtifactId,
        input_hash: &InputHash,
    ) -> Result<GeneratedCandidate, LocalDemoError> {
        let output = demo_candidate_output(artifact_id)
            .map_err(|_| LocalDemoError("candidate cannot be generated"))?;
        Ok(GeneratedCandidate {
            output,
            input_hash: input_hash.clone(),
        })
    }

    fn request_for_key(key: &IdempotencyKey) -> Result<CandidateRequest, LocalDemoError> {
        // `{operation}:{artifact_id}:{input_hash}`, as the job runtime derives it.
        // Artifact IDs cannot contain ':', so the first separator after the
        // operation ends the artifact ID.
        let rest = key
            .as_str()
            .strip_prefix(GENERATE_OPERATION)
            .and_then(|rest| rest.strip_prefix(':'))
            .ok_or(LocalDemoError(
                "idempotency key is not an image.generate key",
            ))?;
        let (artifact_id, input_hash) = rest
            .split_once(':')
            .ok_or(LocalDemoError("idempotency key has no input hash"))?;
        Ok(CandidateRequest {
            artifact_id: ArtifactId::new(artifact_id)
                .map_err(|_| LocalDemoError("idempotency key names an invalid artifact"))?,
            input_hash: InputHash::new(input_hash)
                .map_err(|_| LocalDemoError("idempotency key has an invalid input hash"))?,
        })
    }
}

impl PaidJobAdapter for LocalDemoAdapter {
    type Request = CandidateRequest;
    type Output = GeneratedCandidate;
    type Error = LocalDemoError;

    fn estimate_cost(&self, _request: &CandidateRequest) -> Result<CostEstimate, LocalDemoError> {
        Ok(CostEstimate {
            call_count: 1,
            amount_micros: 0,
            currency: CurrencyCode::new(*b"USD").expect("static currency code is valid"),
            estimated_duration_ms: Some(0),
        })
    }

    fn invoke(
        &mut self,
        request: &CandidateRequest,
        _idempotency_key: &IdempotencyKey,
    ) -> Result<Invocation<GeneratedCandidate>, LocalDemoError> {
        Self::generated(&request.artifact_id, &request.input_hash).map(Invocation::Completed)
    }

    fn reconcile(
        &mut self,
        _provider_job_id: &ProviderJobId,
    ) -> Result<Reconciliation<GeneratedCandidate>, LocalDemoError> {
        // The demo generator never hands out remote job handles.
        Ok(Reconciliation::NotFound)
    }

    fn reconcile_by_idempotency_key(
        &mut self,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Reconciliation<GeneratedCandidate>, LocalDemoError> {
        let request = Self::request_for_key(idempotency_key)?;
        Ok(Reconciliation::Completed {
            output: Self::generated(&request.artifact_id, &request.input_hash)?,
            provider_job_id: None,
        })
    }

    fn classify_error(&self, _error: &LocalDemoError) -> ErrorClass {
        // A deterministic local computation that failed once fails again.
        ErrorClass::Terminal
    }
}

/// A step a generation passes durably. Tests stop the process at these points
/// to prove recovery; production passes [`NoHooks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationPoint {
    /// The write-ahead intent for candidate `index` is on disk.
    Prepared { index: usize },
    /// Every candidate has a result and no canonical state has changed.
    Invoked,
    /// The generation is in canonical state; the intents are not yet
    /// acknowledged.
    Attached,
}

pub trait GenerationHooks {
    fn reached(&mut self, point: GenerationPoint);
}

pub struct NoHooks;

impl GenerationHooks for NoHooks {
    fn reached(&mut self, _point: GenerationPoint) {}
}

#[derive(Debug)]
pub enum GenerationError {
    Shot(ShotWorkflowError),
    Runtime(JobRuntimeError),
    Provider(LocalDemoError),
    JobId(JobValueError),
    /// The provider reported something the demo generator never does, such as
    /// a pending remote job.
    Unexpected(&'static str),
}

impl fmt::Display for GenerationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shot(error) => error.fmt(formatter),
            Self::Runtime(error) => error.fmt(formatter),
            Self::Provider(error) => error.fmt(formatter),
            Self::JobId(error) => error.fmt(formatter),
            Self::Unexpected(message) => formatter.write_str(message),
        }
    }
}

impl Error for GenerationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Shot(error) => Some(error),
            Self::Runtime(error) => Some(error),
            Self::Provider(error) => Some(error),
            Self::JobId(error) => Some(error),
            Self::Unexpected(_) => None,
        }
    }
}

impl From<ShotWorkflowError> for GenerationError {
    fn from(error: ShotWorkflowError) -> Self {
        Self::Shot(error)
    }
}

impl From<JobRuntimeError> for GenerationError {
    fn from(error: JobRuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<JobValueError> for GenerationError {
    fn from(error: JobValueError) -> Self {
        Self::JobId(error)
    }
}

/// The job runtime for the local demo provider on one project.
fn runtime(project: &CanonicalProject) -> JobRuntime<'_> {
    let provider = ProviderKey::new(DEMO_PROVIDER).expect("static provider key is valid");
    // A local deterministic computation has no provider-side concurrency
    // limit. The runtime refuses, rather than waits, when a ceiling is reached,
    // so a real provider will need a scheduler that waits for a permit.
    let execution = ProviderExecutionPolicy::new(
        NonZeroU16::MAX,
        BackoffPolicy::new(0, 0, 3).expect("static backoff policy is valid"),
    );
    JobRuntime::new(
        project,
        provider,
        JobRuntimePolicy {
            execution,
            // The demo generator is free, and no paid fallback exists for it.
            fallback_policy: FallbackPolicy {
                paid_fallback: PaidFallback::Forbidden,
                max_paid_amount_micros: Some(0),
                require_user_approval: true,
            },
        },
    )
}

/// Generate the shot's next candidates as write-ahead jobs.
pub fn generate(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    hooks: &mut dyn GenerationHooks,
) -> Result<AttachOutcome, GenerationError> {
    let plan = shot.plan_generation(project)?;
    complete(project, shot, &plan, hooks)
}

/// Drive a planned generation to the end from whatever it reached durably.
pub fn complete(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    plan: &GenerationPlan,
    hooks: &mut dyn GenerationHooks,
) -> Result<AttachOutcome, GenerationError> {
    let runtime = runtime(project);
    let mut adapter = LocalDemoAdapter;
    let mut job_ids = Vec::with_capacity(plan.candidate_ids().len());
    let mut outputs = Vec::with_capacity(plan.candidate_ids().len());

    for (index, candidate) in plan.candidate_ids().iter().enumerate() {
        let job_id = JobId::new(candidate.as_str())?;
        let request = CandidateRequest {
            artifact_id: candidate.clone(),
            input_hash: plan.input_hash().clone(),
        };
        let state = match runtime.prepare(
            job_id.clone(),
            GENERATE_OPERATION,
            plan.input_hash().clone(),
            candidate.clone(),
        ) {
            Ok(PrepareOutcome::Created(record) | PrepareOutcome::Existing(record)) => {
                Some(record.intent().state())
            }
            // Acknowledged long ago and aged out of retention: already attached.
            Err(JobRuntimeError::ReconciledIntentPruned) => None,
            Err(error) => return Err(error.into()),
        };
        hooks.reached(GenerationPoint::Prepared { index });

        let generated = match state {
            Some(IntentState::Prepared) => {
                match runtime.invoke_prepared(&job_id, &mut adapter, &request) {
                    Ok(DispatchOutcome::Completed { output, .. }) => output,
                    Ok(DispatchOutcome::Pending { .. }) => {
                        return Err(GenerationError::Unexpected(
                            "the demo generator returned a pending job",
                        ));
                    }
                    Err(InvokePreparedError::Runtime(error)) => return Err(error.into()),
                    Err(InvokePreparedError::Provider { source, .. }) => {
                        return Err(GenerationError::Provider(source));
                    }
                }
            }
            // The provider call already happened; recover its result instead
            // of calling again.
            Some(IntentState::Invoked | IntentState::Reconciled) | None => {
                LocalDemoAdapter::generated(&request.artifact_id, &request.input_hash)
                    .map_err(GenerationError::Provider)?
            }
        };
        if state.is_some() {
            job_ids.push(job_id);
        }
        outputs.push(generated.output);
    }
    hooks.reached(GenerationPoint::Invoked);

    let outcome = shot.attach_generation(project, plan, &outputs)?;
    hooks.reached(GenerationPoint::Attached);

    for job_id in &job_ids {
        // Retention warnings are housekeeping after the durable acknowledgement
        // and do not change the result.
        let _ = runtime.acknowledge_result(job_id)?;
    }
    Ok(outcome)
}

/// Unfinished generations found in a project's job intents.
#[derive(Debug, Default)]
pub struct Unfinished {
    /// One plan per interrupted generation, each ready for [`complete`].
    pub plans: Vec<GenerationPlan>,
    /// Intents this build cannot read or cannot place; they are left alone and
    /// need a person to look at them.
    pub needs_attention: usize,
}

/// Find generations that stopped before they were acknowledged.
///
/// Runs the job runtime's startup reconciliation for the demo provider, then
/// regroups the candidates it reports into the plans they belong to.
pub fn unfinished(project: &CanonicalProject) -> Result<Unfinished, JobRuntimeError> {
    let runtime = runtime(project);
    let mut unfinished = Unfinished::default();
    for outcome in runtime.reconcile_startup(&mut LocalDemoAdapter)? {
        match outcome {
            StartupOutcome::ResultReady { job_id, output } => {
                let plan = ArtifactId::new(job_id.as_str()).ok().and_then(|candidate| {
                    GenerationPlan::for_candidate(&candidate, output.input_hash).ok()
                });
                match plan {
                    Some(plan) if !unfinished.plans.contains(&plan) => unfinished.plans.push(plan),
                    Some(_) => {}
                    None => unfinished.needs_attention += 1,
                }
            }
            StartupOutcome::StillPending { .. }
            | StartupOutcome::RetryPrepared { .. }
            | StartupOutcome::NeedsAttention { .. }
            | StartupOutcome::UnreadableIntent { .. }
            | StartupOutcome::ProviderError { .. } => unfinished.needs_attention += 1,
        }
    }
    unfinished
        .plans
        .sort_by_key(|plan| (plan.shot_number(), plan.revision()));
    Ok(unfinished)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kineto_project::manifest::{
        PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource, ProjectWorkflow,
    };
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "kineto-generation-test-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn project(target: &Path) -> CanonicalProject {
        CanonicalProject::create(
            target,
            ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                project_id: "generation_test".to_owned(),
                title: "Generation Test".to_owned(),
                created_at: "2026-09-30T00:00:00Z".to_owned(),
                source: ProjectSource {
                    kind: "text".to_owned(),
                    path: "source/story.txt".to_owned(),
                    extra: BTreeMap::new(),
                },
                defaults: ProjectDefaults {
                    language: "en".to_owned(),
                    extra: BTreeMap::new(),
                },
                workflow: ProjectWorkflow {
                    recipe_id: "default-film".to_owned(),
                    recipe_version: 1,
                    extra: BTreeMap::new(),
                },
                extra: BTreeMap::new(),
            },
            b"source\n",
        )
        .unwrap()
    }

    fn intent_states(target: &Path) -> Vec<String> {
        let mut states = fs::read_dir(target.join(".kineto/jobs/intents"))
            .map(|entries| {
                entries
                    .map(|entry| {
                        let bytes = fs::read(entry.unwrap().path()).unwrap();
                        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                        value["state"].as_str().unwrap().to_owned()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        states.sort();
        states
    }

    /// Stops a generation at one point, as a crash there would: nothing after
    /// the point runs and nothing is rolled back.
    struct StopAt(GenerationPoint);

    impl GenerationHooks for StopAt {
        fn reached(&mut self, point: GenerationPoint) {
            if point == self.0 {
                panic!("stopped at {point:?}");
            }
        }
    }

    fn stop_at(target: &Path, point: GenerationPoint) {
        let project = project(target);
        let mut shot = ShotWorkflow::load(&project, 1).unwrap();
        let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            generate(&project, &mut shot, &mut StopAt(point))
        }));
        assert!(stopped.is_err(), "generation should stop at {point:?}");
    }

    fn recover(target: &Path) -> (usize, AttachOutcome) {
        let project = CanonicalProject::open(target).unwrap();
        let found = unfinished(&project).unwrap();
        assert_eq!(found.needs_attention, 0);
        assert_eq!(found.plans.len(), 1, "exactly one interrupted generation");
        let plan = &found.plans[0];
        let mut shot = ShotWorkflow::load(&project, plan.shot_number()).unwrap();
        let outcome = complete(&project, &mut shot, plan, &mut NoHooks).unwrap();
        assert!(unfinished(&project).unwrap().plans.is_empty());
        (found.plans.len(), outcome)
    }

    fn assert_one_generation(target: &Path) {
        let project = CanonicalProject::open(target).unwrap();
        let snapshot = ShotWorkflow::load(&project, 1).unwrap().snapshot().unwrap();
        assert!(snapshot.generated);
        assert_eq!(snapshot.generation_revision, 1);
        assert_eq!(snapshot.candidate_count, 3);
        assert_eq!(snapshot.superseded_count, 0);
        assert_eq!(intent_states(target), vec!["reconciled"; 3]);
    }

    #[test]
    fn a_generation_leaves_one_reconciled_intent_per_candidate() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, 1).unwrap();

        assert_eq!(
            generate(&project, &mut shot, &mut NoHooks).unwrap(),
            AttachOutcome::Attached
        );
        drop(project);
        assert_one_generation(&target);

        let project = CanonicalProject::open(&target).unwrap();
        assert!(unfinished(&project).unwrap().plans.is_empty());
    }

    #[test]
    fn a_generation_stopped_after_its_first_intent_completes_on_recovery() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        stop_at(&target, GenerationPoint::Prepared { index: 0 });
        assert_eq!(intent_states(&target), vec!["prepared"]);

        assert_eq!(recover(&target), (1, AttachOutcome::Attached));
        assert_one_generation(&target);
    }

    #[test]
    fn a_generation_stopped_before_attaching_attaches_on_recovery() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        stop_at(&target, GenerationPoint::Invoked);
        assert_eq!(intent_states(&target), vec!["invoked"; 3]);

        assert_eq!(recover(&target), (1, AttachOutcome::Attached));
        assert_one_generation(&target);
    }

    #[test]
    fn a_generation_stopped_after_attaching_is_only_acknowledged_on_recovery() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        stop_at(&target, GenerationPoint::Attached);
        assert_eq!(intent_states(&target), vec!["invoked"; 3]);
        let attached =
            fs::read(target.join("scenes/scene_001/shots/shot_001/artifacts.json")).unwrap();

        assert_eq!(recover(&target), (1, AttachOutcome::AlreadyAttached));
        assert_one_generation(&target);
        assert_eq!(
            fs::read(target.join("scenes/scene_001/shots/shot_001/artifacts.json")).unwrap(),
            attached
        );
    }

    #[test]
    fn the_demo_adapter_recomputes_the_same_result_from_its_key() {
        let candidate = ArtifactId::new("shot_001_g0001_candidate_b").unwrap();
        let input = InputHash::new(format!("sha256:{}", "a".repeat(64))).unwrap();
        let key = IdempotencyKey::new(format!(
            "{GENERATE_OPERATION}:{}:{}",
            candidate.as_str(),
            input.as_str()
        ))
        .unwrap();
        let mut adapter = LocalDemoAdapter;
        let request = CandidateRequest {
            artifact_id: candidate,
            input_hash: input,
        };

        let Invocation::Completed(invoked) = adapter.invoke(&request, &key).unwrap() else {
            panic!("demo generation completes synchronously");
        };
        let Reconciliation::Completed { output, .. } =
            adapter.reconcile_by_idempotency_key(&key).unwrap()
        else {
            panic!("demo generation is always reconcilable by key");
        };
        assert_eq!(output, invoked);
        assert!(
            adapter
                .reconcile_by_idempotency_key(&IdempotencyKey::new("video.generate:x:y").unwrap())
                .is_err()
        );
    }
}
