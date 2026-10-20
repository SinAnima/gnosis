//! Gnosis — enterprise knowledge compiler.
//!
//! Library API for scanning repositories, extracting structured knowledge, and
//! exporting OKF-style bundles. The `gnosis` binary is a thin CLI over this crate.

pub mod config;
pub mod connectors;
pub mod content;
pub mod error;
pub mod events;
pub mod exporter;
pub mod ids;
pub mod intrinsic;
pub mod jobs;
pub mod knowledge;
pub mod knowledge_db;
pub mod link;
pub mod okf;
pub mod pipeline;
pub mod prompts;
pub mod provider;
pub mod providers;
pub mod query;
pub mod review;
pub mod scrub;
pub mod status;
pub mod store;
pub mod tui;
pub mod watch;

pub use config::{LlmConfig, LlmMetrics, LlmMode, ScanConfig, ScanMetrics, ScanSource};
pub use connectors::{
    is_s3_uri, parse_s3_uri, FilesystemConnector, GitContext, GitProtoData, MemoryS3Backend,
    ObjectDescriptor, ProtoData, S3Backend, S3Connector, S3Location,
};
pub use content::{BytesContentReader, ContentReader, LimitedReader};
pub use error::{GnosisError, Result};
pub use events::PipelineEvent;
pub use exporter::Exporter;
pub use ids::{EntityId, ObjectId, ProviderId, RelationshipId};
pub use jobs::{
    format_job_detail, format_job_list, format_job_list_filtered, format_job_list_line,
    new_scan_id, parse_age, parse_job_id_list, pause_jobs, pause_scan, prepare_rerun,
    prepare_rerun_scan, resolve_job_id, resolve_scan_id, stop_jobs, stop_scan, unpause_jobs,
    unpause_scan, AnalyzeObjectArgs, AnalyzeObjectResult, Job, JobExecutor, JobId, JobListFilter,
    JobStatus, JobStore, JobSummary, JobWorkerPool, MemoryJobStore, RedbJobStore, RerunReport,
    RetryPolicy, KIND_ANALYZE_OBJECT,
};
pub use knowledge::{
    AnalysisResult, AttributeMap, Confidence, Diagnostic, DiagnosticSeverity, Entity, Evidence,
    KnowledgeRecord, Relationship, SourceSpan,
};
pub use knowledge_db::{format_diff, KnowledgeDb, ReanalyzeReason, Snapshot};
pub use okf::OkfExporter;
pub use pipeline::{Pipeline, PipelineHandle};
pub use prompts::{select_prompt_kind, PromptKind, PromptPack, RESPONSE_SCHEMA};
pub use provider::{ProviderRegistry, Support, UnderstandingProvider};
pub use providers::{
    build_registry, default_registry, CSharpProvider, CppProvider, CsvProvider, ElixirProvider,
    ExifProvider, GenericMetadataProvider, HtmlProvider, JavaProvider, JavaScriptProvider,
    JsonProvider, MarkdownProvider, OfficeProvider, PdfProvider, PlainTextProvider, PythonProvider,
    RustProvider, TomlProvider, TypeScriptProvider, YamlProvider,
};
#[cfg(feature = "llm")]
pub use providers::{
    parse_llm_json, strip_json_fence, AnthropicAdapter, GeminiAdapter, LlmCompletion,
    LlmSemanticProvider, LlmStamp, LlmVendor, OllamaAdapter, OpenAiAdapter, StaticBackend,
};
pub use query::{ExplainResult, GraphResult, QueryEngine};
pub use status::UnderstandingStatus;
pub use store::{FindResults, GraphNeighborhood, InventoryCounts, KnowledgeStore, StoredObject};
pub use tui::{drain_events_headless, TuiApp};
