//! Provider-neutral chat interface. Vendor adapters implement [`LlmBackend`].

use crate::error::{GnosisError, Result};
use crate::scrub::scrub_secrets;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatResponseFormat, ContentPart};
use genai::Client;

/// Which chat vendor handles a model id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LlmVendor {
    OpenAi,
    Anthropic,
    Gemini,
    Ollama,
}

impl LlmVendor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::Ollama => "ollama",
        }
    }

    /// Namespace prefix genai uses to force an adapter (`openai::gpt-4o-mini`).
    pub fn namespace(self) -> &'static str {
        self.as_str()
    }

    /// Infer a vendor from a model id. An explicit `vendor::model` prefix wins.
    /// Names that are not OpenAI, Anthropic, or Gemini fall through to Ollama
    /// (`tinyllama:latest`, `gpt-oss:20b`).
    pub fn from_model(model: &str) -> Self {
        let (ns, name) = split_namespace(model);
        if let Some(ns) = ns {
            return match ns.to_ascii_lowercase().as_str() {
                "openai" => Self::OpenAi,
                "anthropic" | "claude" => Self::Anthropic,
                "gemini" | "google" => Self::Gemini,
                "ollama" => Self::Ollama,
                _ => Self::infer(name),
            };
        }
        Self::infer(model)
    }

    fn infer(model: &str) -> Self {
        let m = model.to_ascii_lowercase();
        if m.starts_with("claude") {
            Self::Anthropic
        } else if m.starts_with("gemini") {
            Self::Gemini
        } else if m.starts_with("gpt-oss") || m.contains(':') {
            Self::Ollama
        } else if m.starts_with("gpt")
            || m.starts_with("o1")
            || m.starts_with("o3")
            || m.starts_with("o4")
            || m.starts_with("chatgpt")
        {
            Self::OpenAi
        } else {
            Self::Ollama
        }
    }
}

/// Force `vendor::name`, dropping any prefix already on `model`.
pub fn route_model(vendor: LlmVendor, model: &str) -> String {
    let name = split_namespace(model).1;
    format!("{}::{name}", vendor.namespace())
}

fn split_namespace(model: &str) -> (Option<&str>, &str) {
    match model.split_once("::") {
        Some((ns, name)) if !ns.is_empty() && !name.is_empty() => (Some(ns), name),
        _ => (None, model),
    }
}

/// One completion request. Text is required; an image is attached for vision packs.
#[derive(Clone, Debug)]
pub struct LlmRequest {
    pub system: String,
    pub user_text: String,
    pub image: Option<LlmImage>,
    /// When set, the adapter asks the vendor to stop after this many completion tokens.
    pub max_tokens: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct LlmImage {
    pub media_type: String,
    pub base64: String,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmCompletion {
    pub text: String,
    pub prompt_tokens: Option<i32>,
    pub completion_tokens: Option<i32>,
    pub total_tokens: Option<i32>,
}

/// Chat backend used by the semantic provider. Implementations must not leak vendor SDK types.
pub trait LlmBackend: Send + Sync {
    fn vendor(&self) -> LlmVendor;
    fn complete(&self, model: &str, request: &LlmRequest) -> Result<LlmCompletion>;
}

/// Returns a recorded completion. Integration tests use this instead of the network.
pub struct StaticBackend {
    vendor: LlmVendor,
    completion: LlmCompletion,
}

impl StaticBackend {
    pub fn new(vendor: LlmVendor, completion: LlmCompletion) -> Self {
        Self { vendor, completion }
    }
}

impl LlmBackend for StaticBackend {
    fn vendor(&self) -> LlmVendor {
        self.vendor
    }

    fn complete(&self, _model: &str, _request: &LlmRequest) -> Result<LlmCompletion> {
        Ok(self.completion.clone())
    }
}

pub(crate) fn execute(
    client: &Client,
    vendor: LlmVendor,
    model: &str,
    request: &LlmRequest,
) -> Result<LlmCompletion> {
    let routed = route_model(vendor, model);
    let mut parts = vec![ContentPart::from_text(request.user_text.clone())];
    if let Some(image) = &request.image {
        parts.push(ContentPart::from_binary_base64(
            image.media_type.clone(),
            image.base64.clone(),
            image.name.clone(),
        ));
    }
    let chat_req = ChatRequest::new(vec![
        ChatMessage::system(request.system.clone()),
        ChatMessage::user(parts),
    ]);
    let mut options = ChatOptions::default()
        .with_temperature(0.1)
        .with_response_format(ChatResponseFormat::JsonMode);
    if let Some(max_tokens) = request.max_tokens {
        options = options.with_max_tokens(max_tokens);
    }
    let client = client.clone();
    block_on(async move {
        let response = client
            .exec_chat(&routed, chat_req, Some(&options))
            .await
            .map_err(|e| {
                GnosisError::provider(
                    "llm-semantic",
                    scrub_secrets(&format!(
                        "{vendor} chat failed: {e}",
                        vendor = vendor.as_str()
                    )),
                )
            })?;
        let text = response
            .first_text()
            .map(str::to_string)
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| GnosisError::provider("llm-semantic", "empty model response"))?;
        Ok(LlmCompletion {
            text,
            prompt_tokens: response.usage.prompt_tokens,
            completion_tokens: response.usage.completion_tokens,
            total_tokens: response.usage.total_tokens,
        })
    })
}

fn block_on<F, T>(fut: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| {
                    GnosisError::provider("llm-semantic", format!("tokio runtime: {e}"))
                })?;
            rt.block_on(fut)
        }
    }
}

/// `http://127.0.0.1:11434` or `…/v1` becomes the OpenAI-compatible base `…/v1/`.
pub fn ollama_openai_base(host: &str) -> String {
    let trimmed = host.trim().trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        format!("{trimmed}/")
    } else {
        format!("{trimmed}/v1/")
    }
}

pub fn ollama_host_from_env() -> String {
    std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_routing() {
        assert_eq!(LlmVendor::from_model("gpt-4o-mini"), LlmVendor::OpenAi);
        assert_eq!(
            LlmVendor::from_model("claude-3-5-haiku-20241022"),
            LlmVendor::Anthropic
        );
        assert_eq!(LlmVendor::from_model("gemini-2.5-flash"), LlmVendor::Gemini);
        assert_eq!(LlmVendor::from_model("tinyllama:latest"), LlmVendor::Ollama);
        assert_eq!(LlmVendor::from_model("gpt-oss:20b"), LlmVendor::Ollama);
        assert_eq!(
            LlmVendor::from_model("ollama::tinyllama:latest"),
            LlmVendor::Ollama
        );
        assert_eq!(
            LlmVendor::from_model("openai::gpt-4o-mini"),
            LlmVendor::OpenAi
        );
        assert_eq!(
            route_model(LlmVendor::Ollama, "tinyllama:latest"),
            "ollama::tinyllama:latest"
        );
        assert_eq!(
            route_model(LlmVendor::OpenAi, "openai::gpt-4o-mini"),
            "openai::gpt-4o-mini"
        );
        assert_eq!(
            ollama_openai_base("http://127.0.0.1:11434"),
            "http://127.0.0.1:11434/v1/"
        );
        assert_eq!(
            ollama_openai_base("http://127.0.0.1:11434/v1/"),
            "http://127.0.0.1:11434/v1/"
        );
    }
}
