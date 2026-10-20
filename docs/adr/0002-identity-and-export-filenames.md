# ADR 0002: Identity and export filenames

**Status:** Accepted in v0.8.0.

## Context

Two sections can share a display name. The OKF exporter used that name as the filename and cut it at 120 characters, so the files collided. The object id and the content fingerprint were already separate facts.

## Decision

The relative path remains the object locator. The content fingerprint remains the content id. An OKF entity or relationship file is named from a short readable prefix plus the full BLAKE3 hash of the stable id. The hash is 64 hex characters and is never shortened. The filename stays at most 120 characters. The frontmatter `id` is still the real entity or relationship id.

A later scan may add a `derived_from` edge when one removed object and one new object share a fingerprint. Several matches add no edge.

## Consequences

- Incremental export and stale-file removal keep using the stable id, not the display name.
- Query by name returns every match. It does not choose one.
