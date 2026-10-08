//! A free, deterministic `kineto-plugin/1` plugin for tests and for trying
//! the protocol: `image.generate` answers the same small JPEG for every
//! request, its bytes made distinct by a comment holding the request's
//! hash, `tts.speak` a silent WAV as long as the text takes to say, and
//! `video.generate` a job that is finished the first time it is asked
//! about, with the bundled demo clip.
//!
//! An argument makes it misbehave on purpose, for tests of the host. Asked
//! to `invoke`: `--sleep` never answers, `--crash` exits without answering,
//! `--wrong-hash` declares a file it did not write, `--refuse` fails without
//! the provider taking the request, `--fail` fails after it may have, and
//! `--env` reports the names of the environment variables it was given; it
//! describes itself as usual, but for `--sleep` with a one-second deadline.
//! `--garbage` answers every request with something that is not the
//! protocol.
//!
//! With `--state <dir>` it keeps every image it makes there by idempotency
//! key, before it answers, and `reconcile` gives a kept image back: a
//! provider that remembers its answers. `--crash` then keeps the image and
//! exits without answering, as a provider that took the request and was cut
//! off before it said so. Every request it is sent is appended to
//! `requests.log` there, one line each, for tests to read.
//!
//! `--square-only` declares 1:1 images only. Asked about a video job,
//! `--video-fails` says it failed and `--not-mp4` answers with a file that
//! is not an MP4.

use std::{
    env, fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const PROTOCOL: &str = "kineto-plugin/1";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn answer(value: &Value) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

fn file(name: &str, bytes: &[u8]) -> Value {
    fs::write(format!("outputs/{name}"), bytes).expect("the plugin can write its outputs");
    json!({
        "path": format!("outputs/{name}"),
        "size": bytes.len(),
        "sha256": format!("sha256:{}", hex(&Sha256::digest(bytes))),
    })
}

fn describe(mode: &str) -> Value {
    let ratios = if mode == "--square-only" {
        json!(["1:1"])
    } else {
        json!(["1:1", "16:9", "9:16"])
    };
    // A plugin that will sleep says so, so its host gives up soon.
    let deadline = if mode == "--sleep" { 1 } else { 900 };
    json!({
        "name": "Kineto demo plugin",
        "version": env!("CARGO_PKG_VERSION"),
        "invoke_deadline_seconds": deadline,
        "capabilities": {
            "image.generate": {
                "model": "demo-image",
                "aspect_ratios": ratios,
                "reference_roles": ["identity", "composition", "style"],
                "max_reference_images": 2,
                "reference_formats": ["jpeg", "png"],
                "max_reference_bytes": 10_485_760,
                "output_formats": ["jpeg"],
                "max_prompt_chars": 4000,
                "price_per_image_micros": 0
            },
            "tts.speak": {
                "model": "demo-speech",
                "languages": ["en", "zh", "ja"],
                "output_formats": ["wav"],
                "speed_permille": [500, 2000],
                "max_text_chars": 2000,
                "price_per_character_micros": 0
            },
            "video.generate": {
                "model": "demo-video",
                "clips": [{"seconds": 6, "lines": 720, "price_micros": 0}],
                "start_frame_formats": ["jpeg", "png"],
                "max_start_frame_bytes": 10_485_760,
                "end_frame_formats": ["jpeg", "png"],
                "max_end_frame_bytes": 10_485_760,
                "max_prompt_chars": 2000
            }
        }
    })
}

/// The clip every demo video job finishes with.
const DEMO_CLIP: &[u8] = include_bytes!("../../kineto-core/assets/demo-clip.mp4");

/// The picture every demo image shows: a figure in a dark room, 320x180,
/// drawn once with Pillow.
const DEMO_FRAME: &[u8] = include_bytes!("../assets/demo-frame.jpg");

/// [`DEMO_FRAME`] with a comment segment holding the request's hash right
/// after its start marker: a JPEG that decodes to the same picture and
/// differs with every request.
fn image(request: &Value) -> Vec<u8> {
    let digest = Sha256::digest(request.to_string().as_bytes());
    let (start, rest) = DEMO_FRAME.split_at(2);
    let length = u16::try_from(digest.len() + 2).expect("a hash fits a segment");
    let mut bytes = start.to_vec();
    bytes.extend_from_slice(&[0xff, 0xfe]);
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(&digest);
    bytes.extend_from_slice(rest);
    bytes
}

/// A silent 8 kHz mono WAV of `milliseconds`.
fn wav(milliseconds: u32) -> Vec<u8> {
    let samples = 8 * milliseconds;
    let data = samples * 2;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&8000_u32.to_le_bytes());
    bytes.extend_from_slice(&16000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    bytes.resize(44 + data as usize, 0);
    bytes
}

/// A file name for `key` that stays in the state directory.
fn kept(state: &Path, key: &str) -> PathBuf {
    state.join(format!("{}.jpg", hex(&Sha256::digest(key.as_bytes()))))
}

/// Answer one request on stdin as `args` say, as the plugin binary does.
pub fn serve(args: &[String]) {
    let mut state = None;
    let mut mode = String::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--state" {
            state = rest.next().map(PathBuf::from);
        } else if mode.is_empty() {
            mode.clone_from(arg);
        }
    }
    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);
    let request: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
    let invoked = request["operation"] == "invoke";
    match mode.as_str() {
        "--sleep" if invoked => loop {
            thread::sleep(Duration::from_secs(60));
        },
        "--crash" if invoked && state.is_none() => std::process::exit(3),
        "--garbage" => {
            println!("this is not the protocol");
            return;
        }
        _ => {}
    }
    if let Some(state) = &state {
        let _ = fs::create_dir_all(state);
        if let Ok(mut log) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(state.join("requests.log"))
        {
            // One write per line: parallel calls append to the same log.
            let _ = log.write_all(format!("{}\n", line.trim_end()).as_bytes());
        }
    }
    if request["protocol"] != PROTOCOL {
        answer(
            &json!({"protocol": PROTOCOL, "status": "failed", "class": "terminal", "accepted": false, "message": "unknown protocol"}),
        );
        return;
    }
    let operation = request["operation"].as_str().unwrap_or_default();
    let capability = request["capability"].as_str().unwrap_or_default();
    let reply = match (operation, mode.as_str()) {
        ("describe", _) => {
            json!({"protocol": PROTOCOL, "status": "ok", "body": describe(&mode)})
        }
        ("estimate", _) => json!({"protocol": PROTOCOL, "status": "ok", "body": {"micros": 0}}),
        ("voices", _) => json!({"protocol": PROTOCOL, "status": "ok", "body": {"voices": [
            {"id": "demo_plain", "name": "Plain", "description": "An even voice."},
            {"id": "demo_warm", "name": "Warm", "description": "A warm voice."}
        ]}}),
        ("reconcile", _) if capability == "video.generate" => {
            let job = request["job"].as_str().unwrap_or_default();
            match mode.as_str() {
                _ if !job.starts_with("demo-video-") => {
                    json!({"protocol": PROTOCOL, "status": "unknown"})
                }
                "--video-fails" => json!({"protocol": PROTOCOL, "status": "failed",
                    "class": "terminal", "message": "the demo's render failed"}),
                "--not-mp4" => json!({"protocol": PROTOCOL, "status": "completed",
                    "outputs": [file("clip.mp4", &image(&request))]}),
                _ => json!({"protocol": PROTOCOL, "status": "completed",
                    "outputs": [file("clip.mp4", DEMO_CLIP)], "metadata": {"model": "demo-video"}}),
            }
        }
        ("reconcile", _) => {
            let key = request["idempotency_key"].as_str().unwrap_or_default();
            match state.as_deref().map(|state| fs::read(kept(state, key))) {
                Some(Ok(bytes)) => json!({"protocol": PROTOCOL, "status": "completed",
                    "outputs": [file("image.jpg", &bytes)], "metadata": {"model": "demo-image"}}),
                _ => json!({"protocol": PROTOCOL, "status": "unknown"}),
            }
        }
        ("invoke", "--refuse") => {
            json!({"protocol": PROTOCOL, "status": "failed", "class": "terminal", "accepted": false, "message": "the demo refused the request"})
        }
        ("invoke", "--fail") => {
            json!({"protocol": PROTOCOL, "status": "failed", "class": "retryable", "message": "the demo failed after taking the request"})
        }
        ("invoke", "--env") => {
            let mut names: Vec<String> = env::vars_os()
                .map(|(name, _)| name.to_string_lossy().into_owned())
                .collect();
            names.sort();
            json!({"protocol": PROTOCOL, "status": "completed",
                "outputs": [file("image.jpg", &image(&request))],
                "metadata": {"env": names, "cwd": env::current_dir().ok(), "inputs": request["inputs"].clone()}})
        }
        ("invoke", _) if capability == "image.generate" => {
            let bytes = image(&request);
            if let Some(state) = &state {
                let key = request["idempotency_key"].as_str().unwrap_or_default();
                let _ = fs::create_dir_all(state);
                fs::write(kept(state, key), &bytes).expect("the plugin can keep its images");
                if mode == "--crash" {
                    std::process::exit(3);
                }
            }
            let mut declared = file("image.jpg", &bytes);
            if mode == "--wrong-hash" {
                declared["sha256"] = json!(format!("sha256:{}", "0".repeat(64)));
            }
            json!({"protocol": PROTOCOL, "status": "completed", "outputs": [declared],
                "metadata": {"model": "demo-image", "request_id": hex(&Sha256::digest(line.as_bytes()))[..16]}})
        }
        ("invoke", _) if capability == "video.generate" => {
            let frame = request["request"]["first_frame"]
                .as_str()
                .unwrap_or_default();
            let last_frame = request["request"]["last_frame"].as_str();
            if fs::metadata(frame).is_err() {
                json!({"protocol": PROTOCOL, "status": "failed", "class": "terminal",
                    "accepted": false, "message": "the demo needs a first frame"})
            } else if last_frame.is_some_and(|last| fs::metadata(last).is_err()) {
                json!({"protocol": PROTOCOL, "status": "failed", "class": "terminal",
                    "accepted": false, "message": "the demo was not given the last frame it was named"})
            } else {
                let job = format!(
                    "demo-video-{}",
                    &hex(&Sha256::digest(line.as_bytes()))[..16]
                );
                json!({"protocol": PROTOCOL, "status": "pending", "job": job})
            }
        }
        ("invoke", _) if capability == "tts.speak" => {
            let text = request["request"]["text"].as_str().unwrap_or_default();
            let milliseconds =
                300 + 60 * u32::try_from(text.split_whitespace().count()).unwrap_or(0);
            json!({"protocol": PROTOCOL, "status": "completed",
                "outputs": [file("speech.wav", &wav(milliseconds))],
                "metadata": {"model": "demo-speech", "duration_ms": milliseconds}})
        }
        _ => {
            json!({"protocol": PROTOCOL, "status": "failed", "class": "terminal", "accepted": false, "message": format!("the demo does not do {operation} {capability}")})
        }
    };
    answer(&reply);
}
