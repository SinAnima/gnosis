//! Baseline harness for `fixtures/enterprise-mixed`.
//!
//! The golden file is the oracle, so the scan copies the tree and leaves
//! `expected.json` out. PNG, Office, PDF, the broken archive, and the
//! proprietary binary are expected gaps on this release.

use assert_cmd::cargo::cargo_bin;
use assert_cmd::Command;
use gnosis::KnowledgeDb;
#[cfg(unix)]
use gnosis::{default_registry, MemoryJobStore, Pipeline, ScanConfig};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::Duration;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/enterprise-mixed")
}

fn golden_path() -> PathBuf {
    fixture_root().join("expected.json")
}

fn copy_artifacts(dest: &Path) {
    let src = fixture_root();
    for entry in walk(&src) {
        let rel = entry.strip_prefix(&src).unwrap();
        if rel == Path::new("expected.json") {
            continue;
        }
        let target = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&target).unwrap();
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::copy(&entry, &target).unwrap();
        }
    }
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn project(db: &KnowledgeDb) -> Value {
    let mut objects = Vec::new();
    for record in db.load_all().expect("load knowledge") {
        let path = record
            .object
            .descriptor
            .relative_path
            .to_string_lossy()
            .replace('\\', "/");
        let mut entities: Vec<Value> = record
            .entities
            .iter()
            .map(|entity| json!({"kind": entity.kind, "name": entity.name}))
            .collect();
        entities.sort_by(|left, right| {
            (left["kind"].as_str(), left["name"].as_str())
                .cmp(&(right["kind"].as_str(), right["name"].as_str()))
        });
        let mut relationships: Vec<String> = record
            .relationships
            .iter()
            .map(|rel| rel.kind.clone())
            .collect();
        relationships.sort();
        objects.push(json!({
            "path": path,
            "status": record.object.status.as_str(),
            "provider": record.object.provider.as_ref().map(|id| id.as_str()),
            "entities": entities,
            "relationships": relationships,
        }));
    }
    objects.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    json!({ "objects": objects })
}

fn scan(root: &Path, out: &Path, job_db: &Path, knowledge_db: &Path) -> assert_cmd::assert::Assert {
    Command::new(cargo_bin!("gnosis"))
        .args([
            "scan",
            root.to_str().unwrap(),
            "--no-tui",
            "--quiet",
            "--export",
            "--output",
            out.to_str().unwrap(),
            "--job-db",
            job_db.to_str().unwrap(),
            "--knowledge-db",
            knowledge_db.to_str().unwrap(),
        ])
        .assert()
}

#[test]
fn enterprise_mixed_matches_the_golden_and_skips_a_second_scan() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    copy_artifacts(&root);
    let out = dir.path().join("out.okf");
    let job_db = dir.path().join("jobs.redb");
    let knowledge_db = dir.path().join("knowledge.redb");

    let first = scan(&root, &out, &job_db, &knowledge_db).success();
    let first_out = String::from_utf8_lossy(&first.get_output().stdout).to_string();
    assert!(out.join("index.md").is_file());
    assert!(out.join("sidecar.json").is_file());
    assert!(
        !first_out.contains("llm-semantic"),
        "a default scan must not call the model"
    );

    let db = KnowledgeDb::open(&knowledge_db).unwrap();
    let actual = project(&db);
    drop(db);

    let camera = root.join("images/camera.jpg");
    let inspected = Command::new(cargo_bin!("gnosis"))
        .args([
            "inspect",
            camera.to_str().unwrap(),
            "--json",
            "--knowledge-db",
            knowledge_db.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(inspected.status.success(), "{:?}", inspected.stderr);
    let inspected_out = String::from_utf8_lossy(&inspected.stdout);
    assert!(inspected_out.contains("exif:DateTimeOriginal"));
    assert!(inspected_out.contains("fs:mtime"));
    assert!(inspected_out.contains("not_attempted"));

    let golden = golden_path();
    if std::env::var("GNOSIS_WRITE_BASELINE").is_ok() {
        fs::write(
            &golden,
            serde_json::to_string_pretty(&actual).unwrap() + "\n",
        )
        .unwrap();
    }
    let expected: Value = serde_json::from_str(&fs::read_to_string(&golden).unwrap_or_else(|_| {
        panic!(
            "missing {} — run with GNOSIS_WRITE_BASELINE=1 after reviewing the scan",
            golden.display()
        )
    }))
    .unwrap();
    assert_eq!(actual, expected);

    let objects = actual["objects"].as_array().unwrap();
    let price_fns = objects.iter().filter(|object| {
        object["entities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entity| entity["kind"] == "function" && entity["name"] == "price")
    });
    assert!(
        price_fns.count() >= 2,
        "duplicate price functions stay distinct"
    );
    for gap in [
        "images/caption.png",
        "images/IMG_0001.heic",
        "images/IMG_0001.mov",
        "images/clip.webm",
        "office/brief.docx",
        "office/book.xlsx",
        "office/deck.pptx",
        "office/note.pdf",
        "archives/broken.zip",
        "proprietary/widget.bin",
    ] {
        let object = objects
            .iter()
            .find(|object| object["path"] == gap)
            .unwrap_or_else(|| panic!("missing {gap}"));
        assert_ne!(object["status"], "understood", "{gap} is an expected gap");
        assert_ne!(object["provider"], "llm-semantic");
    }
    for path in ["images/IMG_0001.heic", "images/IMG_0001.mov"] {
        let object = objects
            .iter()
            .find(|object| object["path"] == path)
            .unwrap_or_else(|| panic!("missing {path}"));
        assert!(
            object["relationships"].as_array().unwrap().is_empty(),
            "{path} shares only a filename stem and has no relationship"
        );
    }

    let second = scan(&root, &out, &job_db, &knowledge_db).success();
    let second_out = String::from_utf8_lossy(&second.get_output().stdout);
    assert!(
        second_out.contains("analyzed 0, skipped"),
        "second scan should skip unmodified files, got {second_out}"
    );
}

#[cfg(unix)]
#[test]
fn symlink_cycle_does_not_loop() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("real")).unwrap();
    fs::write(root.join("real/a.rs"), "fn kept() {}\n").unwrap();
    std::os::unix::fs::symlink(".", root.join("real/loop")).unwrap();

    let mut config = ScanConfig::with_root(root);
    config.concurrency = 2;
    let pipeline = Pipeline::with_job_store(
        config,
        default_registry(),
        std::sync::Arc::new(MemoryJobStore::new()),
    );
    let mut handle = pipeline.spawn();
    let events = handle.take_events();
    let mut discovered = Vec::new();
    let mut completed = false;
    while let Ok(event) = events.recv_timeout(Duration::from_secs(10)) {
        if let gnosis::PipelineEvent::ObjectDiscovered { path, .. } = &event {
            let text = path.to_string_lossy().replace('\\', "/");
            assert!(
                !text.contains("loop/loop"),
                "walk followed a symlink cycle: {text}"
            );
            discovered.push(text);
        }
        if matches!(event, gnosis::PipelineEvent::ScanCompleted { .. }) {
            completed = true;
            break;
        }
    }
    assert!(completed, "symlink cycle scan did not finish");
    handle.wait().expect("scan");
    assert!(
        discovered.len() <= 3,
        "too many objects for one file and a link: {discovered:?}"
    );
    assert!(
        discovered.iter().any(|path| path.ends_with("a.rs")),
        "the real file should be discovered: {discovered:?}"
    );
}
