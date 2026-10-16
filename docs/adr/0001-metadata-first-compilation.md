# ADR 0001 — Metadata-first compilation

**Status:** Accepted. Implemented in v0.5.0 for signature and EXIF facts, in v0.5.1 for PNG, WebP, AVIF, and PSD container metadata, in v0.5.2 for HEIC/HEIF and video container metadata, and in v0.6.0 for Office Open XML package properties and a structural graph of DOCX, XLSX, and PPTX. PDF remains open.

## Context

Gnosis selects one provider per object. `Support::Full` beats `Partial`, which beats `Weak`. That rule is what keeps a Tree-sitter or EXIF result ahead of the optional model.

It also makes native metadata compete with syntax. `GenericMetadataProvider` is the weak fallback, and a JPEG with EXIF is marked `understood`. That status means the metadata reader succeeded. It does not mean the picture's subject was interpreted.

Later stages need Office properties, PDF Info, PNG text, and database catalogs to land as facts without displacing a structural reader, and without a model.

## Decision

Intrinsic metadata is its own stage. It runs from the bytes and the acquisition context, and it does not compete with a syntax, document, or data reader for a single winning slot.

Each extracted field keeps its source namespace (`exif:DateTimeOriginal`, `dc:creator`, and so on). A normalized projection such as a creation time is a view over those fields. Conflicts stay as separate facts.

`understood / partial / unknown / failed` remains the user-facing projection. Semantic understanding is a separate coverage value. Reading EXIF, a document property, or a PNG text chunk does not mark an image or a document as semantically understood.

`UnderstandingProvider` stays the structural reader. v0.5.0 runs signature and EXIF extractors beside it. v0.5.1 adds PNG, WebP, AVIF, and PSD extractors on that same stage. v0.5.2 adds HEIC, MOV/MP4/M4V, and WebM extractors there too. v0.6.0 adds the `opc` extractor there and `OfficeProvider` for DOCX, XLSX, and PPTX. `ProviderRegistry::select` is unchanged, so EXIF still wins JPEG and TIFF for that slot. The user-facing status for a file whose metadata or Office graph was read is partial.

## Consequences

- New metadata readers are additive. They must not take a file away from a reader that already understands its syntax.
- The optional model stays downstream of deterministic facts, and off unless the build includes `llm` and the scan passes `--llm`.
- A parser crate is chosen only after its license, MSRV, native code, and metadata-without-rendering behavior are written down. v0.5.0 adds Tree-sitter grammars for JavaScript, TypeScript, Python, Java, and C#. v0.5.1 adds the pure-Rust `png`, `image-webp`, `avif-parse`, and `psd` crates for container metadata. v0.5.2 adds `nom-exif` for HEIC/HEIF and video container metadata. v0.6.0 adds `zip` (deflate only) and `quick-xml` for Office Open XML. PDF stays open. None of these releases decodes pixels or frames, calculates a formula, or runs a macro.
- OKF export, identity, and the query's first-name match are unchanged here. They are recorded as limits in `docs/reviews/v1-baseline.md`.
