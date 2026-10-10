//! Bridges from [`super::LlmBackend`] to OpenAI, Anthropic, Gemini, and Ollama.
//!
//! Each adapter forces genai onto that vendor. Model-name guessing stays in
//! [`super::LlmVendor::from_model`]; the adapter then rewrites the id to
//! `vendor::model` so a name like `tinyllama:latest` cannot be sent to OpenAI.

use super::backend::{
    execute, ollama_host_from_env, ollama_openai_base, LlmBackend, LlmCompletion, LlmRequest,
    LlmVendor,
};
use crate::error::Result;
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ServiceTarget};
use std::sync::Arc;

pub struct OpenAiAdapter {
    client: Client,
}

pub struct AnthropicAdapter {
    client: Client,
}

pub struct GeminiAdapter {
    client: Client,
}

pub struct OllamaAdapter {
    client: Client,
}

impl OpenAiAdapter {
    pub fn new() -> Self {
        Self {
            client: Client::default(),
        }
    }
}

impl Default for OpenAiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicAdapter {
    pub fn new() -> Self {
        Self {
            client: Client::default(),
        }
    }
}

impl Default for AnthropicAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl GeminiAdapter {
    pub fn new() -> Self {
        Self {
            client: Client::default(),
        }
    }
}

impl Default for GeminiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl OllamaAdapter {
    pub fn new() -> Self {
        Self::with_host(&ollama_host_from_env())
    }

    pub fn with_host(host: &str) -> Self {
        let base = ollama_openai_base(host);
        let client = Client::builder()
            .with_service_target_resolver_fn(
                move |mut target: ServiceTarget| -> genai::resolver::Result<ServiceTarget> {
                    target.endpoint = Endpoint::from_owned(base.clone());
                    target.auth = AuthData::from_single("ollama");
                    Ok(target)
                },
            )
            .build();
        Self { client }
    }
}

impl Default for OllamaAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmBackend for OpenAiAdapter {
    fn vendor(&self) -> LlmVendor {
        LlmVendor::OpenAi
    }

    fn complete(&self, model: &str, request: &LlmRequest) -> Result<LlmCompletion> {
        execute(&self.client, self.vendor(), model, request)
    }
}

impl LlmBackend for AnthropicAdapter {
    fn vendor(&self) -> LlmVendor {
        LlmVendor::Anthropic
    }

    fn complete(&self, model: &str, request: &LlmRequest) -> Result<LlmCompletion> {
        execute(&self.client, self.vendor(), model, request)
    }
}

impl LlmBackend for GeminiAdapter {
    fn vendor(&self) -> LlmVendor {
        LlmVendor::Gemini
    }

    fn complete(&self, model: &str, request: &LlmRequest) -> Result<LlmCompletion> {
        execute(&self.client, self.vendor(), model, request)
    }
}

impl LlmBackend for OllamaAdapter {
    fn vendor(&self) -> LlmVendor {
        LlmVendor::Ollama
    }

    fn complete(&self, model: &str, request: &LlmRequest) -> Result<LlmCompletion> {
        execute(&self.client, self.vendor(), model, request)
    }
}

/// Pick the adapter for `model`.
pub fn backend_for(model: &str) -> Arc<dyn LlmBackend> {
    match LlmVendor::from_model(model) {
        LlmVendor::OpenAi => Arc::new(OpenAiAdapter::new()),
        LlmVendor::Anthropic => Arc::new(AnthropicAdapter::new()),
        LlmVendor::Gemini => Arc::new(GeminiAdapter::new()),
        LlmVendor::Ollama => Arc::new(OllamaAdapter::new()),
    }
}
