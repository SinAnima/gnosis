//! Remove high-confidence secret material from text that may be logged or stored.

/// Kind of secret found, if any. The matched text itself is not returned.
pub fn secret_kind(text: &str) -> Option<&'static str> {
    if text.contains("-----BEGIN ") && text.contains("PRIVATE KEY-----") {
        return Some("private_key");
    }
    if has_aws_access_key(text) {
        return Some("aws_access_key");
    }
    if has_sk_token(text) {
        return Some("api_token");
    }
    if has_assigned_secret(text) {
        return Some("assigned_secret");
    }
    None
}

/// Replace detected secret spans with `[redacted]` so job errors and the TUI stay safe.
pub fn scrub_secrets(text: &str) -> String {
    if secret_kind(text).is_none() {
        return text.to_string();
    }
    let mut out = text.to_string();
    out = redact_between(&out, "-----BEGIN ", "PRIVATE KEY-----");
    out = redact_marker(&out, "AKIA", 4, 16, is_aws_char);
    out = redact_marker(&out, "sk-", 3, 20, is_token_char);
    for marker in ["api_key=", "api-key=", "apikey=", "API_KEY=", "API-KEY="] {
        out = redact_marker(&out, marker, marker.len(), 16, is_token_char);
    }
    out
}

fn has_aws_access_key(text: &str) -> bool {
    marker_span(text, "AKIA", 4, 16, is_aws_char).is_some()
}

fn has_sk_token(text: &str) -> bool {
    marker_span(text, "sk-", 3, 20, is_token_char).is_some()
}

fn has_assigned_secret(text: &str) -> bool {
    ["api_key=", "api-key=", "apikey=", "API_KEY=", "API-KEY="]
        .iter()
        .any(|m| marker_span(text, m, m.len(), 16, is_token_char).is_some())
}

fn is_aws_char(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit()
}

fn is_token_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

fn marker_span(
    text: &str,
    marker: &str,
    marker_len: usize,
    min_tail: usize,
    tail_ok: fn(u8) -> bool,
) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut search = 0;
    while let Some(rel) = text[search..].find(marker) {
        let start = search + rel;
        let tail_at = start + marker_len;
        if tail_at > bytes.len() {
            return None;
        }
        let mut end = tail_at;
        while end < bytes.len() && tail_ok(bytes[end]) {
            end += 1;
        }
        if end - tail_at >= min_tail {
            return Some((start, end));
        }
        search = start + marker.len();
    }
    None
}

fn redact_marker(
    text: &str,
    marker: &str,
    marker_len: usize,
    min_tail: usize,
    tail_ok: fn(u8) -> bool,
) -> String {
    let Some((start, end)) = marker_span(text, marker, marker_len, min_tail, tail_ok) else {
        return text.to_string();
    };
    let mut out = String::new();
    out.push_str(&text[..start]);
    out.push_str("[redacted]");
    out.push_str(&redact_marker(
        &text[end..],
        marker,
        marker_len,
        min_tail,
        tail_ok,
    ));
    out
}

fn redact_between(text: &str, start_m: &str, end_m: &str) -> String {
    let Some(start) = text.find(start_m) else {
        return text.to_string();
    };
    let Some(end_rel) = text[start..].find(end_m) else {
        return text.to_string();
    };
    let end = start + end_rel + end_m.len();
    let mut out = String::new();
    out.push_str(&text[..start]);
    out.push_str("[redacted]");
    out.push_str(&redact_between(&text[end..], start_m, end_m));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_without_returning_the_secret() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nABC\n-----END RSA PRIVATE KEY-----";
        assert_eq!(secret_kind(pem), Some("private_key"));
        assert!(!scrub_secrets(pem).contains("BEGIN RSA"));
        assert_eq!(
            secret_kind("key AKIAIOSFODNN7EXAMPLE tail"),
            Some("aws_access_key")
        );
        assert_eq!(secret_kind("sk-abcdefghijklmnopqrst"), Some("api_token"));
        assert_eq!(
            secret_kind("api_key=abcdefghijklmnop"),
            Some("assigned_secret")
        );
        assert!(secret_kind("hello world").is_none());
        let scrubbed = scrub_secrets("prefix AKIAIOSFODNN7EXAMPLE suffix");
        assert!(!scrubbed.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(scrubbed.contains("[redacted]"));
    }
}
