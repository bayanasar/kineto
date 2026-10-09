//! Blocking HTTP for API backends.
//!
//! HTTPS everywhere, except plain HTTP to a loopback address, which is how
//! local model servers listen. No redirects, so a key never follows one.
//! Failures are reduced to a kind; no library error text, which could carry
//! request details, is kept.

use std::{fmt, net::IpAddr, sync::Arc, time::Duration};

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

/// Sends one JSON POST. Production uses [`HttpTransport`]; tests replay
/// recorded answers.
pub trait Transport {
    fn post_json(
        &self,
        url: &str,
        bearer: Option<&SecretString>,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure>;
}

/// A transport shared between backends and threads.
pub type SharedTransport = Arc<dyn Transport + Send + Sync>;

impl Transport for SharedTransport {
    fn post_json(
        &self,
        url: &str,
        bearer: Option<&SecretString>,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        self.as_ref().post_json(url, bearer, body)
    }
}

/// Whether `url` may be used: HTTPS, or HTTP to a loopback host.
///
/// The host must be `localhost` or a loopback IP address written as one.
/// A name that only starts like one (`127.0.0.1.example`) may resolve
/// anywhere, and an authority with user information (`localhost:80@host`)
/// is refused, because the host is what follows the `@`.
#[must_use]
pub fn is_permitted(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains(['@', '\\']) {
        return false;
    }
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    host == "localhost"
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

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
        bearer: Option<&SecretString>,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        if !is_permitted(url) {
            return Err(TransportFailure::Insecure);
        }
        let mut request = self
            .agent
            .post(url)
            .header("Content-Type", "application/json");
        if let Some(bearer) = bearer {
            request = request.header("Authorization", format!("Bearer {}", bearer.expose()));
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
    fn only_https_or_loopback_http_is_permitted() {
        for permitted in [
            "https://api.moonshot.ai/v1/chat/completions",
            "http://localhost:11434/v1/chat/completions",
            "http://127.0.0.1:8080/v1",
            "http://127.0.0.2:8080/v1",
            "http://[::1]:8080/v1",
        ] {
            assert!(is_permitted(permitted), "{permitted}");
        }
        for refused in [
            "http://api.moonshot.ai/v1",
            "http://localhost.evil.test/v1",
            "http://127.0.0.1.evil.test/v1",
            "http://127.attacker.example/v1",
            "http://localhost:80@evil.test/v1",
            "http://127.0.0.1@evil.test/v1",
            "http://localhost\\@evil.test/v1",
            "http://[::ffff:8.8.8.8]/v1",
            "http://10.0.0.2/v1",
            "ftp://localhost/v1",
            "localhost:8080",
        ] {
            assert!(!is_permitted(refused), "{refused}");
        }
        assert_eq!(
            HttpTransport::new().post_json("http://example.test/v1", None, b"{}"),
            Err(TransportFailure::Insecure)
        );
    }
}
