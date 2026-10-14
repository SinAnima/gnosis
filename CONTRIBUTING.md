# Contributing to Gnosis

Thanks for helping improve Gnosis.

How to build, what `scripts/ci.sh` runs, and why the libraries were chosen is in the README under [Contributing to Gnosis](README.md#contributing-to-gnosis).

## Development setup

Requirements:

- Rust **1.85+** (edition 2021)
- Optional: `git` on `PATH` for repository enrichment during scans

```bash
git clone https://github.com/SinAnima/gnosis.git
cd gnosis
./scripts/ci.sh
```

`scripts/ci.sh` is the local copy of CI: format, Clippy, tests, the 80% line-coverage gate, `cargo deny`, and `cargo audit` when it is installed.

Headless demo against the fixture:

```bash
cargo run -- scan ./fixtures/mixed-repo --no-tui --quiet
```

## Project layout

| Path | Role |
|------|------|
| `src/lib.rs` | Library API |
| `src/main.rs` | CLI binary |
| `src/providers/` | Understanding providers |
| `src/okf/` | OKF-style exporter |
| `src/tui/` | Ratatui UI |
| `fixtures/mixed-repo/` | Demo / e2e fixture |
| `tests/` | Integration tests |
| `.github/workflows/` | CI, release, dependency updates |

## Coding guidelines

- Keep connectors, providers, presentation, and OKF export as clear modules
- Prefer deterministic providers; do not make an LLM mandatory
- Be honest about unknown / partial / failed understanding
- Match existing module style; avoid drive-by refactors
- Run `cargo deny check` when changing dependencies

## Pull requests

1. Open a PR against `main`
2. Ensure CI is green (fmt, clippy, tests, coverage gate ≥80%)
3. Update `CHANGELOG.md` under `[Unreleased]` when user-facing behavior changes
4. Keep the diff focused on one concern

## Release (maintainers)

See [baoulo/RELEASE.md](baoulo/RELEASE.md) if present, otherwise:

1. Ensure `version` in `Cargo.toml` matches the release (e.g. `0.1.0`)
2. Move `[Unreleased]` notes into a dated section in `CHANGELOG.md`
3. Merge to `main`
4. Ensure repository secrets:
   - `CARGO_REGISTRY_TOKEN` — crates.io API token (`cio_…` value only; no quotes / Bearer / trailing newline)
   - Coveralls enabled for the GitHub repo (uses `GITHUB_TOKEN`)
5. Tag and push:

   ```bash
   git tag -a v0.1.0 -m "v0.1.0"
   git push origin v0.1.0
   ```

6. The Release workflow builds binaries, runs `gnosis about`, dry-runs `cargo publish`, creates a GitHub Release, and then publishes the single **`gnosis`** crate.

Manual dry-run:

```bash
cargo publish --dry-run
```
