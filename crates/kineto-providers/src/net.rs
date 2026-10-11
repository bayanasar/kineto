//! Where a provider may be reached.

use std::net::IpAddr;

/// How many redirects a file is followed through.
pub const MAX_REDIRECTS: usize = 3;

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
    url.starts_with("http://")
        && host(url).is_some_and(|host| {
            host == "localhost"
                || host
                    .parse::<IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        })
}

/// Whether a service at `from` may send kineto on to `to`, as an address
/// in its answer or a redirect: `to` is permitted and names no user, and it
/// is on this computer only when `from` is. A service elsewhere never leads
/// kineto to one on this computer, which it could not reach itself.
#[must_use]
pub fn may_lead_to(from: &str, to: &str) -> bool {
    is_permitted(to) && host(to).is_some() && (is_this_computer(from) || !is_this_computer(to))
}

/// Where a redirect from `from` to `location` leads, when it may be
/// followed ([`may_lead_to`]). A relative `location` is resolved against
/// `from`; one in another scheme, or only a fragment, is not followed.
#[must_use]
pub fn redirect(from: &str, location: &str) -> Option<String> {
    let origin = origin(from)?;
    let scheme = &origin[..=origin.find(':')?];
    let path = &from[origin.len()..from.find(['?', '#']).unwrap_or(from.len())];
    let first = location.split(['/', '?', '#']).next().unwrap_or_default();
    let to = if location.starts_with("https://") || location.starts_with("http://") {
        location.to_owned()
    } else if location.starts_with("//") {
        format!("{scheme}{location}")
    } else if location.starts_with('/') {
        format!("{origin}{location}")
    } else if location.starts_with('?') {
        format!("{origin}{path}{location}")
    } else if location.is_empty() || location.starts_with('#') || first.contains(':') {
        return None;
    } else {
        let directory = &path[..path.rfind('/').map_or(0, |slash| slash + 1)];
        format!("{origin}/{}{location}", directory.trim_start_matches('/'))
    };
    may_lead_to(from, &to).then_some(to)
}

/// The host `url` names, or `None` when it has no scheme or names a user:
/// its host would then be what follows the `@`.
fn host(url: &str) -> Option<&str> {
    let rest = &url[url.find("://")? + "://".len()..];
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains(['@', '\\']) {
        return None;
    }
    Some(if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    })
}

/// Whether `url` names this computer, in any spelling a resolver takes for
/// it: `localhost` and its subdomains, a loopback address, or the
/// unspecified one.
fn is_this_computer(url: &str) -> bool {
    host(url).is_some_and(|host| {
        let name = host.trim_end_matches('.').to_ascii_lowercase();
        name == "localhost"
            || name.ends_with(".localhost")
            || name.parse::<IpAddr>().is_ok_and(|address| {
                let address = address.to_canonical();
                address.is_loopback() || address.is_unspecified()
            })
    })
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
    fn a_service_elsewhere_never_leads_to_this_computer() {
        let api = "https://ark.example/api/v3";
        for allowed in [
            "https://files.example/clip.mp4",
            "https://ark.example/clip.mp4",
        ] {
            assert!(may_lead_to(api, allowed), "{allowed}");
        }
        for refused in [
            "http://127.0.0.1:8080/admin",
            "http://localhost/admin",
            "https://localhost/admin",
            "https://LocalHost./admin",
            "https://app.localhost/admin",
            "https://127.0.0.1/admin",
            "https://[::1]/admin",
            "https://[::ffff:127.0.0.1]/admin",
            "https://0.0.0.0/admin",
            "http://10.0.0.2/admin",
            "http://169.254.169.254/latest/meta-data",
            "https://user@files.example/clip.mp4",
            "file:///etc/passwd",
        ] {
            assert!(!may_lead_to(api, refused), "{refused}");
        }
        // A service on this computer may point at a file on it.
        let local = "http://127.0.0.1:8080/api/v3";
        assert!(may_lead_to(local, "http://127.0.0.1:8081/clip.mp4"));
        assert!(may_lead_to(local, "https://files.example/clip.mp4"));
        assert!(!may_lead_to(local, "http://10.0.0.2/clip.mp4"));
    }

    #[test]
    fn a_redirect_is_resolved_and_checked_before_it_is_followed() {
        let from = "https://files.example/a/b/clip.mp4?sig=1";
        for (location, to) in [
            ("https://cdn.example/x.mp4", "https://cdn.example/x.mp4"),
            ("//cdn.example/x.mp4", "https://cdn.example/x.mp4"),
            ("/x.mp4", "https://files.example/x.mp4"),
            ("x.mp4?sig=2", "https://files.example/a/b/x.mp4?sig=2"),
            ("?sig=2", "https://files.example/a/b/clip.mp4?sig=2"),
        ] {
            assert_eq!(redirect(from, location).as_deref(), Some(to), "{location}");
        }
        assert_eq!(
            redirect("https://files.example", "x.mp4").as_deref(),
            Some("https://files.example/x.mp4")
        );
        for refused in [
            "http://127.0.0.1/admin",
            "http://localhost:3100/api",
            "//127.0.0.1/admin",
            "http://cdn.example/x.mp4",
            "https://user@cdn.example/x.mp4",
            "ftp://cdn.example/x.mp4",
            "javascript:alert(1)",
            "#here",
            "",
        ] {
            assert_eq!(redirect(from, refused), None, "{refused}");
        }
        // On this computer, a redirect may stay on it.
        assert_eq!(
            redirect("http://127.0.0.1:8080/clip.mp4", "/moved.mp4").as_deref(),
            Some("http://127.0.0.1:8080/moved.mp4")
        );
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
