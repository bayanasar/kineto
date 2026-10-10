//! Where a provider may be reached.

use std::net::IpAddr;

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

/// The scheme and authority of `url`, as in `https://example.com:8443`.
#[must_use]
pub fn origin(url: &str) -> Option<&str> {
    let scheme = url.find("://")? + "://".len();
    let end = url[scheme..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |end| scheme + end);
    (end > scheme).then(|| &url[..end])
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
    }

    #[test]
    fn an_origin_is_its_scheme_and_authority() {
        assert_eq!(
            origin("https://a.example:8443/b?c"),
            Some("https://a.example:8443")
        );
        assert_eq!(
            origin("http://localhost:11434"),
            Some("http://localhost:11434")
        );
        assert_eq!(origin("https://"), None);
        assert_eq!(origin("a.example/b"), None);
    }
}
