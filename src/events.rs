use crate::ids::{ObjectId, ProviderId};
use crate::knowledge::Entity;
use crate::status::UnderstandingStatus;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum PipelineEvent {
    ScanStarted {
        root: PathBuf,
        scan_id: String,
    },
    ScanCompleted {
        objects: u64,
        elapsed_ms: u64,
        scan_id: String,
    },
    ObjectDiscovered {
        id: ObjectId,
        path: PathBuf,
    },
    ObjectQueued {
        id: ObjectId,
        queue_depth: usize,
    },
    ProviderSelected {
        id: ObjectId,
        provider: ProviderId,
        support: String,
    },
    AnalysisStarted {
        id: ObjectId,
        provider: ProviderId,
    },
    AnalysisCompleted {
        id: ObjectId,
        provider: ProviderId,
        status: UnderstandingStatus,
        entities: usize,
        relationships: usize,
    },
    EntityCreated {
        entity: Entity,
    },
    RelationshipCreated {
        kind: String,
        from: String,
        to: String,
    },
    ObjectClassified {
        id: ObjectId,
        status: UnderstandingStatus,
        reason: Option<String>,
    },
    Warning {
        message: String,
    },
    Failure {
        id: Option<ObjectId>,
        message: String,
    },
    ExportStarted {
        path: PathBuf,
    },
    ExportCompleted {
        path: PathBuf,
    },
    MetricsSnapshot {
        metrics: MetricsSnapshot,
    },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub objects_discovered: u64,
    pub bytes_considered: u64,
    pub understood: u64,
    pub partial: u64,
    pub unknown: u64,
    pub failed: u64,
    pub entities: u64,
    pub relationships: u64,
    pub queue_depth: usize,
    pub elapsed_ms: u64,
    pub llm_requests: u64,
    pub llm_errors: u64,
    pub llm_cache_hits: u64,
    pub llm_prompt_tokens: u64,
    pub llm_completion_tokens: u64,
    pub llm_total_tokens: u64,
    pub llm_max_requests: u64,
    pub llm_max_tokens: u64,
    #[serde(default)]
    pub crawl_discovered: u64,
    #[serde(default)]
    pub crawl_fetched: u64,
    #[serde(default)]
    pub crawl_blocked: u64,
    #[serde(default)]
    pub crawl_skipped: u64,
    #[serde(default)]
    pub crawl_failed: u64,
    #[serde(default)]
    pub crawl_bytes: u64,
}

impl PipelineEvent {
    pub fn summary(&self) -> String {
        match self {
            Self::ScanStarted { root, scan_id } => {
                format!("scan started: {} ({scan_id})", root.display())
            }
            Self::ScanCompleted {
                objects,
                elapsed_ms,
                scan_id,
            } => {
                format!("scan completed: {objects} objects in {elapsed_ms}ms ({scan_id})")
            }
            Self::ObjectDiscovered { path, .. } => format!("discovered {}", path.display()),
            Self::ObjectQueued { id, queue_depth } => {
                format!("queued {id} (depth {queue_depth})")
            }
            Self::ProviderSelected {
                id,
                provider,
                support,
            } => {
                format!("provider {provider} for {id} ({support})")
            }
            Self::AnalysisStarted { id, provider } => {
                format!("analyzing {id} with {provider}")
            }
            Self::AnalysisCompleted {
                id,
                provider,
                status,
                entities,
                relationships,
            } => format!("analyzed {id} via {provider}: {status} (+{entities}e/{relationships}r)"),
            Self::EntityCreated { entity } => {
                format!("entity {} {}", entity.kind, entity.name)
            }
            Self::RelationshipCreated { kind, from, to } => {
                format!("rel {kind}: {from} -> {to}")
            }
            Self::ObjectClassified { id, status, reason } => {
                let r = reason.as_deref().unwrap_or("");
                format!("classified {id} as {status} {r}")
            }
            Self::Warning { message } => format!("warning: {message}"),
            Self::Failure { id, message } => match id {
                Some(id) => format!("failure {id}: {message}"),
                None => format!("failure: {message}"),
            },
            Self::ExportStarted { path } => format!("export started: {}", path.display()),
            Self::ExportCompleted { path } => format!("export completed: {}", path.display()),
            Self::MetricsSnapshot { metrics } => {
                if metrics.llm_requests > 0 || metrics.llm_errors > 0 {
                    format!(
                        "metrics: {} objs, {} understood, {} unknown; llm {} req ({} err), {} tok (p{}/c{})",
                        metrics.objects_discovered,
                        metrics.understood,
                        metrics.unknown,
                        metrics.llm_requests,
                        metrics.llm_errors,
                        metrics.llm_total_tokens,
                        metrics.llm_prompt_tokens,
                        metrics.llm_completion_tokens
                    )
                } else {
                    format!(
                        "metrics: {} objs, {} understood, {} unknown",
                        metrics.objects_discovered, metrics.understood, metrics.unknown
                    )
                }
            }
        }
    }
}
