//! Manual run of shot planning with Kimi Code, on a project whose
//! screenplay and bible are locked. Kimi Code uses its subscription. Never
//! run by the test suite.
//!
//! ```text
//! cargo run -p kineto-core --example shot_plan_smoke -- <project-dir> <kimi>
//! ```

use std::{path::PathBuf, time::Instant};

use kineto_core::shot_plan;
use kineto_llm::agents::{AgentCli, AgentKind};
use kineto_project::{
    manifest::CanonicalProject,
    revisions,
    shot_plan::{ShotPlan, staleness},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: shot_plan_smoke <dir> <kimi>";
    let project = CanonicalProject::open(PathBuf::from(args.next().ok_or(usage)?))?;
    let executable = PathBuf::from(args.next().ok_or(usage)?);
    let started = Instant::now();
    let record = shot_plan::write(&project, &AgentCli::new(AgentKind::KimiCode, executable))?;
    let plan: ShotPlan = revisions::read(&project, &record)?;
    println!(
        "{} by {:?}/{:?} in {} ms, stale: {:?}",
        record.artifact_id,
        record.provider,
        record.model,
        started.elapsed().as_millis(),
        staleness(&project, &record)?
    );
    for (scene, shot) in plan.shots() {
        println!(
            "  {scene} {} {:?} {} {} {} {}s {:?} | {} | {}",
            shot.id,
            shot.direction,
            shot.framing.as_str(),
            shot.lens,
            shot.camera_motion.as_str(),
            shot.duration_target,
            shot.characters,
            shot.purpose,
            shot.action
        );
    }
    Ok(())
}
