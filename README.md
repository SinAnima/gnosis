# Gnosis

[![CI](https://github.com/SinAnima/gnosis/actions/workflows/ci.yml/badge.svg)](https://github.com/SinAnima/gnosis/actions/workflows/ci.yml)
[![Coverage Status](https://coveralls.io/repos/github/SinAnima/gnosis/badge.svg?branch=main)](https://coveralls.io/github/SinAnima/gnosis?branch=main)
[![crates.io](https://img.shields.io/crates/v/gnosis.svg)](https://crates.io/crates/gnosis)
[![docs.rs](https://docs.rs/gnosis/badge.svg)](https://docs.rs/gnosis)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.85-blue)](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/)

**Gnosis compiles what you have into an [OKF](https://github.com/GoogleCloudPlatform/knowledge-catalog) bundle.**

Point Gnosis at a project folder, an S3 bucket, or an `https://` documentation site. It reads the files and pages there and writes what it can see — modules, functions, types, imports, headings, links, columns, and image metadata — as Markdown with YAML frontmatter. That folder is `knowledge.okf/`. Watch the scan in the terminal, or run it from a script and keep the bundle. The next run reads again only the files and pages that changed.

<img width="600" alt="Gnosis scan in the terminal" src="https://github.com/user-attachments/assets/1dc94cac-473b-4d1f-bc8e-9bcdc4e88d24" />

## Contents

- [Install](#install)
  - [Homebrew](#homebrew)
  - [Scoop (Windows)](#scoop-windows)
  - [mise](#mise)
  - [asdf](#asdf)
  - [install.sh](#installsh)
  - [crates.io](#cratesio)
- [Quick start](#quick-start)
- [Using Gnosis](#using-gnosis)
  - [The OKF bundle](#the-okf-bundle)
  - [Commands](#commands)
  - [Scan options](#scan-options)
    - [Every scan](#every-scan)
    - [Terminal UI](#terminal-ui)
    - [Web crawl options](#web-crawl-options)
    - [Object store](#object-store)
    - [Model](#model)
  - [Jobs](#jobs)
  - [Model-assisted reading](#model-assisted-reading)
  - [What Gnosis reads today](#what-gnosis-reads-today)
- [Incremental compilation](#incremental-compilation)
- [Web crawl](#web-crawl)
- [Adding Gnosis in your CI/CD](#adding-gnosis-in-your-cicd)
- [Developing with Gnosis](#developing-with-gnosis)
  - [From Rust](#from-rust)
  - [From Python](#from-python)
  - [From Elixir](#from-elixir)
- [Contributing to Gnosis](#contributing-to-gnosis)
  - [Build and test](#build-and-test)
  - [Scripts](#scripts)
  - [Layout](#layout)
  - [Architecture](#architecture)
  - [Tech stack](#tech-stack)
  - [This repository's automation](#this-repositorys-automation)
- [Limitations](#limitations)
- [Roadmap](#roadmap)
- [Demo](#demo)
- [Changelog](#changelog)
- [Contributing](#contributing)
- [License](#license)

## Install

Release binaries ship on [GitHub Releases](https://github.com/SinAnima/gnosis/releases) with `SHA256SUMS`. After each release, Homebrew and Scoop formulas take their checksums from that file.

### Homebrew

From [SinAnima/homebrew-tap](https://github.com/SinAnima/homebrew-tap):

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
mise use -g github:SinAnima/gnosis@0.4.1
```

### asdf

From [SinAnima/asdf-gnosis](https://github.com/SinAnima/asdf-gnosis):

```bash
asdf plugin add gnosis https://github.com/SinAnima/asdf-gnosis.git
asdf install gnosis 0.4.1
```

### install.sh

Installs the latest release into `~/.local/bin` (or `GNOSIS_INSTALL_DIR`). Pin a release with `GNOSIS_VERSION=v0.4.1`.

```bash
curl -fsSL https://raw.githubusercontent.com/SinAnima/gnosis/main/scripts/install.sh | sh
```

### crates.io

```bash
cargo install gnosis
```

The published binary analyzes code, documents, and data on its own. Model-assisted extraction is a separate install:

```bash
cargo install gnosis --features llm
```

## Quick start

Scan the project you are in. The terminal UI updates as files are read:

```sh
gnosis scan .
```

Write the OKF bundle to `knowledge.okf/` inside that folder and print a summary. This is the form to use in a script:

```sh
gnosis scan . --no-tui --export
```

Scan a bucket. Keys are treated as file paths. Gnosis uses the default AWS credential chain. Pass `--region` when the bucket is not in the default region:

```sh
gnosis scan s3://my-bucket --no-tui --export
gnosis scan s3://my-bucket/docs/api --region us-east-1
```

A finished headless scan prints how many files were analyzed and how many were already known:

```text
analyzed 3, skipped 40 unchanged
```

## Using Gnosis

A scan walks a folder or lists an S3 prefix. It skips `target`, `node_modules`, `.git`, `knowledge.okf`, and `.gnosis`. On a local folder it also skips anything matched by `.gitignore`, and it does not follow symlinks.

Each file is classified as **understood**, **partial**, **unknown**, or **failed**. Understood files contribute entities and relationships: functions, types, modules, includes, imports, document headings, and data columns. Unknown files stay in the inventory with their path and media type, so a gap is visible.

### The OKF bundle

The scan writes an [OKF](https://github.com/GoogleCloudPlatform/knowledge-catalog) bundle. OKF (Open Knowledge Format) is a directory of Markdown files with YAML frontmatter: one page for each object, entity, and relationship, and `index.md` as the catalog. The default path is `knowledge.okf/`.

```text
knowledge.okf/
├── index.md
├── sidecar.json
├── log.md
├── entities/
├── objects/
└── relationships/
```

`gnosis scan . --no-tui --export` writes that folder. Browse it, commit it, or hand it to another tool. Each page's frontmatter carries confidence, the reader, and understanding status. `sidecar.json` carries the same provenance for the whole bundle.

For a local scan, a relative `--output` path is created inside the scanned folder. For an S3 or web scan it is created in the current working directory.

Gnosis remembers the result in `.gnosis/knowledge.redb`, next to the directory where you ran the command. The job queue, used to resume a scan that was interrupted, lives in `.gnosis/jobs.redb`. `gnosis jobs purge` removes old jobs and leaves the knowledge file alone.

### Commands

| Command | What it does |
|---------|----------------|
| `gnosis scan <path>` | Scan a folder and show the live UI |
| `gnosis scan https://host/docs` | Crawl a site and show the live UI |
| `gnosis scan s3://bucket[/prefix]` | Scan a bucket |
| `gnosis scan <target> --no-tui` | Scan and print a summary |
| `gnosis scan <target> --no-tui --export` | Also write `knowledge.okf/` |
| `gnosis watch <path>` | Scan a folder again after files stop changing |
| `gnosis diff` | Show what the last two scans changed |
| `gnosis review list` | List model claims still waiting for review |
| `gnosis review accept <id>` | Accept one claim |
| `gnosis jobs …` | Inspect, pause, or rerun the scan queue |
| `gnosis about` | Short overview of the product |

### `scan` options

Flags are grouped by the kind of scan they belong to.

#### Every scan

| Flag | Description |
|------|-------------|
| `--max-size <bytes>` | Largest file or page Gnosis will read (default: 2 MiB) |
| `--concurrency <n>` | How many files to analyze at once |
| `--knowledge-db <path>` | Saved analysis (default: `.gnosis/knowledge.redb`) |
| `--reanalyze` | Read and analyze every object on this scan |
| `--job-db <path>` | Job queue file (default: `.gnosis/jobs.redb`) |
| `--max-attempts <n>` | Tries per object, including the first (default: 3). `1` turns retries off |
| `--retry-base-ms <ms>` | First retry wait, doubled after each failure (default: 250) |
| `--retry-max-ms <ms>` | Longest retry wait (default: 30000) |

A file that fails is tried again with a longer wait between tries. After `--max-attempts` it stays **failed**, with the error recorded and secrets removed from the message.

#### Terminal UI

| Flag | Description |
|------|-------------|
| `--no-tui` | Print progress and a summary. Use this in scripts |
| `--quiet` | Hide per-object lines. The summary is still printed |
| `--export` | Write the Markdown bundle when a headless scan finishes |
| `--output <dir>` | Bundle directory (default: `knowledge.okf`) |

With the live UI, type `:` and a command:

| Command | What it does |
|---------|----------------|
| `summary` | Inventory counts. Shortcut: `s` |
| `objects [status]` | List objects, optionally by status |
| `unknown` | Objects still unknown or partial. Shortcut: `u` |
| `providers` | Readers used on this scan |
| `stats` | Scan counters, including crawl and model totals |
| `review` | Model claims waiting for review |
| `review accept <id>` | Accept one claim |
| `jobs [status]` | Queue rows. Shortcut: `J` |
| `job <id>` | One job |
| `find <text>` | Search names |
| `explain <name>` | Why an entity is in the inventory |
| `graph <name>` | Nearby relationships |
| `export okf [path]` | Write the Markdown bundle. Shortcut: `e` |
| `quit` | Leave the UI. Shortcut: `q` |

#### Web crawl options

These flags apply to an `http://` or `https://` scan. A folder or `s3://` scan rejects them.

| Flag | Description |
|------|-------------|
| `--max-pages <n>` | Pages to fetch (default: 50) |
| `--max-depth <n>` | Links followed beyond the start page (default: 2) |
| `--max-concurrency <n>` | Pages fetched at once (default: 4) |
| `--request-delay-ms <ms>` | Wait between page fetches (default: 250) |
| `--include-subdomains` | Also follow subdomains of the start host |
| `--allow-private-network` | Allow loopback, private, and link-local addresses |

#### Object store

| Flag | Description |
|------|-------------|
| `--region <name>` | AWS region for an `s3://` scan. Otherwise the default credential chain |

#### Model

These flags apply when the binary was installed with `--features llm`. A scan without `--llm` never calls a model. See [Model-assisted reading](#model-assisted-reading).

| Flag | Description |
|------|-------------|
| `--llm [MODEL]` | Ask a model about files the built-in readers leave alone. Default model: `gpt-4o-mini` |
| `--llm-mode <mode>` | `fallback` (default) or `always` |
| `--llm-max-requests <n>` | Stop model calls after n requests (`0` means no cap) |
| `--llm-max-tokens <n>` | Stop model calls after n tokens (`0` means no cap) |
| `--llm-allow-secrets` | Send a file even when it looks like it contains a key or token |
| `--llm-cache-dir <path>` | Saved model replies (default: `.gnosis/llm-cache`) |

### Jobs

Every scan has an id like `scan:…`. A rerun gets a new id like `rerun:…`. You can pass a unique prefix anywhere a full id is accepted.

```sh
gnosis jobs scans
gnosis jobs list
gnosis jobs list --status failed
gnosis jobs list --scan-id scan:…
gnosis jobs show <id-or-prefix>
gnosis jobs purge 5d
gnosis jobs purge 12h --dry-run
gnosis jobs purge --scan-id scan:…
gnosis jobs pause --scan-id scan:…
gnosis jobs unpause --scan-id scan:…
gnosis jobs stop --scan-id scan:…
gnosis jobs rerun --scan-id scan:…
gnosis jobs rerun <id>,<id> --no-run
```

`purge` deletes queue rows. It does not delete `.gnosis/knowledge.redb` or an accepted review.

### Model-assisted reading

Install with `--features llm`, then pass `--llm`. A scan without `--llm` never calls a model.

```sh
gnosis scan . --llm --no-tui
gnosis scan . --llm gpt-4o-mini --llm-mode fallback
gnosis scan . --llm claude-haiku-4-5-20251001 --llm-mode always
gnosis scan . --llm tinyllama:latest --no-tui
```

`fallback` asks the model about images, PDFs, and files no built-in reader claims. `always` also asks about leftover text, and the built-in code, Markdown, data, and JPEG/TIFF readers still win when they understand the file.

The model name selects the service: OpenAI (`gpt-…`, `OPENAI_API_KEY`), Anthropic (`claude-…`, `ANTHROPIC_API_KEY`), Gemini (`gemini-…`, `GEMINI_API_KEY`), or Ollama for anything else, including `tinyllama:latest`. Prefix the name with `openai::`, `anthropic::`, `gemini::`, or `ollama::` to force one.

A model claim is stored as an inference with `review=pending`. It stays in the bundle until you accept it:

```sh
gnosis review list
gnosis review accept ent:module:auth@src/lib.rs
```

The same commands in the live UI are `:review` and `:review accept <id>`. An accepted mark is saved in `.gnosis/knowledge.redb`. If `knowledge.okf/` is present, the mark is updated there too.

Identical file contents, model, and prompt reuse `.gnosis/llm-cache` and skip the network call. The file is still part of the scan. `--llm-max-requests` and `--llm-max-tokens` stop further calls and record those files as partial with reason `llm budget exhausted`. A file that looks like a private key, an AWS access key, or an API token is withheld unless you pass `--llm-allow-secrets`.

### What Gnosis reads today

Gnosis scans three kinds of sources. A built-in reader turns a matching file into pages in the OKF bundle. Anything else is kept in the inventory with a path and a file-type guess.

**Where a scan looks**

- A local folder, including Git metadata when `git` is available
- An `s3://bucket[/prefix]` prefix
- An `http://` or `https://` page, then the HTML links it follows, within the crawl limits

Each object is capped by `--max-size` (2 MiB by default).

**What a built-in reader understands**

| Artifact | Extensions | What is extracted |
|----------|------------|-------------------|
| C and C++ | `.c`, `.h`, `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`, `.hxx` | Modules, types, functions, includes, inheritance, from the syntax |
| Rust | `.rs` | Modules, types, functions, and the relationships the syntax shows |
| Elixir | `.ex`, `.exs` | Modules, functions, and the relationships the syntax shows |
| Markdown | `.md`, `.markdown`, `.mdx` | Headings and document structure |
| HTML | `.html`, `.htm`, or `text/html` | Title, headings, description, language, canonical link, and links. No JavaScript |
| Plain text | `.txt`, `.text`, `.log` | A document plus a short preview. Other `text/*` files get a partial reading |
| JSON | `.json` | Document structure and key paths |
| YAML | `.yaml`, `.yml` | Document structure and key paths |
| TOML | `.toml` | Document structure and key paths |
| CSV and TSV | `.csv`, `.tsv` | A dataset and its columns |
| JPEG and TIFF | `.jpg`, `.jpeg`, `.tif`, `.tiff`, or `image/jpeg`, `image/tiff` | EXIF: camera make and model, the original date, orientation, exposure, and GPS when the file has them |

**Scanned, then left as unknown or partial**

CSS, JavaScript, and fonts are recorded and not read as text. PNG, GIF, SVG, and WebP stay in the inventory with a guess such as `image-provider`. PDFs and other binaries are inventoried the same way.

**Optional model**

With a build that includes `--features llm` and a scan that passes `--llm`, the model can claim images, PDFs, and other objects the readers above leave alone. Those claims stay inferences until accepted. A normal build does not call a model.

## Incremental compilation

Gnosis has incremental compilation. The first scan of a folder reads every file and saves what it learned. The next scan loads that save and leaves a file alone when all of these are the same as last time:

- the file's path inside the folder
- the file's contents
- which reader handled it
- that reader's version

You see this as `skipped N unchanged`. Those files keep their entities, relationships, and accepted review marks.

Gnosis avoids rereading a local file when the size and modification time are unchanged. If the time changed and the contents did not, it updates the saved time and still skips analysis. For S3, a matching size and ETag skips the download. For a web page, a matching ETag or `Last-Modified` sends a conditional request. A `304` keeps the stored page, including its entities, relationships, and review marks.

A file is analyzed again when it is new, its contents changed, or the reader that should handle it changed (including a newer version of that reader). A deleted file is removed from the saved inventory, along with the entities and relationships that came from it. A rename is saved as a removal of the old path and an addition of the new one, so an accepted review mark stays on the old path.

When a changed or deleted file is named by another file's `#include` or `import`, that other file is analyzed again too. The match is a path suffix: `a.h` matches `src/a.h`. Gnosis does not search header directories. A reader-version change on a header does not, by itself, pull in the files that include it.

Turn incremental compilation off for one scan:

```sh
gnosis scan . --reanalyze --no-tui
```

`--reanalyze` still writes the new result back to the knowledge file. Point `--knowledge-db` at another file when you want a separate memory, for example a throwaway run in CI.

Compare the last two completed scans without analyzing anything:

```sh
gnosis diff
gnosis diff snap:abc snap:def
```

The report lists added and removed files, status changes, and entity and relationship ids that appeared or disappeared.

On a local folder, `gnosis watch` scans, waits until changes have been quiet (default 400 ms, `--debounce-ms`), and scans again. It ignores writes under `.gnosis` and `knowledge.okf`, so saving the database or the export does not start another scan. Watch is for a folder on disk. An `s3://` or `http(s)` target is rejected.

```sh
gnosis watch .
gnosis watch ./src --debounce-ms 800
```

## Web crawl

```sh
gnosis scan https://example.com/docs --no-tui
```

A web scan starts at one URL and follows HTML links. The defaults are 50 pages, two links deep, four pages at a time, 250 ms between fetches, a 15 second timeout, and five redirects. Each page is capped by `--max-size` (2 MiB). The crawl stays on the same origin, reads `robots.txt`, and sends `gnosis/<version>` as the user agent. `--include-subdomains` also follows hosts under the start host. Those flags, and `--allow-private-network`, are rejected on a folder or bucket scan.

Gnosis does not run JavaScript and does not send cookies or authorization headers. Pages are stored in the same `.gnosis/knowledge.redb` as files. There is no second crawl database. A later scan skips a page whose ETag or modification time is unchanged. A URL that is no longer linked is removed. Files and bucket objects already in the knowledge file stay where they are.

Loopback, private, link-local, and cloud metadata addresses are rejected on the start URL and on every redirect. `--allow-private-network` is the explicit opt-in for a local server. Gnosis checks the address before the request and again on each redirect it follows. It does not re-check that address at the moment a connection is opened, so this is not a DNS-rebinding guarantee.

`gnosis watch` does not crawl the web.

## Adding Gnosis in your CI/CD

Run a headless scan on every push. Cache `.gnosis/knowledge.redb` so the job analyzes the files that changed since the previous successful run. The cache key changes every commit, and `restore-keys` loads the newest earlier save.

```yaml
name: knowledge
on: [push]
jobs:
  scan:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install Gnosis
        run: curl -fsSL https://raw.githubusercontent.com/SinAnima/gnosis/main/scripts/install.sh | sh
      - name: Restore the last analysis
        uses: actions/cache@v4
        with:
          path: .gnosis/knowledge.redb
          key: gnosis-knowledge-${{ runner.os }}-${{ github.sha }}
          restore-keys: |
            gnosis-knowledge-${{ runner.os }}-
      - name: Scan
        run: gnosis scan . --no-tui --quiet --export
      - uses: actions/upload-artifact@v4
        with:
          name: knowledge
          path: knowledge.okf
```

The scan step prints `analyzed N, skipped M unchanged` and a summary. It exits non-zero when the scan itself fails. Unknown files are listed in the summary. They do not fail the job.

Pass `--reanalyze` on a scheduled job when you want a full read, for example after upgrading Gnosis. Leave `.gnosis/jobs.redb` out of the cache. That file is the queue for one run. `gnosis watch` stays on a workstation. CI runs one scan and exits.

The same command fits any system that can install a binary and cache one file:

```sh
gnosis scan . --no-tui --quiet --export --knowledge-db "$CACHE_DIR/knowledge.redb"
```

## Developing with Gnosis

Gnosis is also a library. Your application can scan a folder, read the in-memory inventory, and write the same Markdown bundle the CLI writes. `Pipeline::new` opens `.gnosis/jobs.redb` and `.gnosis/knowledge.redb` in the process working directory. Set `ScanConfig.knowledge_db_path` and `job_db_path` when those files should live somewhere else.

There is no published Python or Elixir package. The tutorials below build a small native extension that calls the library. Shelling out to the `gnosis` binary is the smaller integration when you only need a scan and a bundle on disk.

### From Rust

```toml
[dependencies]
gnosis = "0.3"
```

```rust
use gnosis::{
    default_registry, Exporter, OkfExporter, Pipeline, PipelineEvent, QueryEngine, Result,
    ScanConfig,
};

fn main() -> Result<()> {
    let config = ScanConfig::with_root("./my-project");
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

`default_registry()` is the set of built-in readers. `Pipeline::spawn` runs the scan on a background thread and sends `PipelineEvent`s for progress. `KnowledgeStore` is the inventory the summary and the exporter read. A second `Pipeline::new` against the same `knowledge_db_path` loads the previous analysis and skips unchanged files, the same way the CLI does.

API reference: [docs.rs/gnosis](https://docs.rs/gnosis).

### From Python

This tutorial builds a native module with [PyO3](https://pyo3.rs/) 0.29 and [maturin](https://www.maturin.rs/). The scan detaches from the Python runtime while it runs, so the rest of the process can keep working.

Create a crate named `gnosis_py`:

```toml
[package]
name = "gnosis_py"
version = "0.1.0"
edition = "2021"

[lib]
name = "gnosis_py"
crate-type = ["cdylib"]

[dependencies]
gnosis = "0.3"
pyo3 = { version = "0.29", features = ["extension-module"] }
```

`src/lib.rs`:

```rust
use gnosis::{default_registry, Pipeline, PipelineEvent, QueryEngine, ScanConfig};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

fn scan_path(path: String) -> Result<String, String> {
    let config = ScanConfig::with_root(path);
    let pipeline = Pipeline::new(config, default_registry()).map_err(|e| e.to_string())?;
    let mut handle = pipeline.spawn();
    let events = handle.take_events();
    while let Ok(event) = events.recv() {
        if matches!(event, PipelineEvent::ScanCompleted { .. }) {
            break;
        }
    }
    handle.wait().map_err(|e| e.to_string())?;
    let store = handle.store.lock().map_err(|e| e.to_string())?;
    Ok(QueryEngine::new(&store).summary())
}

#[pymodule]
mod gnosis_py {
    use super::*;

    #[pyfunction]
    fn scan(py: Python<'_>, path: String) -> PyResult<String> {
        py.detach(|| scan_path(path)).map_err(PyRuntimeError::new_err)
    }
}
```

Build and call it:

```sh
pip install maturin
maturin develop
```

```python
import gnosis_py

print(gnosis_py.scan("/path/to/project"))
```

The summary is the same text the CLI prints. Analysis is saved in `.gnosis/knowledge.redb` under the process working directory, so a second `scan` of an unchanged project skips those files. Call `scan` from a worker thread when you need the caller to stay responsive. The function returns after the scan finishes.

### From Elixir

This tutorial builds a NIF with [Rustler](https://hexdocs.pm/rustler) 0.36. The scan is marked `DirtyCpu` so a long folder runs on a dirty scheduler and leaves the normal BEAM schedulers free.

In `mix.exs`:

```elixir
defp deps do
  [{:rustler, "~> 0.36", runtime: false}]
end
```

`lib/my_app/gnosis.ex`:

```elixir
defmodule MyApp.Gnosis do
  use Rustler, otp_app: :my_app, crate: :gnosis_nif

  def scan(_path), do: :erlang.nif_error(:nif_not_loaded)
end
```

`native/gnosis_nif/Cargo.toml`:

```toml
[package]
name = "gnosis_nif"
version = "0.1.0"
edition = "2021"

[lib]
name = "gnosis_nif"
path = "src/lib.rs"
crate-type = ["cdylib"]

[dependencies]
gnosis = "0.3"
rustler = "0.36"
```

`native/gnosis_nif/src/lib.rs`:

```rust
use gnosis::{default_registry, Pipeline, PipelineEvent, QueryEngine, ScanConfig};

#[rustler::nif(schedule = "DirtyCpu")]
fn scan(path: String) -> Result<String, String> {
    let config = ScanConfig::with_root(path);
    let pipeline = Pipeline::new(config, default_registry()).map_err(|e| e.to_string())?;
    let mut handle = pipeline.spawn();
    let events = handle.take_events();
    while let Ok(event) = events.recv() {
        if matches!(event, PipelineEvent::ScanCompleted { .. }) {
            break;
        }
    }
    handle.wait().map_err(|e| e.to_string())?;
    let store = handle.store.lock().map_err(|e| e.to_string())?;
    Ok(QueryEngine::new(&store).summary())
}

rustler::init!("Elixir.MyApp.Gnosis", [scan]);
```

```elixir
MyApp.Gnosis.scan("/path/to/project")
```

`scan/1` returns the summary string. The knowledge file is `.gnosis/knowledge.redb` in the BEAM working directory, which is the Mix project directory when you run `iex -S mix`. Call `scan/1` from a `Task` when the caller should keep processing messages. Rustler loads the NIF on the first call. Until `mix compile` has produced it, the function returns `:nif_not_loaded`.

## Contributing to Gnosis

### Build and test

Requirements: Rust **1.85** or newer. `git` on `PATH` lets filesystem scans attach branch and commit metadata. Model tests that talk to a live service are ignored. The default test run stays offline.

```bash
git clone https://github.com/SinAnima/gnosis.git
cd gnosis
cargo build --release
./target/release/gnosis scan ./fixtures/mixed-repo
```

Headless scan of the fixture, the same shape CI uses:

```bash
cargo run -- scan ./fixtures/mixed-repo --no-tui --export
```

`scripts/ci.sh` is the local copy of GitHub Actions: format, Clippy, tests, the 80% line-coverage gate, `cargo deny`, `cargo audit` when it is installed, and a release build that runs `gnosis about`.

```bash
./scripts/ci.sh
```

Individual checks:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo llvm-cov --all-features --fail-under-lines 80
cargo deny check
cargo audit
```

`scripts/install.sh` downloads a release binary for users. It is not how you build from this checkout.

`fixtures/mixed-repo` is the demo tree used by the end-to-end tests. `cargo test -- --ignored` is the live model recorder. It writes under `fixtures/llm/recorded/` and is not part of CI.

### Scripts

| Script | Purpose |
|--------|---------|
| `scripts/ci.sh` | Run the same checks as `.github/workflows/ci.yml` |
| `scripts/install.sh` | Install a released binary into `~/.local/bin` |

### Layout

| Path | Role |
|------|------|
| `src/lib.rs` | Library API |
| `src/main.rs` | `gnosis` binary |
| `src/connectors/` | Folder walk and S3 listing |
| `src/pipeline.rs` | Discovery, job queue, analysis |
| `src/knowledge_db.rs` | Saved analysis, skip decision, snapshots |
| `src/providers/` | Readers for code, documents, data, and the optional model |
| `src/okf/` | Markdown bundle |
| `src/tui/` | Terminal UI |
| `src/watch.rs` | Debounced folder watch |
| `tests/` | Integration tests |
| `fixtures/mixed-repo/` | Demo tree |

### Architecture

One crate builds the binary and the library.

```text
folder or S3
    → discover files
    → durable analyze_object jobs (.gnosis/jobs.redb)
    → pick a reader
    → entities and relationships
    → in-memory inventory, saved to .gnosis/knowledge.redb
    → summary, TUI, gnosis diff, knowledge.okf/
```

Discovery and analysis are separated so a crashed scan can resume from the job queue, and so the terminal UI can follow progress on a channel without sharing worker code. The in-memory inventory is what queries and export read. The knowledge file is the copy that makes the next scan incremental. Each finished file is written as soon as its job finishes, so a crash keeps the files already completed.

A reader claims a file as full, partial, or none. A full claim wins over a partial one. The optional model reader is partial, so a code, Markdown, or data reader that fully understands a file keeps that result.

### Tech stack

| Library | What it does here | Why this one |
|---------|-------------------|--------------|
| tree-sitter | Structure of C++, Rust, and Elixir | A grammar yields functions, types, and imports without running each language's compiler |
| redb | Job queue and knowledge file | An embedded database is one file, with no server to operate. Jobs and knowledge are separate files, so purging the queue cannot delete what was learned |
| blake3 | Content fingerprint | A fast, stable hash answers "did these bytes change?" |
| ignore | Folder walk | The same skip rules people already have in `.gitignore` |
| notify | `gnosis watch` | Operating-system file events, debounced into one scan |
| clap | Command line | Typed flags and subcommands |
| ratatui, crossterm | Terminal UI | A live view in the terminal, with no GUI toolkit |
| tokio, aws-sdk-s3 | Job workers and S3 | Listing and downloads are async. Workers already run on that runtime. The SDK uses the default AWS credential chain |
| serde, JSON, YAML, TOML, CSV | Data files and saved rows | The formats people already store, plus a single encoding for a knowledge row |
| pulldown-cmark | Markdown | Headings and structure from documents |
| genai | Optional model calls | One interface for OpenAI, Anthropic, Gemini, and Ollama. A normal build leaves this dependency out |
| crossbeam-channel | Progress events | The UI and a headless script share one stream of scan events |
| uuid | Scan and job ids | Ids you can pass back to `gnosis jobs` and `gnosis diff` |

### This repository's automation

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| [ci.yml](.github/workflows/ci.yml) | push and pull request to `main` | fmt, clippy, tests on Linux, macOS, and Windows, coverage, audit, deny, release build, package check |
| [release.yml](.github/workflows/release.yml) | `v*` tag | Binaries, GitHub Release, crates.io publish |
| [dependencies.yml](.github/workflows/dependencies.yml) | weekly or manual | Dependency update pull requests |

Packaging formulas under [`packaging/`](packaging/) are generated with [sastri](https://crates.io/crates/sastri) (`sastri generate --force`). `Sastri.toml` keeps `ci` and `github_releases` off, so those workflows stay owned by this repo.

Release setup: enable the repo on [Coveralls](https://coveralls.io), and set `CARGO_REGISTRY_TOKEN` for crates.io. The checklist is in [CONTRIBUTING.md](CONTRIBUTING.md#release-maintainers). Tag an annotated `v*` that matches `version` in `Cargo.toml`.

Pull requests: one concern, CI green, and a note under `[Unreleased]` in [CHANGELOG.md](CHANGELOG.md) when the behavior a user sees has changed.

## Limitations

- A web crawl does not run JavaScript, does not send cookies or authorization, and is not watched. There is no separate crawl database
- `gnosis watch` is a local folder. An `s3://` or `http(s)` target is rejected
- Include and import invalidation matches the included path as a suffix. A rename keeps an accepted review mark on the old path
- Model reading is opt-in (`--features llm` and `--llm`). Model claims stay inferences until accepted. There is no vector index
- Tree-sitter follows syntax. It is not a substitute for a compiler or type checker
- Git metadata comes from the `git` command during folder scans
- The bundle follows [OKF](https://github.com/GoogleCloudPlatform/knowledge-catalog) v0.1. `sidecar.json` carries confidence, the reader, and status next to the frontmatter
- Configuration is command-line flags. There is no `gnosis.toml` yet

## Roadmap

Release numbers are the order of the work, not a set of dates. A release ships when its exit criteria are met. 0.1.0 through 0.4.1 are published. 0.4.2, in this tree, is the baseline harness described in [docs/reviews/v1-baseline.md](docs/reviews/v1-baseline.md).

| Release | Theme | What it adds |
|---------|--------|----------------|
| 0.1.0 | Trust the scan | A repeatable scan of a local folder into a Markdown bundle |
| 0.2.0 | Assisted understanding | Optional model reading that does not replace deterministic facts |
| 0.3.0 | Recompile, do not rescan | A saved knowledge file, incremental scans, `gnosis diff`, and filesystem watch |
| 0.4.0 | Web ingestion | A bounded `https://` crawl in the same pipeline |
| 0.4.2 | Baseline | A regression harness and a written gap matrix. No new readers |
| 0.5.0 | Understand more | Deeper C++, Rust, and Elixir reading; documentation linked to code; evidence and confidence you can inspect; coverage by path, file type, and reader |
| 0.6.0 | Teach Gnosis | A stable way to add readers and domain rules without changing Gnosis itself |
| 0.7.0 | Object stores | Production-scale S3 and MinIO inventory, resume, sampling, and cost controls. A prefix scan of S3 already ships |
| 0.8.0 | Knowledge access | A stable query API, plus MCP and local HTTP adapters for other tools |
| 1.0.0 | Stable knowledge infrastructure | Supported schemas, upgrades, and an operating model teams can build on |

## Demo

See [demo.md](demo.md).

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## Contributing

Build, test, and release steps are in [Contributing to Gnosis](#contributing-to-gnosis). Review expectations for pull requests are also in [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT — see [LICENSE](LICENSE).
