//! Blocking HTTP for API backends.
//!
//! HTTPS everywhere, except plain HTTP to a loopback address, which is how
//! local model servers listen. No redirects, so a key never follows one.
//! Failures are reduced to a kind; no library error text, which could carry
//! request details, is kept.

use std::{fmt, sync::Arc, time::Duration};

use kineto_providers::secret::SecretString;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Long answers from large models take minutes.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
/// A completion is text; anything far larger is not one.
pub const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFailure {
    Connect,
    Timeout,
    Tls,
    Io,
    TooLarge,
    Protocol,
    /// The URL is not HTTPS and not a loopback address.
    Insecure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// How a request carries its key.
#[derive(Debug, Clone, Copy)]
pub enum Auth<'a> {
    /// `Authorization: Bearer <key>`, as OpenAI's protocol takes it.
    Bearer(&'a SecretString),
    /// `x-api-key: <key>`, as Anthropic's protocol takes it.
    ApiKey(&'a SecretString),
}

impl Auth<'_> {
    /// The key it carries.
    #[must_use]
    pub const fn key(&self) -> &SecretString {
        match self {
            Self::Bearer(key) | Self::ApiKey(key) => key,
        }
    }
}

/// Sends one JSON POST. Production uses [`HttpTransport`]; tests replay
/// recorded answers.
pub trait Transport {
    /// POST `body` to `url` with the key `auth` carries, if any, and the
    /// protocol's own `headers`.
    fn post_json(
        &self,
        url: &str,
        auth: Option<Auth<'_>>,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure>;
}

/// A transport shared between backends and threads.
pub type SharedTransport = Arc<dyn Transport + Send + Sync>;

impl Transport for SharedTransport {
    fn post_json(
        &self,
        url: &str,
        auth: Option<Auth<'_>>,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        self.as_ref().post_json(url, auth, headers, body)
    }
}

pub use kineto_providers::net::is_permitted;

#[derive(Clone)]
pub struct HttpTransport {
    agent: ureq::Agent,
}

impl HttpTransport {
    #[must_use]
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            // Checked per request instead: loopback HTTP is allowed.
            .https_only(false)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REQUEST_TIMEOUT))
            .user_agent(concat!("kineto/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self { agent }
    }
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for HttpTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HttpTransport")
    }
}

impl Transport for HttpTransport {
    fn post_json(
        &self,
        url: &str,
        auth: Option<Auth<'_>>,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        if !is_permitted(url) {
            return Err(TransportFailure::Insecure);
        }
        let mut request = self
            .agent
            .post(url)
            .header("Content-Type", "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        match auth {
            Some(Auth::Bearer(key)) => {
                request = request.header("Authorization", format!("Bearer {}", key.expose()));
            }
            Some(Auth::ApiKey(key)) => request = request.header("x-api-key", key.expose()),
            None => {}
        }
        let mut response = request.send(body).map_err(failure)?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(failure)?;
        Ok(HttpResponse { status, body })
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_address_elsewhere_is_never_asked() {
        assert_eq!(
            HttpTransport::new().post_json("http://example.test/v1", None, &[], b"{}"),
            Err(TransportFailure::Insecure)
        );
    }
}
