//! Manual smoke test of an OpenAI-compatible backend. Paid when pointed at
//! Kimi; never run by the test suite.
//!
//! ```text
//! KINETO_KIMI_API_KEY=... cargo run -p kineto-llm --example api_smoke
//! cargo run -p kineto-llm --example api_smoke -- http://localhost:11434/v1 qwen3
//! ```

use kineto_llm::{LlmBackend, LlmRequest, Message, openai::OpenAiCompatible};
use kineto_providers::secret::{EnvSecretSource, SecretSource};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let backend = match (args.next(), args.next()) {
        (Some(base_url), Some(model)) => OpenAiCompatible::local(&base_url, &model),
        _ => OpenAiCompatible::kimi(
            EnvSecretSource::process()
                .api_key("kimi")?
                .ok_or("KINETO_KIMI_API_KEY is not set")?,
            std::env::var("KINETO_KIMI_MODEL").ok().as_deref(),
            None,
        ),
    };
    let info = backend.info();
    let response = backend.complete(&LlmRequest {
        messages: vec![Message::user("Reply with exactly one word: ready")],
        ..LlmRequest::default()
    })?;
    println!(
        "{} {}: {:?} ({:?})",
        info.provider, response.model, response.text, response.usage
    );
    Ok(())
}
