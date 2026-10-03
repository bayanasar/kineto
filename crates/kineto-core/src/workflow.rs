//! Where a project stands in its workflow recipe.
//!
//! Every step's state is computed from the project's canonical files on
//! each read: what the step made, whether the director approved it, and
//! which of its parents changed since. Nothing is stored beside them, so
//! deleting `.kineto/` changes no step.

use std::{error::Error, fmt};

use kineto_project::{
    bible as bible_document, cast as cast_document,
    manifest::CanonicalProject,
    recipe::{self, ArtifactKind, RecipeError},
    revisions::{self, DocumentKind, RevisionStatus},
    shot::{ShotWorkflow, VideoState},
    source, style as style_document,
};

use crate::{cast, generation, session::SHOT_COUNT};

/// Where one step stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    /// Nothing made yet, and a step it follows is not done.
    Waiting,
    /// Nothing made yet; every step it follows is done.
    Ready,
    /// Made, and waiting for the director to approve it.
    AwaitingApproval,
    /// Approved, or made where the recipe asks for no approval.
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepStatus {
    pub id: String,
    pub produces: ArtifactKind,
    pub approval: bool,
    pub description: String,
    pub state: StepState,
    /// The steps it follows that are not done.
    pub waiting_on: Vec<String>,
    /// What changed since this step's work was made from it, by artifact ID,
    /// as `<owner>: <parent>` where one step's work has several owners
    /// (characters, shots); empty when it is fresh.
    pub stale: Vec<String>,
}

#[derive(Debug)]
pub enum WorkflowError {
    /// The project names a recipe this build cannot run.
    Recipe(RecipeError),
    /// A canonical record could not be read.
    Unreadable(Box<dyn Error + Send + Sync>),
}

impl fmt::Display for WorkflowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Recipe(error) => error.fmt(formatter),
            Self::Unreadable(error) => write!(formatter, "the project could not be read: {error}"),
        }
    }
}

impl Error for WorkflowError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Recipe(error) => Some(error),
            Self::Unreadable(error) => Some(error.as_ref()),
        }
    }
}

fn unreadable(error: impl Error + Send + Sync + 'static) -> WorkflowError {
    WorkflowError::Unreadable(Box::new(error))
}

/// What one kind of work has reached.
#[derive(Debug, Default)]
struct Progress {
    made: bool,
    approved: bool,
    stale: Vec<String>,
}

/// Where `project` stands in the recipe it names, step by step in recipe
/// order. Reads the project's files; never writes or calls anything.
pub fn status(project: &CanonicalProject) -> Result<Vec<StepStatus>, WorkflowError> {
    let recipe = recipe::of(project).map_err(WorkflowError::Recipe)?;
    let mut statuses: Vec<StepStatus> = Vec::with_capacity(recipe.steps.len());
    for step in &recipe.steps {
        let progress = progress(project, step.produces)?;
        let waiting_on: Vec<String> = step
            .after
            .iter()
            .filter(|parent| {
                statuses
                    .iter()
                    .find(|status| &status.id == *parent)
                    .is_none_or(|status| status.state != StepState::Done)
            })
            .cloned()
            .collect();
        let done = if step.approval {
            progress.approved
        } else {
            progress.made
        };
        let state = if done {
            StepState::Done
        } else if progress.made {
            StepState::AwaitingApproval
        } else if waiting_on.is_empty() {
            StepState::Ready
        } else {
            StepState::Waiting
        };
        statuses.push(StepStatus {
            id: step.id.clone(),
            produces: step.produces,
            approval: step.approval,
            description: step.description.clone(),
            state,
            waiting_on,
            stale: progress.stale,
        });
    }
    Ok(statuses)
}

fn progress(project: &CanonicalProject, kind: ArtifactKind) -> Result<Progress, WorkflowError> {
    match kind {
        ArtifactKind::Source => Ok(Progress {
            made: source::current(project).is_ok(),
            ..Progress::default()
        }),
        ArtifactKind::StoryAnalysis => story_analysis(project),
        ArtifactKind::Screenplay => screenplay(project),
        ArtifactKind::Bible => bible(project),
        ArtifactKind::Style => style(project),
        ArtifactKind::Cast => casting(project),
        // Shot plans are not made yet; the shot planner records them.
        ArtifactKind::ShotPlan => Ok(Progress::default()),
        ArtifactKind::Keyframe | ArtifactKind::Video => shots(project, kind),
    }
}

fn story_analysis(project: &CanonicalProject) -> Result<Progress, WorkflowError> {
    let Some(record) =
        revisions::current(project, DocumentKind::StoryAnalysis).map_err(unreadable)?
    else {
        return Ok(Progress::default());
    };
    let source = source::current(project).map_err(unreadable)?;
    let source_id = format!("source_r{:04}", source.revision);
    let fresh = record.dependencies.iter().any(|dependency| {
        dependency.artifact_id == source_id && dependency.content_hash == source.content_hash
    });
    Ok(Progress {
        made: true,
        approved: false,
        stale: if fresh { Vec::new() } else { vec![source_id] },
    })
}

fn screenplay(project: &CanonicalProject) -> Result<Progress, WorkflowError> {
    let all = revisions::revisions(project, DocumentKind::Screenplay).map_err(unreadable)?;
    let Some(locked) = all
        .iter()
        .find(|record| record.status == RevisionStatus::Locked)
    else {
        return Ok(Progress {
            made: !all.is_empty(),
            ..Progress::default()
        });
    };
    let analysis = revisions::current(project, DocumentKind::StoryAnalysis).map_err(unreadable)?;
    let fresh = analysis.as_ref().is_some_and(|analysis| {
        locked.dependencies.iter().any(|dependency| {
            dependency.artifact_id == analysis.artifact_id
                && dependency.content_hash == analysis.content_hash
        })
    });
    Ok(Progress {
        made: true,
        approved: true,
        stale: if fresh {
            Vec::new()
        } else {
            vec![analysis.map_or_else(
                || "story_analysis".to_owned(),
                |analysis| analysis.artifact_id,
            )]
        },
    })
}

fn bible(project: &CanonicalProject) -> Result<Progress, WorkflowError> {
    let all = revisions::revisions(project, DocumentKind::Bible).map_err(unreadable)?;
    let Some(locked) = all
        .iter()
        .find(|record| record.status == RevisionStatus::Locked)
    else {
        return Ok(Progress {
            made: !all.is_empty(),
            ..Progress::default()
        });
    };
    let stale = bible_document::staleness(project, locked)
        .map_err(unreadable)?
        .into_iter()
        .map(|cause| match cause {
            bible_document::StaleCause::NoLockedScreenplay => "screenplay".to_owned(),
            bible_document::StaleCause::Material(id)
            | bible_document::StaleCause::SceneChanged(id)
            | bible_document::StaleCause::SceneRemoved(id)
            | bible_document::StaleCause::SceneAdded(id) => id,
        })
        .collect();
    Ok(Progress {
        made: true,
        approved: true,
        stale,
    })
}

fn style(project: &CanonicalProject) -> Result<Progress, WorkflowError> {
    let approved = style_document::locked(project)
        .map_err(unreadable)?
        .is_some();
    Ok(Progress {
        made: approved
            || !style_document::samples(project)
                .map_err(unreadable)?
                .is_empty(),
        approved,
        stale: Vec::new(),
    })
}

/// Done when every main character is cast; stale for each cast whose
/// bible entry or style changed, named `<character>: <parent>`.
fn casting(project: &CanonicalProject) -> Result<Progress, WorkflowError> {
    let characters = cast::main_characters(project).map_err(unreadable)?;
    let mut progress = Progress {
        approved: !characters.is_empty(),
        ..Progress::default()
    };
    for character in &characters {
        let locked = cast_document::locked(project, character)
            .map_err(unreadable)?
            .is_some();
        progress.approved &= locked;
        progress.made |= locked
            || !cast_document::images(project, character)
                .map_err(unreadable)?
                .is_empty();
        progress.stale.extend(
            cast::staleness(project, character)
                .map_err(unreadable)?
                .into_iter()
                .map(|parent| format!("{character}: {parent}")),
        );
    }
    Ok(progress)
}

/// Done when every shot's keyframe (or video) is locked; stale for each
/// shot whose locked keyframe no longer follows its locks, or whose video
/// no longer follows its keyframe.
fn shots(project: &CanonicalProject, kind: ArtifactKind) -> Result<Progress, WorkflowError> {
    let mut progress = Progress {
        approved: true,
        ..Progress::default()
    };
    for number in 1..=u32::try_from(SHOT_COUNT).expect("shot count fits in u32") {
        let shot = ShotWorkflow::load(project, number).map_err(unreadable)?;
        let snapshot = shot.snapshot().map_err(unreadable)?;
        let shot_id = format!("shot_{number:03}");
        if kind == ArtifactKind::Keyframe {
            progress.made |= snapshot.generated;
            progress.approved &= snapshot.locked;
            if snapshot.locked {
                if snapshot.stale {
                    progress.stale.push(format!("{shot_id}: intent"));
                }
                let selected = usize::from(snapshot.selected_index.unwrap_or_default());
                progress.stale.extend(
                    generation::keyframe_staleness(project, &shot)
                        .map_err(unreadable)?
                        .get(selected)
                        .into_iter()
                        .flatten()
                        .map(|parent| format!("{shot_id}: {parent}")),
                );
            }
        } else {
            progress.made |= snapshot.video != VideoState::None;
            progress.approved &= snapshot.video == VideoState::Locked;
            if snapshot.video == VideoState::Stale {
                progress.stale.push(format!("{shot_id}: keyframe"));
            }
        }
    }
    Ok(progress)
}
