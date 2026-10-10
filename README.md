# Gnosis

[![CI](https://github.com/SinAnima/gnosis/actions/workflows/ci.yml/badge.svg)](https://github.com/SinAnima/gnosis/actions/workflows/ci.yml)
[![Coverage Status](https://coveralls.io/repos/github/SinAnima/gnosis/badge.svg?branch=main)](https://coveralls.io/github/SinAnima/gnosis?branch=main)
[![crates.io](https://img.shields.io/crates/v/gnosis.svg)](https://crates.io/crates/gnosis)
[![docs.rs](https://docs.rs/gnosis/badge.svg)](https://docs.rs/gnosis)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.85-blue)](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/)

**Gnosis compiles what you have into what you know.**

Gnosis is an enterprise knowledge compiler: a local-first Rust tool that discovers digital objects in a repository or S3 bucket, extracts structured knowledge with deterministic providers (Tree-sitter and lightweight document/data parsers), shows its work in a live TUI, and exports an [OKF](https://github.com/GoogleCloudPlatform/knowledge-catalog) v0.1-style bundle.

This is a proof of concept — not a search engine, vector database, or chat UI.

<img width="600" alt="Screenshot 2026-07-24 at 15 40 14" src="https://github.com/user-attachments/assets/1dc94cac-473b-4d1f-bc8e-9bcdc4e88d24" />


## Install

Release binaries ship on [GitHub Releases](https://github.com/SinAnima/gnosis/releases) (with `SHA256SUMS`). Prefer a package manager or `install.sh` for day-to-day use; `cargo install` is the contributor path. Packaging is scaffolded with [sastri](https://crates.io/crates/sastri) (`Sastri.toml`). CI and the release workflow stay in this repo and are not regenerated.

### Homebrew

From [SinAnima/homebrew-tap](https://github.com/SinAnima/homebrew-tap) (fill formula checksums from `SHA256SUMS` after each release):

```bash
brew install SinAnima/tap/gnosis
```

### Scoop (Windows)

From [SinAnima/scoop-bucket](https://github.com/SinAnima/scoop-bucket):

```powershell
scoop bucket add SinAnima https://github.com/SinAnima/scoop-bucket
scoop install gnosis
```

### mise

See [packaging/mise/README.md](packaging/mise/README.md).

```bash
mise use -g github:SinAnima/gnosis@0.2.0
```

### asdf

From [SinAnima/asdf-gnosis](https://github.com/SinAnima/asdf-gnosis) (push `packaging/asdf-gnosis` there):

```bash
asdf plugin add gnosis https://github.com/SinAnima/asdf-gnosis.git
asdf install gnosis 0.2.0
```

### install.sh

Installs the latest tag into `~/.local/bin` (or `GNOSIS_INSTALL_DIR`). Pin with `GNOSIS_VERSION=v0.2.0`.

```bash
curl -fsSL https://raw.githubusercontent.com/SinAnima/gnosis/main/scripts/install.sh | sh
```

### From crates.io

```bash
cargo install gnosis
# with optional LLM semantic extraction:
cargo install gnosis --features llm
```

### From source

```bash
cargo install --git https://github.com/SinAnima/gnosis
# or from a checkout:
cargo install --path .
cargo install --path . --features llm
```

Templates under [`packaging/`](packaging/) are regenerated with `sastri generate --force` (CI and release workflows are owned by this repo and stay out of Sastri generation).

## Quick start

```sh
cargo build --release
./target/release/gnosis scan ./fixtures/mixed-repo
```

Headless (CI / scripting):

```sh
cargo run -- scan ./fixtures/mixed-repo --no-tui --export
```

S3 (bucket = root, keys = paths; default AWS credentials):

```sh
gnosis scan s3://my-bucket --no-tui --export
gnosis scan s3://my-bucket/path/prefix --region us-east-1
```

## Commands

| Command | Description |
|---------|-------------|
| `gnosis scan <path>` | Live TUI scan (local directory) |
| `gnosis scan s3://bucket[/prefix]` | Scan an S3 bucket |
| `gnosis scan <target> --no-tui` | Headless scan + summary |
| `gnosis scan <target> --no-tui --export` | Also write `knowledge.okf/` |
| `gnosis about` | Product overview |

### `scan` options

| Flag | Description |
|------|-------------|
| `--no-tui` | Headless mode (prints summary) |
| `--quiet` | Suppress per-object event lines in headless mode |
| `--export` | Write OKF when the scan finishes (headless) |
| `--output <dir>` | OKF output directory (default: `knowledge.okf`) |
| `--max-size <bytes>` | Max bytes read per object (default: 2 MiB) |
| `--concurrency <n>` | Analysis worker count |
| `--region <name>` | AWS region for `s3://` scans (default credential chain otherwise) |
| `--job-db <path>` | Persistent job queue database (default: `.gnosis/jobs.redb`) |
| `--max-attempts <n>` | Attempts per job including the first (default: 3; `1` disables retries) |
| `--retry-base-ms <ms>` | Initial retry backoff, doubled after each failure (default: 250) |
| `--retry-max-ms <ms>` | Upper bound on retry backoff (default: 30000) |
| `--llm [MODEL]` | Enable LLM semantic provider (needs `--features llm`). Default model: `gpt-4o-mini` |
| `--llm-mode <mode>` | `fallback` (images/OCR/unknown; default) or `always` (Partial on all; Full deterministic still wins) |
| `--llm-max-requests <n>` | Stop further model calls after n requests (`0` = unlimited) |
| `--llm-max-tokens <n>` | Stop further model calls after n total tokens (`0` = unlimited) |
| `--llm-allow-secrets` | Send objects even when they look like they contain secrets |
| `--llm-cache-dir <path>` | Cached replies (default `.gnosis/llm-cache`) |

Local scans are recursive under `<path>` (via `ignore::WalkBuilder`). Descent skips `.gitignore` matches, `target` / `node_modules` / `.git` / `knowledge.okf` / `.gnosis`, and does not follow symlinks.

S3 scans list object keys under the bucket (or prefix), skip directory markers (`…/`), and skip keys whose path components match the same basename excludes (`target`, `node_modules`, `.git`, `knowledge.okf`, `.gnosis`).

Every discovered artifact is enqueued as a durable `analyze_object` job (function kind + JSON args + result/error). Async workers claim jobs from the store; the default backend is [redb](https://docs.rs/redb).

A job that fails is retried automatically with exponential backoff: after attempt `n` it goes back to `pending` with `available_at = now + min(retry_max_ms, retry_base_ms * 2^(n-1))`, and no worker can claim it until then. It is only marked `failed` once `--max-attempts` is exhausted; the stored error keeps the attempt count.

### LLM semantic extraction (optional)

Build with `--features llm`, then:

```sh
# Fallback: images / PDFs / unknown binaries (deterministic Full providers still win)
gnosis scan ./repo --llm --no-tui
gnosis scan ./repo --llm gpt-4o-mini --llm-mode fallback

# Prefer Partial LLM over plaintext for leftover text/*; Full Tree-sitter still wins
gnosis scan ./repo --llm claude-3-haiku-20240307 --llm-mode always

# Local Ollama
gnosis scan ./repo --llm tinyllama:latest --no-tui
```

The model id selects an adapter: OpenAI (`gpt-…`, `OPENAI_API_KEY`), Anthropic (`claude-…`, `ANTHROPIC_API_KEY`), Gemini (`gemini-…`, `GEMINI_API_KEY`), or Ollama (anything else, including `tinyllama:latest`; no key). Prefix the id with `openai::`, `anthropic::`, `gemini::`, or `ollama::` to force one. A scan without `--llm` never calls a model. Full Tree-sitter, Markdown, and data providers still win over the LLM.

Prompt packs are chosen per object (`code`, `code_review`, `docs`, `data`, `image`, `ocr`, `generic`).

Every LLM entity and relationship is stamped `origin=llm`, the model id, the prompt kind, a prompt version, and `review=pending`. Pending claims stay in the OKF bundle. Accept one after export:

```sh
gnosis review list
gnosis review accept ent:module:auth@src/lib.rs
```

During a TUI scan the same commands are `:review` and `:review accept <id>`. Accepted marks in the live scan are in memory only; `gnosis review accept` edits the exported bundle.

Repeat scans reuse `.gnosis/llm-cache` when the content fingerprint, model, prompt kind, and prompt version match. That skips the model call. It does not skip visiting the file.

```sh
gnosis scan ./repo --llm tinyllama:latest --llm-max-requests 20 --llm-max-tokens 100000
```

`0` on either cap means unlimited. Hitting a cap records the object as partial with reason `llm budget exhausted` and continues the scan. Objects that look like a private key, an AWS access key, or an API token are not sent unless you pass `--llm-allow-secrets`. Token totals in the TUI depend on the provider; Ollama often reports 0.

### Jobs CLI

Each scan assigns a `scan:…` id; every job is linked to it. Reruns create a fresh `rerun:…` id.

```sh
gnosis jobs scans                    # list scan ids + counts
gnosis jobs list
gnosis jobs list --status failed
gnosis jobs list --scan-id scan:…    # filter by scan (unique prefix ok)
gnosis jobs list --status completed --limit 20
gnosis jobs show job:xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
gnosis jobs show <short-id-prefix>   # unique prefix also works
gnosis jobs purge 5d                 # delete jobs older than 5 days
gnosis jobs purge 12h --dry-run      # preview only
gnosis jobs purge --scan-id scan:…   # delete every job in a scan
gnosis jobs purge 5d --scan-id scan:…  # age filter within a scan
gnosis jobs pause --scan-id scan:…     # pause pending/running in a scan
gnosis jobs pause abc123de,def456
gnosis jobs unpause --scan-id scan:…
gnosis jobs stop --scan-id scan:…      # cancel pending/paused/running
gnosis jobs rerun job:aaa,job:bbb    # requeue + re-execute
gnosis jobs rerun --scan-id scan:…   # requeue + re-execute whole scan
gnosis jobs rerun abc123de,def456 --no-run
```

### TUI commands (`:`)

`summary` · `objects [status]` · `unknown` · `providers` · `stats` · `review` · `review accept <id>` · `jobs [status]` · `job <id>` · `find <text>` · `explain <name>` · `graph <name>` · `export okf [path]` · `quit`

Shortcuts: `s` summary · `u` unknown · `e` export · `J` jobs · `q` quit

## What you get

1. A live (or printed) inventory of what was **understood**, **partial**, **unknown**, or **failed**
2. Extracted entities and relationships (functions, classes, modules, documents, CSV columns, …)
3. Optional **OKF bundle** (`knowledge.okf/` by default) — markdown + YAML you can browse, commit, or feed to other tools

## Good fit today

- Point at an unfamiliar local Git/repo tree and see structure emerge
- Scan an S3 bucket the same way (keys as file paths) from a laptop or CI
- Demo / PoC of “compile tree → structured knowledge”
- Export a portable knowledge directory for humans or agents

Not for yet: remote Git hosts, web crawl, office docs as first-class parsers, vectors/RAG, or chat (see [Limitations](#limitations)).

## Library usage

One crate provides both the CLI and the library. Depend on it as:

```toml
[dependencies]
gnosis = "0.1"
```

Public modules include pipeline/store/query APIs plus `providers`, `okf`, and `tui` for embedding.

Headless scan + export sketch:

```rust
use gnosis::{
    default_registry, Exporter, OkfExporter, Pipeline, PipelineEvent, QueryEngine, Result,
    ScanConfig,
};

fn main() -> Result<()> {
    let config = ScanConfig::with_root("./my-repo");
    let pipeline = Pipeline::new(config.clone(), default_registry())?;
    let mut handle = pipeline.spawn();
    let events = handle.take_events();

    while let Ok(ev) = events.recv() {
        if matches!(ev, PipelineEvent::ScanCompleted { .. }) {
            break;
        }
    }
    handle.wait()?;

    let store = handle.store.lock().unwrap();
    print!("{}", QueryEngine::new(&store).summary());
    OkfExporter::new().export(&store, &config.output_path)?;
    Ok(())
}
```

Typical flow: `default_registry()` → `Pipeline::spawn` → observe `PipelineEvent`s / query `KnowledgeStore` → `OkfExporter::export`.

## Architecture

```text
gnosis (one crate)
├── binary: gnosis
└── library modules:
    ├── connectors, pipeline, store, query
    ├── providers (tree-sitter + docs/data + generic [+ optional llm])
    ├── prompts (LLM prompt packs by content kind)
    ├── okf (export)
    └── tui (Ratatui UI)
```

Pipeline: Connector → Discovery → **Job queue** (`analyze_object`) → ProtoData → Provider selection → Analysis → Knowledge store → Query / OKF export.

Events flow over a bounded channel so the TUI observes progress without coupling to workers.

## What the PoC understands

- **Code:** C++, Rust, Elixir via Tree-sitter (modules, types, functions, imports/includes, inheritance where syntactic)
- **Docs:** Markdown, plain text
- **Data:** JSON, YAML, TOML, CSV (bounded)
- **LLM (optional `--features llm`):** prompt packs for code, code review, docs, data, image analysis, OCR, and generic objects — emits inferred entities/relationships when `--llm` is set
- **Everything else:** honest unknown / partial with metadata and a candidate future provider category

## Limitations

- Connectors today: local filesystem and S3 (no remote Git hosts, web crawl)
- LLM provider is opt-in (`--features llm` + `--llm`). LLM claims are inferences (`origin=llm`, `review=pending`) and never replace a Full deterministic provider. No vectors or persistent semantic index
- Tree-sitter is syntactic — not compiler-grade semantics
- Git enrichment uses the `git` CLI when available (filesystem scans only; not a pure-Rust Git library)
- OKF export is a hand-written markdown+YAML bundle plus `sidecar.json` (not the `okf` crates.io crate)
- No `gnosis.toml` yet — configuration is CLI flags only

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
cargo deny check
cargo run -- scan ./fixtures/mixed-repo --no-tui
```

### CI/CD

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| [ci.yml](.github/workflows/ci.yml) | push/PR to `main` | fmt, clippy, test (Linux/macOS/Windows), coverage → Coveralls, audit/deny, release build, crates.io package check |
| [release.yml](.github/workflows/release.yml) | `v*` tag | multi-platform binaries, GitHub Release, publish crates to crates.io |
| [dependencies.yml](.github/workflows/dependencies.yml) | weekly / manual | dependency update PRs |

Secrets / external setup:

- Enable the repo on [Coveralls](https://coveralls.io) (action uses `GITHUB_TOKEN`)
- Set `CARGO_REGISTRY_TOKEN` for crates.io publish on release tags

Releases: see [CONTRIBUTING.md](CONTRIBUTING.md#release-maintainers). Push an annotated `v*` tag matching `version` in `Cargo.toml` (publishes the single `gnosis` crate).

## Demo

See [demo.md](demo.md).

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for release history.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, PR expectations, and the maintainer release checklist.

## License

MIT — see [LICENSE](LICENSE).
