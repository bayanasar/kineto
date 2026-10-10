//! Google's video models over the Gemini API: Gemini Omni Flash
//! ([`omni`]), and Veo 3.1 ([`veo`]), whose preview models Google shuts
//! down on 2026-10-22; and Gemini's image models for a connection the
//! director adds ([`image`]).
//!
//! The key goes in an `x-goog-api-key` header, and only to the API's own
//! host. A download that is redirected elsewhere is fetched without it.

pub mod image;
pub mod omni;
pub mod veo;

use std::{fmt, sync::Arc, time::Duration};

use kineto_providers::secret::SecretString;

pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_MESSAGE_CHARS: usize = 200;
/// Largest answer to a POST read: an image answers inline, base64 encoded.
const MAX_POST_ANSWER_BYTES: u64 = 64 * 1024 * 1024;

/// What a transport failed at. Library error text is deliberately not kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFailure {
    Connect,
    Timeout,
    Tls,
    Io,
    TooLarge,
    Protocol,
}

/// An HTTP response as the client needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Talks to the Gemini API. Production uses [`HttpsTransport`]; tests
/// replay recorded responses.
pub trait Transport {
    /// A POST of a JSON body to the API, with the key.
    fn post_json(
        &self,
        url: &str,
        api_key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure>;

    /// A GET from the API, with the key, of an answer at most `max_bytes`
    /// long.
    fn get(
        &self,
        url: &str,
        api_key: &SecretString,
        max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure>;

    /// A GET of a video file on the API, with the key, at most `max_bytes`
    /// long. A redirect to another host is followed without the key.
    fn download(
        &self,
        url: &str,
        api_key: &SecretString,
        max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure>;
}

/// A transport shared between clients and threads.
pub type SharedTransport = Arc<dyn Transport + Send + Sync>;

impl Transport for SharedTransport {
    fn post_json(
        &self,
        url: &str,
        api_key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        self.as_ref().post_json(url, api_key, body)
    }

    fn get(
        &self,
        url: &str,
        api_key: &SecretString,
        max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure> {
        self.as_ref().get(url, api_key, max_bytes)
    }

    fn download(
        &self,
        url: &str,
        api_key: &SecretString,
        max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure> {
        self.as_ref().download(url, api_key, max_bytes)
    }
}

/// Blocking HTTPS with rustls and the bundled Mozilla roots.
#[derive(Clone)]
pub struct HttpsTransport {
    agent: ureq::Agent,
    files: ureq::Agent,
    redirected: ureq::Agent,
}

impl HttpsTransport {
    #[must_use]
    pub fn new() -> Self {
        let agent = |timeout, redirects| -> ureq::Agent {
            ureq::Agent::config_builder()
                .https_only(true)
                .max_redirects(redirects)
                .http_status_as_error(false)
                .timeout_connect(Some(CONNECT_TIMEOUT))
                .timeout_global(Some(timeout))
                .user_agent(concat!("kineto/", env!("CARGO_PKG_VERSION")))
                .build()
                .into()
        };
        Self {
            // A redirect would carry the key to wherever it points.
            agent: agent(REQUEST_TIMEOUT, 0),
            files: agent(DOWNLOAD_TIMEOUT, 0),
            // Where a file was redirected to carries no key.
            redirected: agent(DOWNLOAD_TIMEOUT, 3),
        }
    }
}

impl Default for HttpsTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for HttpsTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HttpsTransport")
    }
}

impl Transport for HttpsTransport {
    fn post_json(
        &self,
        url: &str,
        api_key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        let response = self
            .agent
            .post(url)
            .header("x-goog-api-key", api_key.expose())
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(failure)?;
        read(response, MAX_POST_ANSWER_BYTES)
    }

    fn get(
        &self,
        url: &str,
        api_key: &SecretString,
        max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure> {
        let response = self
            .agent
            .get(url)
            .header("x-goog-api-key", api_key.expose())
            .call()
            .map_err(failure)?;
        read(response, max_bytes)
    }

    fn download(
        &self,
        url: &str,
        api_key: &SecretString,
        max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure> {
        let response = self
            .files
            .get(url)
            .header("x-goog-api-key", api_key.expose())
            .call()
            .map_err(failure)?;
        if !response.status().is_redirection() {
            return read(response, max_bytes);
        }
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .ok_or(TransportFailure::Protocol)?
            .to_owned();
        let response = if same_host(url, &location) {
            self.files
                .get(&location)
                .header("x-goog-api-key", api_key.expose())
                .call()
        } else {
            self.redirected.get(&location).call()
        }
        .map_err(failure)?;
        read(response, max_bytes)
    }
}

/// The scheme and host of `url`, as in `https://example.com`.
pub(crate) fn origin(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some(&url[.."https://".len() + end])
}

pub(crate) fn same_host(one: &str, other: &str) -> bool {
    origin(one).is_some_and(|origin| Some(origin) == self::origin(other))
}

fn read(
    mut response: ureq::http::Response<ureq::Body>,
    max_bytes: u64,
) -> Result<HttpResponse, TransportFailure> {
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(max_bytes)
        .read_to_vec()
        .map_err(failure)?;
    Ok(HttpResponse { status, body })
}

fn failure(error: ureq::Error) -> TransportFailure {
    match error {
        ureq::Error::Timeout(_) => TransportFailure::Timeout,
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => TransportFailure::Connect,
        ureq::Error::Io(_) => TransportFailure::Io,
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::Pem(_) => TransportFailure::Tls,
        ureq::Error::BodyExceedsLimit(_) => TransportFailure::TooLarge,
        _ => TransportFailure::Protocol,
    }
}

/// `message` trimmed and cut to a length an error can carry.
pub(crate) fn clipped(message: &str) -> String {
    let message = message.trim();
    match message.char_indices().nth(MAX_MESSAGE_CHARS) {
        Some((end, _)) => format!("{}…", &message[..end]),
        None => message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_compared_by_scheme_and_host() {
        assert!(same_host(
            "https://generativelanguage.googleapis.com/v1beta/files/abc:download?alt=media",
            DEFAULT_BASE_URL
        ));
        assert!(!same_host(
            "https://generativelanguage.googleapis.com.example.com/x",
            DEFAULT_BASE_URL
        ));
        assert!(!same_host(
            "http://generativelanguage.googleapis.com/x",
            DEFAULT_BASE_URL
        ));
        assert_eq!(origin("https://a.example/b?c"), Some("https://a.example"));
    }
}
