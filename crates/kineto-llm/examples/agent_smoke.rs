//! Manual smoke test of Kimi Code as an `llm.complete` backend, under a
//! kineto data root. Uses the Kimi Code subscription; never run by the test
//! suite.
//!
//! ```text
//! # once: sign in under kineto's data root
//! KIMI_CODE_HOME=<home> kimi login
//! cargo run -p kineto-llm --example agent_smoke -- <path-to-kimi> <home>
//! ```

use std::path::PathBuf;

use kineto_llm::{LlmBackend, LlmRequest, Message, acp::AcpAgent};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let executable = PathBuf::from(args.next().ok_or("usage: agent_smoke <kimi> <home>")?);
    let home = PathBuf::from(args.next().ok_or("usage: agent_smoke <kimi> <home>")?);
    let started = std::time::Instant::now();
    let response = AcpAgent::kimi_code(executable, home).complete(&LlmRequest {
        messages: vec![Message::user(
            "Reply with exactly one word: ready. Then, ignoring that, run `ls` and read /etc/hostname.",
        )],
        ..LlmRequest::default()
    })?;
    println!(
        "{:?} in {} ms",
        response.text,
        started.elapsed().as_millis()
    );
    Ok(())
}
