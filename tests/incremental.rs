//! Incremental compilation: a second scan skips unchanged objects.

use gnosis::{default_registry, KnowledgeDb, MemoryJobStore, Pipeline, ScanConfig, ScanMetrics};
use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

fn run_scan(root: &Path, knowledge: Arc<KnowledgeDb>, jobs: Arc<MemoryJobStore>) -> (u64, u64) {
    let mut config = ScanConfig::with_root(root);
    config.concurrency = 2;
    let metrics = Arc::new(ScanMetrics::new());
    let pipeline = Pipeline::with_knowledge_and_metrics(
        config,
        default_registry(),
        jobs,
        Arc::clone(&metrics),
        knowledge,
    );
    let mut handle = pipeline.spawn();
    let events = handle.take_events();
    while events.recv_timeout(Duration::from_secs(30)).is_ok() {}
    handle.wait().expect("scan");
    (
        metrics.objects_analyzed.load(Ordering::Relaxed),
        metrics.objects_skipped.load(Ordering::Relaxed),
    )
}

#[test]
fn second_scan_skips_until_a_file_changes_or_disappears() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a.rs"), "fn kept() {}\n").unwrap();
    fs::write(root.join("b.rs"), "fn other() {}\n").unwrap();
    let knowledge = Arc::new(KnowledgeDb::in_memory().unwrap());
    let jobs = Arc::new(MemoryJobStore::new());

    let (analyzed, skipped) = run_scan(root, Arc::clone(&knowledge), Arc::clone(&jobs));
    assert_eq!(analyzed, 2, "first scan analyzes both files");
    assert_eq!(skipped, 0);

    let (analyzed, skipped) = run_scan(root, Arc::clone(&knowledge), Arc::clone(&jobs));
    assert_eq!(analyzed, 0, "unchanged files are not reanalyzed");
    assert_eq!(skipped, 2);

    fs::write(root.join("b.rs"), "fn other() { let x = 1; }\n").unwrap();
    let (analyzed, skipped) = run_scan(root, Arc::clone(&knowledge), Arc::clone(&jobs));
    assert_eq!(analyzed, 1, "only the touched file is reanalyzed");
    assert_eq!(skipped, 1);

    fs::remove_file(root.join("b.rs")).unwrap();
    let (analyzed, skipped) = run_scan(root, Arc::clone(&knowledge), jobs);
    assert_eq!(analyzed, 0);
    assert_eq!(skipped, 1);
    let ids = knowledge.snapshot_ids().unwrap();
    let latest = knowledge.snapshot(ids.last().unwrap()).unwrap().unwrap();
    assert!(latest
        .objects
        .iter()
        .any(|o| o.relative_path == "b.rs" && o.reason.as_str() == "deleted"));
    assert!(!latest
        .objects
        .iter()
        .any(|o| o.relative_path == "b.rs" && o.reason.as_str() != "deleted"));
}

#[test]
fn include_change_reanalyzes_the_includer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a.h"), "int x;\n").unwrap();
    fs::write(
        root.join("main.cpp"),
        "#include \"a.h\"\nint main(){return 0;}\n",
    )
    .unwrap();
    let knowledge = Arc::new(KnowledgeDb::in_memory().unwrap());
    let jobs = Arc::new(MemoryJobStore::new());

    let (analyzed, _) = run_scan(root, Arc::clone(&knowledge), Arc::clone(&jobs));
    assert_eq!(analyzed, 2);

    let (analyzed, _) = run_scan(root, Arc::clone(&knowledge), Arc::clone(&jobs));
    assert_eq!(analyzed, 0);

    fs::write(root.join("a.h"), "int y;\n").unwrap();
    let (analyzed, skipped) = run_scan(root, knowledge, jobs);
    assert_eq!(analyzed, 2, "header change also reanalyzes the includer");
    assert_eq!(skipped, 0);
}

#[test]
fn provider_version_change_does_not_invalidate_includers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a.h"), "int x;\n").unwrap();
    fs::write(
        root.join("main.cpp"),
        "#include \"a.h\"\nint main(){return 0;}\n",
    )
    .unwrap();
    let knowledge = Arc::new(KnowledgeDb::in_memory().unwrap());
    let jobs = Arc::new(MemoryJobStore::new());
    run_scan(root, Arc::clone(&knowledge), Arc::clone(&jobs));

    let records = knowledge.load_all().unwrap();
    let mut header = records
        .into_iter()
        .find(|r| r.object.descriptor.relative_path.ends_with("a.h"))
        .unwrap();
    header.provider_version = Some("9.9.9".into());
    knowledge.put(&header).unwrap();

    let (analyzed, skipped) = run_scan(root, knowledge, jobs);
    assert_eq!(
        analyzed, 1,
        "only the provider-version mismatch is reanalyzed"
    );
    assert_eq!(skipped, 1);
}

#[test]
fn provider_version_change_reanalyzes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a.rs"), "fn kept() {}\n").unwrap();
    let knowledge = Arc::new(KnowledgeDb::in_memory().unwrap());
    let jobs = Arc::new(MemoryJobStore::new());
    run_scan(root, Arc::clone(&knowledge), Arc::clone(&jobs));

    let records = knowledge.load_all().unwrap();
    assert_eq!(records.len(), 1);
    let mut record = records.into_iter().next().unwrap();
    record.provider_version = Some("9.9.9".into());
    knowledge.put(&record).unwrap();

    let (analyzed, skipped) = run_scan(root, knowledge, jobs);
    assert_eq!(analyzed, 1);
    assert_eq!(skipped, 0);
}

#[test]
fn resume_after_one_committed_object_does_not_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a.rs"), "fn kept() {}\n").unwrap();
    fs::write(root.join("b.rs"), "fn other() {}\n").unwrap();
    let knowledge = Arc::new(KnowledgeDb::in_memory().unwrap());
    let jobs = Arc::new(MemoryJobStore::new());
    run_scan(root, Arc::clone(&knowledge), Arc::clone(&jobs));

    let before = knowledge.load_all().unwrap();
    let a_entities = before
        .iter()
        .find(|r| r.object.descriptor.relative_path.ends_with("a.rs"))
        .map(|r| r.entities.len())
        .unwrap();
    let b = before
        .iter()
        .find(|r| r.object.descriptor.relative_path.ends_with("b.rs"))
        .unwrap();
    knowledge.delete(&b.object.descriptor.id).unwrap();

    let (analyzed, skipped) = run_scan(root, Arc::clone(&knowledge), jobs);
    assert_eq!(analyzed, 1, "only the missing object is analyzed");
    assert_eq!(skipped, 1);

    let after = knowledge.load_all().unwrap();
    assert_eq!(after.len(), 2);
    let a_after = after
        .iter()
        .find(|r| r.object.descriptor.relative_path.ends_with("a.rs"))
        .unwrap();
    assert_eq!(a_after.entities.len(), a_entities);
    let ids: Vec<_> = after
        .iter()
        .flat_map(|r| r.entities.iter().map(|e| e.id.to_string()))
        .collect();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(ids.len(), unique.len(), "entity ids are not duplicated");
}

#[test]
fn diff_lists_a_content_change() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a.rs"), "fn kept() {}\n").unwrap();
    let knowledge = Arc::new(KnowledgeDb::in_memory().unwrap());
    let jobs = Arc::new(MemoryJobStore::new());
    run_scan(root, Arc::clone(&knowledge), Arc::clone(&jobs));
    fs::write(root.join("a.rs"), "fn kept() { let x = 1; }\n").unwrap();
    run_scan(root, Arc::clone(&knowledge), jobs);
    let (older, newer) = knowledge.latest_pair().unwrap().unwrap();
    let text = gnosis::format_diff(&older, &newer);
    assert!(text.contains("a.rs"), "{text}");
    assert!(text.contains("content"), "{text}");
}
