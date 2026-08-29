//! `ADR-0007`'s strict `Origin` allowlist and normalization.
//!
//! Two independent checks apply at both ticket issuance and WebSocket upgrade
//! (`collab-protocol-v1.md` "鉴权与连接" point 1/3): the request's `Origin` must normalize into
//! the server's configured allowlist, *and*, at upgrade time, must exactly equal the origin the
//! ticket was bound to at issuance. This module only does the first half (parsing + allowlist);
//! the second half is a plain string comparison against `collab_tickets.origin`, done by
//! [`super::ticket`].

/// Normalizes a raw `Origin` header value to `scheme://host[:port]`, lowercasing scheme and host.
///
/// Returns `None` for anything that is not a well-formed `scheme://authority` with no path/query
/// (a browser-sent `Origin` header is never anything else; malformed input is a fail-closed
/// rejection here, not a best-effort parse).
#[must_use]
pub fn normalize(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    let (scheme, rest) = trimmed.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    if rest.is_empty() || rest.contains('/') || rest.contains('?') || rest.contains('#') {
        return None;
    }
    Some(format!("{scheme}://{}", rest.to_ascii_lowercase()))
}

/// Whether a normalized origin belongs to the configured allowlist. `allowlist` entries are
/// expected to already be normalized (`flow.collab_allowed_origins` is validated at config load).
#[must_use]
pub fn is_allowed(normalized_origin: &str, allowlist: &[String]) -> bool {
    allowlist.iter().any(|allowed| allowed == normalized_origin)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::{is_allowed, normalize};

    #[test]
    fn normalize_lowercases_scheme_and_host() {
        assert_eq!(
            normalize("HTTPS://Sylvode.Example:8443").as_deref(),
            Some("https://sylvode.example:8443")
        );
    }

    #[test]
    fn normalize_rejects_a_path_or_query() {
        assert!(normalize("https://sylvode.example/path").is_none());
        assert!(normalize("https://sylvode.example?x=1").is_none());
    }

    #[test]
    fn normalize_rejects_a_non_http_scheme() {
        assert!(normalize("ftp://sylvode.example").is_none());
        assert!(normalize("javascript:alert(1)").is_none());
    }

    #[test]
    fn normalize_rejects_empty_and_whitespace() {
        assert!(normalize("").is_none());
        assert!(normalize("https:// sylvode.example").is_none());
    }

    #[test]
    fn allowlist_match_is_exact_and_case_normalized_first() {
        let allowlist = vec!["https://sylvode.example".to_string()];
        let normalized = normalize("HTTPS://SYLVODE.EXAMPLE").expect("normalizes");
        assert!(is_allowed(&normalized, &allowlist));
        assert!(!is_allowed("https://evil.example", &allowlist));
    }

    #[test]
    fn empty_allowlist_allows_nothing() {
        assert!(!is_allowed("https://sylvode.example", &[]));
    }
}
