//! JSON files of model replies, keyed by content fingerprint, model, and prompt version.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedReply {
    raw: String,
}

pub fn cache_key(
    fingerprint: &str,
    model: &str,
    prompt_kind: &str,
    prompt_version: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(fingerprint.as_bytes());
    hasher.update(b"\0");
    hasher.update(model.as_bytes());
    hasher.update(b"\0");
    hasher.update(prompt_kind.as_bytes());
    hasher.update(b"\0");
    hasher.update(prompt_version.as_bytes());
    hasher.finalize().to_hex().to_string()
}

pub fn cache_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.json"))
}

pub fn load(dir: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(cache_path(dir, key)).ok()?;
    let cached: CachedReply = serde_json::from_str(&text).ok()?;
    Some(cached.raw)
}

pub fn store(dir: &Path, key: &str, raw: &str) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let body = serde_json::to_string(&CachedReply {
        raw: raw.to_string(),
    })
    .map_err(std::io::Error::other)?;
    fs::write(cache_path(dir, key), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("gnosis-llm-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let key = cache_key("fp", "gemma:2b", "code", "1");
        store(&dir, &key, "{\"summary\":\"x\"}").unwrap();
        assert_eq!(load(&dir, &key).as_deref(), Some("{\"summary\":\"x\"}"));
        assert!(load(&dir, "missing").is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
