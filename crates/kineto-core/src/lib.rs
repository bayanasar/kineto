pub mod bible;
pub mod bootstrap;
pub mod cast;
pub mod dialogue;
pub mod documents;
pub mod dubbing;
pub mod export;
pub mod generation;
pub mod keyframe_prompt;
pub mod learning;
pub mod llm;
pub mod minimax;
pub mod paid_image;
pub mod plugin;
pub mod preferences;
pub mod references;
pub mod screenplay;
pub mod session;
pub mod shot_plan;
pub mod speech;
pub mod speech_prompt;
pub mod story;
pub mod style;
pub mod style_prompt;
pub mod timeline;
pub mod video;
pub mod video_prompt;
pub mod voice;
pub mod workflow;

/// Process-local engine state shared by all machine-local Kineto work.
///
/// The Flutter desktop application embeds this engine in-process. Expensive
/// work may still use Rust threads, GPU runtimes, provider clients, ffmpeg, or
/// dedicated child processes, but the UI/native boundary itself is FFI.
#[derive(Debug)]
pub struct Engine {
    // Keep the bootstrap handle non-zero-sized without inventing scheduler or
    // resource policy before real workloads exist.
    _anchor: u8,
}

impl Engine {
    #[must_use]
    pub const fn new() -> Self {
        Self { _anchor: 0 }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_constructs_without_starting_background_runtime() {
        let _engine = Engine::new();
    }
}
