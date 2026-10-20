use crate::config::{ScanConfig, ScanMetrics, ScanSource};
use crate::connectors::catalog::{CatalogBackend, CatalogConnector, PostgresCatalogBackend};
use crate::connectors::filesystem::{
    collect_neighbors, fingerprint_bytes, permissions_string, read_object_bytes,
    FilesystemConnector,
};
use crate::connectors::git::GitContext;
use crate::connectors::s3::{parse_s3_uri, AwsS3Backend, S3Backend, S3Connector};
use crate::connectors::types::S3ProtoData;
use crate::connectors::types::{ObjectDescriptor, ProtoData};
use crate::connectors::web::WebAcquirer;
use crate::content::BytesContentReader;
use crate::error::{GnosisError, Result};
use crate::events::PipelineEvent;
use crate::jobs::{
    prepare_rerun, prepare_rerun_scan, AnalyzeObjectArgs, AnalyzeObjectResult, Job, JobExecutor,
    JobStore, JobWorkerPool, RedbJobStore, RerunReport, KIND_ANALYZE_OBJECT,
};
use crate::knowledge::{Diagnostic, DiagnosticSeverity};
use crate::knowledge_db::{
    classify_object, collect_deps, dep_matches, unix_ms, Classify, ClassifyInput, KnowledgeDb,
    PersistedObject, ReanalyzeReason, Snapshot, SnapshotObject,
};
use crate::provider::ProviderRegistry;
use crate::status::UnderstandingStatus;
use crate::store::{KnowledgeStore, ReviewSink, StoredObject};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::collections::HashSet;
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
    knowledge: Arc<KnowledgeDb>,
    web_acquirer: Option<Arc<dyn crate::connectors::web::WebAcquirer>>,
    s3_backend: Option<Arc<dyn S3Backend>>,
    catalog_backend: Option<Arc<dyn CatalogBackend>>,
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
    S3(Arc<S3Connector>),
    Web(crate::connectors::web::BodyCache),
    /// Filesystem + optional S3 backend (for job reruns with mixed sources).
    Mixed {
        s3: Option<Arc<dyn S3Backend>>,
        web: crate::connectors::web::BodyCache,
    },
    Catalog(Arc<CatalogConnector>),
}

impl ObjectAccess {
    fn connector_name(&self, object: &ObjectDescriptor) -> &'static str {
        if path_looks_like_s3(&object.path) {
            "s3"
        } else if path_looks_like_http(&object.path) {
            "web"
        } else if path_looks_like_db(&object.path) {
            "catalog"
        } else {
            match self {
                Self::Filesystem | Self::Mixed { .. } => "filesystem",
                Self::S3(_) => "s3",
                Self::Web(_) => "web",
                Self::Catalog(_) => "catalog",
            }
        }
    }

    fn read_bytes(&self, object: &ObjectDescriptor, max_size: u64) -> Result<Vec<u8>> {
        match self {
            Self::Filesystem => read_object_bytes(&object.path, max_size),
            Self::S3(c) => c.read_object_bytes(object, max_size),
            Self::Web(cache) => read_web_bytes(cache, object, max_size),
            Self::Catalog(_) => Ok(Vec::new()),
            Self::Mixed { s3, web } => {
                if path_looks_like_db(&object.path) {
                    Ok(Vec::new())
                } else if path_looks_like_s3(&object.path) {
                    let backend = s3.as_ref().ok_or_else(|| {
                        GnosisError::Job("s3 job requires AWS credentials / region".into())
                    })?;
                    let (bucket, key) = s3_bucket_and_key(&object.path)?;
                    backend.get_object(&bucket, &key, max_size)
                } else if path_looks_like_http(&object.path) {
                    read_web_bytes(web, object, max_size)
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
            Self::Web(_) | Self::Catalog(_) => Vec::new(),
            Self::Mixed { .. } => {
                if path_looks_like_s3(&object.path)
                    || path_looks_like_http(&object.path)
                    || path_looks_like_db(&object.path)
                {
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
            Self::S3(_) | Self::Web(_) | Self::Catalog(_) => None,
            Self::Mixed { .. } => {
                if path_looks_like_s3(&object.path)
                    || path_looks_like_http(&object.path)
                    || path_looks_like_db(&object.path)
                {
                    None
                } else {
                    permissions_string(&object.path)
                }
            }
        }
    }

    fn page(&self, id: &str) -> Option<crate::connectors::web::PageBody> {
        match self {
            Self::Web(cache) => cache.get(id),
            Self::Mixed { web, .. } => web.get(id),
            Self::Filesystem | Self::S3(_) | Self::Catalog(_) => None,
        }
    }

    fn s3_proto(&self, object: &ObjectDescriptor) -> Option<S3ProtoData> {
        match self {
            Self::S3(connector) => connector.proto_for(&object.id),
            _ => None,
        }
    }

    fn s3_notes(&self, object: &ObjectDescriptor) -> Vec<String> {
        match self {
            Self::S3(connector) => connector.notes_for(&object.id),
            _ => Vec::new(),
        }
    }

    fn withheld_body(&self, object: &ObjectDescriptor) -> bool {
        match self {
            Self::S3(connector) => connector.withheld_body(object),
            _ => false,
        }
    }

    fn catalog_proto(
        &self,
        object: &ObjectDescriptor,
    ) -> Option<crate::connectors::types::CatalogProtoData> {
        match self {
            Self::Catalog(connector) => connector.proto_for(&object.id),
            _ => None,
        }
    }
}

fn stored_web_validators(
    knowledge: &Arc<crate::knowledge_db::KnowledgeDb>,
) -> std::collections::HashMap<String, crate::connectors::web::StoredValidators> {
    let Ok(records) = knowledge.load_all() else {
        return std::collections::HashMap::new();
    };
    records
        .into_iter()
        .filter(|record| record.object.descriptor.id.as_str().starts_with("obj:web:"))
        .map(|record| {
            (
                record.object.descriptor.id.as_str().to_string(),
                crate::connectors::web::StoredValidators {
                    etag: record.etag.clone(),
                    last_modified: None,
                    size: record.size,
                    modified: record.modified_unix_ms.and_then(|ms| {
                        std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_millis(ms))
                    }),
                    media_type: record.object.descriptor.media_type.clone(),
                },
            )
        })
        .collect()
}

fn record_crawl(
    metrics: &crate::config::ScanMetrics,
    summary: &crate::connectors::web::CrawlSummary,
) {
    metrics
        .crawl_discovered
        .store(summary.discovered, std::sync::atomic::Ordering::Relaxed);
    metrics
        .crawl_fetched
        .store(summary.fetched, std::sync::atomic::Ordering::Relaxed);
    metrics
        .crawl_blocked
        .store(summary.blocked, std::sync::atomic::Ordering::Relaxed);
    metrics
        .crawl_skipped
        .store(summary.skipped, std::sync::atomic::Ordering::Relaxed);
    metrics
        .crawl_failed
        .store(summary.failed, std::sync::atomic::Ordering::Relaxed);
    metrics
        .crawl_bytes
        .store(summary.bytes, std::sync::atomic::Ordering::Relaxed);
}

fn path_looks_like_http(path: &std::path::Path) -> bool {
    let text = path.to_string_lossy();
    text.starts_with("http://") || text.starts_with("https://")
}

fn read_web_bytes(
    cache: &crate::connectors::web::BodyCache,
    object: &ObjectDescriptor,
    max_size: u64,
) -> Result<Vec<u8>> {
    if let Some(page) = cache.get(object.id.as_str()) {
        if page.bytes.len() as u64 > max_size {
            return Err(GnosisError::Pipeline(format!(
                "web object exceeds max size: {}",
                object.path.display()
            )));
        }
        return Ok(page.bytes);
    }
    crate::connectors::web::refetch_body(&object.path.to_string_lossy(), max_size)
}

fn path_looks_like_s3(path: &std::path::Path) -> bool {
    path.to_string_lossy()
        .get(..5)
        .is_some_and(|s| s.eq_ignore_ascii_case("s3://"))
}

fn path_looks_like_db(path: &std::path::Path) -> bool {
    path.to_string_lossy()
        .get(..5)
        .is_some_and(|s| s.eq_ignore_ascii_case("db://"))
}

fn stored_catalog(
    knowledge: &KnowledgeDb,
    object: &ObjectDescriptor,
) -> Option<crate::connectors::types::CatalogProtoData> {
    if !path_looks_like_db(&object.path) {
        return None;
    }
    knowledge
        .get(&object.id)
        .ok()
        .flatten()
        .and_then(|record| record.object.proto.catalog)
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
    knowledge: Arc<KnowledgeDb>,
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
            &self.knowledge,
        )?;
        serde_json::to_value(result).map_err(|e| GnosisError::Job(format!("encode result: {e}")))
    }
}

impl Pipeline {
    pub fn new(config: ScanConfig, providers: ProviderRegistry) -> Result<Self> {
        let job_store: Arc<dyn JobStore> = Arc::new(RedbJobStore::open(&config.job_db_path)?);
        let knowledge = Arc::new(KnowledgeDb::open(&config.knowledge_db_path)?);
        Ok(Self::with_knowledge(
            config, providers, job_store, knowledge,
        ))
    }

    /// Build a pipeline with an injected [`JobStore`] (tests / custom backends).
    ///
    /// Uses an empty in-memory knowledge database so tests do not share `.gnosis/`.
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
        let knowledge = Arc::new(KnowledgeDb::in_memory().expect("in-memory knowledge db"));
        Self::with_knowledge(config, providers, job_store, knowledge).with_metrics(metrics)
    }

    /// Build a pipeline that loads and commits [`KnowledgeDb`].
    pub fn with_knowledge(
        config: ScanConfig,
        providers: ProviderRegistry,
        job_store: Arc<dyn JobStore>,
        knowledge: Arc<KnowledgeDb>,
    ) -> Self {
        Self::with_knowledge_and_metrics(
            config,
            providers,
            job_store,
            Arc::new(ScanMetrics::new()),
            knowledge,
        )
    }

    pub fn with_knowledge_and_metrics(
        config: ScanConfig,
        providers: ProviderRegistry,
        job_store: Arc<dyn JobStore>,
        metrics: Arc<ScanMetrics>,
        knowledge: Arc<KnowledgeDb>,
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
        store.set_review_sink(Arc::clone(&knowledge) as Arc<dyn ReviewSink>);
        Self {
            config,
            providers,
            store: Arc::new(Mutex::new(store)),
            metrics,
            cancel: Arc::new(AtomicBool::new(false)),
            job_store,
            knowledge,
            web_acquirer: None,
            s3_backend: None,
            catalog_backend: None,
        }
    }

    /// Use a scripted catalog instead of connecting to PostgreSQL.
    pub fn with_catalog_backend(mut self, backend: Arc<dyn CatalogBackend>) -> Self {
        self.catalog_backend = Some(backend);
        self
    }

    /// Use a memory or other test backend instead of the AWS client.
    pub fn with_s3_backend(mut self, backend: Arc<dyn S3Backend>) -> Self {
        self.s3_backend = Some(backend);
        self
    }

    /// Use a test acquirer instead of the production crawler.
    pub fn with_web_acquirer(
        mut self,
        acquirer: Arc<dyn crate::connectors::web::WebAcquirer>,
    ) -> Self {
        self.web_acquirer = Some(acquirer);
        self
    }

    fn with_metrics(mut self, metrics: Arc<ScanMetrics>) -> Self {
        self.metrics = metrics;
        self
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
                Some(Arc::new(AwsS3Backend::new(region, None, false)?) as Arc<dyn S3Backend>)
            } else {
                None
            },
            web: crate::connectors::web::BodyCache::new(),
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
            knowledge: Arc::new(KnowledgeDb::open(&config.knowledge_db_path)?),
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
            ScanSource::S3 {
                location,
                region,
                options,
            } => {
                let backend: Arc<dyn S3Backend> = if let Some(backend) = &self.s3_backend {
                    Arc::clone(backend)
                } else {
                    Arc::new(AwsS3Backend::new(
                        region.as_deref(),
                        options.endpoint.as_deref(),
                        options.insecure,
                    )?)
                };
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
            ScanSource::Catalog { location, options } => {
                let backend: Arc<dyn CatalogBackend> = if let Some(backend) = &self.catalog_backend
                {
                    Arc::clone(backend)
                } else {
                    Arc::new(PostgresCatalogBackend::new(location.clone()))
                };
                let connector = Arc::new(CatalogConnector::new(
                    location.clone(),
                    options.clone(),
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
                (objects, ObjectAccess::Catalog(connector))
            }
            ScanSource::Web { start_url, policy } => {
                let cache = crate::connectors::web::BodyCache::new();
                let known = if self.config.reanalyze {
                    std::collections::HashMap::new()
                } else {
                    stored_web_validators(&self.knowledge)
                };
                let summary = if let Some(acquirer) = &self.web_acquirer {
                    acquirer.crawl(start_url, policy, &cache, &known, &self.cancel)?
                } else {
                    let acquirer = crate::connectors::web::SpiderAcquirer;
                    acquirer.crawl(start_url, policy, &cache, &known, &self.cancel)?
                };
                record_crawl(&self.metrics, &summary);
                for outcome in &summary.outcomes {
                    let _ = events.send(PipelineEvent::Warning {
                        message: format!(
                            "crawl {} {}: {}",
                            outcome.kind.as_str(),
                            outcome.url,
                            outcome.detail
                        ),
                    });
                }
                {
                    let mut store = self.store.lock().unwrap();
                    for outcome in &summary.outcomes {
                        store.record_acquisition(
                            outcome.url.clone(),
                            outcome.kind.as_str().to_string(),
                            outcome.detail.clone(),
                        );
                    }
                }
                (
                    summary.objects.into_iter().map(|o| o.descriptor).collect(),
                    ObjectAccess::Web(cache),
                )
            }
        };

        self.metrics
            .objects_discovered
            .store(objects.len() as u64, Ordering::Relaxed);

        let objects = self.plan_analysis(&objects, &access)?;

        let executor: Arc<dyn JobExecutor> = Arc::new(AnalyzeExecutor {
            providers: Arc::new(self.providers),
            config: self.config.clone(),
            git,
            use_git: self.config.source.uses_git(),
            access,
            store: Arc::clone(&self.store),
            metrics: Arc::clone(&self.metrics),
            events: events.clone(),
            knowledge: Arc::clone(&self.knowledge),
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

        let changed = {
            let mut store = self.store.lock().unwrap();
            crate::link::link_store(&mut store)
        };
        persist_linked(&self.knowledge, &self.store, &changed)?;

        write_snapshot(&self.knowledge, &self.store, &scan_id)?;

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

    /// Load the knowledge database, drop missing objects, and return only objects
    /// that need analysis. Unchanged objects stay in the in-memory store.
    fn plan_analysis(
        &self,
        discovered: &[ObjectDescriptor],
        access: &ObjectAccess,
    ) -> Result<Vec<ObjectDescriptor>> {
        let records = self.knowledge.load_all()?;
        let mut stored: std::collections::HashMap<String, PersistedObject> = records
            .into_iter()
            .map(|record| (record.object.descriptor.id.as_str().to_string(), record))
            .collect();
        {
            let mut store = self.store.lock().unwrap();
            for record in stored.values() {
                store.install_record(
                    record.object.clone(),
                    record.entities.clone(),
                    record.relationships.clone(),
                );
                if let Some(evidence) = &record.evidence {
                    let path = record
                        .object
                        .descriptor
                        .relative_path
                        .to_string_lossy()
                        .to_string();
                    store.set_object_facts(
                        &record.object.descriptor.id,
                        &path,
                        fact_pairs(evidence),
                    );
                }
            }
        }

        let seen: HashSet<String> = discovered
            .iter()
            .map(|object| object.id.as_str().to_string())
            .collect();
        let web_scan = matches!(self.config.source, ScanSource::Web { .. });
        let catalog_scan = matches!(self.config.source, ScanSource::Catalog { .. });
        let stale: Vec<_> = stored
            .keys()
            .filter(|id| {
                if seen.contains(id.as_str()) {
                    return false;
                }
                let web_id = id.starts_with("obj:web:");
                let db_id = id.starts_with("obj:db:");
                if web_scan {
                    web_id
                } else if catalog_scan {
                    db_id
                } else {
                    !web_id && !db_id
                }
            })
            .cloned()
            .collect();
        let mut changed_paths = Vec::new();
        for id in stale {
            let object_id = crate::ids::ObjectId::new(id);
            if let Some(record) = stored.remove(object_id.as_str()) {
                changed_paths.push(
                    record
                        .object
                        .descriptor
                        .relative_path
                        .to_string_lossy()
                        .to_string(),
                );
            }
            {
                let mut store = self.store.lock().unwrap();
                store.remove_object(&object_id);
            }
            self.knowledge.delete(&object_id)?;
        }

        let mut decided: Vec<(ObjectDescriptor, ReanalyzeReason, bool)> = Vec::new();
        for object in discovered {
            let mut input = classify_input(object, stored.get(object.id.as_str()), &self.providers);
            input.reanalyze = self.config.reanalyze;
            let mut decision = classify_object(&input);
            if matches!(decision, Classify::NeedBytes) {
                let over_size =
                    path_looks_like_s3(&object.path) && object.size > self.config.max_object_size;
                if access.withheld_body(object) || over_size {
                    decision = Classify::Analyze(ReanalyzeReason::Content);
                } else {
                    match access.read_bytes(object, self.config.max_object_size) {
                        Ok(bytes) => {
                            input.fingerprint = Some(fingerprint_bytes(&bytes));
                            decision = classify_object(&input);
                        }
                        Err(err) if err.to_string().contains("s3 size limit") => {
                            decision = Classify::Analyze(ReanalyzeReason::Content);
                        }
                        Err(err) => return Err(err),
                    }
                }
            }
            match decision {
                Classify::Keep { touch_stat } => {
                    let stamp = crate::intrinsic::extractor_stamp();
                    let stored_stamp = stored
                        .get(object.id.as_str())
                        .and_then(|record| record.intrinsic_version.clone());
                    if stored_stamp.as_deref() != Some(stamp.as_str()) {
                        decided.push((object.clone(), ReanalyzeReason::Intrinsic, true));
                    } else {
                        if touch_stat {
                            self.knowledge.update_stat(
                                &object.id,
                                object.size,
                                unix_ms(object.modified),
                                object.etag.clone(),
                            )?;
                        }
                        decided.push((object.clone(), ReanalyzeReason::Unchanged, false));
                    }
                }
                Classify::NeedBytes => {
                    decided.push((object.clone(), ReanalyzeReason::Content, true));
                }
                Classify::Analyze(reason) => {
                    if matches!(reason, ReanalyzeReason::New | ReanalyzeReason::Content) {
                        changed_paths.push(object.relative_path.to_string_lossy().to_string());
                    }
                    decided.push((object.clone(), reason, true));
                }
            }
        }

        for item in &mut decided {
            if item.2 {
                continue;
            }
            let Some(record) = stored.get(item.0.id.as_str()) else {
                continue;
            };
            let hit = record
                .deps
                .iter()
                .any(|dep| changed_paths.iter().any(|path| dep_matches(dep, path)));
            if hit {
                item.1 = ReanalyzeReason::Dependency;
                item.2 = true;
            }
        }

        let mut analyze = Vec::new();
        for (object, reason, should_analyze) in decided {
            self.store
                .lock()
                .unwrap()
                .set_reason(&object.id, reason.as_str());
            if should_analyze {
                analyze.push(object);
                continue;
            }
            let status = self
                .store
                .lock()
                .unwrap()
                .object_status(&object.id)
                .unwrap_or(UnderstandingStatus::Unknown);
            self.metrics.record_status(status);
            self.metrics.mark_counted(object.id.as_str());
            self.metrics.objects_skipped.fetch_add(1, Ordering::Relaxed);
            let (entities, rels) = {
                let store = self.store.lock().unwrap();
                let entities = store
                    .get_object(&object.id)
                    .map(|o| o.entity_ids.len())
                    .unwrap_or(0);
                let rels = store.relationship_ids_for(&object.id).len();
                (entities, rels)
            };
            self.metrics
                .entities
                .fetch_add(entities as u64, Ordering::Relaxed);
            self.metrics
                .relationships
                .fetch_add(rels as u64, Ordering::Relaxed);
        }
        Ok(analyze)
    }
}

fn write_snapshot(
    knowledge: &KnowledgeDb,
    store: &Mutex<KnowledgeStore>,
    scan_id: &str,
) -> Result<()> {
    let records = knowledge.load_all()?;
    let store = store.lock().unwrap();
    let mut objects = Vec::new();
    for record in records {
        let id = record.object.descriptor.id.clone();
        let reason = store
            .reason(&id)
            .and_then(parse_reason)
            .unwrap_or(record.reason);
        objects.push(SnapshotObject {
            id: id.as_str().to_string(),
            relative_path: record
                .object
                .descriptor
                .relative_path
                .to_string_lossy()
                .to_string(),
            fingerprint: record.fingerprint.clone(),
            status: record.object.status.as_str().to_string(),
            provider_id: record.provider_id.clone(),
            provider_version: record.provider_version.clone(),
            reason,
            entity_ids: record
                .object
                .entity_ids
                .iter()
                .map(|e| e.to_string())
                .collect(),
            relationship_ids: record
                .relationships
                .iter()
                .map(|r| r.id.as_str().to_string())
                .collect(),
        });
    }
    for deleted in store.deleted_objects() {
        objects.push(SnapshotObject {
            id: deleted.id.clone(),
            relative_path: deleted.relative_path.clone(),
            fingerprint: deleted.fingerprint.clone(),
            status: deleted.status.clone(),
            provider_id: deleted.provider_id.clone(),
            provider_version: deleted.provider_version.clone(),
            reason: ReanalyzeReason::Deleted,
            entity_ids: deleted.entity_ids.clone(),
            relationship_ids: deleted.relationship_ids.clone(),
        });
    }
    objects.sort_by(|a, b| a.id.cmp(&b.id));
    let snapshot = Snapshot {
        id: format!("snap:{}", Uuid::new_v4()),
        scan_id: scan_id.to_string(),
        created_unix_ms: unix_ms(Some(std::time::SystemTime::now())).unwrap_or(0),
        objects,
    };
    knowledge.put_snapshot(&snapshot)
}

#[allow(clippy::too_many_arguments)]
fn parse_reason(value: &str) -> Option<ReanalyzeReason> {
    Some(match value {
        "new" => ReanalyzeReason::New,
        "content" => ReanalyzeReason::Content,
        "provider" => ReanalyzeReason::Provider,
        "dependency" => ReanalyzeReason::Dependency,
        "unchanged" => ReanalyzeReason::Unchanged,
        "deleted" => ReanalyzeReason::Deleted,
        "intrinsic" => ReanalyzeReason::Intrinsic,
        _ => return None,
    })
}

fn classify_input(
    object: &ObjectDescriptor,
    stored: Option<&PersistedObject>,
    providers: &ProviderRegistry,
) -> ClassifyInput {
    let proto = ProtoData {
        media_type: object.media_type.clone(),
        extension: object.extension.clone(),
        relative_path: object.relative_path.clone(),
        filename: object
            .relative_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size: object.size,
        ..ProtoData::default()
    };
    let selected = providers.select(object, &proto);
    ClassifyInput {
        reanalyze: false,
        has_stored: stored.is_some(),
        stored_fingerprint: stored.and_then(|r| r.fingerprint.clone()),
        stored_provider_id: stored.and_then(|r| r.provider_id.clone()),
        stored_provider_version: stored.and_then(|r| r.provider_version.clone()),
        stored_size: stored.map(|r| r.size).unwrap_or(0),
        stored_modified_unix_ms: stored.and_then(|r| r.modified_unix_ms),
        stored_etag: stored.and_then(|r| r.etag.clone()),
        size: object.size,
        modified_unix_ms: unix_ms(object.modified),
        etag: object.etag.clone(),
        provider_id: selected.map(|(p, _)| p.id().as_str().to_string()),
        provider_version: selected.map(|(p, _)| p.version().to_string()),
        fingerprint: None,
        is_s3: path_looks_like_s3(&object.path),
    }
}

fn persist_current(
    knowledge: &KnowledgeDb,
    store: &Mutex<KnowledgeStore>,
    object: &ObjectDescriptor,
    provider_version: Option<&str>,
    entities: &[crate::knowledge::Entity],
    relationships: &[crate::knowledge::Relationship],
    evidence: Option<&crate::intrinsic::ArtifactEvidenceRecord>,
) -> Result<()> {
    let (stored, reason) = {
        let st = store.lock().unwrap();
        let reason = st.reason(&object.id).unwrap_or("content").to_string();
        (st.get_object(&object.id).cloned(), reason)
    };
    let Some(stored) = stored else {
        return Ok(());
    };
    let record = PersistedObject {
        fingerprint: stored.proto.fingerprint.clone(),
        provider_id: stored.provider.as_ref().map(|p| p.as_str().to_string()),
        provider_version: provider_version.map(str::to_string),
        size: object.size,
        modified_unix_ms: unix_ms(object.modified),
        etag: object.etag.clone(),
        deps: collect_deps(entities, relationships),
        reason: parse_reason(&reason).unwrap_or(ReanalyzeReason::Content),
        object: stored,
        entities: entities.to_vec(),
        relationships: relationships.to_vec(),
        evidence: evidence.cloned(),
        intrinsic_version: Some(crate::intrinsic::extractor_stamp()),
    };
    knowledge.put(&record)
}

fn persist_linked(
    knowledge: &KnowledgeDb,
    store: &Mutex<KnowledgeStore>,
    ids: &[crate::ids::ObjectId],
) -> Result<()> {
    for id in ids {
        let Some(mut record) = knowledge.get(id)? else {
            continue;
        };
        let (entities, relationships) = {
            let stored = store.lock().unwrap();
            (stored.entities_of(id), stored.relationships_of(id))
        };
        record.deps = collect_deps(&entities, &relationships);
        record.entities = entities;
        record.relationships = relationships;
        knowledge.put(&record)?;
    }
    Ok(())
}

fn fact_pairs(evidence: &crate::intrinsic::ArtifactEvidenceRecord) -> Vec<(String, String)> {
    evidence
        .intrinsic
        .facts
        .iter()
        .map(|fact| (fact.key.clone(), fact_value_text(&fact.value)))
        .collect()
}

fn fact_value_text(value: &crate::intrinsic::FactValue) -> String {
    match value {
        crate::intrinsic::FactValue::String(text)
        | crate::intrinsic::FactValue::Decimal(text)
        | crate::intrinsic::FactValue::Timestamp(text)
        | crate::intrinsic::FactValue::Uri(text)
        | crate::intrinsic::FactValue::Digest(text) => text.clone(),
        crate::intrinsic::FactValue::Integer(number) => number.to_string(),
        crate::intrinsic::FactValue::Bool(flag) => flag.to_string(),
        crate::intrinsic::FactValue::Array(items) => items.join(","),
        crate::intrinsic::FactValue::Null => String::new(),
    }
}

fn note_status(
    metrics: &ScanMetrics,
    id: &str,
    prior: Option<UnderstandingStatus>,
    status: UnderstandingStatus,
) {
    let counted = metrics.take_counted(id);
    if counted && prior == Some(status) {
        metrics.mark_counted(id);
        return;
    }
    metrics.record_status(status);
    if counted {
        if let Some(prev) = prior {
            if prev != status {
                metrics.unrecord_status(prev);
            }
        }
    }
    metrics.mark_counted(id);
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
    knowledge: &KnowledgeDb,
) -> Result<AnalyzeObjectResult> {
    metrics.objects_analyzed.fetch_add(1, Ordering::Relaxed);
    metrics
        .bytes_considered
        .fetch_add(object.size, Ordering::Relaxed);

    let bytes = match access.read_bytes(object, config.max_object_size) {
        Ok(bytes) => bytes,
        Err(err) if err.to_string().contains("s3 size limit") => Vec::new(),
        Err(err) => return Err(err),
    };
    let fingerprint = fingerprint_bytes(&bytes);
    let page = access.page(object.id.as_str());

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
        fingerprint: Some(fingerprint.clone()),
        git: if use_git {
            git.enrich(&object.path)
        } else {
            None
        },
        http_status: page.as_ref().map(|p| p.status),
        requested_url: page.as_ref().map(|p| p.requested_url.clone()),
        final_url: page.as_ref().map(|p| p.final_url.clone()),
        s3: access.s3_proto(object),
        catalog: access
            .catalog_proto(object)
            .or_else(|| stored_catalog(knowledge, object)),
    };

    let mut evidence = crate::intrinsic::extract_intrinsic(object, &proto, &bytes);
    for message in access.s3_notes(object) {
        evidence.intrinsic.diagnostics.push(Diagnostic {
            severity: DiagnosticSeverity::Warning,
            message,
        });
    }

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
            let prior = store.lock().unwrap().object_status(&object.id);
            store
                .lock()
                .unwrap()
                .replace_analysis(stored, Vec::new(), Vec::new());
            note_status(
                metrics,
                object.id.as_str(),
                prior,
                UnderstandingStatus::Unknown,
            );
            persist_current(knowledge, store, object, None, &[], &[], Some(&evidence))?;
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

    let refresh_only = store
        .lock()
        .unwrap()
        .reason(&object.id)
        .is_some_and(|reason| reason == "intrinsic");
    if refresh_only {
        if let Some(prior) = knowledge.get(&object.id)? {
            let same = prior.fingerprint.as_deref() == Some(fingerprint.as_str())
                && prior.provider_id.as_deref() == Some(provider.id().as_str())
                && prior.provider_version.as_deref() == Some(provider.version());
            if same {
                let raw = prior
                    .evidence
                    .as_ref()
                    .map(|item| item.structural_status)
                    .unwrap_or(prior.object.status);
                crate::intrinsic::apply_structural(
                    &mut evidence,
                    provider.id().as_str(),
                    raw,
                    prior.object.classification_reason.clone(),
                );
                let version = provider.version().to_string();
                let entity_ids: Vec<_> = prior
                    .entities
                    .iter()
                    .map(|entity| entity.id.clone())
                    .collect();
                store.lock().unwrap().replace_analysis(
                    StoredObject {
                        descriptor: object.clone(),
                        proto: proto.clone(),
                        status: evidence.user_status,
                        provider: Some(provider.id()),
                        classification_reason: evidence.status_reason.clone(),
                        entity_ids,
                        diagnostics: prior.object.diagnostics.clone(),
                    },
                    prior.entities.clone(),
                    prior.relationships.clone(),
                );
                persist_current(
                    knowledge,
                    store,
                    object,
                    Some(&version),
                    &prior.entities,
                    &prior.relationships,
                    Some(&evidence),
                )?;
                let _ = events.send(PipelineEvent::ObjectClassified {
                    id: object.id.clone(),
                    status: evidence.user_status,
                    reason: evidence.status_reason.clone(),
                });
                return Ok(AnalyzeObjectResult {
                    object_id: object.id.as_str().to_string(),
                    status: evidence.user_status,
                    provider: Some(provider.id().as_str().to_string()),
                    entities: prior.entities.len(),
                    relationships: prior.relationships.len(),
                    classification_reason: evidence.status_reason,
                });
            }
        }
    }

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
            let version = provider.version().to_string();
            store.lock().unwrap().replace_analysis(
                StoredObject {
                    descriptor: object.clone(),
                    proto,
                    status: UnderstandingStatus::Failed,
                    provider: Some(provider.id()),
                    classification_reason: Some(message.clone()),
                    entity_ids: Vec::new(),
                    diagnostics: vec![message.clone()],
                },
                Vec::new(),
                Vec::new(),
            );
            note_status(
                metrics,
                object.id.as_str(),
                prior,
                UnderstandingStatus::Failed,
            );
            crate::intrinsic::apply_structural(
                &mut evidence,
                provider.id().as_str(),
                UnderstandingStatus::Failed,
                Some(message.clone()),
            );
            persist_current(
                knowledge,
                store,
                object,
                Some(&version),
                &[],
                &[],
                Some(&evidence),
            )?;
            let _ = events.send(PipelineEvent::ObjectClassified {
                id: object.id.clone(),
                status: UnderstandingStatus::Failed,
                reason: Some(message.clone()),
            });
            return Err(GnosisError::provider(provider.id().as_str(), message));
        }
    };

    let entity_ids: Vec<_> = result
        .record
        .entities
        .iter()
        .map(|e| e.id.clone())
        .collect();
    let prior = store.lock().unwrap().object_status(&object.id);
    for entity in &result.record.entities {
        let _ = events.send(PipelineEvent::EntityCreated {
            entity: entity.clone(),
        });
        metrics.entities.fetch_add(1, Ordering::Relaxed);
    }
    for rel in &result.record.relationships {
        let _ = events.send(PipelineEvent::RelationshipCreated {
            kind: rel.kind.clone(),
            from: rel.from.to_string(),
            to: rel.to.to_string(),
        });
        metrics.relationships.fetch_add(1, Ordering::Relaxed);
    }
    let diagnostics: Vec<String> = result
        .record
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect();
    crate::intrinsic::apply_structural(
        &mut evidence,
        provider.id().as_str(),
        result.status,
        result.classification_reason.clone(),
    );
    if access.withheld_body(object) && evidence.user_status == UnderstandingStatus::Understood {
        evidence.user_status = UnderstandingStatus::PartiallyUnderstood;
        evidence.status_reason = Some("s3 listing only; object body not read".into());
    }
    let status = evidence.user_status;
    let classification_reason = evidence.status_reason.clone();
    let version = provider.version().to_string();
    {
        let mut guard = store.lock().unwrap();
        guard.replace_analysis(
            StoredObject {
                descriptor: object.clone(),
                proto,
                status,
                provider: Some(provider.id()),
                classification_reason: classification_reason.clone(),
                entity_ids,
                diagnostics,
            },
            result.record.entities.clone(),
            result.record.relationships.clone(),
        );
        let path = object.relative_path.to_string_lossy().to_string();
        guard.set_object_facts(&object.id, &path, fact_pairs(&evidence));
    }
    note_status(metrics, object.id.as_str(), prior, status);
    persist_current(
        knowledge,
        store,
        object,
        Some(&version),
        &result.record.entities,
        &result.record.relationships,
        Some(&evidence),
    )?;
    let _ = events.send(PipelineEvent::AnalysisCompleted {
        id: object.id.clone(),
        provider: provider.id(),
        status,
        entities: result.record.entities.len(),
        relationships: result.record.relationships.len(),
    });
    let _ = events.send(PipelineEvent::ObjectClassified {
        id: object.id.clone(),
        status,
        reason: classification_reason.clone(),
    });

    Ok(AnalyzeObjectResult {
        object_id: object.id.as_str().to_string(),
        status,
        provider: Some(provider.id().as_str().to_string()),
        entities: result.record.entities.len(),
        relationships: result.record.relationships.len(),
        classification_reason,
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
