//! Manual smoke test against the live MiniMax API. This is a paid call and is
//! never run by the test suite. Without `<voice-id>` it lists the system
//! voices instead, which is free.
//!
//! ```text
//! KINETO_MINIMAX_API_KEY=... cargo run -p kineto-provider-minimax \
//!     --example speech_smoke -- [<output-file> <voice-id> [text]]
//! ```

use std::time::Instant;

use kineto_provider_minimax::{MinimaxClient, PROVIDER, SpeechRequest};
use kineto_providers::{
    AudioFormat,
    secret::{EnvSecretSource, SecretSource},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let key = EnvSecretSource::process()
        .api_key(PROVIDER)?
        .ok_or("KINETO_MINIMAX_API_KEY is not set")?;
    let client = MinimaxClient::new(key);
    let (Some(output), Some(voice_id)) = (args.next(), args.next()) else {
        for voice in client.speech_voices()? {
            println!("{}\t{}\t{}", voice.id, voice.name, voice.description);
        }
        return Ok(());
    };
    let text = args
        .next()
        .unwrap_or_else(|| "I waited for you. All night.".to_owned());

    let started = Instant::now();
    let speech = client.synthesize(&SpeechRequest {
        text,
        voice_id,
        language: "en".to_owned(),
        emotion: None,
        speed_permille: 1000,
        pitch_semitones: 0,
        format: AudioFormat::Mp3,
    })?;
    std::fs::write(&output, &speech.audio)?;
    println!(
        "{} bytes, {} ms of speech, {} characters billed, {} sentences timed, {} ms -> {output}",
        speech.audio.len(),
        speech.duration_ms,
        speech.billed_characters,
        speech.sentences.len(),
        started.elapsed().as_millis()
    );
    Ok(())
}
