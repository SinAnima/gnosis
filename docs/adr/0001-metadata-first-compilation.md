# ADR 0001 — Metadata-first compilation

**Status:** Accepted. Implemented in v0.5.0 for signature and EXIF facts, and in v0.5.1 for PNG, WebP, AVIF, and PSD container metadata. Office and PDF readers are still open.

## Context

Gnosis selects one provider per object. `Support::Full` beats `Partial`, which beats `Weak`. That rule is what keeps a Tree-sitter or EXIF result ahead of the optional model.

It also makes native metadata compete with syntax. `GenericMetadataProvider` is the weak fallback, and a JPEG with EXIF is marked `understood`. That status means the metadata reader succeeded. It does not mean the picture's subject was interpreted.

Later stages need Office properties, PDF Info, PNG text, and database catalogs to land as facts without displacing a structural reader, and without a model.

## Decision

Intrinsic metadata is its own stage. It runs from the bytes and the acquisition context, and it does not compete with a syntax, document, or data reader for a single winning slot.

Each extracted field keeps its source namespace (`exif:DateTimeOriginal`, `dc:creator`, and so on). A normalized projection such as a creation time is a view over those fields. Conflicts stay as separate facts.

`understood / partial / unknown / failed` remains the user-facing projection. Semantic understanding is a separate coverage value. Reading EXIF, a document property, or a PNG text chunk does not mark an image or a document as semantically understood.

`UnderstandingProvider` stays the structural reader. v0.5.0 runs signature and EXIF extractors beside it. v0.5.1 adds PNG, WebP, AVIF, and PSD extractors on that same stage. `ProviderRegistry::select` is unchanged, so EXIF still wins JPEG and TIFF for that slot. The user-facing status for a file whose metadata was read is partial.

## Consequences

- New metadata readers are additive. They must not take a file away from a reader that already understands its syntax.
- The optional model stays downstream of deterministic facts, and off unless the build includes `llm` and the scan passes `--llm`.
- A parser crate is chosen only after its license, MSRV, native code, and metadata-without-rendering behavior are written down. v0.5.0 adds Tree-sitter grammars for JavaScript, TypeScript, Python, Java, and C#. v0.5.1 adds the pure-Rust `png`, `image-webp`, `avif-parse`, and `psd` crates for container metadata. It adds no Office or PDF parser, and it does not decode pixels.
- OKF export, identity, and the query's first-name match are unchanged here. They are recorded as limits in `docs/reviews/v1-baseline.md`.
