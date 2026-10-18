//! URL identity for web objects. The fragment is not part of the id.

use url::Url;

/// How `gnosis scan` should treat a target string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetClass {
    Filesystem,
    S3,
    Web(String),
    /// `postgresql://` or `postgres://`. The raw string is parsed later so a password can be dropped.
    Catalog(String),
    Unsupported(String),
}

/// Classify a scan target. An existing local path wins over a string that
/// merely contains a colon. `scheme://` is required before a string is a URL.
pub fn classify_target(raw: &str, path_exists: bool) -> TargetClass {
    if path_exists {
        return TargetClass::Filesystem;
    }
    let Some((scheme, _)) = raw.split_once("://") else {
        return TargetClass::Filesystem;
    };
    if !scheme
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '.' || c == '-')
        || scheme.is_empty()
    {
        return TargetClass::Filesystem;
    }
    match scheme.to_ascii_lowercase().as_str() {
        "s3" => TargetClass::S3,
        "postgresql" | "postgres" => TargetClass::Catalog(raw.to_string()),
        "http" | "https" => match canonicalize(raw) {
            Ok(url) => TargetClass::Web(url),
            Err(message) => TargetClass::Unsupported(message),
        },
        "file" => TargetClass::Unsupported(
            "file:// URLs are not supported; pass a filesystem path".into(),
        ),
        other => TargetClass::Unsupported(format!("unsupported URL scheme '{other}'")),
    }
}

/// Canonical identity: lowercase scheme and host, drop the default port and
/// the fragment, keep the query in its original order.
pub fn canonicalize(raw: &str) -> Result<String, String> {
    let mut url = Url::parse(raw).map_err(|e| format!("invalid URL: {e}"))?;
    match url.scheme() {
        "http" | "https" => {}
        "file" => {
            return Err("file:// URLs are not supported; pass a filesystem path".into());
        }
        other => return Err(format!("unsupported URL scheme '{other}'")),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLs with credentials are not supported".into());
    }
    url.set_fragment(None);
    if url.path().is_empty() {
        url.set_path("/");
    }
    Ok(url.to_string())
}

pub fn object_id_for(canonical_url: &str) -> String {
    format!("obj:web:{canonical_url}")
}

/// Join a link against a page URL and return a canonical http(s) URL.
pub fn resolve_link(page: &str, link: &str) -> Option<String> {
    let base = Url::parse(page).ok()?;
    let joined = base.join(link).ok()?;
    if joined.scheme() != "http" && joined.scheme() != "https" {
        return None;
    }
    canonicalize(joined.as_str()).ok()
}

pub fn same_origin(start: &str, candidate: &str) -> bool {
    let (Ok(a), Ok(b)) = (Url::parse(start), Url::parse(candidate)) else {
        return false;
    };
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

pub fn in_scope(start: &str, candidate: &str, include_subdomains: bool) -> bool {
    if same_origin(start, candidate) {
        return true;
    }
    if !include_subdomains {
        return false;
    }
    let (Ok(a), Ok(b)) = (Url::parse(start), Url::parse(candidate)) else {
        return false;
    };
    if a.scheme() != b.scheme() {
        return false;
    }
    let (Some(host_a), Some(host_b)) = (a.host_str(), b.host_str()) else {
        return false;
    };
    let host_a = host_a.to_ascii_lowercase();
    let host_b = host_b.to_ascii_lowercase();
    host_b == host_a || host_b.ends_with(&format!(".{host_a}"))
}

pub fn extension_for_media(media: &str) -> Option<String> {
    let media = media
        .split(';')
        .next()
        .unwrap_or(media)
        .trim()
        .to_ascii_lowercase();
    let ext = match media.as_str() {
        "text/html" | "application/xhtml+xml" => "html",
        "text/markdown" | "text/x-markdown" => "md",
        "text/plain" => "txt",
        "application/json" | "text/json" => "json",
        "text/yaml" | "application/yaml" | "application/x-yaml" | "text/x-yaml" => "yaml",
        "application/toml" | "text/toml" => "toml",
        "text/csv" => "csv",
        "text/css" => "css",
        "text/javascript" | "application/javascript" | "application/x-javascript" => "js",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "font/woff2" => "woff2",
        "font/woff" => "woff",
        _ => return None,
    };
    Some(ext.into())
}

pub fn sanitize_display(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Drop a CSI / OSC sequence so titles cannot rewrite the terminal.
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        if c.is_control() && c != '\n' && c != '\t' {
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_paths_are_not_urls() {
        assert_eq!(
            classify_target("./fixtures/mixed-repo", true),
            TargetClass::Filesystem
        );
        assert_eq!(
            classify_target("C:/work/repo", false),
            TargetClass::Filesystem
        );
        assert_eq!(
            classify_target("notes:draft", false),
            TargetClass::Filesystem
        );
    }

    #[test]
    fn http_urls_canonicalize_and_reject_secrets() {
        assert_eq!(
            classify_target("HTTPS://Example.COM:443/a#frag", false),
            TargetClass::Web("https://example.com/a".into())
        );
        assert!(matches!(
            classify_target("https://user:pw@example.com/", false),
            TargetClass::Unsupported(_)
        ));
        assert!(matches!(
            classify_target("file:///tmp/x", false),
            TargetClass::Unsupported(_)
        ));
        assert!(matches!(
            classify_target("postgresql://user:s3cret@localhost/trading", false),
            TargetClass::Catalog(_)
        ));
        assert!(matches!(
            classify_target("postgres://localhost/trading", false),
            TargetClass::Catalog(_)
        ));
        assert!(matches!(
            classify_target("unknown://example.com", false),
            TargetClass::Unsupported(_)
        ));
    }

    #[test]
    fn scope_is_origin_or_subdomain() {
        assert!(same_origin("https://ex.com/a", "https://ex.com/b"));
        assert!(!same_origin("https://ex.com/a", "https://other.com/"));
        assert!(in_scope("https://ex.com/", "https://a.ex.com/x", true));
        assert!(!in_scope("https://ex.com/", "https://a.ex.com/x", false));
        assert!(!in_scope("https://ex.com/", "https://notex.com/", true));
    }
}
