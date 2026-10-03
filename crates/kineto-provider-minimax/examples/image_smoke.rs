//! Manual smoke test against the live MiniMax API. This is a paid call and is
//! never run by the test suite.
//!
//! ```text
//! KINETO_MINIMAX_API_KEY=... cargo run -p kineto-provider-minimax \
//!     --example image_smoke -- <output-file> [prompt]
//! ```

use std::time::Instant;

use kineto_provider_minimax::{AspectRatio, ImageRequest, MinimaxClient, PROVIDER};
use kineto_providers::secret::{EnvSecretSource, SecretSource};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = args
        .next()
        .ok_or("usage: image_smoke <output-file> [prompt]")?;
    let prompt = args.next().unwrap_or_else(|| {
        "Medium close up shot. A filmmaker hears the final line across a table at night. \
         Photorealistic cinematic film still."
            .to_owned()
    });
    let key = EnvSecretSource::process()
        .api_key(PROVIDER)?
        .ok_or("KINETO_MINIMAX_API_KEY is not set")?;

    let started = Instant::now();
    let image = MinimaxClient::new(key).generate_image(&ImageRequest {
        prompt,
        aspect_ratio: AspectRatio::Landscape16x9,
        subject_reference: None,
    })?;
    std::fs::write(&output, &image.bytes)?;
    println!(
        "{:?}, {} bytes, request {}, {} ms -> {output}",
        image.format,
        image.bytes.len(),
        image.request_id,
        started.elapsed().as_millis()
    );
    Ok(())
}
