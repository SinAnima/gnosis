use crate::config::{ScanConfig, ScanMetrics, ScanSource};
use crate::connectors::filesystem::{
    collect_neighbors, fingerprint_bytes, permissions_string, read_object_bytes,
    FilesystemConnector,
};
use crate::connectors::git::GitContext;
use crate::connectors::s3::{parse_s3_uri, AwsS3Backend, S3Backend, S3Connector};
use crate::connectors::types::{ObjectDescriptor, ProtoData};
use crate::content::BytesContentReader;
use crate::error::{GnosisError, Result};
use crate::events::PipelineEvent;
use crate::jobs::{
    prepare_rerun, prepare_rerun_scan, AnalyzeObjectArgs, AnalyzeObjectResult, Job, JobExecutor,
    JobStore, JobWorkerPool, RedbJobStore, RerunReport, KIND_ANALYZE_OBJECT,
};
use crate::provider::ProviderRegistry;
use crate::status::UnderstandingStatus;
use crate::store::{KnowledgeStore, StoredObject};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use uuid::Uuid;

pub struct Pipeline {
    config: ScanConfig,
    providers: ProviderRegistry,
    store: Arc<Mutex<KnowledgeStore>>,
    metrics: Arc<ScanMetrics>,
    cancel: Arc<AtomicBool>,
    job_store: Arc<dyn JobStore>,
}

pub struct PipelineHandle {
    pub events: Option<Receiver<PipelineEvent>>,
    pub store: Arc<Mutex<KnowledgeStore>>,
    pub metrics: Arc<ScanMetrics>,
    pub cancel: Arc<AtomicBool>,
    pub job_store: Arc<dyn JobStore>,
    join: Option<thread::JoinHandle<Result<()>>>,
}

impl PipelineHandle {
    pub fn take_events(&mut self) -> Receiver<PipelineEvent> {
        self.events
            .take()
            .expect("events already taken from PipelineHandle")
    }

    pub fn wait(&mut self) -> Result<()> {
        if let Some(handle) = self.join.take() {
            handle
                .join()
                .map_err(|_| GnosisError::Pipeline("worker panicked".into()))?
        } else {
            Ok(())
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// How workers fetch object bytes / neighbors for a scan.
#[derive(Clone)]
enum ObjectAccess {
    Filesystem,
    S3(Arc<S3Connector<AwsS3Backend>>),
    /// Filesystem + optional S3 backend (for job reruns with mixed sources).
    Mixed {
        s3: Option<Arc<AwsS3Backend>>,
    },
}

impl ObjectAccess {
    fn connector_name(&self, object: &ObjectDescriptor) -> &'static str {
        if path_looks_like_s3(&object.path) {
            "s3"
        } else {
            match self {
                Self::Filesystem | Self::Mixed { .. } => "filesystem",
                Self::S3(_) => "s3",
            }
        }
    }

    fn read_bytes(&self, object: &ObjectDescriptor, max_size: u64) -> Result<Vec<u8>> {
        match self {
            Self::Filesystem => read_object_bytes(&object.path, max_size),
            Self::S3(c) => c.read_object_bytes(object, max_size),
            Self::Mixed { s3 } => {
                if path_looks_like_s3(&object.path) {
                    let backend = s3.as_ref().ok_or_else(|| {
                        GnosisError::Job("s3 job requires AWS credentials / region".into())
                    })?;
                    let (bucket, key) = s3_bucket_and_key(&object.path)?;
                    backend.get_object(&bucket, &key, max_size)
                } else {
                    read_object_bytes(&object.path, max_size)
                }
            }
        }
    }

    fn neighbors(&self, object: &ObjectDescriptor, limit: usize) -> Vec<String> {
        match self {
            Self::Filesystem => collect_neighbors(&object.path, limit),
            Self::S3(c) => c.collect_neighbors(object, limit),
            Self::Mixed { .. } => {
                if path_looks_like_s3(&object.path) {
                    Vec::new()
                } else {
                    collect_neighbors(&object.path, limit)
                }
            }
        }
    }

    fn permissions(&self, object: &ObjectDescriptor) -> Option<String> {
        match self {
            Self::Filesystem => permissions_string(&object.path),
            Self::S3(_) => None,
            Self::Mixed { .. } => {
                if path_looks_like_s3(&object.path) {
                    None
                } else {
                    permissions_string(&object.path)
                }
            }
        }
    }
}

fn path_looks_like_s3(path: &std::path::Path) -> bool {
    path.to_string_lossy()
        .get(..5)
        .is_some_and(|s| s.eq_ignore_ascii_case("s3://"))
}

fn s3_bucket_and_key(path: &std::path::Path) -> Result<(String, String)> {
    let s = path.to_string_lossy();
    let loc = parse_s3_uri(&s)?;
    if loc.prefix.is_empty() {
        return Err(GnosisError::Job(format!("s3 path missing object key: {s}")));
    }
    Ok((loc.bucket, loc.prefix))
}

struct AnalyzeExecutor {
    providers: Arc<ProviderRegistry>,
    config: ScanConfig,
    git: GitContext,
    use_git: bool,
    access: ObjectAccess,
    store: Arc<Mutex<KnowledgeStore>>,
    metrics: Arc<ScanMetrics>,
    events: Sender<PipelineEvent>,
}

impl JobExecutor for AnalyzeExecutor {
    fn execute(&self, job: &Job) -> Result<serde_json::Value> {
        if job.kind != KIND_ANALYZE_OBJECT {
            return Err(GnosisError::Job(format!("unknown job kind: {}", job.kind)));
        }
        let args = AnalyzeObjectArgs::from_json(&job.args)
            .map_err(|e| GnosisError::Job(format!("bad analyze_object args: {e}")))?;
        let object = args.to_descriptor();
        let result = process_object(
            &object,
            &self.providers,
            &self.config,
            &self.git,
            self.use_git,
            &self.access,
            &self.store,
            &self.metrics,
            &self.events,
        )?;
        serde_json::to_value(result).map_err(|e| GnosisError::Job(format!("encode result: {e}")))
    }
}

impl Pipeline {
    pub fn new(config: ScanConfig, providers: ProviderRegistry) -> Result<Self> {
        let job_store: Arc<dyn JobStore> = Arc::new(RedbJobStore::open(&config.job_db_path)?);
        Ok(Self::with_job_store(config, providers, job_store))
    }

    /// Build a pipeline with an injected [`JobStore`] (tests / custom backends).
    pub fn with_job_store(
        config: ScanConfig,
        providers: ProviderRegistry,
        job_store: Arc<dyn JobStore>,
    ) -> Self {
        Self::with_job_store_and_metrics(config, providers, job_store, Arc::new(ScanMetrics::new()))
    }

    /// Build a pipeline with shared [`ScanMetrics`] (so LLM counters match the TUI).
    pub fn with_job_store_and_metrics(
        config: ScanConfig,
        providers: ProviderRegistry,
        job_store: Arc<dyn JobStore>,
        metrics: Arc<ScanMetrics>,
    ) -> Self {
        let mut store = KnowledgeStore::new();
        store.set_root(config.root.clone());
        store.set_connector(config.connector_name().to_string());
        store.set_enabled_providers(
            providers
                .coverage_summary()
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
        );
        Self {
            config,
            providers,
            store: Arc::new(Mutex::new(store)),
            metrics,
            cancel: Arc::new(AtomicBool::new(false)),
            job_store,
        }
    }

    pub fn store(&self) -> Arc<Mutex<KnowledgeStore>> {
        Arc::clone(&self.store)
    }

    pub fn job_store(&self) -> Arc<dyn JobStore> {
        Arc::clone(&self.job_store)
    }

    pub fn metrics(&self) -> Arc<ScanMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Requeue and re-execute jobs identified by id tokens (full id or unique prefix).
    ///
    /// Jobs are reset to `Pending` under a fresh `rerun:…` scan id, then processed
    /// by the same analyze workers used during a scan.
    pub fn rerun_jobs(
        job_store: Arc<dyn JobStore>,
        queries: &[String],
        concurrency: usize,
        max_object_size: u64,
        region: Option<&str>,
        retry: crate::jobs::RetryPolicy,
    ) -> Result<RerunReport> {
        let report = prepare_rerun(Arc::clone(&job_store), queries)?;
        Self::execute_rerun(
            job_store,
            report,
            concurrency,
            max_object_size,
            region,
            retry,
        )
    }

    /// Requeue and re-execute every job belonging to `source_scan` (id or unique prefix).
    pub fn rerun_scan(
        job_store: Arc<dyn JobStore>,
        source_scan: &str,
        concurrency: usize,
        max_object_size: u64,
        region: Option<&str>,
        retry: crate::jobs::RetryPolicy,
    ) -> Result<RerunReport> {
        let report = prepare_rerun_scan(Arc::clone(&job_store), source_scan)?;
        Self::execute_rerun(
            job_store,
            report,
            concurrency,
            max_object_size,
            region,
            retry,
        )
    }

    fn execute_rerun(
        job_store: Arc<dyn JobStore>,
        report: RerunReport,
        concurrency: usize,
        max_object_size: u64,
        region: Option<&str>,
        retry: crate::jobs::RetryPolicy,
    ) -> Result<RerunReport> {
        let needs_s3 = report.requeued.iter().any(|id| {
            job_store
                .get(id)
                .ok()
                .flatten()
                .map(|j| {
                    j.args
                        .get("path")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .unwrap_or_default()
                })
                .map(|p| p.to_ascii_lowercase().starts_with("s3://"))
                .unwrap_or(false)
        });

        let access = ObjectAccess::Mixed {
            s3: if needs_s3 {
                Some(Arc::new(AwsS3Backend::new(region)?))
            } else {
                None
            },
        };

        let config = ScanConfig {
            max_object_size,
            concurrency: concurrency.max(1),
            retry,
            ..ScanConfig::default()
        };

        let providers = crate::providers::default_registry();
        let mut knowledge = KnowledgeStore::new();
        knowledge.set_connector("rerun".to_string());
        knowledge.set_enabled_providers(
            providers
                .coverage_summary()
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
        );

        let (tx, _rx) = bounded(64);
        let metrics = Arc::new(ScanMetrics::new());
        metrics.start();
        metrics
            .objects_discovered
            .store(report.requeued.len() as u64, Ordering::Relaxed);

        let executor: Arc<dyn JobExecutor> = Arc::new(AnalyzeExecutor {
            providers: Arc::new(providers),
            config: config.clone(),
            git: GitContext::default(),
            use_git: false,
            access,
            store: Arc::new(Mutex::new(knowledge)),
            metrics: Arc::clone(&metrics),
            events: tx,
        });

        let cancel = Arc::new(AtomicBool::new(false));
        let poll = Duration::from_millis(config.job_poll_ms.max(1));
        let workers = config.concurrency.max(1);
        let pool = JobWorkerPool::with_retry(
            Arc::clone(&job_store),
            executor,
            report.scan_id.clone(),
            Arc::clone(&cancel),
            workers,
            poll,
            config.retry,
        );
        pool.mark_enqueue_done();

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("gnosis-rerun")
            .worker_threads(workers.clamp(2, 8))
            .build()
            .map_err(|e| GnosisError::Pipeline(format!("tokio runtime: {e}")))?;

        runtime.block_on(pool.run())?;

        let summary = job_store.summary(&report.scan_id)?;
        eprintln!(
            "{}",
            report.format_summary(summary.completed, summary.failed)
        );
        Ok(report)
    }

    /// Run the scan on a background thread; returns an event receiver.
    pub fn spawn(self) -> PipelineHandle {
        let (tx, rx) = bounded(self.config.event_history_len.max(64));
        let store = Arc::clone(&self.store);
        let metrics = Arc::clone(&self.metrics);
        let cancel = Arc::clone(&self.cancel);
        let job_store = Arc::clone(&self.job_store);
        let join = thread::spawn(move || self.run(tx));
        PipelineHandle {
            events: Some(rx),
            store,
            metrics,
            cancel,
            job_store,
            join: Some(join),
        }
    }

    /// Run synchronously, forwarding events to `tx`.
    pub fn run(self, events: Sender<PipelineEvent>) -> Result<()> {
        self.metrics.start();
        let root = self.config.root.clone();
        let scan_id = format!("scan:{}", Uuid::new_v4());
        {
            let mut store = self.store.lock().unwrap();
            store.set_scan_id(scan_id.clone());
        }
        let _ = events.send(PipelineEvent::ScanStarted {
            root: root.clone(),
            scan_id: scan_id.clone(),
        });

        let job_store = Arc::clone(&self.job_store);
        let _ = job_store.reclaim_stale(&scan_id)?;

        let git = if self.config.source.uses_git() {
            let git = GitContext::detect(&root);
            {
                let mut store = self.store.lock().unwrap();
                store.set_git_branch(git.branch.clone());
            }
            git
        } else {
            GitContext::default()
        };

        let (objects, access) = match &self.config.source {
            ScanSource::Filesystem { .. } => {
                let connector = FilesystemConnector::new(self.config.clone());
                let objects = match connector.discover(&events, &self.cancel) {
                    Ok(o) => o,
                    Err(GnosisError::Cancelled) => return Err(GnosisError::Cancelled),
                    Err(e) => {
                        let _ = events.send(PipelineEvent::Failure {
                            id: None,
                            message: e.to_string(),
                        });
                        return Err(e);
                    }
                };
                (objects, ObjectAccess::Filesystem)
            }
            ScanSource::S3 { location, region } => {
                let backend = Arc::new(AwsS3Backend::new(region.as_deref())?);
                let connector = Arc::new(S3Connector::new(
                    location.clone(),
                    self.config.clone(),
                    backend,
                ));
                let objects = match connector.discover(&events, &self.cancel) {
                    Ok(o) => o,
                    Err(GnosisError::Cancelled) => return Err(GnosisError::Cancelled),
                    Err(e) => {
                        let _ = events.send(PipelineEvent::Failure {
                            id: None,
                            message: e.to_string(),
                        });
                        return Err(e);
                    }
                };
                (objects, ObjectAccess::S3(connector))
            }
        };

        self.metrics
            .objects_discovered
            .store(objects.len() as u64, Ordering::Relaxed);

        let executor: Arc<dyn JobExecutor> = Arc::new(AnalyzeExecutor {
            providers: Arc::new(self.providers),
            config: self.config.clone(),
            git,
            use_git: self.config.source.uses_git(),
            access,
            store: Arc::clone(&self.store),
            metrics: Arc::clone(&self.metrics),
            events: events.clone(),
        });

        let poll = Duration::from_millis(self.config.job_poll_ms.max(1));
        let workers = self.config.concurrency.max(1);
        let pool = JobWorkerPool::with_retry(
            Arc::clone(&job_store),
            executor,
            scan_id.clone(),
            Arc::clone(&self.cancel),
            workers,
            poll,
            self.config.retry,
        );
        let enqueue_done = pool.enqueue_done_flag();

        // Enqueue every discovered artifact as a persistent job, then run async workers.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("gnosis-jobs")
            .worker_threads(workers.clamp(2, 8))
            .build()
            .map_err(|e| GnosisError::Pipeline(format!("tokio runtime: {e}")))?;

        let enqueue_result = runtime.block_on(async {
            let store = Arc::clone(&job_store);
            let cancel = Arc::clone(&self.cancel);
            let events = events.clone();
            let scan_id = scan_id.clone();

            let worker_handle = tokio::spawn(async move { pool.run().await });

            for object in objects {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let args = AnalyzeObjectArgs::from_descriptor(&object);
                let job = Job::new(scan_id.clone(), KIND_ANALYZE_OBJECT, args.to_json());
                let depth = store.summary(&scan_id)?.pending as usize;
                store.enqueue(job)?;
                let _ = events.send(PipelineEvent::ObjectQueued {
                    id: object.id.clone(),
                    queue_depth: depth + 1,
                });
            }
            enqueue_done.store(true, Ordering::Relaxed);

            worker_handle
                .await
                .map_err(|e| GnosisError::Job(format!("worker pool join: {e}")))?
        });

        if let Err(e) = enqueue_result {
            if !matches!(e, GnosisError::Cancelled) {
                let _ = events.send(PipelineEvent::Failure {
                    id: None,
                    message: e.to_string(),
                });
            }
            return Err(e);
        }

        let elapsed = self.metrics.elapsed_ms();
        let discovered = self.metrics.objects_discovered.load(Ordering::Relaxed);
        let _ = events.send(PipelineEvent::MetricsSnapshot {
            metrics: self.metrics.snapshot(0),
        });
        let _ = events.send(PipelineEvent::ScanCompleted {
            objects: discovered,
            elapsed_ms: elapsed,
            scan_id,
        });

        thread::sleep(Duration::from_millis(10));
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn process_object(
    object: &ObjectDescriptor,
    providers: &ProviderRegistry,
    config: &ScanConfig,
    git: &GitContext,
    use_git: bool,
    access: &ObjectAccess,
    store: &Mutex<KnowledgeStore>,
    metrics: &ScanMetrics,
    events: &Sender<PipelineEvent>,
) -> Result<AnalyzeObjectResult> {
    metrics
        .bytes_considered
        .fetch_add(object.size, Ordering::Relaxed);

    let bytes = access.read_bytes(object, config.max_object_size)?;
    let fingerprint = fingerprint_bytes(&bytes);

    let proto = ProtoData {
        connector: access.connector_name(object).into(),
        path: object.path.clone(),
        relative_path: object.relative_path.clone(),
        filename: object
            .relative_path
            .file_name()
            .or_else(|| object.path.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        extension: object.extension.clone(),
        parent_path: object.relative_path.parent().map(|p| p.to_path_buf()),
        neighbor_names: access.neighbors(object, 16),
        media_type: object.media_type.clone(),
        size: object.size,
        modified: object.modified,
        permissions: access.permissions(object),
        fingerprint: Some(fingerprint),
        git: if use_git {
            git.enrich(&object.path)
        } else {
            None
        },
    };

    let (provider, support) = match providers.select(object, &proto) {
        Some(pair) => pair,
        None => {
            let reason = "no provider available".to_string();
            let stored = StoredObject {
                descriptor: object.clone(),
                proto,
                status: UnderstandingStatus::Unknown,
                provider: None,
                classification_reason: Some(reason.clone()),
                entity_ids: Vec::new(),
                diagnostics: Vec::new(),
            };
            store.lock().unwrap().upsert_object(stored);
            metrics.record_status(UnderstandingStatus::Unknown);
            let _ = events.send(PipelineEvent::ObjectClassified {
                id: object.id.clone(),
                status: UnderstandingStatus::Unknown,
                reason: Some(reason.clone()),
            });
            return Ok(AnalyzeObjectResult {
                object_id: object.id.as_str().to_string(),
                status: UnderstandingStatus::Unknown,
                provider: None,
                entities: 0,
                relationships: 0,
                classification_reason: Some(reason),
            });
        }
    };

    let _ = events.send(PipelineEvent::ProviderSelected {
        id: object.id.clone(),
        provider: provider.id(),
        support: support.as_str().into(),
    });
    let _ = events.send(PipelineEvent::AnalysisStarted {
        id: object.id.clone(),
        provider: provider.id(),
    });

    let mut reader = BytesContentReader::new(bytes);
    let result = match provider.analyze(object, &proto, &mut reader) {
        Ok(result) => result,
        Err(err) => {
            let message = crate::scrub::scrub_secrets(&err.to_string());
            let prior = store.lock().unwrap().object_status(&object.id);
            if prior != Some(UnderstandingStatus::Failed) {
                if let Some(prev) = prior {
                    metrics.unrecord_status(prev);
                }
                metrics.record_status(UnderstandingStatus::Failed);
            }
            store.lock().unwrap().upsert_object(StoredObject {
                descriptor: object.clone(),
                proto,
                status: UnderstandingStatus::Failed,
                provider: Some(provider.id()),
                classification_reason: Some(message.clone()),
                entity_ids: Vec::new(),
                diagnostics: vec![message.clone()],
            });
            let _ = events.send(PipelineEvent::ObjectClassified {
                id: object.id.clone(),
                status: UnderstandingStatus::Failed,
                reason: Some(message.clone()),
            });
            return Err(GnosisError::provider(provider.id().as_str(), message));
        }
    };

    let mut entity_ids = Vec::new();
    let prior = store.lock().unwrap().object_status(&object.id);
    {
        let mut st = store.lock().unwrap();
        for entity in &result.record.entities {
            entity_ids.push(entity.id.clone());
            let _ = events.send(PipelineEvent::EntityCreated {
                entity: entity.clone(),
            });
            st.add_entity(entity.clone());
            metrics.entities.fetch_add(1, Ordering::Relaxed);
        }
        for rel in &result.record.relationships {
            let _ = events.send(PipelineEvent::RelationshipCreated {
                kind: rel.kind.clone(),
                from: rel.from.to_string(),
                to: rel.to.to_string(),
            });
            st.add_relationship(rel.clone());
            metrics.relationships.fetch_add(1, Ordering::Relaxed);
        }

        let diagnostics: Vec<String> = result
            .record
            .diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect();

        st.upsert_object(StoredObject {
            descriptor: object.clone(),
            proto,
            status: result.status,
            provider: Some(provider.id()),
            classification_reason: result.classification_reason.clone(),
            entity_ids,
            diagnostics,
        });
    }

    metrics.record_status(result.status);
    if let Some(prev) = prior {
        if prev != result.status {
            metrics.unrecord_status(prev);
        }
    }
    let _ = events.send(PipelineEvent::AnalysisCompleted {
        id: object.id.clone(),
        provider: provider.id(),
        status: result.status,
        entities: result.record.entities.len(),
        relationships: result.record.relationships.len(),
    });
    let _ = events.send(PipelineEvent::ObjectClassified {
        id: object.id.clone(),
        status: result.status,
        reason: result.classification_reason.clone(),
    });

    Ok(AnalyzeObjectResult {
        object_id: object.id.as_str().to_string(),
        status: result.status,
        provider: Some(provider.id().as_str().to_string()),
        entities: result.record.entities.len(),
        relationships: result.record.relationships.len(),
        classification_reason: result.classification_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn s3_path_helpers() {
        assert!(path_looks_like_s3(Path::new("s3://bucket/key")));
        assert!(path_looks_like_s3(Path::new("S3://bucket/key")));
        assert!(!path_looks_like_s3(Path::new("/tmp/file.rs")));

        let (bucket, key) = s3_bucket_and_key(Path::new("s3://b/path/to/obj")).unwrap();
        assert_eq!(bucket, "b");
        assert_eq!(key, "path/to/obj");
        assert!(s3_bucket_and_key(Path::new("s3://b")).is_err());
        assert!(s3_bucket_and_key(Path::new("/not/s3")).is_err());
    }
}
