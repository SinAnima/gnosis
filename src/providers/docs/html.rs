//! Deterministic HTML extraction. No JavaScript.

use crate::providers::util::{attrs, entity, relationship};
use crate::{
    AnalysisResult, Confidence, ContentReader, Diagnostic, DiagnosticSeverity, Entity, EntityId,
    KnowledgeRecord, ObjectDescriptor, ProtoData, ProviderId, Relationship, Result, Support,
    UnderstandingProvider, UnderstandingStatus,
};
use scraper::{Html, Selector};

pub struct HtmlProvider;

impl UnderstandingProvider for HtmlProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("html")
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        let media = object.media_type.split(';').next().unwrap_or("").trim();
        if media.eq_ignore_ascii_case("text/html")
            || media.eq_ignore_ascii_case("application/xhtml+xml")
            || matches!(object.extension.as_deref(), Some("html" | "htm"))
        {
            Support::Full
        } else {
            Support::None
        }
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        proto: &ProtoData,
        content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        let mut buf = Vec::new();
        content.read_to_end(&mut buf)?;
        let source = String::from_utf8_lossy(&buf);
        let provider = self.id();
        let path = &object.relative_path;
        let document = Html::parse_document(&source);
        let mut diagnostics = Vec::new();
        if source.trim().is_empty() {
            diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                message: "empty HTML document".into(),
            });
        }

        let title = text_of(&document, "title")
            .map(|t| crate::connectors::web::sanitize_display(&t))
            .filter(|t| !t.is_empty());
        let description = meta_content(&document, "description");
        let language = document
            .root_element()
            .value()
            .attr("lang")
            .map(|s| s.to_string());
        let canonical = link_rel(&document, "canonical");
        let mut headings = Vec::new();
        for level in 1..=6 {
            let Ok(selector) = Selector::parse(&format!("h{level}")) else {
                continue;
            };
            for node in document.select(&selector) {
                let text =
                    crate::connectors::web::sanitize_display(&node.text().collect::<String>());
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if !text.is_empty() {
                    headings.push((level, text));
                }
            }
        }

        let page_name = title.clone().unwrap_or_else(|| proto.filename.clone());
        let mut page_attrs =
            attrs(&[("url", proto.final_url.as_deref().unwrap_or(&proto.filename))]);
        if let Some(title) = &title {
            page_attrs.insert("title".into(), title.clone());
        }
        if let Some(description) = description {
            page_attrs.insert("description".into(), description);
        }
        if let Some(language) = language {
            page_attrs.insert("language".into(), language);
        }
        if let Some(canonical) = &canonical {
            page_attrs.insert("canonical".into(), canonical.clone());
        }
        if let Some(status) = proto.http_status {
            page_attrs.insert("http_status".into(), status.to_string());
        }

        let page = entity(
            "document",
            &page_name,
            path,
            &object.id,
            &provider,
            1,
            page_attrs,
            Confidence::High,
        );
        let mut entities = vec![page.clone()];
        let mut relationships = Vec::new();

        for (level, text) in headings.iter().take(32) {
            let heading = entity(
                "heading",
                text,
                path,
                &object.id,
                &provider,
                1,
                attrs(&[("level", &level.to_string())]),
                Confidence::High,
            );
            relationships.push(relationship(
                "contains",
                &page.id,
                &heading.id,
                &provider,
                path,
                1,
                Confidence::High,
            ));
            entities.push(heading);
        }

        let mut seen_links = std::collections::BTreeSet::new();
        if let Ok(selector) = Selector::parse("a[href]") {
            for node in document.select(&selector) {
                let Some(href) = node.value().attr("href") else {
                    continue;
                };
                let base = proto
                    .final_url
                    .as_deref()
                    .or_else(|| object.relative_path.to_str())
                    .unwrap_or("");
                let Some(abs) = crate::connectors::web::resolve_link(base, href) else {
                    continue;
                };
                if !seen_links.insert(abs.clone()) {
                    continue;
                }
                let kind = link_kind(proto, &abs);
                push_link(
                    &mut entities,
                    &mut relationships,
                    &page.id,
                    &object.id,
                    &provider,
                    path,
                    kind,
                    &abs,
                );
            }
        }

        if let Some(canonical) = canonical {
            if let Some(abs) = crate::connectors::web::resolve_link(
                proto.final_url.as_deref().unwrap_or(""),
                &canonical,
            ) {
                push_link(
                    &mut entities,
                    &mut relationships,
                    &page.id,
                    &object.id,
                    &provider,
                    path,
                    "declares_canonical",
                    &abs,
                );
            }
        }

        if let (Some(requested), Some(final_url)) = (&proto.requested_url, &proto.final_url) {
            if requested != final_url {
                push_link(
                    &mut entities,
                    &mut relationships,
                    &page.id,
                    &object.id,
                    &provider,
                    path,
                    "redirects_to",
                    final_url,
                );
            }
        }

        if let Ok(selector) = Selector::parse("script[type='application/ld+json']") {
            let mut count = 0;
            for node in document.select(&selector) {
                if count >= 4 {
                    break;
                }
                let raw = node.text().collect::<String>();
                if raw.len() > 8_192 {
                    diagnostics.push(Diagnostic {
                        severity: DiagnosticSeverity::Info,
                        message: "JSON-LD block exceeded the size bound and was skipped".into(),
                    });
                    continue;
                }
                if serde_json::from_str::<serde_json::Value>(&raw).is_ok() {
                    let ld = entity(
                        "json_ld",
                        &format!("json-ld-{count}"),
                        path,
                        &object.id,
                        &provider,
                        1,
                        attrs(&[("json", &raw)]),
                        Confidence::Medium,
                    );
                    relationships.push(relationship(
                        "contains",
                        &page.id,
                        &ld.id,
                        &provider,
                        path,
                        1,
                        Confidence::Medium,
                    ));
                    entities.push(ld);
                    count += 1;
                }
            }
        }

        let status = if diagnostics
            .iter()
            .any(|d| matches!(d.severity, DiagnosticSeverity::Warning))
            && title.is_none()
            && headings.is_empty()
        {
            UnderstandingStatus::PartiallyUnderstood
        } else {
            UnderstandingStatus::Understood
        };
        Ok(AnalysisResult {
            record: KnowledgeRecord {
                entities,
                relationships,
                diagnostics,
                status: Some(status),
                classification_reason: Some("html structure".into()),
            },
            status,
            classification_reason: Some("html structure".into()),
        })
    }
}

fn push_link(
    entities: &mut Vec<Entity>,
    relationships: &mut Vec<Relationship>,
    page: &EntityId,
    source: &crate::ids::ObjectId,
    provider: &ProviderId,
    path: &std::path::Path,
    kind: &str,
    url: &str,
) {
    let target = entity(
        "web_resource",
        url,
        path,
        source,
        provider,
        1,
        attrs(&[("url", url), ("placeholder", "true")]),
        Confidence::High,
    );
    relationships.push(relationship(
        kind,
        page,
        &target.id,
        provider,
        path,
        1,
        Confidence::High,
    ));
    entities.push(target);
}

fn link_kind(proto: &ProtoData, target: &str) -> &'static str {
    let page = proto.final_url.as_deref().unwrap_or("");
    if is_asset(target) {
        return "references_asset";
    }
    if let (Ok(a), Ok(b)) = (url::Url::parse(page), url::Url::parse(target)) {
        if a.scheme() == b.scheme() && a.host_str() == b.host_str() {
            return "links_to";
        }
    }
    "references_external"
}

fn is_asset(url: &str) -> bool {
    let path = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
    [
        ".css", ".js", ".mjs", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".woff", ".woff2",
        ".ico",
    ]
    .iter()
    .any(|ext| path.ends_with(ext))
}

fn text_of(document: &Html, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    document.select(&selector).next().map(|node| {
        node.text()
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    })
}

fn meta_content(document: &Html, name: &str) -> Option<String> {
    let selector = Selector::parse("meta").ok()?;
    for node in document.select(&selector) {
        let matches = node
            .value()
            .attr("name")
            .or_else(|| node.value().attr("property"))
            .is_some_and(|n| {
                n.eq_ignore_ascii_case(name) || n.eq_ignore_ascii_case(&format!("og:{name}"))
            });
        if matches {
            if let Some(content) = node.value().attr("content") {
                let content = crate::connectors::web::sanitize_display(content);
                if !content.is_empty() {
                    return Some(content);
                }
            }
        }
    }
    None
}

fn link_rel(document: &Html, rel: &str) -> Option<String> {
    let selector = Selector::parse("link").ok()?;
    for node in document.select(&selector) {
        if node.value().attr("rel").is_some_and(|value| {
            value
                .split_whitespace()
                .any(|part| part.eq_ignore_ascii_case(rel))
        }) {
            return node.value().attr("href").map(|s| s.to_string());
        }
    }
    None
}
