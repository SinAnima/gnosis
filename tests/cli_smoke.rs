use assert_cmd::cargo::cargo_bin;
use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;
use tempfile::TempDir;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/mixed-repo")
}

#[test]
fn cli_about_and_help() {
    Command::new(cargo_bin!("gnosis"))
        .arg("about")
        .assert()
        .success()
        .stdout(predicate::str::contains("Enterprise Knowledge Compiler"));

    Command::new(cargo_bin!("gnosis"))
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("scan"));
}

#[test]
fn cli_scan_headless_summary() {
    let dir = TempDir::new().unwrap();
    let job_db = dir.path().join("jobs.redb");
    let knowledge_db = dir.path().join("knowledge.redb");
    Command::new(cargo_bin!("gnosis"))
        .args([
            "scan",
            fixture().to_str().unwrap(),
            "--no-tui",
            "--quiet",
            "--job-db",
            job_db.to_str().unwrap(),
            "--knowledge-db",
            knowledge_db.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Gnosis summary"))
        .stdout(predicate::str::contains("unknown / partial"));
}

#[test]
fn cli_scan_headless_export() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("out.okf");
    let job_db = dir.path().join("jobs.redb");
    let knowledge_db = dir.path().join("knowledge.redb");
    Command::new(cargo_bin!("gnosis"))
        .args([
            "scan",
            fixture().to_str().unwrap(),
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
        .success()
        .stdout(predicate::str::contains("export complete"));
    assert!(out.join("index.md").exists());
    assert!(out.join("sidecar.json").exists());
}

#[test]
fn cli_scan_missing_path_fails() {
    Command::new(cargo_bin!("gnosis"))
        .args(["scan", "/no/such/gnosis/path", "--no-tui"])
        .assert()
        .failure();
}

#[test]
fn cli_scan_invalid_s3_uri_fails() {
    Command::new(cargo_bin!("gnosis"))
        .args(["scan", "s3://", "--no-tui"])
        .assert()
        .failure();
}

#[test]
fn cli_jobs_list_and_show() {
    let dir = TempDir::new().unwrap();
    let job_db = dir.path().join("jobs.redb");
    let knowledge_db = dir.path().join("knowledge.redb");
    // Seed the db via a headless scan.
    Command::new(cargo_bin!("gnosis"))
        .args([
            "scan",
            fixture().to_str().unwrap(),
            "--no-tui",
            "--quiet",
            "--job-db",
            job_db.to_str().unwrap(),
            "--knowledge-db",
            knowledge_db.to_str().unwrap(),
        ])
        .assert()
        .success();

    Command::new(cargo_bin!("gnosis"))
        .args([
            "jobs",
            "list",
            "--job-db",
            job_db.to_str().unwrap(),
            "--status",
            "completed",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("STATUS"))
        .stdout(predicate::str::contains("completed"));

    // Grab a job id from a completed list for show.
    let output = Command::new(cargo_bin!("gnosis"))
        .args([
            "jobs",
            "list",
            "--job-db",
            job_db.to_str().unwrap(),
            "--limit",
            "1",
            "--status",
            "completed",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Data rows: STATUS KIND SHORT_ID PATH… (kind is analyze_object)
    let short = stdout
        .lines()
        .find(|l| l.contains("analyze_object"))
        .and_then(|l| l.split_whitespace().nth(2))
        .expect("expected a job data row with an id column");

    Command::new(cargo_bin!("gnosis"))
        .args(["jobs", "show", short, "--job-db", job_db.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("id:"))
        .stdout(predicate::str::contains("kind:"));

    Command::new(cargo_bin!("gnosis"))
        .args([
            "jobs",
            "purge",
            "1s",
            "--dry-run",
            "--job-db",
            job_db.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("dry-run"));

    // Age everything by waiting isn't reliable; purge with a huge age should remove none.
    Command::new(cargo_bin!("gnosis"))
        .args([
            "jobs",
            "purge",
            "365d",
            "--job-db",
            job_db.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("purged 0"));

    Command::new(cargo_bin!("gnosis"))
        .args([
            "jobs",
            "rerun",
            short,
            "--no-run",
            "--job-db",
            job_db.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("requeued"));

    // Scan id listing / filter / purge / rerun.
    let scans = Command::new(cargo_bin!("gnosis"))
        .args(["jobs", "scans", "--job-db", job_db.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(scans.status.success());
    let scans_out = String::from_utf8_lossy(&scans.stdout);
    let scan_id = scans_out
        .lines()
        .find(|l| l.starts_with("scan:") || l.starts_with("rerun:"))
        .and_then(|l| l.split_whitespace().next())
        .expect("expected a scan id line")
        .to_string();

    Command::new(cargo_bin!("gnosis"))
        .args([
            "jobs",
            "list",
            "--scan-id",
            &scan_id,
            "--job-db",
            job_db.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(&scan_id));

    Command::new(cargo_bin!("gnosis"))
        .args([
            "jobs",
            "rerun",
            "--scan-id",
            &scan_id,
            "--no-run",
            "--job-db",
            job_db.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("requeued"))
        .stdout(predicate::str::contains("rerun:"));

    // After rerun --no-run, original scan should be empty; purge the new rerun scan.
    let scans2 = Command::new(cargo_bin!("gnosis"))
        .args(["jobs", "scans", "--job-db", job_db.to_str().unwrap()])
        .output()
        .unwrap();
    let scans2_out = String::from_utf8_lossy(&scans2.stdout);
    let rerun_id = scans2_out
        .lines()
        .find(|l| l.starts_with("rerun:"))
        .and_then(|l| l.split_whitespace().next())
        .expect("expected a rerun: scan id")
        .to_string();

    Command::new(cargo_bin!("gnosis"))
        .args([
            "jobs",
            "purge",
            "--scan-id",
            &rerun_id,
            "--dry-run",
            "--job-db",
            job_db.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("dry-run"));
}
