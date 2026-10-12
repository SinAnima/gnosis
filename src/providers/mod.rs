//! Understanding providers for Gnosis.
//!
//! Deterministic Tree-sitter and lightweight document/data parsers that emit
//! structured knowledge records for the Gnosis pipeline. Optional LLM semantic
//! extraction is available behind the `llm` feature.

#![allow(clippy::too_many_arguments)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::manual_strip)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::match_like_matches_macro)]
#![allow(clippy::single_match)]

mod code;
mod data;
mod docs;
mod generic;
#[cfg(feature = "llm")]
mod llm;
mod util;

pub use code::{CppProvider, ElixirProvider, RustProvider};
pub use data::{CsvProvider, JsonProvider, TomlProvider, YamlProvider};
pub use docs::{HtmlProvider, MarkdownProvider, PlainTextProvider};
pub use generic::GenericMetadataProvider;
#[cfg(feature = "llm")]
pub use llm::{
    parse_llm_json, strip_json_fence, AnthropicAdapter, GeminiAdapter, LlmCompletion,
    LlmSemanticProvider, LlmStamp, LlmVendor, OllamaAdapter, OpenAiAdapter, StaticBackend,
};

use crate::config::{LlmConfig, LlmMetrics};
use crate::ProviderRegistry;
use std::sync::Arc;

/// Build the default deterministic provider set in registration priority order.
pub fn default_registry() -> ProviderRegistry {
    build_registry(None, None)
}

/// Build providers, optionally registering the LLM semantic provider.
///
/// Requires the `llm` Cargo feature for `llm` to take effect. Without it, `llm`
/// is ignored (the CLI rejects `--llm` when the feature is off).
///
/// Pass the same [`LlmMetrics`] arc that lives on [`crate::ScanMetrics`] so the
/// TUI can show request / token counts.
///
/// - `LlmMode::Fallback`: LLM is registered just before the generic metadata
///   provider (Partial for images/OCR/unknown; Full deterministic providers win).
/// - `LlmMode::Always`: LLM is registered before plaintext so Partial text/* can
///   prefer the model; Full Tree-sitter / Markdown / data providers still win.
pub fn build_registry(
    llm: Option<&LlmConfig>,
    llm_metrics: Option<Arc<LlmMetrics>>,
) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(CppProvider));
    registry.register(Box::new(RustProvider));
    registry.register(Box::new(ElixirProvider));
    registry.register(Box::new(MarkdownProvider));
    registry.register(Box::new(HtmlProvider));

    #[cfg(feature = "llm")]
    if let Some(cfg) = llm {
        if cfg.mode == crate::config::LlmMode::Always {
            registry.register(Box::new(LlmSemanticProvider::new(
                cfg.clone(),
                llm_metrics.clone(),
            )));
        }
    }
    #[cfg(not(feature = "llm"))]
    {
        let _ = (&llm, &llm_metrics);
    }

    registry.register(Box::new(PlainTextProvider));
    registry.register(Box::new(JsonProvider));
    registry.register(Box::new(YamlProvider));
    registry.register(Box::new(TomlProvider));
    registry.register(Box::new(CsvProvider));

    #[cfg(feature = "llm")]
    if let Some(cfg) = llm {
        if cfg.mode == crate::config::LlmMode::Fallback {
            registry.register(Box::new(LlmSemanticProvider::new(cfg.clone(), llm_metrics)));
        }
    }

    // Weak fallback — always last among competing Full/Partial peers for unsupported types.
    registry.register(Box::new(GenericMetadataProvider));
    registry
}
