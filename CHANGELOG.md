# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0] - 2026-10-19

### Added

- **Linking** — After a scan finishes, one pass resolves path-shaped references. A single path match is `exact`. No match stays `unresolved`. Several matches stay `ambiguous` and list the paths. Markdown and HTML links, Office hyperlinks, in-workbook formula ranges, catalog foreign keys, and code imports and includes use that rule. A bare word does not match a file that merely contains the word.

### Changed

- `explain` and `graph` list every entity that shares a name.
- `why`, `where`, `meta`, and `deps` read a relationship, an entity's source file, a fact key, and an object's dependencies.
- OKF entity and relationship filenames end with the full BLAKE3 hash of the stable id, so two sections with the same name do not share a file. The path is still the object locator. The content fingerprint is still the content id.

### Notes

- `defines` and `belongs_to` are in the vocabulary and unused. No row is read. No formula is calculated. An unresolved foreign key may point at an entity id that is not in the store. `why` says so and keeps the edge.

## [0.7.1] - 2026-10-18

### Added

- **Database catalog** — `postgresql://` and `postgres://` inventory tables, views, and materialized views. Each relation is one object. Columns, keys, indexes, and declared foreign keys hang off it. A foreign key becomes a `references` edge when the target relation was also discovered. A view dependency outside the schema allowlist is an unresolved fact. Comments stay literal. The default schema is `public`. `--schema` adds others. `--db-max-objects` defaults to 200. `--db-statement-timeout-ms` defaults to 5000.

### Notes

- No user-table rows are read. The session is one read-only transaction. System schemas are never listed. A privilege failure is a warning on that schema. CI uses a scripted catalog. A live Postgres scan is manual. A catalog scan removes only `obj:db:` ids, so a workbook and a table can share one knowledge file. Semantic coverage stays `not_attempted`.

## [0.7.0] - 2026-10-17

### Added

- **Object storage** — an S3 listing records storage class, ETag, ETag kind, and version id when the service sends one. User metadata and tags stay capped and load only with `--s3-inspect`. `--s3-versions` keeps the latest live version. `--s3-metadata-only` skips the body. `--s3-max-requests` caps list, head, tag, and get calls. `--s3-endpoint` uses path-style addressing, and `--s3-insecure` skips TLS verification for that endpoint.

### Notes

- A multipart ETag is not a content digest. CI uses the in-memory backend. MinIO is a manual demo, not a container job. HTTP 429 and 503 retry three times (100 ms, 200 ms, 400 ms). An expired token is not retried. Error text strips access keys and `X-Amz-Signature`. Semantic coverage stays `not_attempted`.

## [0.6.1] - 2026-10-16

### Added

- **PDF** — a `.pdf` yields Info properties (`pdf:title`, `pdf:author`, `pdf:subject`, `pdf:creator`, `pdf:producer`, `pdf:creation_date`, `pdf:mod_date`, `pdf:page_count`, `pdf:encrypted`, `pdf:signature_present`) and a structural graph: pages, bookmarks, and capped embedded text. A `/Metadata` stream sets `pdf:xmp_present`. A file whose graph was read is partial. Semantic coverage stays `not_attempted`.

### Fixed

- **Install-channel push** — the release job authenticates to the Homebrew, Scoop, and asdf repositories with Basic auth. A Bearer header was rejected, and the runner then failed with `could not read Username`.
- **crates.io order** — a release publishes to crates.io only after Homebrew, Scoop, and asdf succeed.

### Notes

- No page is rendered and no OCR text is invented. Embedded text is literal `Tj` / `TJ` strings. An image-only page has no `embedded_text` and stays partial. An encrypted trailer stays a diagnostic. A signature flag is recorded and not checked. Actions are not run. Attachment bytes and font programs are not stored. The reader is a classic xref table; `lopdf` was not added because it pulls unmaintained `ttf-parser` (RUSTSEC-2026-0192). Those gaps are listed in `baoulo/library-gaps.md`.
- `zip` stays at 6.0.0. RUSTSEC-2026-0367, RUSTSEC-2026-0368, and RUSTSEC-2026-0369 apply to `ZipArchive::extract`, which this crate does not call. The patched `zip` 9 line needs Rust 1.88. `cargo deny` and `cargo audit` both ignore those three ids.

## [0.6.0] - 2026-10-16

### Added

- **Office Open XML** — DOCX, XLSX, and PPTX yield package properties (`opc:title`, `opc:creator`, `opc:subject`, `opc:modified`, `opc:revision`, `opc:application`) and a structural graph: headings, sheets and formula source, and slides. A file whose graph was read is partial. Semantic coverage stays `not_attempted`.

### Fixed

- **Install channels** — Homebrew, Scoop, and asdf are published from the GitHub release. Those repositories were empty, so the documented install commands had nothing to fetch.

### Notes

- Formulas are not calculated. Cached cell values are not stored. The A1 subset is `Sheet!A1`, quoted sheet names, `$` absolutes, and `A1:B2`. An external workbook reference stays on the formula with an unresolved diagnostic. Macros are not run, and `vbaProject.bin` is not loaded. Legacy `.doc`, `.xls`, and `.ppt` stay inventoried with `unsupported_legacy_format`. Encrypted packages stay a diagnostic. Those gaps are listed in `baoulo/library-gaps.md`.

## [0.5.2] - 2026-10-16

### Added

- **Phone and video container metadata** — HEIC/HEIF stills (brand, dimensions, EXIF, GPS), MOV/MP4/M4V (duration, dimensions, creation time, GPS), and WebM (duration and dimensions) are intrinsic facts via `nom-exif`. A file whose container metadata was read is partial. Semantic coverage stays `not_attempted`.

### Notes

- No frame decode. The Apple Live Photo content identifier is not an edge, and a matching filename stem is not one either. Codec fourcc names and rotation (`tkhd` matrix, HEIF `irot`) are not read. Those gaps are listed in `baoulo/library-gaps.md`.

## [0.5.1] - 2026-10-15

### Added

- **Image container metadata** — PNG (`IHDR` and text chunks), WebP (dimensions, EXIF, and XMP presence), AVIF (sequence-header dimensions), and PSD (header and layer names) are intrinsic facts. A file whose container metadata was read is partial. Semantic coverage stays `not_attempted`. No pixel decode.

### Notes

- Structured XMP and IPTC fields, ICC profile bodies, and AVIF EXIF, orientation, and XMP are not read. The private list of those gaps is `baoulo/library-gaps.md`. GIF and SVG stay inventoried. Office and PDF readers are still open.

## [0.5.0] - 2026-10-14

### Added

- **Intrinsic evidence** — signature and EXIF facts are stored beside the winning reader. `gnosis inspect <file> --json` prints proto, sniff, and those facts separately from structural entities. Semantic coverage stays `not_attempted` when only EXIF was read, and a JPEG or TIFF with EXIF is partial in the scan.
- **More syntax readers** — JavaScript (`.js`, `.mjs`, `.cjs`), TypeScript (`.ts`, `.tsx`, `.mts`, `.cts`), Python (`.py`), Java (`.java`), and C# (`.cs`) at the same depth as Rust, Elixir, and C++. `--llm` does not take a file one of these readers claims.

### Notes

- No PNG, Office, or PDF readers. Go, PHP, Ruby, Kotlin, and Swift stay inventoried. The path is still the object id. OKF filenames are unchanged.

## [0.4.2] - 2026-10-12

### Added

- **Baseline harness** — `fixtures/enterprise-mixed/` is a synthetic tree (code, Markdown, JSON, CSV, JPEG/TIFF EXIF, a PNG with text, minimal DOCX/XLSX/PPTX/PDF, a truncated zip, and an unknown binary). `tests/enterprise_mixed.rs` checks the scan against `expected.json`, checks that a second scan skips unmodified files, and on Unix checks that a symlink cycle is not walked. PNG, Office, PDF, the archive, and the binary stay expected gaps.
- **Baseline record** — `docs/reviews/v1-baseline.md` is the command matrix, the gap matrix, and the OKF v0.1 differences. `docs/adr/0001-metadata-first-compilation.md` records metadata as its own stage. This release does not change the pipeline.

## [0.4.1] - 2026-10-12

### Changed

- **README** — a contents list, a roadmap, scan flags grouped by use case (every scan, terminal UI, web crawl, object store, and model), and a table of the artifacts a built-in reader understands.

## [0.4.0] - 2026-10-11

### Added

- **Web crawl** — `gnosis scan https://…` fetches a bounded set of pages (50 pages, depth 2, 4 at a time, 250 ms delay, 15 s timeout, 5 redirects, same origin, `robots.txt`). `--max-pages`, `--max-depth`, `--max-concurrency`, `--request-delay-ms`, `--include-subdomains`, and `--allow-private-network` apply only to web scans.
- **HTML reader** — title, headings, description, language, canonical link, and links (`links_to`, `references_external`, `references_asset`, `redirects_to`, `declares_canonical`). CSS, JavaScript, images, and fonts stay unknown. The HTML reader wins over the optional model.
- **Conditional refetch** — a later scan sends `If-None-Match` / `If-Modified-Since`. A `304`, or a matching fingerprint and HTML reader version, skips analysis and keeps entities, relationships, and review marks. A URL that disappears is removed. Other stored objects stay.

### Notes

- No JavaScript, no cookies or authorization headers, no web watch, and no separate crawl database. Addresses are checked before each request and on each redirect Gnosis follows. That is not a DNS-rebinding guarantee at connect time.

## [Unreleased]

### Added

- **EXIF reader** — JPEG and TIFF yield an image entity with standard EXIF, including the extended Exif IFD and GPS. Maker notes and thumbnails are skipped. A file with no EXIF segment stays a partial image. PNG, GIF, SVG, and WebP stay inventoried.
- **LLM vendor adapters** — chat goes through an OpenAI, Anthropic, Gemini, or Ollama adapter selected from the model id (`openai::`, `anthropic::`, `gemini::`, `ollama::` force one). Recorded replies in `fixtures/llm/recorded/` replay in tests so CI does not call a model. The parser accepts `type` as an alias of `kind`.
- **Install channels** — Homebrew, Scoop, mise, asdf, `scripts/install.sh`, and a setup Action, scaffolded with [sastri](https://crates.io/crates/sastri). Existing CI and release workflows stay in-repo.
- **`scripts/ci.sh`** — local run of the CI checks (fmt, Clippy, tests, coverage, deny, audit).

### Changed

- **CI** — runs on pull requests to `main` only, so a merge does not start the same workflow again. Release builds and the crates.io dry-run run in the tag workflow.

### Fixed

- **YAML parser** — documents and OKF frontmatter use `yaml-rust2` instead of archived `serde_yaml`, which depended on unmaintained `unsafe-libyaml` (`RUSTSEC-2024-0449`).
- **`cargo audit`** — `h2` 0.4.20 (`RUSTSEC-2026-0258`), `rustls` 0.23.45 (`RUSTSEC-2026-0285`), and unyanked `chacha20` 0.10.2. `aws-sdk-s3` 1.146.1 drops unsound `lru` 0.16 (`RUSTSEC-2026-0253`).
- **`cargo audit`** — `quinn-proto` 0.11.19 (`RUSTSEC-2026-0338`, `RUSTSEC-2026-0339`, `RUSTSEC-2026-0340`, `RUSTSEC-2026-0341`, `RUSTSEC-2026-0342`).
- **`cargo deny`** — `rustls` 0.23.46 (`RUSTSEC-2026-0346`, `RUSTSEC-2026-0347`, `RUSTSEC-2026-0348`, `RUSTSEC-2026-0349`).
- **crates.io package size** — the published crate omits the demo PDF and PNGs under `fixtures/mixed-repo/docs/`. Those files stay in git; crates.io rejects uploads over 10 MiB.

## [0.3.0] - 2026-10-11

### Added

- **Incremental compilation** — scans load `.gnosis/knowledge.redb` and skip objects whose bytes, provider, and provider version are unchanged. Filesystem size and mtime, and S3 size and ETag, avoid rereading unchanged objects. `--reanalyze` ignores the store. `--knowledge-db` selects the file.
- **`gnosis diff`** — compares the latest two scan snapshots (or two snapshot ids) for object, status, entity, and relationship changes.
- **`gnosis watch <path>`** — filesystem-only. After each scan, waits for a quiet period and scans again. S3 is not watched.
- **Review marks persist** in the knowledge database. `gnosis review accept` updates that store and, when the bundle is present, the OKF mark.

### Changed

- OKF export rewrites `index.md` from the full store and leaves pages whose object did not change. Deleted objects are removed from the bundle.
- A changed `#include` or `import` path reanalyzes files that name it. Matching is a path suffix, not a header search.

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

[Unreleased]: https://github.com/SinAnima/gnosis/compare/v0.8.0...HEAD
[0.8.0]: https://github.com/SinAnima/gnosis/compare/v0.7.1...v0.8.0
[0.7.1]: https://github.com/SinAnima/gnosis/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/SinAnima/gnosis/compare/v0.6.1...v0.7.0
[0.6.1]: https://github.com/SinAnima/gnosis/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/SinAnima/gnosis/compare/v0.5.2...v0.6.0
[0.5.2]: https://github.com/SinAnima/gnosis/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/SinAnima/gnosis/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/SinAnima/gnosis/compare/v0.4.2...v0.5.0
[0.4.2]: https://github.com/SinAnima/gnosis/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/SinAnima/gnosis/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/SinAnima/gnosis/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/SinAnima/gnosis/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/SinAnima/gnosis/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/SinAnima/gnosis/releases/tag/v0.1.0
