//! Prompt packs for LLM semantic extraction.
//!
//! Selection is driven by object extension / media type so scans can use
//! specialized instructions (code review, image analysis, OCR, …) without
//! changing the provider wiring.

mod packs;

use crate::connectors::types::ObjectDescriptor;
use serde::{Deserialize, Serialize};

/// Bump when prompt-pack text changes. Included in LLM provenance and cache keys.
pub const PROMPT_PACK_VERSION: &str = "1";

/// Which prompt pack to use for an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    /// Source-code semantic extraction (purpose, APIs, concepts).
    Code,
    /// Diff / patch / review-oriented analysis.
    CodeReview,
    /// Documentation and prose.
    Docs,
    /// Structured config / tabular data.
    Data,
    /// Visual description and UI/diagram understanding.
    Image,
    /// Text extraction from scans / PDFs / TIFF.
    Ocr,
    /// Catch-all when nothing else fits.
    Generic,
}

impl PromptKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::CodeReview => "code_review",
            Self::Docs => "docs",
            Self::Data => "data",
            Self::Image => "image",
            Self::Ocr => "ocr",
            Self::Generic => "generic",
        }
    }

    /// Whether the user turn should attach binary content (images / PDFs).
    pub fn wants_binary(self) -> bool {
        matches!(self, Self::Image | Self::Ocr)
    }
}

/// Resolved prompt text for one LLM call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptPack {
    pub kind: PromptKind,
    pub system: String,
    /// User prefix before the object metadata / content payload.
    pub user_preamble: &'static str,
}

impl PromptPack {
    pub fn get(kind: PromptKind) -> Self {
        packs::pack(kind)
    }
}

/// Pick a prompt kind from object metadata.
pub fn select_prompt_kind(object: &ObjectDescriptor) -> PromptKind {
    let ext = object
        .extension
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let media = object.media_type.as_str();
    let path = object.relative_path.to_string_lossy().to_ascii_lowercase();

    if matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico"
    ) || media.starts_with("image/")
    {
        return PromptKind::Image;
    }

    if matches!(ext.as_str(), "pdf" | "tif" | "tiff")
        || media == "application/pdf"
        || media == "image/tiff"
    {
        return PromptKind::Ocr;
    }

    if matches!(ext.as_str(), "diff" | "patch")
        || path.contains("/review/")
        || path.contains("code_review")
        || path.ends_with("_review.md")
    {
        return PromptKind::CodeReview;
    }

    if matches!(
        ext.as_str(),
        "rs" | "cpp"
            | "cc"
            | "cxx"
            | "c"
            | "h"
            | "hpp"
            | "ex"
            | "exs"
            | "py"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "go"
            | "java"
            | "kt"
            | "swift"
            | "rb"
            | "php"
            | "cs"
            | "scala"
    ) {
        return PromptKind::Code;
    }

    let is_structured_text = matches!(
        ext.as_str(),
        "json" | "yaml" | "yml" | "toml" | "csv" | "tsv"
    );
    if matches!(
        ext.as_str(),
        "md" | "markdown" | "mdx" | "rst" | "adoc" | "txt" | "text" | "log"
    ) || (media.starts_with("text/") && !is_structured_text)
    {
        return PromptKind::Docs;
    }

    if matches!(
        ext.as_str(),
        "json" | "yaml" | "yml" | "toml" | "csv" | "tsv" | "xml" | "ini"
    ) {
        return PromptKind::Data;
    }

    PromptKind::Generic
}

/// Shared JSON schema instructions appended to every system prompt.
pub const RESPONSE_SCHEMA: &str = r#"Respond with a single JSON object only (no markdown fences) using this schema:
{
  "summary": "one-paragraph understanding of the object",
  "status": "understood" | "partial" | "unknown",
  "entities": [
    {
      "kind": "string (e.g. concept, api, module, finding, person, ui_element, extracted_text)",
      "name": "short name",
      "attributes": { "key": "value" },
      "confidence": "high" | "medium" | "low" | "inferred"
    }
  ],
  "relationships": [
    {
      "kind": "string (e.g. depends_on, documents, implements, mentions)",
      "from": "entity name",
      "to": "entity name",
      "confidence": "high" | "medium" | "low" | "inferred"
    }
  ],
  "diagnostics": [
    { "severity": "info" | "warning" | "error", "message": "string" }
  ]
}
Prefer precise, grounded entities. Mark uncertain items confidence=inferred. Do not invent file paths that are not evidenced."#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::types::ObjectDescriptor;
    use crate::ids::ObjectId;
    use std::path::PathBuf;

    fn desc(rel: &str, media: &str) -> ObjectDescriptor {
        let path = PathBuf::from(rel);
        let extension = path.extension().map(|e| e.to_string_lossy().to_string());
        ObjectDescriptor {
            id: ObjectId::new(format!("obj:{rel}")),
            path: path.clone(),
            relative_path: path,
            is_dir: false,
            size: 1,
            modified: None,
            media_type: media.into(),
            extension,
        }
    }

    #[test]
    fn selects_image_ocr_code_docs_data() {
        assert_eq!(
            select_prompt_kind(&desc("assets/logo.png", "image/png")),
            PromptKind::Image
        );
        assert_eq!(
            select_prompt_kind(&desc("scan.pdf", "application/pdf")),
            PromptKind::Ocr
        );
        assert_eq!(
            select_prompt_kind(&desc("src/main.rs", "text/x-rust")),
            PromptKind::Code
        );
        assert_eq!(
            select_prompt_kind(&desc("changes.diff", "text/x-diff")),
            PromptKind::CodeReview
        );
        assert_eq!(
            select_prompt_kind(&desc("README.md", "text/markdown")),
            PromptKind::Docs
        );
        assert_eq!(
            select_prompt_kind(&desc("config.yaml", "application/yaml")),
            PromptKind::Data
        );
        assert_eq!(
            select_prompt_kind(&desc("blob.bin", "application/octet-stream")),
            PromptKind::Generic
        );
    }

    #[test]
    fn every_kind_has_a_pack() {
        for kind in [
            PromptKind::Code,
            PromptKind::CodeReview,
            PromptKind::Docs,
            PromptKind::Data,
            PromptKind::Image,
            PromptKind::Ocr,
            PromptKind::Generic,
        ] {
            let pack = PromptPack::get(kind);
            assert_eq!(pack.kind, kind);
            assert!(!pack.system.is_empty());
            assert!(!pack.user_preamble.is_empty());
        }
    }
}
