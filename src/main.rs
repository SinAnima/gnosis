use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use gnosis::{
    build_registry, drain_events_headless, format_job_detail, format_job_list_filtered, is_s3_uri,
    parse_age, parse_job_id_list, parse_s3_uri, pause_jobs, pause_scan, prepare_rerun,
    prepare_rerun_scan, resolve_scan_id, stop_jobs, stop_scan, unpause_jobs, unpause_scan,
    Exporter, JobId, JobListFilter, JobStatus, JobStore, LlmConfig, OkfExporter, Pipeline,
    QueryEngine, RedbJobStore, RetryPolicy, ScanConfig, ScanSource, TuiApp,
};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "gnosis",
    version,
    about = "Gnosis — enterprise knowledge compiler",
    long_about = "Compile a local repository, S3 bucket, or documentation site into structured, traceable knowledge (OKF)."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Discover and analyze a local directory, Git repository, or S3 bucket
    Scan {
        /// Local path or `s3://bucket[/prefix]`
        path: String,
        /// Run without the interactive TUI (for CI / scripting)
        #[arg(long)]
        no_tui: bool,
        /// Suppress per-object event lines in headless mode
        #[arg(long)]
        quiet: bool,
        /// Maximum object size in bytes
        #[arg(long, default_value_t = 2 * 1024 * 1024)]
        max_size: u64,
        /// Worker concurrency
        #[arg(long)]
        concurrency: Option<usize>,
        /// OKF export directory (created on `export okf`)
        #[arg(long, default_value = "knowledge.okf")]
        output: PathBuf,
        /// Automatically export OKF when the scan finishes (headless)
        #[arg(long)]
        export: bool,
        /// AWS region for `s3://` scans (otherwise default credential chain)
        #[arg(long)]
        region: Option<String>,
        /// S3-compatible endpoint, for example `http://127.0.0.1:9000` (path-style)
        #[arg(long)]
        s3_endpoint: Option<String>,
        /// Skip TLS verification for `--s3-endpoint`
        #[arg(long)]
        s3_insecure: bool,
        /// HeadObject and GetObjectTagging for each key
        #[arg(long)]
        s3_inspect: bool,
        /// List versions and keep the latest live version. A delete marker is not downloaded
        #[arg(long)]
        s3_versions: bool,
        /// Inventory listing facts and do not call GetObject
        #[arg(long)]
        s3_metadata_only: bool,
        /// Cap on list pages, head, tag, and get (`0` means no cap)
        #[arg(long, default_value_t = 0)]
        s3_max_requests: u64,
        /// Schema to include besides `public`. Repeat to add another. System schemas are never listed
        #[arg(long = "schema")]
        schema: Vec<String>,
        /// Maximum relations a catalog scan inventories
        #[arg(long, default_value_t = 200)]
        db_max_objects: u64,
        /// Catalog statement timeout in milliseconds
        #[arg(long, default_value_t = 5000)]
        db_statement_timeout_ms: u64,
        /// Persistent job queue database (redb). Default: `.gnosis/jobs.redb`
        #[arg(long, default_value = ".gnosis/jobs.redb")]
        job_db: PathBuf,
        /// Persistent knowledge database (redb). Default: `.gnosis/knowledge.redb`
        #[arg(long, default_value = ".gnosis/knowledge.redb")]
        knowledge_db: PathBuf,
        /// Analyze every object even when the fingerprint and provider version match
        #[arg(long)]
        reanalyze: bool,
        /// Max attempts per job including the first (default: 3; `1` disables retries)
        #[arg(long, default_value_t = 3)]
        max_attempts: u32,
        /// Initial retry backoff in milliseconds (doubles each failure)
        #[arg(long, default_value_t = 250)]
        retry_base_ms: u64,
        /// Maximum retry backoff in milliseconds
        #[arg(long, default_value_t = 30_000)]
        retry_max_ms: u64,
        /// Enable LLM semantic extraction (requires build with `--features llm`).
        /// Optional model name; default `gpt-4o-mini`. Provider is inferred by genai
        /// (OpenAI / Anthropic / Gemini / Ollama / …) from the model id + env API key.
        #[arg(long, num_args = 0..=1, default_missing_value = "gpt-4o-mini")]
        llm: Option<String>,
        /// When to use the LLM: `fallback` (images/OCR/unknown) or `always` (Partial on all)
        #[arg(long, default_value = "fallback")]
        llm_mode: String,
        /// Stop LLM calls after this many requests (`0` = unlimited)
        #[arg(long, default_value_t = 0)]
        llm_max_requests: u64,
        /// Stop LLM calls after this many total tokens (`0` = unlimited)
        #[arg(long, default_value_t = 0)]
        llm_max_tokens: u64,
        /// Send objects even when they look like they contain secrets
        #[arg(long)]
        llm_allow_secrets: bool,
        /// Directory for cached LLM replies
        #[arg(long, default_value = ".gnosis/llm-cache")]
        llm_cache_dir: PathBuf,
        /// Maximum HTML pages to fetch (web scans)
        #[arg(long)]
        max_pages: Option<u64>,
        /// Link depth beyond the start URL (web scans)
        #[arg(long)]
        max_depth: Option<u32>,
        /// Simultaneous page fetches (web scans)
        #[arg(long)]
        max_concurrency: Option<usize>,
        /// Delay between page fetches in milliseconds (web scans)
        #[arg(long)]
        request_delay_ms: Option<u64>,
        /// Follow links on subdomains of the start host (web scans)
        #[arg(long)]
        include_subdomains: bool,
        /// Allow loopback, private, and link-local addresses (web scans)
        #[arg(long)]
        allow_private_network: bool,
    },
    /// List or accept pending LLM review marks in an OKF bundle
    Review {
        #[command(subcommand)]
        command: ReviewCommands,
    },
    /// Inspect persisted jobs (list / detail)
    Jobs {
        #[command(subcommand)]
        command: JobsCommands,
    },
    /// Compare two knowledge snapshots (default: the latest two)
    Diff {
        /// Older snapshot id or unique prefix
        from: Option<String>,
        /// Newer snapshot id or unique prefix
        to: Option<String>,
        /// Persistent knowledge database
        #[arg(long, default_value = ".gnosis/knowledge.redb")]
        knowledge_db: PathBuf,
    },
    /// Re-run an incremental filesystem scan when files change
    Watch {
        /// Local directory
        path: String,
        /// Quiet period after the last change, in milliseconds
        #[arg(long, default_value_t = 400)]
        debounce_ms: u64,
        /// Persistent job queue database
        #[arg(long, default_value = ".gnosis/jobs.redb")]
        job_db: PathBuf,
        /// Persistent knowledge database
        #[arg(long, default_value = ".gnosis/knowledge.redb")]
        knowledge_db: PathBuf,
        /// Maximum object size in bytes
        #[arg(long, default_value_t = 2 * 1024 * 1024)]
        max_size: u64,
    },
    /// Print proto, signature, and intrinsic facts for one local file
    Inspect {
        /// File to inspect
        path: PathBuf,
        /// Print JSON
        #[arg(long)]
        json: bool,
        /// Read the stored evidence record when this database already has the file
        #[arg(long)]
        knowledge_db: Option<PathBuf>,
    },
    /// Show product overview
    About,
}

#[derive(Subcommand, Debug)]
enum JobsCommands {
    /// List jobs (optionally filter by status / scan)
    List {
        /// Filter: pending | running | paused | completed | failed | stopped
        #[arg(long, short = 's')]
        status: Option<String>,
        /// Restrict to a scan id (`scan:…` / unique prefix)
        #[arg(long)]
        scan_id: Option<String>,
        /// Max rows (default 100)
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Persistent job queue database
        #[arg(long, default_value = ".gnosis/jobs.redb")]
        job_db: PathBuf,
    },
    /// List known scan ids
    Scans {
        /// Persistent job queue database
        #[arg(long, default_value = ".gnosis/jobs.redb")]
        job_db: PathBuf,
    },
    /// Show a single job in detail
    Show {
        /// Job id (full `job:…` or unique prefix)
        id: String,
        /// Persistent job queue database
        #[arg(long, default_value = ".gnosis/jobs.redb")]
        job_db: PathBuf,
    },
    /// Delete jobs by age and/or scan id
    Purge {
        /// Age threshold (e.g. `5d`, `12h`) — optional when `--scan-id` is set
        age: Option<String>,
        /// Delete all jobs for this scan (or intersect with age)
        #[arg(long)]
        scan_id: Option<String>,
        /// Persistent job queue database
        #[arg(long, default_value = ".gnosis/jobs.redb")]
        job_db: PathBuf,
        /// Print what would be deleted without removing
        #[arg(long)]
        dry_run: bool,
    },
    /// Pause pending/running jobs (ids and/or --scan-id)
    Pause {
        /// Comma-delimited job ids (optional when `--scan-id` is set)
        ids: Option<String>,
        /// Pause every pending/running job in this scan
        #[arg(long)]
        scan_id: Option<String>,
        /// Persistent job queue database
        #[arg(long, default_value = ".gnosis/jobs.redb")]
        job_db: PathBuf,
    },
    /// Resume paused jobs (ids and/or --scan-id)
    Unpause {
        /// Comma-delimited job ids (optional when `--scan-id` is set)
        ids: Option<String>,
        /// Unpause every paused job in this scan
        #[arg(long)]
        scan_id: Option<String>,
        /// Persistent job queue database
        #[arg(long, default_value = ".gnosis/jobs.redb")]
        job_db: PathBuf,
    },
    /// Stop pending/paused/running jobs (ids and/or --scan-id)
    Stop {
        /// Comma-delimited job ids (optional when `--scan-id` is set)
        ids: Option<String>,
        /// Stop every active job in this scan
        #[arg(long)]
        scan_id: Option<String>,
        /// Persistent job queue database
        #[arg(long, default_value = ".gnosis/jobs.redb")]
        job_db: PathBuf,
    },
    /// Requeue and re-execute jobs by id list and/or scan id
    Rerun {
        /// Comma-delimited job ids (optional when `--scan-id` is set)
        ids: Option<String>,
        /// Requeue every job belonging to this scan
        #[arg(long)]
        scan_id: Option<String>,
        /// Persistent job queue database
        #[arg(long, default_value = ".gnosis/jobs.redb")]
        job_db: PathBuf,
        /// Worker concurrency
        #[arg(long)]
        concurrency: Option<usize>,
        /// Maximum object size in bytes when re-reading artifacts
        #[arg(long, default_value_t = 2 * 1024 * 1024)]
        max_size: u64,
        /// AWS region for `s3://` artifacts
        #[arg(long)]
        region: Option<String>,
        /// Requeue only; do not execute workers
        #[arg(long)]
        no_run: bool,
        /// Max attempts per job including the first (default: 3)
        #[arg(long, default_value_t = 3)]
        max_attempts: u32,
        /// Initial retry backoff in milliseconds
        #[arg(long, default_value_t = 250)]
        retry_base_ms: u64,
        /// Maximum retry backoff in milliseconds
        #[arg(long, default_value_t = 30_000)]
        retry_max_ms: u64,
    },
}

#[derive(Subcommand, Debug)]
enum ReviewCommands {
    /// List LLM claims whose review mark is pending
    List {
        /// OKF bundle directory
        #[arg(long, default_value = "knowledge.okf")]
        okf: PathBuf,
        /// Persistent knowledge database
        #[arg(long, default_value = ".gnosis/knowledge.redb")]
        knowledge_db: PathBuf,
    },
    /// Mark one LLM claim accepted (id or unique prefix)
    Accept {
        id: String,
        /// OKF bundle directory
        #[arg(long, default_value = "knowledge.okf")]
        okf: PathBuf,
        /// Persistent knowledge database
        #[arg(long, default_value = ".gnosis/knowledge.redb")]
        knowledge_db: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::About => {
            println!("{HELP_TEXT}");
            Ok(())
        }
        Commands::Scan {
            path,
            no_tui,
            quiet,
            max_size,
            concurrency,
            output,
            export,
            region,
            s3_endpoint,
            s3_insecure,
            s3_inspect,
            s3_versions,
            s3_metadata_only,
            s3_max_requests,
            schema,
            db_max_objects,
            db_statement_timeout_ms,
            job_db,
            max_attempts,
            retry_base_ms,
            retry_max_ms,
            llm,
            llm_mode,
            llm_max_requests,
            llm_max_tokens,
            llm_allow_secrets,
            llm_cache_dir,
            knowledge_db,
            reanalyze,
            max_pages,
            max_depth,
            max_concurrency,
            request_delay_ms,
            include_subdomains,
            allow_private_network,
        } => run_scan(
            path,
            no_tui,
            quiet,
            max_size,
            concurrency,
            output,
            export,
            region,
            gnosis::config::S3ScanOptions {
                endpoint: s3_endpoint,
                insecure: s3_insecure,
                inspect: s3_inspect,
                versions: s3_versions,
                metadata_only: s3_metadata_only,
                max_requests: s3_max_requests,
            },
            gnosis::config::CatalogOptions {
                schemas: schema,
                max_objects: db_max_objects,
                statement_timeout_ms: db_statement_timeout_ms,
            },
            job_db,
            RetryPolicy::new(max_attempts, retry_base_ms, retry_max_ms),
            llm,
            llm_mode,
            llm_max_requests,
            llm_max_tokens,
            llm_allow_secrets,
            llm_cache_dir,
            knowledge_db,
            reanalyze,
            WebFlags {
                max_pages,
                max_depth,
                max_concurrency,
                request_delay_ms,
                include_subdomains,
                allow_private_network,
            },
        ),
        Commands::Diff {
            from,
            to,
            knowledge_db,
        } => run_diff(from, to, knowledge_db),
        Commands::Inspect {
            path,
            json,
            knowledge_db,
        } => run_inspect(&path, json, knowledge_db.as_deref()),
        Commands::Watch {
            path,
            debounce_ms,
            job_db,
            knowledge_db,
            max_size,
        } => run_watch(path, debounce_ms, job_db, knowledge_db, max_size),
        Commands::Review { command } => run_review(command),
        Commands::Jobs { command } => run_jobs(command),
    }
}

fn run_review(command: ReviewCommands) -> Result<()> {
    match command {
        ReviewCommands::List { okf, knowledge_db } => {
            if knowledge_db.exists() {
                let db = gnosis::KnowledgeDb::open(&knowledge_db)
                    .with_context(|| format!("open knowledge db {}", knowledge_db.display()))?;
                let rows = db.review_rows().map_err(|e| anyhow::anyhow!(e))?;
                println!("{}", gnosis::review::format_reviews(&rows));
                return Ok(());
            }
            let rows = gnosis::review::list_okf(&okf).map_err(|e| anyhow::anyhow!(e))?;
            println!("{}", gnosis::review::format_reviews(&rows));
            Ok(())
        }
        ReviewCommands::Accept {
            id,
            okf,
            knowledge_db,
        } => {
            let mut accepted = None;
            if knowledge_db.exists() {
                let db = gnosis::KnowledgeDb::open(&knowledge_db)
                    .with_context(|| format!("open knowledge db {}", knowledge_db.display()))?;
                let rows = db.review_rows().map_err(|e| anyhow::anyhow!(e))?;
                let target = gnosis::review::resolve_review_id(&rows, &id)
                    .map_err(|e| anyhow::anyhow!(e))?;
                if db
                    .persist_review(&target, "accepted")
                    .map_err(|e| anyhow::anyhow!(e))?
                {
                    accepted = Some(target);
                }
            }
            if okf.exists() {
                let from_okf =
                    gnosis::review::accept_okf(&okf, &id).map_err(|e| anyhow::anyhow!(e))?;
                accepted = Some(from_okf);
            }
            match accepted {
                Some(id) => {
                    println!("accepted {id}");
                    Ok(())
                }
                None => bail!("review target not found: {id}"),
            }
        }
    }
}

fn run_inspect(
    path: &std::path::Path,
    json: bool,
    knowledge_db: Option<&std::path::Path>,
) -> Result<()> {
    if let Some(db_path) = knowledge_db {
        let db = gnosis::KnowledgeDb::open(db_path).context("open knowledge db")?;
        let records = db.load_all().context("load knowledge")?;
        let wanted = path.to_string_lossy().replace('\\', "/");
        if let Some(record) = records.iter().find(|record| {
            let rel = record
                .object
                .descriptor
                .relative_path
                .to_string_lossy()
                .replace('\\', "/");
            rel == wanted || wanted.ends_with(&rel) || rel.ends_with(&wanted)
        }) {
            let structural = Some(gnosis::intrinsic::StructuralView {
                provider: record.provider_id.clone().unwrap_or_default(),
                status: record.object.status,
                entities: record
                    .entities
                    .iter()
                    .map(|entity| gnosis::intrinsic::InspectEntity {
                        kind: entity.kind.clone(),
                        name: entity.name.clone(),
                    })
                    .collect(),
            });
            let evidence = record
                .evidence
                .clone()
                .ok_or_else(|| anyhow::anyhow!("stored object has no evidence record"))?;
            let report = gnosis::intrinsic::InspectReport {
                evidence,
                structural,
                semantic: "not_attempted".into(),
            };
            return print_inspect(&report, json);
        }
    }
    let report = gnosis::intrinsic::inspect_file(path).context("inspect file")?;
    print_inspect(&report, json)
}

fn print_inspect(report: &gnosis::intrinsic::InspectReport, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(report).context("encode inspect json")?
        );
    } else {
        println!(
            "status {}  semantic {}",
            report.evidence.user_status, report.semantic
        );
        for fact in &report.evidence.intrinsic.facts {
            println!("{}  {}", fact.key, fact_text(&fact.value));
        }
    }
    Ok(())
}

fn fact_text(value: &gnosis::intrinsic::FactValue) -> String {
    match value {
        gnosis::intrinsic::FactValue::String(text)
        | gnosis::intrinsic::FactValue::Decimal(text)
        | gnosis::intrinsic::FactValue::Timestamp(text)
        | gnosis::intrinsic::FactValue::Uri(text)
        | gnosis::intrinsic::FactValue::Digest(text) => text.clone(),
        gnosis::intrinsic::FactValue::Integer(value) => value.to_string(),
        gnosis::intrinsic::FactValue::Bool(value) => value.to_string(),
        gnosis::intrinsic::FactValue::Array(values) => values.join(","),
        gnosis::intrinsic::FactValue::Null => "null".into(),
    }
}

fn run_diff(from: Option<String>, to: Option<String>, knowledge_db: PathBuf) -> Result<()> {
    let db = gnosis::KnowledgeDb::open(&knowledge_db)
        .with_context(|| format!("open knowledge db {}", knowledge_db.display()))?;
    let (older, newer) = match (from, to) {
        (Some(from), Some(to)) => (
            db.resolve_snapshot(&from).map_err(|e| anyhow::anyhow!(e))?,
            db.resolve_snapshot(&to).map_err(|e| anyhow::anyhow!(e))?,
        ),
        (None, None) => db
            .latest_pair()
            .map_err(|e| anyhow::anyhow!(e))?
            .ok_or_else(|| anyhow::anyhow!("need at least two snapshots; scan twice first"))?,
        _ => bail!("pass both snapshot ids, or neither to compare the latest two"),
    };
    println!("{}", gnosis::format_diff(&older, &newer));
    Ok(())
}

struct WebFlags {
    max_pages: Option<u64>,
    max_depth: Option<u32>,
    max_concurrency: Option<usize>,
    request_delay_ms: Option<u64>,
    include_subdomains: bool,
    allow_private_network: bool,
}

impl WebFlags {
    fn any(&self) -> bool {
        self.max_pages.is_some()
            || self.max_depth.is_some()
            || self.max_concurrency.is_some()
            || self.request_delay_ms.is_some()
            || self.include_subdomains
            || self.allow_private_network
    }
}

fn run_watch(
    path: String,
    debounce_ms: u64,
    job_db: PathBuf,
    knowledge_db: PathBuf,
    max_size: u64,
) -> Result<()> {
    if is_s3_uri(&path)
        || matches!(
            gnosis::connectors::web::classify_target(&path, false),
            gnosis::connectors::web::TargetClass::Web(_)
                | gnosis::connectors::web::TargetClass::S3
                | gnosis::connectors::web::TargetClass::Catalog(_)
        )
    {
        bail!("gnosis watch is filesystem-only");
    }
    let root = PathBuf::from(&path);
    if !root.exists() {
        bail!("path does not exist: {}", root.display());
    }
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalize {}", root.display()))?;
    loop {
        run_scan(
            root.display().to_string(),
            true,
            true,
            max_size,
            None,
            PathBuf::from("knowledge.okf"),
            false,
            None,
            gnosis::config::S3ScanOptions::default(),
            gnosis::config::CatalogOptions::default(),
            job_db.clone(),
            RetryPolicy::new(3, 250, 30_000),
            None,
            "fallback".to_string(),
            0,
            0,
            false,
            PathBuf::from(".gnosis/llm-cache"),
            knowledge_db.clone(),
            false,
            WebFlags {
                max_pages: None,
                max_depth: None,
                max_concurrency: None,
                request_delay_ms: None,
                include_subdomains: false,
                allow_private_network: false,
            },
        )?;
        eprintln!(
            "watching {} — next scan after {}ms of quiet (Ctrl-C to stop)",
            root.display(),
            debounce_ms
        );
        gnosis::watch::wait_for_change(&root, Duration::from_millis(debounce_ms.max(1)))
            .map_err(|e| anyhow::anyhow!(e))?;
    }
}

fn run_jobs(command: JobsCommands) -> Result<()> {
    match command {
        JobsCommands::List {
            status,
            scan_id,
            limit,
            job_db,
        } => {
            let status = match status {
                Some(s) => Some(JobStatus::from_str(&s).map_err(|e| anyhow::anyhow!(e))?),
                None => None,
            };
            let store = RedbJobStore::open(&job_db)
                .with_context(|| format!("open job db {}", job_db.display()))?;
            let resolved_scan = match scan_id {
                Some(q) => Some(resolve_scan_id(&store, &q).map_err(|e| anyhow::anyhow!(e))?),
                None => None,
            };
            let filter = JobListFilter {
                scan_id: resolved_scan.clone(),
                status,
                limit: Some(limit),
            };
            let summary = match &resolved_scan {
                Some(sid) => store.summary(sid).map_err(|e| anyhow::anyhow!(e))?,
                None => store.summary_all().map_err(|e| anyhow::anyhow!(e))?,
            };
            let jobs = store.list(&filter).map_err(|e| anyhow::anyhow!(e))?;
            println!(
                "{}",
                format_job_list_filtered(&jobs, Some(&summary), status, resolved_scan.as_deref())
            );
            Ok(())
        }
        JobsCommands::Scans { job_db } => {
            let store = RedbJobStore::open(&job_db)
                .with_context(|| format!("open job db {}", job_db.display()))?;
            let ids = store.list_scan_ids().map_err(|e| anyhow::anyhow!(e))?;
            if ids.is_empty() {
                println!("(no scans)");
            } else {
                for id in ids {
                    let s = store.summary(&id).map_err(|e| anyhow::anyhow!(e))?;
                    println!(
                        "{id}  total={}  pending={} running={} paused={} completed={} failed={} stopped={}",
                        s.total(),
                        s.pending,
                        s.running,
                        s.paused,
                        s.completed,
                        s.failed,
                        s.stopped
                    );
                }
            }
            Ok(())
        }
        JobsCommands::Show { id, job_db } => {
            let store = RedbJobStore::open(&job_db)
                .with_context(|| format!("open job db {}", job_db.display()))?;
            let job = resolve_job_cli(&store, &id)?;
            println!("{}", format_job_detail(&job));
            Ok(())
        }
        JobsCommands::Purge {
            age,
            scan_id,
            job_db,
            dry_run,
        } => {
            if age.is_none() && scan_id.is_none() {
                bail!("purge requires an age (e.g. 5d) and/or --scan-id");
            }
            let store = RedbJobStore::open(&job_db)
                .with_context(|| format!("open job db {}", job_db.display()))?;
            let resolved_scan = match scan_id {
                Some(q) => Some(resolve_scan_id(&store, &q).map_err(|e| anyhow::anyhow!(e))?),
                None => None,
            };

            if let Some(age) = age {
                let duration = parse_age(&age).map_err(|e| anyhow::anyhow!(e))?;
                let cutoff = chrono::Utc::now() - duration;
                let scope = resolved_scan
                    .as_ref()
                    .map(|s| format!(" in scan {s}"))
                    .unwrap_or_default();
                if dry_run {
                    let filter = JobListFilter {
                        scan_id: resolved_scan.clone(),
                        ..Default::default()
                    };
                    let n = store
                        .list(&filter)
                        .map_err(|e| anyhow::anyhow!(e))?
                        .into_iter()
                        .filter(|j| j.updated_at < cutoff)
                        .count();
                    println!("dry-run: would purge {n} job(s) older than {age}{scope}");
                } else {
                    let n = store
                        .purge_older_than(cutoff, resolved_scan.as_deref())
                        .map_err(|e| anyhow::anyhow!(e))?;
                    println!("purged {n} job(s) older than {age}{scope}");
                }
            } else {
                let sid = resolved_scan.expect("scan_id required when age omitted");
                if dry_run {
                    let n = store.summary(&sid).map_err(|e| anyhow::anyhow!(e))?.total();
                    println!("dry-run: would purge {n} job(s) in scan {sid}");
                } else {
                    let n = store.purge_scan(&sid).map_err(|e| anyhow::anyhow!(e))?;
                    println!("purged {n} job(s) in scan {sid}");
                }
            }
            Ok(())
        }
        JobsCommands::Pause {
            ids,
            scan_id,
            job_db,
        } => run_job_control(JobControl::Pause, ids, scan_id, job_db),
        JobsCommands::Unpause {
            ids,
            scan_id,
            job_db,
        } => run_job_control(JobControl::Unpause, ids, scan_id, job_db),
        JobsCommands::Stop {
            ids,
            scan_id,
            job_db,
        } => run_job_control(JobControl::Stop, ids, scan_id, job_db),
        JobsCommands::Rerun {
            ids,
            scan_id,
            job_db,
            concurrency,
            max_size,
            region,
            no_run,
            max_attempts,
            retry_base_ms,
            retry_max_ms,
        } => {
            match (&ids, &scan_id) {
                (None, None) => bail!("rerun requires comma-delimited ids and/or --scan-id"),
                (Some(_), Some(_)) => {
                    bail!("pass either comma-delimited ids or --scan-id, not both")
                }
                _ => {}
            }

            let store: Arc<dyn JobStore> = Arc::new(
                RedbJobStore::open(&job_db)
                    .with_context(|| format!("open job db {}", job_db.display()))?,
            );

            if no_run {
                let report = if let Some(sid) = &scan_id {
                    prepare_rerun_scan(Arc::clone(&store), sid).map_err(|e| anyhow::anyhow!(e))?
                } else {
                    let queries = parse_job_id_list(ids.as_deref().unwrap())
                        .map_err(|e| anyhow::anyhow!(e))?;
                    prepare_rerun(Arc::clone(&store), &queries).map_err(|e| anyhow::anyhow!(e))?
                };
                println!(
                    "requeued {} job(s) under scan {} (not executed; omit --no-run to process)",
                    report.requeued.len(),
                    report.scan_id
                );
                for id in &report.requeued {
                    println!("  {id}");
                }
                return Ok(());
            }

            let workers = concurrency.unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4)
                    .min(8)
            });
            let retry = RetryPolicy::new(max_attempts, retry_base_ms, retry_max_ms);

            if let Some(sid) = &scan_id {
                Pipeline::rerun_scan(store, sid, workers, max_size, region.as_deref(), retry)
                    .map_err(|e| anyhow::anyhow!(e))?;
            } else {
                let queries =
                    parse_job_id_list(ids.as_deref().unwrap()).map_err(|e| anyhow::anyhow!(e))?;
                Pipeline::rerun_jobs(store, &queries, workers, max_size, region.as_deref(), retry)
                    .map_err(|e| anyhow::anyhow!(e))?;
            }
            Ok(())
        }
    }
}

enum JobControl {
    Pause,
    Unpause,
    Stop,
}

fn run_job_control(
    action: JobControl,
    ids: Option<String>,
    scan_id: Option<String>,
    job_db: PathBuf,
) -> Result<()> {
    match (&ids, &scan_id) {
        (None, None) => bail!(
            "{} requires comma-delimited ids and/or --scan-id",
            match action {
                JobControl::Pause => "pause",
                JobControl::Unpause => "unpause",
                JobControl::Stop => "stop",
            }
        ),
        (Some(_), Some(_)) => {
            bail!("pass either comma-delimited ids or --scan-id, not both")
        }
        _ => {}
    }

    let store =
        RedbJobStore::open(&job_db).with_context(|| format!("open job db {}", job_db.display()))?;

    let changed = if let Some(sid) = &scan_id {
        match action {
            JobControl::Pause => pause_scan(&store, sid),
            JobControl::Unpause => unpause_scan(&store, sid),
            JobControl::Stop => stop_scan(&store, sid),
        }
        .map_err(|e| anyhow::anyhow!(e))?
    } else {
        let queries = parse_job_id_list(ids.as_deref().unwrap()).map_err(|e| anyhow::anyhow!(e))?;
        match action {
            JobControl::Pause => pause_jobs(&store, &queries),
            JobControl::Unpause => unpause_jobs(&store, &queries),
            JobControl::Stop => stop_jobs(&store, &queries),
        }
        .map_err(|e| anyhow::anyhow!(e))?
    };

    let verb = match action {
        JobControl::Pause => "paused",
        JobControl::Unpause => "unpaused",
        JobControl::Stop => "stopped",
    };
    println!("{verb} {} job(s)", changed.len());
    for id in &changed {
        println!("  {id}");
    }
    Ok(())
}

fn resolve_job_cli(store: &RedbJobStore, query: &str) -> Result<gnosis::Job> {
    let id = JobId::new(query);
    if let Some(job) = store.get(&id).map_err(|e| anyhow::anyhow!(e))? {
        return Ok(job);
    }
    if !query.starts_with("job:") {
        let prefixed = JobId::new(format!("job:{query}"));
        if let Some(job) = store.get(&prefixed).map_err(|e| anyhow::anyhow!(e))? {
            return Ok(job);
        }
    }
    let all = store
        .list(&JobListFilter::default())
        .map_err(|e| anyhow::anyhow!(e))?;
    let q = query.to_ascii_lowercase();
    let matches: Vec<_> = all
        .into_iter()
        .filter(|j| {
            let id = j.id.as_str().to_ascii_lowercase();
            id == q || id.ends_with(&q) || id.contains(&q)
        })
        .collect();
    match matches.len() {
        0 => bail!("job not found: {query}"),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => bail!("ambiguous job id '{query}' ({n} matches); use a longer prefix"),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_scan(
    path: String,
    no_tui: bool,
    quiet: bool,
    max_size: u64,
    concurrency: Option<usize>,
    output: PathBuf,
    auto_export: bool,
    region: Option<String>,
    s3: gnosis::config::S3ScanOptions,
    catalog: gnosis::config::CatalogOptions,
    job_db: PathBuf,
    retry: RetryPolicy,
    llm: Option<String>,
    llm_mode: String,
    llm_max_requests: u64,
    llm_max_tokens: u64,
    llm_allow_secrets: bool,
    llm_cache_dir: PathBuf,
    knowledge_db: PathBuf,
    reanalyze: bool,
    web: WebFlags,
) -> Result<()> {
    let exists = PathBuf::from(&path).exists();
    let mut config = match gnosis::connectors::web::classify_target(&path, exists) {
        gnosis::connectors::web::TargetClass::S3 => {
            if web.any() {
                bail!("web crawl flags apply only to http(s) scans");
            }
            if s3.insecure && s3.endpoint.is_none() {
                bail!("--s3-insecure requires --s3-endpoint");
            }
            let location = parse_s3_uri(&path).map_err(|e| anyhow::anyhow!(e))?;
            ScanConfig::with_source(ScanSource::S3 {
                location,
                region,
                options: s3,
            })
        }
        gnosis::connectors::web::TargetClass::Web(start_url) => {
            let mut policy =
                gnosis::connectors::web::CrawlPolicy::default().with_object_limit(max_size);
            if let Some(n) = web.max_pages {
                policy.max_pages = n;
            }
            if let Some(n) = web.max_depth {
                policy.max_depth = n;
            }
            if let Some(n) = web.max_concurrency {
                policy.max_concurrency = n.max(1);
            }
            if let Some(n) = web.request_delay_ms {
                policy.delay_ms = n;
            }
            if web.include_subdomains {
                policy.scope = gnosis::connectors::web::CrawlScope::IncludeSubdomains;
            }
            policy.allow_private_network = web.allow_private_network;
            ScanConfig::with_source(ScanSource::Web { start_url, policy })
        }
        gnosis::connectors::web::TargetClass::Catalog(raw) => {
            if web.any() {
                bail!("web crawl flags apply only to http(s) scans");
            }
            let location = gnosis::connectors::catalog::parse_catalog_uri(&raw)
                .map_err(|e| anyhow::anyhow!(e))?;
            ScanConfig::with_source(ScanSource::Catalog {
                location,
                options: catalog,
            })
        }
        gnosis::connectors::web::TargetClass::Unsupported(message) => bail!("{message}"),
        gnosis::connectors::web::TargetClass::Filesystem => {
            if web.any() {
                bail!("web crawl flags apply only to http(s) scans");
            }
            let path = PathBuf::from(&path);
            if !path.exists() {
                bail!("path does not exist: {}", path.display());
            }
            ScanConfig::with_root(
                path.canonicalize()
                    .with_context(|| format!("canonicalize {}", path.display()))?,
            )
        }
    };
    config.max_object_size = max_size;
    if let Some(c) = concurrency {
        config.concurrency = c.max(1);
    }
    config.output_path = output.clone();
    config.job_db_path = job_db;
    config.knowledge_db_path = knowledge_db;
    config.reanalyze = reanalyze;
    config.retry = retry;
    config.llm = resolve_llm_config(
        llm,
        &llm_mode,
        llm_max_requests,
        llm_max_tokens,
        llm_allow_secrets,
        llm_cache_dir,
    )?;

    let metrics = Arc::new(gnosis::ScanMetrics::new());
    let providers = build_registry(config.llm.as_ref(), Some(Arc::clone(&metrics.llm)));
    let job_store: Arc<dyn gnosis::JobStore> = Arc::new(
        RedbJobStore::open(&config.job_db_path)
            .with_context(|| format!("open job db {}", config.job_db_path.display()))?,
    );
    let knowledge = Arc::new(
        gnosis::KnowledgeDb::open(&config.knowledge_db_path)
            .with_context(|| format!("open knowledge db {}", config.knowledge_db_path.display()))?,
    );
    let pipeline = Pipeline::with_knowledge_and_metrics(
        config.clone(),
        providers,
        job_store,
        metrics.clone(),
        knowledge,
    );
    let mut handle = pipeline.spawn();
    let events = handle.take_events();

    let resolve_output = |output: &PathBuf| -> PathBuf {
        if output.is_absolute() {
            output.clone()
        } else if matches!(
            config.source,
            ScanSource::S3 { .. } | ScanSource::Web { .. } | ScanSource::Catalog { .. }
        ) {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(output)
        } else {
            config.root.join(output)
        }
    };

    if no_tui {
        drain_events_headless(events, quiet);
        handle.wait().context("pipeline failed")?;

        let analyzed = handle
            .metrics
            .objects_analyzed
            .load(std::sync::atomic::Ordering::Relaxed);
        let skipped = handle
            .metrics
            .objects_skipped
            .load(std::sync::atomic::Ordering::Relaxed);
        println!("analyzed {analyzed}, skipped {skipped} unchanged");
        let fetched = handle
            .metrics
            .crawl_fetched
            .load(std::sync::atomic::Ordering::Relaxed);
        if fetched > 0
            || handle
                .metrics
                .crawl_discovered
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0
        {
            println!(
                "crawl discovered {}, fetched {fetched}, blocked {}, skipped {}, failed {}, bytes {}",
                handle.metrics.crawl_discovered.load(std::sync::atomic::Ordering::Relaxed),
                handle.metrics.crawl_blocked.load(std::sync::atomic::Ordering::Relaxed),
                handle.metrics.crawl_skipped.load(std::sync::atomic::Ordering::Relaxed),
                handle.metrics.crawl_failed.load(std::sync::atomic::Ordering::Relaxed),
                handle.metrics.crawl_bytes.load(std::sync::atomic::Ordering::Relaxed),
            );
        }

        let store = handle.store.lock().unwrap();
        let q = QueryEngine::new(&store);
        print!("{}", q.summary());

        if auto_export {
            let exporter = OkfExporter::new();
            let out = resolve_output(&output);
            println!("exporting OKF to {} …", out.display());
            exporter.export(&store, &out).context("OKF export failed")?;
            println!("export complete");
        }

        let unknown = q.unknown();
        if !unknown.is_empty() {
            println!("\nunknown / partial ({}):", unknown.len());
            for o in unknown.iter().take(20) {
                println!(
                    "  [{}] {} — {}",
                    o.status,
                    o.descriptor.relative_path.display(),
                    o.classification_reason.as_deref().unwrap_or("")
                );
            }
        }
        return Ok(());
    }

    let export_path = if output.is_absolute() {
        output.clone()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| config.root.clone())
            .join(&output)
    };

    let mut app = TuiApp::new(
        config.root.clone(),
        handle.store.clone(),
        handle.job_store.clone(),
        handle.metrics.clone(),
        events,
        config.event_history_len,
        export_path,
    );
    app.set_export_handler(|store, path| {
        OkfExporter::new()
            .export(store, path)
            .map_err(|e| anyhow::anyhow!(e))
    });

    let result = app.run();
    handle.cancel();
    thread::sleep(Duration::from_millis(50));
    let _ = handle.wait();
    result.context("TUI exited with error")?;
    Ok(())
}

fn resolve_llm_config(
    llm: Option<String>,
    llm_mode: &str,
    max_requests: u64,
    max_tokens: u64,
    allow_secrets: bool,
    cache_dir: PathBuf,
) -> Result<Option<LlmConfig>> {
    let Some(model) = llm else {
        return Ok(None);
    };
    #[cfg(not(feature = "llm"))]
    {
        let _ = (
            model,
            llm_mode,
            max_requests,
            max_tokens,
            allow_secrets,
            cache_dir,
        );
        bail!(
            "LLM support is not enabled in this build; reinstall with `cargo install gnosis --features llm` (or `cargo build --features llm`)"
        );
    }
    #[cfg(feature = "llm")]
    {
        let mode = gnosis::LlmMode::parse(llm_mode).map_err(|e| anyhow::anyhow!(e))?;
        let mut cfg = LlmConfig::new(model, mode);
        cfg.max_requests = max_requests;
        cfg.max_tokens = max_tokens;
        cfg.allow_secrets = allow_secrets;
        cfg.cache_dir = cache_dir;
        Ok(Some(cfg))
    }
}

const HELP_TEXT: &str = r#"Gnosis — Enterprise Knowledge Compiler

  gnosis scan <path>                 Live TUI scan (local directory)
  gnosis scan https://host/docs      Bounded web crawl (no JavaScript, no auth)
  gnosis scan s3://bucket[/prefix]   Scan an S3 bucket (keys = paths)
  gnosis scan postgresql://host/db   Read-only catalog (no table rows)
  gnosis scan <target> --no-tui      Headless scan (prints summary)
  gnosis scan <target> --no-tui --export
  gnosis scan <target> --llm [MODEL] [--llm-mode fallback|always]
  gnosis scan <target> --llm --llm-max-requests N --llm-max-tokens N
  gnosis scan <target> --reanalyze     Ignore the knowledge store for one scan
  gnosis watch <path>                Incremental filesystem scan on change
  gnosis scan https://host --max-pages N --max-depth N --include-subdomains
  gnosis diff                        Compare the latest two snapshots
  gnosis review list [--okf knowledge.okf]
  gnosis review accept <id> [--okf knowledge.okf]

  Each scan creates a scan id (`scan:…`); every job is linked to it.

  gnosis jobs scans                  List scan ids with job counts
  gnosis jobs list [--status S] [--scan-id ID]
  gnosis jobs show <id>              Job detail (args / result / error)
  gnosis jobs pause [--scan-id ID] [ids]
  gnosis jobs unpause [--scan-id ID] [ids]
  gnosis jobs stop [--scan-id ID] [ids]
  gnosis jobs purge <age>            Delete jobs older than age (e.g. 5d)
  gnosis jobs purge --scan-id ID     Delete all jobs for a scan
  gnosis jobs purge 5d --scan-id ID  Age filter within a scan
  gnosis jobs rerun <id,id,…>        Requeue and re-run jobs by id
  gnosis jobs rerun --scan-id ID     Requeue and re-run an entire scan

Artifacts are processed as durable jobs (default store: .gnosis/jobs.redb).
S3 uses the default AWS credential chain; pass --region to override.
A custom endpoint (--s3-endpoint) uses path-style addressing. TLS stays
verified unless --s3-insecure is set for that endpoint. The default scan
uses the listing. --s3-inspect, --s3-versions, and GetObject are opt-in.
--s3-metadata-only skips the body. --s3-max-requests caps list, head, tag,
and get calls (0 means no cap).
A postgresql:// scan lists relations, columns, and keys. It does not read table rows.
The default schema is public. Repeat --schema to add another. Passwords are not stored.
LLM extraction requires a build with `--features llm`. It is off unless you pass `--llm`.
The model id selects an adapter: OpenAI (`gpt-…`), Anthropic (`claude-…`), Gemini (`gemini-…`), or Ollama (anything else, including `tinyllama:latest`). Prefix `openai::`, `anthropic::`, `gemini::`, or `ollama::` to force one.
Full deterministic providers still win. LLM claims stay `review=pending` until accepted.
Secrets are not sent unless `--llm-allow-secrets`. Token totals depend on the provider (Ollama often reports 0).

TUI commands (press :):
  summary | objects | unknown | providers | stats
  review | review accept <id>
  jobs [status] | job <id>
  find <text> | explain <name> | graph <name>
  export okf [path] | help | quit

Shortcuts: s summary · u unknown · e export · J jobs · q quit
"#;
