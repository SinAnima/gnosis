//! Parse model JSON into [`AnalysisResult`].

use crate::connectors::types::ObjectDescriptor;
use crate::error::{GnosisError, Result};
use crate::ids::{EntityId, ProviderId, RelationshipId};
use crate::knowledge::{
    AnalysisResult, AttributeMap, Confidence, Diagnostic, DiagnosticSeverity, Entity, Evidence,
    KnowledgeRecord, Relationship,
};
use crate::status::UnderstandingStatus;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
struct LlmPayload {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    entities: Vec<LlmEntity>,
    #[serde(default)]
    relationships: Vec<LlmRel>,
    #[serde(default)]
    diagnostics: Vec<LlmDiag>,
}

#[derive(Debug, Deserialize)]
struct LlmEntity {
    /// Models often emit `type` instead of `kind`.
    #[serde(alias = "type")]
    kind: String,
    name: String,
    #[serde(default)]
    attributes: BTreeMap<String, Value>,
    #[serde(default, deserialize_with = "flexible_string")]
    confidence: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LlmRel {
    kind: String,
    from: String,
    to: String,
    #[serde(default)]
    confidence: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LlmDiag {
    #[serde(default)]
    severity: Option<String>,
    message: String,
}

/// Strip optional ```json fences some models still emit.
pub fn strip_json_fence(raw: &str) -> &str {
    let trimmed = raw.trim();
    let trimmed = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```JSON"))
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix("```").unwrap_or(trimmed);
    trimmed.trim()
}

/// Provenance the model is not allowed to override.
#[derive(Clone, Debug)]
pub struct LlmStamp {
    pub model: String,
    pub prompt_kind: String,
}

pub fn parse_llm_json(
    raw: &str,
    object: &ObjectDescriptor,
    provider: &ProviderId,
    stamp: &LlmStamp,
) -> Result<AnalysisResult> {
    let body = strip_json_fence(raw);
    let payload: LlmPayload = serde_json::from_str(body).map_err(|e| {
        GnosisError::provider(
            "llm-semantic",
            format!("invalid JSON from model: {e}; body={}", truncate(body, 240)),
        )
    })?;

    let path_str = object.relative_path.to_string_lossy();
    let mut name_to_id: BTreeMap<String, EntityId> = BTreeMap::new();
    let mut entities = Vec::new();

    for ent in payload.entities {
        let confidence = parse_confidence(ent.confidence.as_deref());
        let attributes = flatten_attrs(&ent.attributes);
        let name = ent.name;
        let kind = ent.kind;
        let id = EntityId::generate(&kind, &name, &path_str);
        name_to_id.insert(name.clone(), id.clone());
        entities.push(Entity {
            id,
            kind,
            name: name.clone(),
            attributes,
            evidence: vec![Evidence {
                summary: format!("llm entity {name}"),
                span: None,
                provider: provider.clone(),
            }],
            confidence,
            source_object: object.id.clone(),
        });
    }

    if entities.is_empty() {
        if let Some(summary) = payload.summary.clone() {
            let name = object
                .relative_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("object")
                .to_string();
            let id = EntityId::generate("summary", &name, &path_str);
            name_to_id.insert(name.clone(), id.clone());
            let mut attributes = AttributeMap::new();
            attributes.insert("summary".into(), summary);
            entities.push(Entity {
                id,
                kind: "summary".into(),
                name,
                attributes,
                evidence: vec![Evidence {
                    summary: "llm summary".into(),
                    span: None,
                    provider: provider.clone(),
                }],
                confidence: Confidence::Inferred,
                source_object: object.id.clone(),
            });
        }
    }

    let mut relationships = Vec::new();
    for rel in payload.relationships {
        let Some(from) = name_to_id.get(&rel.from) else {
            continue;
        };
        let Some(to) = name_to_id.get(&rel.to) else {
            continue;
        };
        relationships.push(Relationship {
            id: RelationshipId::generate(&rel.kind, from.as_str(), to.as_str()),
            kind: rel.kind,
            from: from.clone(),
            to: to.clone(),
            attributes: AttributeMap::new(),
            evidence: vec![Evidence {
                summary: format!("llm {}", rel.from),
                span: None,
                provider: provider.clone(),
            }],
            confidence: parse_confidence(rel.confidence.as_deref()),
        });
    }

    let diagnostics = payload
        .diagnostics
        .into_iter()
        .map(|d| Diagnostic {
            severity: parse_severity(d.severity.as_deref()),
            message: d.message,
        })
        .collect();

    let status = parse_status(payload.status.as_deref(), !entities.is_empty());
    let reason = payload
        .summary
        .unwrap_or_else(|| "llm-semantic extraction".into());

    let mut result = AnalysisResult {
        record: KnowledgeRecord {
            entities,
            relationships,
            diagnostics,
            status: Some(status),
            classification_reason: Some(reason.clone()),
        },
        status,
        classification_reason: Some(reason),
    };
    apply_stamp(&mut result, stamp);
    Ok(result)
}

fn apply_stamp(result: &mut AnalysisResult, stamp: &LlmStamp) {
    let pairs = [
        ("origin", "llm".to_string()),
        ("model", stamp.model.clone()),
        ("prompt_kind", stamp.prompt_kind.clone()),
        (
            "prompt_version",
            crate::prompts::PROMPT_PACK_VERSION.to_string(),
        ),
        ("review", "pending".to_string()),
    ];
    for entity in &mut result.record.entities {
        for (k, v) in &pairs {
            entity.attributes.insert((*k).to_string(), v.clone());
        }
    }
    for rel in &mut result.record.relationships {
        for (k, v) in &pairs {
            rel.attributes.insert((*k).to_string(), v.clone());
        }
    }
}

fn flatten_attrs(src: &BTreeMap<String, Value>) -> AttributeMap {
    let mut out = AttributeMap::new();
    for (k, v) in src {
        let s = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        out.insert(k.clone(), s);
    }
    out
}

fn flexible_string<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(match value {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(other) => Some(other.to_string()),
    })
}

fn parse_confidence(raw: Option<&str>) -> Confidence {
    match raw.map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("high") => Confidence::High,
        Some("medium") => Confidence::Medium,
        Some("low") => Confidence::Low,
        _ => Confidence::Inferred,
    }
}

fn parse_severity(raw: Option<&str>) -> DiagnosticSeverity {
    match raw.map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("error") => DiagnosticSeverity::Error,
        Some("warning") => DiagnosticSeverity::Warning,
        _ => DiagnosticSeverity::Info,
    }
}

fn parse_status(raw: Option<&str>, has_entities: bool) -> UnderstandingStatus {
    match raw.map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("understood") => UnderstandingStatus::Understood,
        Some("unknown") => UnderstandingStatus::Unknown,
        Some("partial") | Some("partially_understood") | Some("partiallyunderstood") => {
            UnderstandingStatus::PartiallyUnderstood
        }
        _ if has_entities => UnderstandingStatus::PartiallyUnderstood,
        _ => UnderstandingStatus::Unknown,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ObjectId;
    use std::path::PathBuf;

    fn object() -> ObjectDescriptor {
        ObjectDescriptor {
            id: ObjectId::new("obj:x.rs"),
            path: PathBuf::from("x.rs"),
            relative_path: PathBuf::from("x.rs"),
            is_dir: false,
            size: 1,
            modified: None,
            media_type: "text/x-rust".into(),
            extension: Some("rs".into()),
            etag: None,
        }
    }

    #[test]
    fn parses_entities_and_relationships() {
        let raw = r#"{
          "summary": "auth module",
          "status": "understood",
          "entities": [
            {"kind": "module", "name": "auth", "attributes": {"role": "security", "origin": "human", "review": "accepted"}, "confidence": "medium"},
            {"kind": "concept", "name": "session", "confidence": "inferred"}
          ],
          "relationships": [
            {"kind": "manages", "from": "auth", "to": "session", "confidence": "low"}
          ],
          "diagnostics": [{"severity": "info", "message": "ok"}]
        }"#;
        let provider = ProviderId::new("llm-semantic");
        let stamp = LlmStamp {
            model: "gemma:2b".into(),
            prompt_kind: "code".into(),
        };
        let result = parse_llm_json(raw, &object(), &provider, &stamp).unwrap();
        assert_eq!(result.status, UnderstandingStatus::Understood);
        assert_eq!(result.record.entities.len(), 2);
        assert_eq!(result.record.relationships.len(), 1);
        assert_eq!(result.record.diagnostics.len(), 1);
        assert_eq!(
            result.record.entities[0]
                .attributes
                .get("role")
                .map(String::as_str),
            Some("security")
        );
        assert_eq!(
            result.record.entities[0]
                .attributes
                .get("origin")
                .map(String::as_str),
            Some("llm")
        );
        assert_eq!(
            result.record.entities[0]
                .attributes
                .get("review")
                .map(String::as_str),
            Some("pending")
        );
        assert_eq!(
            result.record.entities[0]
                .attributes
                .get("model")
                .map(String::as_str),
            Some("gemma:2b")
        );
        assert_eq!(
            result.record.entities[0]
                .attributes
                .get("prompt_version")
                .map(String::as_str),
            Some(crate::prompts::PROMPT_PACK_VERSION)
        );
        assert_eq!(
            result.record.relationships[0]
                .attributes
                .get("origin")
                .map(String::as_str),
            Some("llm")
        );
    }

    #[test]
    fn strips_fence_and_summary_fallback() {
        let raw = "```json\n{\"summary\":\"hello\",\"status\":\"partial\",\"entities\":[]}\n```";
        let provider = ProviderId::new("llm-semantic");
        let stamp = LlmStamp {
            model: "gemma:2b".into(),
            prompt_kind: "docs".into(),
        };
        let result = parse_llm_json(raw, &object(), &provider, &stamp).unwrap();
        assert_eq!(result.status, UnderstandingStatus::PartiallyUnderstood);
        assert_eq!(result.record.entities.len(), 1);
        assert_eq!(result.record.entities[0].kind, "summary");
        assert_eq!(
            result.record.entities[0]
                .attributes
                .get("review")
                .map(String::as_str),
            Some("pending")
        );
    }

    #[test]
    fn fixture_reply_is_stamped() {
        let raw = include_str!("../../../fixtures/llm/auth_module.json");
        let provider = ProviderId::new("llm-semantic");
        let stamp = LlmStamp {
            model: "qwen3-coder-next".into(),
            prompt_kind: "code".into(),
        };
        let result = parse_llm_json(raw, &object(), &provider, &stamp).unwrap();
        assert_eq!(result.record.entities.len(), 2);
        assert!(result.record.entities.iter().all(|e| e
            .attributes
            .get("origin")
            .map(String::as_str)
            == Some("llm")
            && e.attributes.get("review").map(String::as_str) == Some("pending")));
    }
}
