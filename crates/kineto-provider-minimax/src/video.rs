//! MiniMax Hailuo image-to-video: submit, query, fetch.
//!
//! A video is a remote task. Submitting is the paid call and returns a task
//! ID at once; querying it is free and can be repeated after a restart;
//! fetching resolves the finished file and downloads it from MiniMax's content
//! host, without the API key.

use base64::Engine as _;
use kineto_jobs::{CostEstimate, CurrencyCode};
use kineto_providers::{
    CameraControl, ImageFormat, Price, VideoCapabilities, VideoContainer, VideoMode,
};
use serde::{Deserialize, Serialize};

use crate::{MinimaxClient, MinimaxError, Transport, WireBaseResp, api_error};

/// The video model this adapter calls.
pub const VIDEO_MODEL: &str = "MiniMax-Hailuo-02";
/// The longest video prompt Hailuo accepts, in characters.
pub const MAX_VIDEO_PROMPT_CHARS: usize = 2000;
/// A 10-second 1080P clip is tens of MiB; anything far larger is not one.
pub const MAX_VIDEO_BYTES: u64 = 256 * 1024 * 1024;
/// Hailuo accepts a first frame of at most 20 MB.
const MAX_FIRST_FRAME_BYTES: usize = 20 * 1024 * 1024;
const MAX_TASK_ID_LEN: usize = 128;

/// What `MiniMax-Hailuo-02` does through this adapter: image-to-video from
/// a first frame, at 768P, in clips of 6 or 10 seconds. The prompt takes
/// Hailuo's bracketed camera commands. A task outlives the client and is
/// followed again after a restart. The model can take a last frame too, but
/// this adapter does not send one, so none is declared.
#[must_use]
pub fn video_capabilities() -> VideoCapabilities {
    VideoCapabilities {
        modes: [VideoMode::ImageToVideo].into_iter().collect(),
        min_duration_ms: Some(6_000),
        max_duration_ms: Some(10_000),
        start_frame: true,
        camera_control: CameraControl::Parametric,
        async_jobs: true,
        resumable_remote_handle: true,
        output_containers: [VideoContainer::Mp4].into_iter().collect(),
        clip_seconds: [VideoDuration::Six, VideoDuration::Ten]
            .map(VideoDuration::seconds)
            .into_iter()
            .collect(),
        max_prompt_chars: Some(MAX_VIDEO_PROMPT_CHARS),
        start_frame_formats: [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Webp]
            .into_iter()
            .collect(),
        max_start_frame_bytes: Some(MAX_FIRST_FRAME_BYTES as u64),
        clip_prices: [VideoDuration::Six, VideoDuration::Ten]
            .map(|duration| {
                (
                    duration.seconds(),
                    Price::usd(clip_price_micros(duration, VideoResolution::P768)),
                )
            })
            .into_iter()
            .collect(),
        ..VideoCapabilities::default()
    }
}

/// Hailuo's bracketed camera command for a planned camera motion, where it
/// has one. A slow move keeps its pace in the words beside the command.
#[must_use]
pub fn camera_command(motion: &str) -> Option<&'static str> {
    Some(match motion {
        "static" => "Static shot",
        "slow_dolly_in" | "dolly_in" => "Push in",
        "slow_dolly_out" | "dolly_out" => "Pull out",
        "pan_left" => "Pan left",
        "pan_right" => "Pan right",
        "tilt_up" => "Tilt up",
        "tilt_down" => "Tilt down",
        "tracking" => "Tracking shot",
        "handheld" => "Shake",
        _ => return None,
    })
}

/// Published price of one Hailuo-02 clip.
const fn clip_price_micros(duration: VideoDuration, resolution: VideoResolution) -> u64 {
    match (resolution, duration) {
        (VideoResolution::P768, VideoDuration::Six) => 280_000,
        (VideoResolution::P768, VideoDuration::Ten) => 560_000,
    }
}

/// Clip lengths Hailuo generates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoDuration {
    Six,
    Ten,
}

impl VideoDuration {
    #[must_use]
    pub const fn seconds(self) -> u8 {
        match self {
            Self::Six => 6,
            Self::Ten => 10,
        }
    }

    /// The clip length closest to `seconds`; ties go to the shorter, cheaper
    /// clip.
    #[must_use]
    pub fn nearest(seconds: f64) -> Self {
        if seconds > 8.0 { Self::Ten } else { Self::Six }
    }
}

/// Output resolutions this adapter requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoResolution {
    P768,
}

impl VideoResolution {
    const fn as_str(self) -> &'static str {
        match self {
            Self::P768 => "768P",
        }
    }
}

/// One image-to-video generation.
#[derive(Clone, PartialEq, Eq)]
pub struct VideoRequest {
    pub prompt: String,
    pub first_frame: Vec<u8>,
    pub first_frame_format: ImageFormat,
    pub duration: VideoDuration,
    pub resolution: VideoResolution,
}

impl std::fmt::Debug for VideoRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VideoRequest")
            .field("prompt", &self.prompt)
            .field("first_frame", &self.first_frame.len())
            .field("first_frame_format", &self.first_frame_format)
            .field("duration", &self.duration)
            .field("resolution", &self.resolution)
            .finish()
    }
}

/// MiniMax's ID for a submitted video task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoTask(String);

impl VideoTask {
    /// A task ID as MiniMax issues it: bounded ASCII letters and digits.
    pub fn new(value: impl Into<String>) -> Result<Self, MinimaxError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_TASK_ID_LEN
            || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(MinimaxError::Malformed("task ID is not a MiniMax task ID"));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where a submitted video task stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoTaskState {
    /// Preparing, queueing or processing.
    Pending,
    /// Finished; the file can be fetched.
    Succeeded { file_id: String },
}

#[derive(Serialize)]
struct WireSubmit<'a> {
    model: &'a str,
    prompt: &'a str,
    first_frame_image: String,
    duration: u8,
    resolution: &'a str,
    prompt_optimizer: bool,
}

#[derive(Deserialize)]
struct WireSubmitted {
    #[serde(default)]
    task_id: String,
    base_resp: WireBaseResp,
}

#[derive(Deserialize)]
struct WireQuery {
    #[serde(default)]
    status: String,
    #[serde(default)]
    file_id: String,
    base_resp: WireBaseResp,
}

#[derive(Deserialize)]
struct WireRetrieved {
    #[serde(default)]
    file: Option<WireFile>,
    base_resp: WireBaseResp,
}

#[derive(Deserialize)]
struct WireFile {
    #[serde(default)]
    download_url: String,
}

/// Parse an API answer, preferring MiniMax's own refusal over the HTTP status.
fn answer<'a, W: Deserialize<'a>>(
    status: u16,
    body: &'a [u8],
    base: impl Fn(&W) -> &WireBaseResp,
) -> Result<W, MinimaxError> {
    let parsed = serde_json::from_slice::<W>(body);
    if !(200..300).contains(&status) {
        return Err(match parsed {
            Ok(wire) if base(&wire).status_code != 0 => api_error(base(&wire)),
            _ => MinimaxError::Http { status },
        });
    }
    let wire = parsed.map_err(|_| MinimaxError::Malformed("body is not a video response"))?;
    if base(&wire).status_code != 0 {
        return Err(api_error(base(&wire)));
    }
    Ok(wire)
}

fn mime(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Png => "image/png",
        ImageFormat::Webp => "image/webp",
    }
}

impl<T: Transport> MinimaxClient<T> {
    /// What one clip costs before it is requested.
    #[must_use]
    pub fn estimate_video_cost(
        &self,
        duration: VideoDuration,
        resolution: VideoResolution,
    ) -> CostEstimate {
        CostEstimate {
            call_count: 1,
            amount_micros: clip_price_micros(duration, resolution),
            currency: CurrencyCode::new(*b"USD").expect("static currency code is valid"),
            estimated_duration_ms: Some(240_000),
        }
    }

    /// Submit an image-to-video task. This is the paid call.
    pub fn submit_video(&self, request: &VideoRequest) -> Result<VideoTask, MinimaxError> {
        let chars = request.prompt.chars().count();
        if request.prompt.trim().is_empty() || chars > MAX_VIDEO_PROMPT_CHARS {
            return Err(MinimaxError::InvalidPrompt);
        }
        if request.first_frame.is_empty() || request.first_frame.len() > MAX_FIRST_FRAME_BYTES {
            return Err(MinimaxError::InvalidRequest(
                "first frame is empty or too large",
            ));
        }
        let first_frame_image = format!(
            "data:{};base64,{}",
            mime(request.first_frame_format),
            base64::engine::general_purpose::STANDARD.encode(&request.first_frame)
        );
        let body = serde_json::to_vec(&WireSubmit {
            model: VIDEO_MODEL,
            prompt: &request.prompt,
            first_frame_image,
            duration: request.duration.seconds(),
            resolution: request.resolution.as_str(),
            prompt_optimizer: false,
        })
        .map_err(|_| MinimaxError::InvalidRequest("request did not encode"))?;
        let response = self
            .transport
            .post_json(
                &format!("{}/v1/video_generation", self.base_url),
                &self.api_key,
                &body,
            )
            .map_err(MinimaxError::Transport)?;
        let submitted: WireSubmitted =
            answer(response.status, &response.body, |wire: &WireSubmitted| {
                &wire.base_resp
            })?;
        VideoTask::new(submitted.task_id)
    }

    /// Where a task stands. Free; safe to repeat after a restart.
    pub fn query_video(&self, task: &VideoTask) -> Result<VideoTaskState, MinimaxError> {
        let response = self
            .transport
            .get(
                &format!(
                    "{}/v1/query/video_generation?task_id={}",
                    self.base_url,
                    task.as_str()
                ),
                &self.api_key,
            )
            .map_err(MinimaxError::Transport)?;
        let query: WireQuery = answer(response.status, &response.body, |wire: &WireQuery| {
            &wire.base_resp
        })?;
        match query.status.as_str() {
            "Preparing" | "Queueing" | "Processing" => Ok(VideoTaskState::Pending),
            "Success"
                if !query.file_id.is_empty()
                    && query.file_id.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                Ok(VideoTaskState::Succeeded {
                    file_id: query.file_id,
                })
            }
            "Success" => Err(MinimaxError::Malformed("finished task has no file ID")),
            "Fail" => Err(MinimaxError::TaskFailed),
            _ => Err(MinimaxError::Malformed("task status is not recognised")),
        }
    }

    /// Download a finished task's video. Free.
    pub fn fetch_video(&self, file_id: &str) -> Result<Vec<u8>, MinimaxError> {
        if file_id.is_empty() || !file_id.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(MinimaxError::Malformed("file ID is not a MiniMax file ID"));
        }
        let response = self
            .transport
            .get(
                &format!("{}/v1/files/retrieve?file_id={file_id}", self.base_url),
                &self.api_key,
            )
            .map_err(MinimaxError::Transport)?;
        let retrieved: WireRetrieved =
            answer(response.status, &response.body, |wire: &WireRetrieved| {
                &wire.base_resp
            })?;
        let url = retrieved
            .file
            .map(|file| file.download_url)
            .filter(|url| url.starts_with("https://"))
            .ok_or(MinimaxError::Malformed("file has no https download URL"))?;
        let download = self
            .transport
            .download(&url, MAX_VIDEO_BYTES)
            .map_err(MinimaxError::Transport)?;
        if !(200..300).contains(&download.status) {
            return Err(MinimaxError::Http {
                status: download.status,
            });
        }
        if download.body.len() < 12 || &download.body[4..8] != b"ftyp" {
            return Err(MinimaxError::Malformed("download is not an MP4 file"));
        }
        Ok(download.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HttpResponse, TransportFailure};
    use kineto_jobs::ErrorClass;
    use kineto_providers::secret::SecretString;
    use std::{collections::VecDeque, sync::Mutex};

    const KEY: &str = "sk-test-video-0123456789";
    const MP4: &[u8] = b"\0\0\0\x18ftypisom\0\0\x02\0isommp41";

    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        Post(String),
        Get(String),
        Download(String),
    }

    #[derive(Default)]
    struct Recorded {
        answers: Mutex<VecDeque<Result<HttpResponse, TransportFailure>>>,
        calls: Mutex<Vec<(Call, Option<String>)>>,
        bodies: Mutex<Vec<Vec<u8>>>,
    }

    impl Recorded {
        fn next(&self) -> Result<HttpResponse, TransportFailure> {
            self.answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("a recorded answer")
        }
    }

    impl Transport for Recorded {
        fn post_json(
            &self,
            url: &str,
            api_key: &SecretString,
            body: &[u8],
        ) -> Result<HttpResponse, TransportFailure> {
            self.calls.lock().unwrap().push((
                Call::Post(url.to_owned()),
                Some(api_key.expose().to_owned()),
            ));
            self.bodies.lock().unwrap().push(body.to_vec());
            self.next()
        }

        fn get(&self, url: &str, api_key: &SecretString) -> Result<HttpResponse, TransportFailure> {
            self.calls
                .lock()
                .unwrap()
                .push((Call::Get(url.to_owned()), Some(api_key.expose().to_owned())));
            self.next()
        }

        fn download(&self, url: &str, max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
            assert_eq!(max_bytes, MAX_VIDEO_BYTES);
            self.calls
                .lock()
                .unwrap()
                .push((Call::Download(url.to_owned()), None));
            self.next()
        }
    }

    fn client(
        answers: impl IntoIterator<Item = Result<HttpResponse, TransportFailure>>,
    ) -> MinimaxClient<Recorded> {
        MinimaxClient::with_transport(
            Recorded {
                answers: Mutex::new(answers.into_iter().collect()),
                ..Recorded::default()
            },
            SecretString::new(KEY.to_owned()).unwrap(),
            "https://example.test",
        )
    }

    fn json(status: u16, body: &str) -> Result<HttpResponse, TransportFailure> {
        Ok(HttpResponse {
            status,
            body: body.as_bytes().to_vec(),
        })
    }

    fn request() -> VideoRequest {
        VideoRequest {
            prompt: "Slow dolly in on the filmmaker.".to_owned(),
            first_frame: vec![0xff, 0xd8, 0xff, 0xe0],
            first_frame_format: ImageFormat::Jpeg,
            duration: VideoDuration::Six,
            resolution: VideoResolution::P768,
        }
    }

    #[test]
    fn declared_capabilities_match_what_the_client_sends_and_charges() {
        let capabilities = video_capabilities();
        assert_eq!(
            capabilities.modes,
            [VideoMode::ImageToVideo].into_iter().collect()
        );
        assert!(capabilities.start_frame && !capabilities.end_frame);
        assert!(capabilities.async_jobs && capabilities.resumable_remote_handle);
        assert_eq!(capabilities.camera_control, CameraControl::Parametric);
        assert_eq!(capabilities.max_prompt_chars, Some(MAX_VIDEO_PROMPT_CHARS));
        assert_eq!(
            capabilities.max_start_frame_bytes,
            Some(MAX_FIRST_FRAME_BYTES as u64)
        );
        for format in &capabilities.start_frame_formats {
            assert!(!mime(*format).is_empty());
        }
        let client = client([]);
        for duration in [VideoDuration::Six, VideoDuration::Ten] {
            assert!(capabilities.clip_seconds.contains(&duration.seconds()));
            assert_eq!(
                capabilities.clip_prices[&duration.seconds()].amount_micros,
                client
                    .estimate_video_cost(duration, VideoResolution::P768)
                    .amount_micros
            );
        }
        assert_eq!(capabilities.clip_seconds.len(), 2);
    }

    #[test]
    fn every_planned_camera_motion_has_a_command() {
        for motion in [
            "static",
            "slow_dolly_in",
            "dolly_in",
            "slow_dolly_out",
            "dolly_out",
            "pan_left",
            "pan_right",
            "tilt_up",
            "tilt_down",
            "tracking",
            "handheld",
        ] {
            assert!(camera_command(motion).is_some(), "{motion}");
        }
        assert_eq!(camera_command("slow_dolly_in"), Some("Push in"));
        assert_eq!(camera_command("restrained_pan"), None);
    }

    #[test]
    fn a_task_is_submitted_queried_and_fetched_without_the_key_leaving_the_api() {
        let client = client([
            json(
                200,
                r#"{"task_id":"447576615924185","base_resp":{"status_code":0,"status_msg":"success"}}"#,
            ),
            json(
                200,
                r#"{"task_id":"447576615924185","status":"Processing","file_id":"","base_resp":{"status_code":0}}"#,
            ),
            json(
                200,
                r#"{"task_id":"447576615924185","status":"Success","file_id":"447576721990100","video_width":910,"video_height":512,"base_resp":{"status_code":0}}"#,
            ),
            json(
                200,
                r#"{"file":{"file_id":447576721990100,"bytes":0,"filename":"output_aigc.mp4","purpose":"video_generation","download_url":"https://cdn.example.test/v.mp4"},"base_resp":{"status_code":0}}"#,
            ),
            Ok(HttpResponse {
                status: 200,
                body: MP4.to_vec(),
            }),
        ]);

        let task = client.submit_video(&request()).unwrap();
        assert_eq!(task.as_str(), "447576615924185");
        assert_eq!(client.query_video(&task).unwrap(), VideoTaskState::Pending);
        let VideoTaskState::Succeeded { file_id } = client.query_video(&task).unwrap() else {
            panic!("the task finished");
        };
        assert_eq!(client.fetch_video(&file_id).unwrap(), MP4);

        let sent: serde_json::Value =
            serde_json::from_slice(&client.transport.bodies.lock().unwrap()[0]).unwrap();
        assert_eq!(sent["model"], "MiniMax-Hailuo-02");
        assert_eq!(sent["duration"], 6);
        assert_eq!(sent["resolution"], "768P");
        assert_eq!(sent["first_frame_image"], "data:image/jpeg;base64,/9j/4A==");
        let calls = client.transport.calls.lock().unwrap();
        assert_eq!(
            calls.iter().map(|(call, _)| call).collect::<Vec<_>>(),
            [
                &Call::Post("https://example.test/v1/video_generation".to_owned()),
                &Call::Get(
                    "https://example.test/v1/query/video_generation?task_id=447576615924185"
                        .to_owned()
                ),
                &Call::Get(
                    "https://example.test/v1/query/video_generation?task_id=447576615924185"
                        .to_owned()
                ),
                &Call::Get(
                    "https://example.test/v1/files/retrieve?file_id=447576721990100".to_owned()
                ),
                &Call::Download("https://cdn.example.test/v.mp4".to_owned()),
            ]
        );
        assert_eq!(calls[4].1, None, "the download never carries the key");
    }

    #[test]
    fn failures_are_classified_for_the_job_runtime() {
        type Attempt = Box<dyn Fn(&MinimaxClient<Recorded>) -> MinimaxError>;
        let cases: [(Attempt, _, _); 7] = [
            (
                Box::new(|client| client.submit_video(&request()).unwrap_err()),
                json(
                    200,
                    r#"{"task_id":"","base_resp":{"status_code":1008,"status_msg":"insufficient balance"}}"#,
                ),
                ErrorClass::Terminal,
            ),
            (
                Box::new(|client| client.submit_video(&request()).unwrap_err()),
                json(
                    200,
                    r#"{"task_id":"","base_resp":{"status_code":1002,"status_msg":"rate limit"}}"#,
                ),
                ErrorClass::Retryable,
            ),
            (
                Box::new(|client| client.submit_video(&request()).unwrap_err()),
                json(
                    200,
                    r#"{"task_id":"12/../x","base_resp":{"status_code":0}}"#,
                ),
                ErrorClass::Terminal,
            ),
            (
                Box::new(|client| {
                    client
                        .query_video(&VideoTask::new("1").unwrap())
                        .unwrap_err()
                }),
                json(200, r#"{"status":"Fail","base_resp":{"status_code":0}}"#),
                ErrorClass::Terminal,
            ),
            (
                Box::new(|client| {
                    client
                        .query_video(&VideoTask::new("1").unwrap())
                        .unwrap_err()
                }),
                Err(TransportFailure::Timeout),
                ErrorClass::Retryable,
            ),
            (
                Box::new(|client| client.fetch_video("1").unwrap_err()),
                json(
                    200,
                    r#"{"file":{"download_url":"http://cdn.example.test/v.mp4"},"base_resp":{"status_code":0}}"#,
                ),
                ErrorClass::Terminal,
            ),
            (
                Box::new(|client| client.fetch_video("../1").unwrap_err()),
                json(200, "{}"),
                ErrorClass::Terminal,
            ),
        ];
        for (call, answer, class) in cases {
            let client = client([answer]);
            let error = call(&client);
            assert_eq!(error.class(), class, "{error}");
            assert!(!format!("{error:?}").contains(KEY));
        }
    }

    #[test]
    fn a_submission_is_known_unaccepted_only_if_it_never_arrived_or_was_refused() {
        let cases = [
            (
                json(
                    200,
                    r#"{"task_id":"","base_resp":{"status_code":1002,"status_msg":"rate limit"}}"#,
                ),
                true,
            ),
            (json(400, r#"{"error":"bad request"}"#), true),
            (Err(TransportFailure::Connect), true),
            // Each of these may have reached MiniMax and created a task.
            (Err(TransportFailure::Timeout), false),
            (Err(TransportFailure::Io), false),
            (Err(TransportFailure::Tls), false),
            (json(502, "bad gateway"), false),
            (
                json(
                    200,
                    r#"{"task_id":"12/../x","base_resp":{"status_code":0}}"#,
                ),
                false,
            ),
        ];
        for (answer, refused) in cases {
            let error = client([answer]).submit_video(&request()).unwrap_err();
            assert_eq!(error.certainly_not_accepted(), refused, "{error}");
        }
        let unsent = VideoRequest {
            first_frame: Vec::new(),
            ..request()
        };
        let error = client([]).submit_video(&unsent).unwrap_err();
        assert_eq!(
            error,
            MinimaxError::InvalidRequest("first frame is empty or too large")
        );
        assert!(error.certainly_not_accepted());
    }

    #[test]
    fn a_download_that_is_not_an_mp4_is_refused() {
        let client = client([
            json(
                200,
                r#"{"file":{"download_url":"https://cdn.example.test/v.mp4"},"base_resp":{"status_code":0}}"#,
            ),
            Ok(HttpResponse {
                status: 200,
                body: b"<html>expired</html>".to_vec(),
            }),
        ]);
        assert_eq!(
            client.fetch_video("447576721990100"),
            Err(MinimaxError::Malformed("download is not an MP4 file"))
        );
    }

    #[test]
    fn clip_length_and_price_follow_the_published_table() {
        assert_eq!(VideoDuration::nearest(4.0), VideoDuration::Six);
        assert_eq!(VideoDuration::nearest(8.0), VideoDuration::Six);
        assert_eq!(VideoDuration::nearest(9.5), VideoDuration::Ten);
        let client = client([]);
        assert_eq!(
            client
                .estimate_video_cost(VideoDuration::Six, VideoResolution::P768)
                .amount_micros,
            280_000
        );
        assert!(
            client
                .submit_video(&VideoRequest {
                    prompt: " ".to_owned(),
                    ..request()
                })
                .is_err()
        );
        assert!(!format!("{:?}", request()).contains("255"));
    }
}
