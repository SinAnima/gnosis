# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **LLM vendor adapters** — chat goes through an OpenAI, Anthropic, Gemini, or Ollama adapter selected from the model id (`openai::`, `anthropic::`, `gemini::`, `ollama::` force one). Recorded replies in `fixtures/llm/recorded/` replay in tests so CI does not call a model. The parser accepts `type` as an alias of `kind`.
- **Install channels** — Homebrew, Scoop, mise, asdf, `scripts/install.sh`, and a setup Action, scaffolded with [sastri](https://crates.io/crates/sastri). Existing CI and release workflows stay in-repo.
- **`scripts/ci.sh`** — local run of the CI checks (fmt, Clippy, tests, coverage, deny, audit, release binary).

### Fixed

- **`cargo audit`** — `h2` 0.4.20 (`RUSTSEC-2026-0258`), `rustls` 0.23.45 (`RUSTSEC-2026-0285`), and unyanked `chacha20` 0.10.2. `aws-sdk-s3` 1.146.1 drops unsound `lru` 0.16 (`RUSTSEC-2026-0253`).
- **crates.io package size** — the published crate omits the demo PDF and PNGs under `fixtures/mixed-repo/docs/`. Those files stay in git; crates.io rejects uploads over 10 MiB.

## [0.2.0] - 2026-10-10

### Added

- **Assisted understanding** (`--features llm`) — optional multi-provider semantic extraction via [`genai`](https://crates.io/crates/genai) and prompt packs (code, code review, docs, data, image analysis, OCR, generic). CLI: `scan --llm [MODEL] --llm-mode fallback|always`. Fallback claims images, PDFs, and unknown binaries; always claims Partial on every object. Deterministic Full providers still win. Every LLM claim is stamped `origin=llm`, `model`, `prompt_kind`, `prompt_version`, and `review=pending`. `gnosis review list|accept` edits that mark in an OKF bundle; the TUI command is `:review`. Replies are cached under `.gnosis/llm-cache` by content fingerprint, model, and prompt version. `--llm-max-requests` and `--llm-max-tokens` stop further calls without aborting the scan (`0` is unlimited). Content that looks like a private key, AWS access key, or API token is not sent unless `--llm-allow-secrets`. The TUI shows request, error, cache-hit, and token counters (Ollama often reports 0 tokens).
- **Failed analysis stays in the knowledge store** — an analyze error upserts the object as `failed` (with a secret-scrubbed reason) before the job is retried, so OKF includes it.
- **Persistent async job queue** — every discovered artifact is enqueued as an `analyze_object` job. Persistence is abstracted by `JobStore` (default: **redb** at `.gnosis/jobs.redb`). Jobs store function kind, JSON args, and result/error. CLI: `--job-db`.
- **Job inspection** — `gnosis jobs list [--status …]` and `gnosis jobs show <id>`; TUI `:jobs [status]` (list) and `:job <id>` (detail). Shortcut `J`.
- **Auto-retry with progressive backoff** — failed jobs are requeued automatically with exponential backoff (`available_at` gates claiming) and only marked `failed` once attempts are exhausted. Tunable via `--max-attempts`, `--retry-base-ms`, `--retry-max-ms` on `scan` and `jobs rerun`.
- **Job pause / unpause / stop** — `gnosis jobs pause|unpause|stop` by job ids or `--scan-id`. Pause suspends pending/running jobs (`paused`); unpause returns them to `pending`; stop cancels them (`stopped`, terminal). In-flight workers ignore complete/fail once a job is paused or stopped.
- **Job purge** — `gnosis jobs purge <age>` removes jobs whose `updated_at` is older than the age (`5d`, `12h`, `30m`, `90s`); `--dry-run` previews. Also `purge --scan-id …` (whole scan) or `purge <age> --scan-id …` (age within a scan).
- **Job rerun** — `gnosis jobs rerun <id,id,…>` requeues listed jobs (comma-delimited ids/prefixes) under a fresh `rerun:…` scan and re-executes them; `--no-run` only requeues. Also `rerun --scan-id …` for an entire scan.
- **Scan ids** — each `gnosis scan` creates a `scan:…` id linked to every job. Filter with `jobs list --scan-id`; list scans with `jobs scans`.
- **S3 connector** — `gnosis scan s3://bucket[/prefix]` treats the bucket (or prefix) as the root folder and object keys as paths/filenames. Uses the default AWS credential chain; optional `--region`. No Git enrichment for S3 sources.

### Fixed

- TUI: `:jobs` (and other command output) is no longer hidden when an object is selected. Command output now takes over the lower panel full-width until you browse objects again with `j`/`k`.

### Changed

- Collapsed the multi-crate workspace into a **single** `gnosis` package (library + `gnosis` binary). Internal modules remain (`providers`, `okf`, `tui`, …); crates.io publish is one crate only.
- `ScanConfig` now carries a `ScanSource` (`Filesystem` or `S3`) instead of assuming a local path only.
- Artifact analysis runs via async job workers claiming from the durable queue (crash recovery via `reclaim_stale`).

## [0.1.0] - 2026-07-24

First public release of **Gnosis** — Enterprise Knowledge Compiler.

### Added

- Cargo workspace with crates:
  - `gnosis` (CLI binary)
  - `gnosis-core` (domain types, filesystem connector, pipeline, store, query)
  - `gnosis-providers` (Tree-sitter C++/Rust/Elixir + Markdown/text/JSON/YAML/TOML/CSV + generic metadata)
  - `gnosis-okf` (OKF-style markdown+YAML export behind an `Exporter` trait)
  - `gnosis-tui` (Ratatui live scan UI)
- `gnosis scan <path>` with live TUI or `--no-tui` headless mode
- Recursive discovery respecting `.gitignore`, hard excludes (`target`, `node_modules`, `.git`, `knowledge.okf`), and no symlink following
- ProtoData collection and local Git enrichment via the `git` CLI when available
- Deterministic understanding status: understood / partial / unknown / failed
- Query commands: `summary`, `objects`, `unknown`, `providers`, `stats`, `find`, `explain`, `graph`, `export okf`
- OKF bundle export (`index.md`, `entities/`, `objects/`, `relationships/`, `sidecar.json`, `log.md`)
- Fixture repo (`fixtures/mixed-repo`) and end-to-end / CLI smoke / provider / TUI tests
- CI: fmt, clippy, multi-OS test, llvm-cov → Coveralls (≥80% lines), audit/`cargo deny`, release build, package checks
- Release workflow: multi-platform binaries + GitHub Release + crates.io publish on `v*` tags
- Dependabot + scheduled dependency workflow

### Notes

- PoC scope: local filesystem only; no LLM providers, vectors, remote connectors, or `gnosis.toml`
- OKF export is a hand-written markdown+YAML bundle (not coupled to a specific OKF library crate)
- MSRV: Rust **1.85**

[Unreleased]: https://github.com/SinAnima/gnosis/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/SinAnima/gnosis/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/SinAnima/gnosis/releases/tag/v0.1.0
