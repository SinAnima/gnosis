# v1 baseline (v0.4.2)

This page records the v0.4.2 measurement. The crate in this tree is 0.7.0: JPEG and TIFF with EXIF are partial, PNG, WebP, AVIF, PSD, HEIC, and video container metadata is partial, DOCX, XLSX, PPTX, and PDF have a structural graph and stay partial, and JavaScript, TypeScript, Python, Java, and C# have Tree-sitter readers. See [CHANGELOG.md](../../CHANGELOG.md).

Ground truth for the tree that adds the Stage 0 harness on top of commit `4f84c54` (`0.4.2/Baseline_audit-regression_harness`). The matrix below was run on that working tree. This document does not claim a new reader.

## Pin

| Item | Value |
| --- | --- |
| Parent commit | `4f84c54db90f7282099f3d1a2a28bb0819a3a785` |
| Crate after this stage | 0.4.2 |
| MSRV | 1.85 |
| Features | `default = ["web"]`, optional `llm` |

`llm` is off unless the crate is built with `--features llm` and the scan passes `--llm`. The golden scan does neither.

## Command matrix

Run on this machine after the harness compiled. Each command exited 0.

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --all-targets --all-features -- -D warnings` | pass |
| `cargo test --all-targets` | pass |
| `cargo test --all-targets --all-features` | pass (one ignored live-model recorder) |
| `cargo test --no-default-features` | pass |
| `cargo doc --no-deps` | pass |
| `cargo audit` | pass (`quinn-proto` 0.11.19) |
| `cargo deny check` | pass |

CI already runs fmt, Clippy with `--all-features`, `cargo test --all-targets` on Linux, macOS, and Windows, coverage, audit, deny, a release build, and a crates.io dry-run. The baseline job on Ubuntu adds `--all-features` tests, `--no-default-features` tests, and `cargo doc --no-deps`. `scripts/ci.sh` runs the same three commands locally.

## What a scan of the fixture reports

`fixtures/enterprise-mixed/` is synthetic. `scripts/generate_enterprise_mixed.py` rebuilds it. The JPEG and TIFF reuse the EXIF segment in `fixtures/images/with-exif.jpg`.

The harness copies the tree without `expected.json`, because that file is the oracle and a scan would treat it as JSON. A direct scan of the directory also reads the oracle.

Demo, which includes the oracle:

```sh
cargo run -- scan fixtures/enterprise-mixed --no-tui --export
```

One run on this Mac reported 18 objects in about 1.5 seconds: 11 understood, 5 partial, 2 unknown, 0 failed. Peak resident size of that `cargo run` process was about 30 MiB. That figure is a measurement, not a CI gate.

The harness's 17 artifacts, oracle excluded:

| Result | Files |
| --- | --- |
| Understood | `src/pricing.rs`, `src/quote.rs` (both define `price`), `cpp/rule.cpp`, `cpp/rule.hpp`, `elixir/entry.ex`, `docs/overview.md`, `data/prices.json`, `data/prices.csv`, `images/camera.jpg`, `images/scan.tif` |
| Partial, generic metadata | `office/brief.docx` (`office-provider`), `office/book.xlsx` (`office-provider`), `office/deck.pptx` (`office-provider`), `office/note.pdf` (`pdf-provider`), `archives/broken.zip` (`archive-provider`) |
| Unknown, generic metadata | `images/caption.png` (`image-provider`), `proprietary/widget.bin` (`binary-or-unknown`) |

`tests/enterprise_mixed.rs` compares that projection to `fixtures/enterprise-mixed/expected.json`, checks that a second scan prints `analyzed 0, skipped`, and on Unix checks that a directory symlink named `loop` does not produce a `loop/loop` walk. The walk sets `follow_links` to false.

S3 coverage stays in `src/connectors/s3.rs` (URI parsing, prefix discovery, ETag) plus the CLI rejection of `s3://`. Web coverage stays in `tests/web.rs` (fixture server, limits, HTML, conditional refetch). This stage does not start MinIO.

## Feature-gap matrix

These are expected gaps. A failing golden test is a behavior change. A gap row is not a failure.

| Capability | v0.4.2 |
| --- | --- |
| Rust, C++, Elixir syntax | Tree-sitter. Include matching is a path-suffix heuristic. |
| Markdown, HTML, plain text, JSON, YAML, TOML, CSV | Deterministic readers. |
| JPEG and TIFF EXIF | `exif` provider. `understood` means the metadata was read. |
| PNG text chunks, WebP, XMP, IPTC | Inventoried. Not read. |
| DOCX, XLSX, PPTX | Inventoried as `office-provider`. Package bytes are not opened. |
| PDF | Inventoried as `pdf-provider`. |
| Archives | Inventoried as `archive-provider`. A truncated zip does not panic. |
| Proprietary bytes | Unknown, candidate `binary-or-unknown`. |
| PostgreSQL and other catalogs | No connector. |
| Intrinsic metadata as its own stage | Decision recorded in [ADR 0001](../adr/0001-metadata-first-compilation.md). Pipeline unchanged. |
| Model | Opt-in. Full readers still win. |

## OKF

Export still writes `index.md`, `log.md`, `sidecar.json`, `entities/`, `objects/`, and `relationships/`. Concept pages have YAML frontmatter with a `type`.

Compared with [OKF v0.1](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md), the bundle is a Gnosis dialect, not a conforming v0.1 bundle:

- The root `index.md` carries `type`, `title`, `id`, `source`, and `gnosis_kind`. The spec allows only `okf_version` there. Gnosis does not write `okf_version`.
- `sidecar.json` is extra. It holds provenance the frontmatter also carries.
- Entity filenames come from kind and name, truncated and sanitized. Two entities with the same kind and display name can overwrite one file. That collision is not fixed in this release.

## Component boundaries

Acquisition (`src/connectors/`: filesystem, git, S3, web) builds `ObjectDescriptor` and `ProtoData`. `src/pipeline.rs` queues one analyze job per object. `src/providers/` selects one reader. Facts land in `src/knowledge.rs` and `src/store.rs`, and the incremental copy is `src/knowledge_db.rs`. `src/okf/` writes the bundle. `src/query.rs` and `src/tui/` read the store. `src/jobs/` is the queue.

Public library surface is `src/lib.rs`: `Pipeline`, connectors, providers, the knowledge store, query, the OKF exporter, jobs, and the TUI.

## Dependency decisions

No parser crate is added in v0.4.2. JPEG and TIFF stay on `kamadak-exif` 0.6 (pure Rust, BSD-2-Clause). Office, PDF, PNG, and WebP parsers are not selected yet. Each later choice needs a written note of license, MSRV, native code, and whether it can read metadata without rendering.

## Known limits left in place

- Object identity is the relative path (`obj:<path>`). A rename leaves review marks on the old path.
- `gnosis` name query returns the first match.
- `AttributeMap` values are strings.
- Default `--max-size` is 2 MiB.
- `src/providers/mod.rs` still allows a block of Clippy lints for the provider modules.
