//! Offline web ingestion: fixture server, policy, HTML, and incremental skip.

use gnosis::connectors::web::{
    AcquisitionKind, BoundedCrawler, CrawlPolicy, CrawlScope, WebAcquirer,
};
use gnosis::{
    default_registry, BytesContentReader, HtmlProvider, KnowledgeDb, MemoryJobStore,
    ObjectDescriptor, Pipeline, ProtoData, ScanConfig, ScanSource, Support, UnderstandingProvider,
    UnderstandingStatus,
};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn policy() -> CrawlPolicy {
    CrawlPolicy {
        delay_ms: 0,
        timeout_ms: 1_000,
        allow_private_network: true,
        respect_robots: true,
        ..CrawlPolicy::default()
    }
}

struct Site {
    base: String,
}

fn serve(handler: impl Fn(&str, &str) -> Response + Send + Sync + 'static) -> Site {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handler = Arc::new(handler);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let handler = Arc::clone(&handler);
            thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut buf = [0u8; 16_384];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/");
                let response = handler(path, &req);
                let reason = match response.status {
                    200 => "OK",
                    302 => "Found",
                    304 => "Not Modified",
                    404 => "Not Found",
                    _ => "OK",
                };
                let mut head = format!(
                    "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response.status,
                    response.body.len()
                );
                if !response.content_type.is_empty() {
                    head.push_str(&format!("Content-Type: {}\r\n", response.content_type));
                }
                for (name, value) in &response.headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&response.body);
            });
        }
    });
    Site { base }
}

struct Response {
    status: u16,
    content_type: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn html(body: &str) -> Response {
    Response {
        status: 200,
        content_type: "text/html; charset=utf-8".into(),
        headers: vec![("etag".into(), "\"v1\"".into())],
        body: body.as_bytes().to_vec(),
    }
}

fn crawl(base: &str, policy: &CrawlPolicy) -> gnosis::connectors::web::CrawlSummary {
    let cache = gnosis::connectors::web::BodyCache::new();
    let crawler = BoundedCrawler;
    crawler
        .crawl(
            base,
            policy,
            &cache,
            &std::collections::HashMap::new(),
            &AtomicBool::new(false),
        )
        .unwrap()
}

fn site_routes(base: &str, path: &str, _req: &str) -> Response {
    let path = path.split('?').next().unwrap_or(path);
    match path {
        "/robots.txt" => Response {
            status: 200,
            content_type: "text/plain".into(),
            headers: Vec::new(),
            body: b"User-agent: gnosis\nAllow: /docs\nDisallow: /private\n\nUser-agent: *\nDisallow: /\n".to_vec(),
        },
        "/" => html(&format!(
            r#"<html lang="en"><head><title>Root &#27;[31mHi</title>
            <link rel="canonical" href="{base}/">
            <meta name="description" content="docs">
            <script type="application/ld+json">{{"@type":"WebPage"}}</script>
            </head><body>
            <h1>Welcome</h1>
            <a href="nested/guide.html">rel</a>
            <a href="{base}/abs">abs</a>
            <a href="https://example.com/out">ext</a>
            <a href="{base}/redir">redir</a>
            <a href="{base}/missing">miss</a>
            <a href="{base}/asset.css">css</a>
            <a href="{base}/dup">dup</a>
            <a href="{base}/dup#section">frag</a>
            <a href="{base}/private/secret">no</a>
            <a href="{base}/loop">loop</a>
            <a href="{base}/file-redir">file</a>
            <a href="{base}/meta-redir">meta</a>
            <a href="{base}/big">big</a>
            </body></html>"#
        )),
        "/nested/guide.html" => html("<html><head><title>Guide</title></head><body><h1>Guide</h1></body></html>"),
        "/abs" => html("<html><head><title>Abs</title></head><body>abs</body></html>"),
        "/dup" => html("<html><head><title>Dup</title></head><body>dup</body></html>"),
        "/redir" => Response {
            status: 302,
            content_type: String::new(),
            headers: vec![("location".into(), format!("{base}/abs"))],
            body: Vec::new(),
        },
        "/missing" => Response {
            status: 404,
            content_type: "text/html".into(),
            headers: Vec::new(),
            body: b"missing".to_vec(),
        },
        "/asset.css" => Response {
            status: 200,
            content_type: "text/css".into(),
            headers: vec![("etag".into(), "\"css\"".into())],
            body: b"body{color:red}".to_vec(),
        },
        "/loop" => Response {
            status: 302,
            content_type: String::new(),
            headers: vec![("location".into(), format!("{base}/loop"))],
            body: Vec::new(),
        },
        "/file-redir" => Response {
            status: 302,
            content_type: String::new(),
            headers: vec![("location".into(), "file:///etc/passwd".into())],
            body: Vec::new(),
        },
        "/meta-redir" => Response {
            status: 302,
            content_type: String::new(),
            headers: vec![("location".into(), "http://169.254.169.254/latest".into())],
            body: Vec::new(),
        },
        "/big" => Response {
            status: 200,
            content_type: "text/html".into(),
            headers: vec![("content-length".into(), "999999".into())],
            body: vec![b'x'; 64],
        },
        "/private/secret" => html("<html><title>secret</title></html>"),
        _ => Response {
            status: 404,
            content_type: "text/plain".into(),
            headers: Vec::new(),
            body: b"no".to_vec(),
        },
    }
}

#[test]
fn fixture_crawl_covers_links_limits_and_blocks() {
    let holder: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&holder);
    let site = serve(move |path, req| {
        let base = slot.lock().unwrap().clone().unwrap_or_default();
        let _ = req;
        site_routes(&base, path, req)
    });
    *holder.lock().unwrap() = Some(site.base.clone());

    let mut policy = policy();
    policy.max_object_bytes = 8_192;
    policy.max_redirects = 2;
    let summary = crawl(&site.base, &policy);
    let ids: Vec<_> = summary
        .objects
        .iter()
        .map(|o| o.descriptor.id.as_str().to_string())
        .collect();
    assert!(ids.iter().any(|id| id.ends_with("/")));
    assert!(ids.iter().any(|id| id.ends_with("/nested/guide.html")));
    assert!(ids.iter().any(|id| id.ends_with("/abs")));
    assert!(ids.iter().any(|id| id.ends_with("/dup")));
    assert!(ids.iter().any(|id| id.ends_with("/asset.css")));
    assert_eq!(ids.iter().filter(|id| id.contains("/dup")).count(), 1);
    assert!(ids.iter().all(|id| !id.contains('#')));
    assert!(ids.iter().all(|id| !id.contains("example.com")));
    assert!(ids.iter().all(|id| !id.contains("/private")));
    assert!(summary
        .outcomes
        .iter()
        .any(|o| { o.kind == AcquisitionKind::BlockedScope && o.url.contains("example.com") }));
    assert!(summary
        .outcomes
        .iter()
        .any(|o| o.kind == AcquisitionKind::BlockedRobots));
    assert!(summary
        .outcomes
        .iter()
        .any(|o| o.kind == AcquisitionKind::BlockedScheme));
    assert!(summary
        .outcomes
        .iter()
        .any(|o| o.kind == AcquisitionKind::BlockedSsrf));
    assert!(summary
        .outcomes
        .iter()
        .any(|o| o.kind == AcquisitionKind::TooLarge || o.kind == AcquisitionKind::SkippedLimit));
    assert!(summary
        .outcomes
        .iter()
        .any(|o| o.kind == AcquisitionKind::HttpError && o.detail.contains("404")));
    let css = summary
        .objects
        .iter()
        .find(|o| o.descriptor.relative_path.ends_with("asset.css"))
        .unwrap();
    assert_eq!(css.descriptor.media_type, "text/css");
    assert_eq!(css.descriptor.extension.as_deref(), Some("css"));
}

#[test]
fn page_depth_and_byte_limits_stop_the_crawl() {
    let site = serve(|path, _| match path {
        "/" => html(r#"<a href="/one">1</a><a href="/two">2</a>"#),
        "/one" => html(r#"<a href="/deep">d</a>"#),
        "/two" => html("two"),
        "/deep" => html("deep"),
        _ => html("x"),
    });
    let mut pages = policy();
    pages.max_pages = 1;
    let summary = crawl(&site.base, &pages);
    assert_eq!(summary.fetched, 1);
    assert!(summary.skipped >= 1);

    let mut depth = policy();
    depth.max_depth = 0;
    let summary = crawl(&site.base, &depth);
    assert_eq!(summary.fetched, 1);

    let mut bytes = policy();
    bytes.max_total_bytes = 1;
    let summary = crawl(&format!("{}/", site.base.trim_end_matches('/')), &bytes);
    assert!(summary.fetched <= 1);
    assert!(
        summary
            .outcomes
            .iter()
            .any(|o| o.kind == AcquisitionKind::SkippedLimit)
            || summary.fetched == 1 && summary.bytes <= 1
            || summary
                .outcomes
                .iter()
                .any(|o| o.kind == AcquisitionKind::TooLarge)
    );
}

#[test]
fn delay_and_concurrency_are_enforced() {
    let site = serve(|path, _| match path {
        "/" => html(r#"<a href="/next">n</a>"#),
        "/next" => html(r#"<a href="/last">l</a>"#),
        _ => html("last"),
    });
    let mut delayed = policy();
    delayed.delay_ms = 80;
    delayed.max_depth = 2;
    let started = Instant::now();
    let summary = crawl(&site.base, &delayed);
    assert!(summary.fetched >= 3);
    assert!(started.elapsed() >= Duration::from_millis(140));

    let inflight = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let current = Arc::clone(&inflight);
    let peak = Arc::clone(&max_seen);
    let slow = serve(move |path, _| {
        if path.starts_with("/slow") {
            let now = current.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(200));
            current.fetch_sub(1, Ordering::SeqCst);
        }
        if path == "/" {
            html(r#"<a href="/slow/a">a</a><a href="/slow/b">b</a>"#)
        } else {
            html("slow")
        }
    });
    let mut parallel = policy();
    parallel.max_concurrency = 2;
    parallel.max_depth = 1;
    let _ = crawl(&slow.base, &parallel);
    assert!(
        max_seen.load(Ordering::SeqCst) >= 2,
        "expected overlapping fetches"
    );
}

#[test]
fn request_timeout_is_an_acquisition_failure() {
    let site = serve(|_, _| {
        thread::sleep(Duration::from_secs(2));
        html("late")
    });
    let mut timed = policy();
    timed.timeout_ms = 200;
    timed.respect_robots = false;
    let summary = crawl(&site.base, &timed);
    assert!(summary
        .outcomes
        .iter()
        .any(|o| o.kind == AcquisitionKind::HttpError));
    assert!(summary.objects.is_empty());
}

#[test]
fn include_subdomains_changes_scope() {
    let start = "https://example.com/docs";
    let sub = "https://docs.example.com/a";
    let other = "https://notexample.com/a";
    assert!(gnosis::connectors::web::in_scope(start, sub, true));
    assert!(!gnosis::connectors::web::in_scope(start, sub, false));
    assert!(!gnosis::connectors::web::in_scope(start, other, true));
    let mut policy = policy();
    policy.scope = CrawlScope::IncludeSubdomains;
    assert!(matches!(policy.scope, CrawlScope::IncludeSubdomains));
}

#[test]
fn html_provider_emits_links_without_llm_stamps() {
    let html = r#"<html lang="en"><head>
        <title>Hello &#27;[2J</title>
        <meta name="description" content="A page">
        <link rel="canonical" href="https://example.com/hello">
        <script type="application/ld+json">{"@type":"Article"}</script>
        </head><body>
        <h1>Hello</h1>
        <a href="/docs">docs</a>
        <a href="https://other.example/x">out</a>
        <a href="/app.css">css</a>
        </body></html>"#;
    let object = ObjectDescriptor {
        id: gnosis::ObjectId::new("obj:web:https://example.com/hello"),
        path: PathBuf::from("https://example.com/hello"),
        relative_path: PathBuf::from("https://example.com/hello"),
        is_dir: false,
        size: html.len() as u64,
        modified: None,
        media_type: "text/html".into(),
        extension: Some("html".into()),
        etag: None,
    };
    let proto = ProtoData {
        filename: "hello".into(),
        final_url: Some("https://example.com/hello".into()),
        media_type: "text/html".into(),
        ..ProtoData::default()
    };
    assert_eq!(HtmlProvider.probe(&object, &proto), Support::Full);
    let mut reader = BytesContentReader::from_slice(html.as_bytes());
    let result = HtmlProvider.analyze(&object, &proto, &mut reader).unwrap();
    assert_eq!(result.status, UnderstandingStatus::Understood);
    let title = result
        .record
        .entities
        .iter()
        .find(|e| e.kind == "document")
        .unwrap();
    assert!(!title.name.contains('\u{1b}'));
    let kinds: Vec<_> = result
        .record
        .relationships
        .iter()
        .map(|r| r.kind.as_str())
        .collect();
    assert!(kinds.contains(&"links_to"));
    assert!(kinds.contains(&"references_external"));
    assert!(kinds.contains(&"references_asset"));
    assert!(kinds.contains(&"declares_canonical"));
    for entity in &result.record.entities {
        assert!(!entity.attributes.contains_key("origin"));
        assert!(!entity.attributes.contains_key("review"));
    }

    let mut broken = BytesContentReader::from_slice(b"<html><title>");
    let partial = HtmlProvider.analyze(&object, &proto, &mut broken).unwrap();
    assert!(matches!(
        partial.status,
        UnderstandingStatus::Understood | UnderstandingStatus::PartiallyUnderstood
    ));
}

#[test]
fn css_and_javascript_stay_unknown() {
    let reg = default_registry();
    for (name, media) in [("app.css", "text/css"), ("app.js", "text/javascript")] {
        let object = ObjectDescriptor {
            id: gnosis::ObjectId::new(format!("obj:web:{name}")),
            path: PathBuf::from(name),
            relative_path: PathBuf::from(name),
            is_dir: false,
            size: 4,
            modified: None,
            media_type: media.into(),
            extension: Some(name.rsplit('.').next().unwrap().into()),
            etag: None,
        };
        let proto = ProtoData {
            filename: name.into(),
            media_type: media.into(),
            extension: object.extension.clone(),
            ..ProtoData::default()
        };
        let (provider, support) = reg.select(&object, &proto).unwrap();
        assert_ne!(provider.id().as_str(), "plaintext");
        assert_eq!(support, Support::Weak);
        let mut reader = BytesContentReader::from_slice(b"x{}");
        let result = provider.analyze(&object, &proto, &mut reader).unwrap();
        assert_eq!(result.status, UnderstandingStatus::Unknown);
    }
}

#[test]
fn fake_page_runs_through_the_pipeline() {
    let start = "https://example.com/docs";
    let fake = gnosis::connectors::web::FakeAcquirer {
        pages: vec![(
            start.into(),
            200,
            "text/html".into(),
            b"<html><head><title>Docs</title></head><body><h1>Docs</h1></body></html>".to_vec(),
        )],
    };
    let mut config = ScanConfig::with_source(ScanSource::Web {
        start_url: start.into(),
        policy: policy(),
    });
    config.concurrency = 1;
    let pipeline =
        Pipeline::with_job_store(config, default_registry(), Arc::new(MemoryJobStore::new()))
            .with_web_acquirer(Arc::new(fake));
    let store = run_pipeline(pipeline);
    let page = store
        .iter()
        .find(|(id, _, _)| id.contains("example.com/docs"))
        .expect("page stored");
    assert_eq!(page.1, UnderstandingStatus::Understood);
    assert_eq!(page.2.as_deref(), Some("html"));
}

#[test]
fn second_scan_skips_304_and_drops_only_missing_urls() {
    let gone = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&gone);
    let site = serve(move |path, req| {
        if path == "/gone" && !flag.load(Ordering::SeqCst) {
            return Response {
                status: 404,
                content_type: "text/plain".into(),
                headers: Vec::new(),
                body: b"gone".to_vec(),
            };
        }
        if req.contains("If-None-Match") && !path.ends_with("/robots.txt") {
            return Response {
                status: 304,
                content_type: String::new(),
                headers: Vec::new(),
                body: Vec::new(),
            };
        }
        match path {
            "/" => html(
                r#"<html><head><title>Home</title></head><body><a href="/gone">g</a></body></html>"#,
            ),
            "/gone" => html("<html><head><title>Gone</title></head><body>gone</body></html>"),
            _ => Response {
                status: 404,
                content_type: "text/plain".into(),
                headers: Vec::new(),
                body: Vec::new(),
            },
        }
    });
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello").unwrap();
    let knowledge = Arc::new(KnowledgeDb::in_memory().unwrap());
    let fs_config = ScanConfig::with_root(dir.path());
    let fs = Pipeline::with_knowledge(
        fs_config,
        default_registry(),
        Arc::new(MemoryJobStore::new()),
        Arc::clone(&knowledge),
    );
    let fs_store = run_pipeline(fs);
    assert!(fs_store.iter().any(|(id, _, _)| id.contains("note.txt")));

    let mut web_config = ScanConfig::with_source(ScanSource::Web {
        start_url: site.base.clone(),
        policy: policy(),
    });
    web_config.concurrency = 1;
    let first = Pipeline::with_knowledge(
        web_config.clone(),
        default_registry(),
        Arc::new(MemoryJobStore::new()),
        Arc::clone(&knowledge),
    );
    let store = run_pipeline(first);
    assert!(store.iter().any(|(id, _, _)| id.ends_with("/gone")));
    assert!(store.iter().any(|(id, _, _)| id.contains("note.txt")));

    gone.store(false, Ordering::SeqCst);
    let second = Pipeline::with_knowledge(
        web_config.clone(),
        default_registry(),
        Arc::new(MemoryJobStore::new()),
        Arc::clone(&knowledge),
    );
    let mut handle = second.spawn();
    drain(&mut handle);
    handle.wait().unwrap();
    let skipped = handle.metrics.objects_skipped.load(Ordering::Relaxed);
    assert!(skipped >= 1, "unchanged page should be skipped");
    let store = handle.store.lock().unwrap();
    assert!(store
        .objects()
        .any(|o| o.descriptor.id.as_str().contains("note.txt")));
    assert!(!store
        .objects()
        .any(|o| o.descriptor.id.as_str().ends_with("/gone")));

    web_config.reanalyze = true;
    let again = Pipeline::with_knowledge(
        web_config,
        default_registry(),
        Arc::new(MemoryJobStore::new()),
        knowledge,
    );
    let mut handle = again.spawn();
    drain(&mut handle);
    handle.wait().unwrap();
    assert!(
        handle.metrics.objects_analyzed.load(Ordering::Relaxed) >= 1,
        "reanalyze fetches and analyzes"
    );
}

#[cfg(feature = "llm")]
#[test]
fn html_full_wins_over_llm_modes() {
    use gnosis::{build_registry, LlmMode};
    for mode in [LlmMode::Fallback, LlmMode::Always] {
        let cfg = gnosis::LlmConfig::new("tinyllama", mode);
        let reg = build_registry(Some(&cfg), None);
        let object = ObjectDescriptor {
            id: gnosis::ObjectId::new("obj:web:https://example.com/"),
            path: PathBuf::from("https://example.com/"),
            relative_path: PathBuf::from("https://example.com/"),
            is_dir: false,
            size: 10,
            modified: None,
            media_type: "text/html".into(),
            extension: Some("html".into()),
            etag: None,
        };
        let proto = ProtoData {
            media_type: "text/html".into(),
            extension: Some("html".into()),
            ..ProtoData::default()
        };
        let (provider, support) = reg.select(&object, &proto).unwrap();
        assert_eq!(provider.id().as_str(), "html");
        assert_eq!(support, Support::Full);
    }
}

fn run_pipeline(pipeline: Pipeline) -> Vec<(String, UnderstandingStatus, Option<String>)> {
    let mut handle = pipeline.spawn();
    drain(&mut handle);
    handle.wait().unwrap();
    let rows = handle
        .store
        .lock()
        .unwrap()
        .objects()
        .map(|o| {
            (
                o.descriptor.id.as_str().to_string(),
                o.status,
                o.provider.as_ref().map(|p| p.as_str().to_string()),
            )
        })
        .collect();
    rows
}

fn drain(handle: &mut gnosis::PipelineHandle) {
    let events = handle.take_events();
    while let Ok(ev) = events.recv() {
        if matches!(ev, gnosis::PipelineEvent::ScanCompleted { .. }) {
            break;
        }
    }
}

#[test]
fn cli_rejects_bad_urls_web_flags_and_web_watch() {
    use assert_cmd::cargo::cargo_bin;
    use assert_cmd::Command;
    let bin = cargo_bin!("gnosis");
    Command::new(bin)
        .args(["scan", "unknown://nope", "--no-tui"])
        .assert()
        .failure();
    Command::new(bin)
        .args(["scan", "file:///tmp", "--no-tui"])
        .assert()
        .failure();
    Command::new(bin)
        .args(["scan", "https://user:pw@example.com/", "--no-tui"])
        .assert()
        .failure();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/mixed-repo");
    Command::new(bin)
        .args([
            "scan",
            fixture.to_str().unwrap(),
            "--no-tui",
            "--max-pages",
            "1",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("web crawl flags"));
    Command::new(bin)
        .args(["watch", "https://example.com/docs"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("filesystem-only"));
}

#[test]
fn cli_scans_the_fixture_server() {
    use assert_cmd::cargo::cargo_bin;
    use assert_cmd::Command;
    let site = serve(|path, _| {
        if path == "/robots.txt" {
            Response {
                status: 200,
                content_type: "text/plain".into(),
                headers: Vec::new(),
                body: b"User-agent: *\nDisallow:\n".to_vec(),
            }
        } else {
            html("<html><head><title>Fixture</title></head><body><h1>Hi</h1></body></html>")
        }
    });
    let dir = tempfile::tempdir().unwrap();
    Command::new(cargo_bin!("gnosis"))
        .args([
            "scan",
            &site.base,
            "--no-tui",
            "--quiet",
            "--allow-private-network",
            "--request-delay-ms",
            "0",
            "--max-pages",
            "5",
            "--job-db",
            dir.path().join("jobs.redb").to_str().unwrap(),
            "--knowledge-db",
            dir.path().join("knowledge.redb").to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("analyzed"))
        .stdout(predicates::str::contains("crawl"));
}
