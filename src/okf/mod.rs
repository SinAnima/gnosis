//! OKF-style exporter for Gnosis knowledge models.
//!
//! Emits a markdown + YAML frontmatter directory bundle plus `sidecar.json`
//! behind the [`crate::Exporter`] trait.

use crate::{Exporter, KnowledgeStore, Result as GnosisResult};
use anyhow::Context;
use serde::Serialize;
use std::fs;
use std::path::Path;

pub struct OkfExporter;

impl OkfExporter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OkfExporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Exporter for OkfExporter {
    fn name(&self) -> &str {
        "okf"
    }

    fn export(&self, store: &KnowledgeStore, output: &Path) -> GnosisResult<()> {
        export_okf(store, output).map_err(|e| crate::GnosisError::Export(e.to_string()))
    }
}

#[derive(Serialize)]
struct Frontmatter {
    id: String,
    #[serde(rename = "type")]
    type_: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    related: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gnosis_kind: Option<String>,
}

fn export_okf(store: &KnowledgeStore, output: &Path) -> anyhow::Result<()> {
    if output.exists() {
        fs::remove_dir_all(output).with_context(|| format!("clear {}", output.display()))?;
    }
    fs::create_dir_all(output)?;
    fs::create_dir_all(output.join("entities"))?;
    fs::create_dir_all(output.join("objects"))?;
    fs::create_dir_all(output.join("relationships"))?;

    // Root index
    let inv = store.inventory();
    let root_body = format!(
        "# Gnosis Knowledge Bundle\n\n\
         Compiled by Gnosis enterprise knowledge compiler.\n\n\
         - Objects: {}\n\
         - Understood: {}\n\
         - Partial: {}\n\
         - Unknown: {}\n\
         - Failed: {}\n\
         - Entities: see `entities/`\n\
         - Relationships: see `relationships/`\n\n\
         ## Provenance\n\n\
         Fields that OKF cannot express natively are preserved in YAML frontmatter\n\
         (`confidence`, `provider`, `status`, `gnosis_kind`) and in `sidecar.json`.\n",
        inv.source_objects, inv.understood, inv.partial, inv.unknown, inv.failed
    );
    write_doc(
        &output.join("index.md"),
        &Frontmatter {
            id: "index".into(),
            type_: "Collection".into(),
            title: "Gnosis Knowledge Bundle".into(),
            source: store.root().map(|p| p.display().to_string()),
            confidence: None,
            provider: None,
            status: None,
            related: Vec::new(),
            gnosis_kind: Some("bundle".into()),
        },
        &root_body,
    )?;

    for entity in store.entities() {
        let related: Vec<String> = store
            .neighborhood(&entity.id, 1)
            .edges
            .iter()
            .map(|r| format!("{}:{}->{}", r.kind, r.from, r.to))
            .take(32)
            .collect();

        let evidence = entity
            .evidence
            .iter()
            .map(|e| {
                let span = e
                    .span
                    .as_ref()
                    .map(|s| format!("{}:{}", s.path.display(), s.start_line))
                    .unwrap_or_default();
                format!("- {} ({span}) [{}]", e.summary, e.provider)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let attrs = entity
            .attributes
            .iter()
            .map(|(k, v)| format!("- `{k}`: {v}"))
            .collect::<Vec<_>>()
            .join("\n");

        let body = format!(
            "# {}\n\n\
             Kind: `{}`\n\n\
             ## Attributes\n\n{}\n\n\
             ## Evidence\n\n{}\n",
            entity.name,
            entity.kind,
            if attrs.is_empty() {
                "- (none)".into()
            } else {
                attrs
            },
            if evidence.is_empty() {
                "- (none)".into()
            } else {
                evidence
            }
        );

        let filename = sanitize(&format!("{}_{}.md", entity.kind, entity.name));
        let provider = entity.evidence.first().map(|e| e.provider.to_string());
        write_doc(
            &output.join("entities").join(&filename),
            &Frontmatter {
                id: entity.id.as_str().into(),
                type_: "Concept".into(),
                title: entity.name.clone(),
                source: Some(entity.source_object.to_string()),
                confidence: Some(entity.confidence.as_str().into()),
                provider,
                status: None,
                related,
                gnosis_kind: Some(entity.kind.clone()),
            },
            &body,
        )?;
    }

    for obj in store.objects() {
        let path = obj.descriptor.relative_path.display().to_string();
        let body = format!(
            "# {}\n\n\
             - Path: `{}`\n\
             - Media type: `{}`\n\
             - Status: `{}`\n\
             - Reason: {}\n\
             - Provider: {}\n\
             - Entities: {}\n",
            path,
            path,
            obj.descriptor.media_type,
            obj.status,
            obj.classification_reason.as_deref().unwrap_or("(none)"),
            obj.provider
                .as_ref()
                .map(|p| p.to_string())
                .unwrap_or_else(|| "(none)".into()),
            obj.entity_ids.len()
        );
        let filename = sanitize(&format!("{path}.md"));
        write_doc(
            &output.join("objects").join(&filename),
            &Frontmatter {
                id: obj.descriptor.id.as_str().into(),
                type_: "SourceObject".into(),
                title: path,
                source: Some(obj.descriptor.path.display().to_string()),
                confidence: None,
                provider: obj.provider.as_ref().map(|p| p.to_string()),
                status: Some(obj.status.as_str().into()),
                related: obj.entity_ids.iter().map(|e| e.to_string()).collect(),
                gnosis_kind: Some("source_object".into()),
            },
            &body,
        )?;
    }

    for rel in store.relationships() {
        let body = format!(
            "# {}\n\n\
             - From: `{}`\n\
             - To: `{}`\n\
             - Kind: `{}`\n\
             {}\n",
            rel.kind,
            rel.from,
            rel.to,
            rel.kind,
            if rel.attributes.is_empty() {
                String::new()
            } else {
                let attrs = rel
                    .attributes
                    .iter()
                    .map(|(k, v)| format!("- `{k}`: {v}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("\n## Attributes\n\n{attrs}\n")
            }
        );
        let filename = sanitize(&format!("{}.md", rel.id.as_str()));
        let provider = rel.evidence.first().map(|e| e.provider.to_string());
        write_doc(
            &output.join("relationships").join(&filename),
            &Frontmatter {
                id: rel.id.as_str().into(),
                type_: "Relationship".into(),
                title: format!("{} {} {}", rel.from, rel.kind, rel.to),
                source: None,
                confidence: Some(rel.confidence.as_str().into()),
                provider,
                status: None,
                related: vec![rel.from.to_string(), rel.to.to_string()],
                gnosis_kind: Some(rel.kind.clone()),
            },
            &body,
        )?;
    }

    // Sidecar for fields OKF may not represent natively.
    let sidecar = serde_json::json!({
        "format": "gnosis-okf-sidecar",
        "version": 1,
        "inventory": {
            "objects": inv.source_objects,
            "understood": inv.understood,
            "partial": inv.partial,
            "unknown": inv.unknown,
            "failed": inv.failed,
            "relationships": inv.relationships,
            "modules": inv.modules,
            "types": inv.types,
            "functions": inv.functions,
            "documents": inv.documents,
            "datasets": inv.datasets,
        },
        "providers": store.enabled_providers(),
        "note": "Confidence, provider provenance, and understanding status are also embedded in document frontmatter."
    });
    fs::write(
        output.join("sidecar.json"),
        serde_json::to_string_pretty(&sidecar)?,
    )?;

    // Simple log
    fs::write(
        output.join("log.md"),
        format!(
            "# Update log\n\n- Exported by Gnosis from `{}`\n",
            store
                .root()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| ".".into())
        ),
    )?;

    Ok(())
}

fn write_doc(path: &Path, fm: &Frontmatter, body: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let yaml = serde_yaml::to_string(fm)?;
    let content = format!("---\n{yaml}---\n\n{body}");
    fs::write(path, content)?;
    Ok(())
}

fn sanitize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii() && (c.is_alphanumeric() || c == '-' || c == '_' || c == '.') {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.len() > 120 {
        out.truncate(120);
    }
    if out.is_empty() {
        "unnamed.md".into()
    } else if !out.ends_with(".md") {
        format!("{out}.md")
    } else {
        out
    }
}
