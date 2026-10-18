//! Structural graph for one catalog relation. Comments stay literal.

use std::path::Path;

use crate::providers::util::{attrs, entity, relationship};
use crate::{
    AnalysisResult, Confidence, ContentReader, Diagnostic, DiagnosticSeverity, Entity, EntityId,
    KnowledgeRecord, ObjectDescriptor, ProtoData, ProviderId, Relationship, Result, Support,
    UnderstandingProvider, UnderstandingStatus,
};

pub const CATALOG_REASON: &str = "catalog structure; semantic understanding not attempted";

pub struct CatalogProvider;

impl UnderstandingProvider for CatalogProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("catalog")
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, object: &ObjectDescriptor, proto: &ProtoData) -> Support {
        if proto.catalog.is_some() || object.path.to_string_lossy().starts_with("db://") {
            Support::Full
        } else {
            Support::None
        }
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        proto: &ProtoData,
        _content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        let Some(catalog) = proto.catalog.as_ref() else {
            return Ok(finish(
                UnderstandingStatus::Failed,
                "catalog snapshot missing",
                Vec::new(),
                Vec::new(),
                vec![error_note("catalog snapshot missing")],
            ));
        };
        let provider = self.id();
        let relation_path = object.path.clone();
        let mut entities = Vec::new();
        let mut relationships = Vec::new();
        let relation_name = format!("{}.{}", catalog.schema, catalog.relation);
        let mut relation_attrs = attrs(&[
            ("kind", &catalog.kind),
            ("schema", &catalog.schema),
            ("relation", &catalog.relation),
        ]);
        if !catalog.primary_key.is_empty() {
            relation_attrs.insert("primary_key".into(), catalog.primary_key.join(","));
        }
        if !catalog.unique_keys.is_empty() {
            let text = catalog
                .unique_keys
                .iter()
                .map(|key| key.join(","))
                .collect::<Vec<_>>()
                .join(";");
            relation_attrs.insert("unique".into(), text);
        }
        if !catalog.indexes.is_empty() {
            relation_attrs.insert("indexes".into(), catalog.indexes.join(","));
        }
        if let Some(comment) = &catalog.comment {
            relation_attrs.insert("comment".into(), comment.clone());
        }
        let relation = entity(
            "relation",
            &relation_name,
            &relation_path,
            &object.id,
            &provider,
            1,
            relation_attrs,
            Confidence::High,
        );
        let relation_id = relation.id.clone();
        entities.push(relation);

        let mut seen_names = vec![relation_name];
        for (index, column) in catalog.columns.iter().enumerate() {
            let name = unique_name(&column.name, &mut seen_names);
            let column_path = Path::new(&crate::connectors::catalog::column_locator(
                &catalog.database,
                &catalog.schema,
                &catalog.relation,
                &column.name,
            ))
            .to_path_buf();
            let mut column_attrs = attrs(&[
                ("data_type", &column.data_type),
                ("nullable", if column.nullable { "true" } else { "false" }),
                ("ordinal", &column.ordinal.to_string()),
            ]);
            if let Some(comment) = &column.comment {
                column_attrs.insert("comment".into(), comment.clone());
            }
            let column_entity = entity(
                "column",
                &name,
                &column_path,
                &object.id,
                &provider,
                (index + 2) as u32,
                column_attrs,
                Confidence::High,
            );
            let column_id = column_entity.id.clone();
            entities.push(column_entity);
            relationships.push(relationship(
                "contains",
                &relation_id,
                &column_id,
                &provider,
                &relation_path,
                (index + 2) as u32,
                Confidence::High,
            ));
        }
        for key in &catalog.foreign_keys {
            if !key.target_discovered {
                continue;
            }
            let pairs = key.columns.len().min(key.target_columns.len());
            for index in 0..pairs {
                let from_path = crate::connectors::catalog::column_locator(
                    &catalog.database,
                    &catalog.schema,
                    &catalog.relation,
                    &key.columns[index],
                );
                let to_path = crate::connectors::catalog::column_locator(
                    &catalog.database,
                    &key.target_schema,
                    &key.target_relation,
                    &key.target_columns[index],
                );
                let from = EntityId::generate("column", &key.columns[index], &from_path);
                let to = EntityId::generate("column", &key.target_columns[index], &to_path);
                relationships.push(relationship(
                    "references",
                    &from,
                    &to,
                    &provider,
                    &relation_path,
                    1,
                    Confidence::High,
                ));
            }
        }
        Ok(finish(
            UnderstandingStatus::PartiallyUnderstood,
            CATALOG_REASON,
            entities,
            relationships,
            vec![note(CATALOG_REASON)],
        ))
    }
}

fn unique_name(name: &str, seen: &mut Vec<String>) -> String {
    if !seen.iter().any(|existing| existing == name) {
        seen.push(name.to_string());
        return name.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{name} #{n}");
        if !seen.iter().any(|existing| existing == &candidate) {
            seen.push(candidate.clone());
            return candidate;
        }
        n += 1;
    }
}

fn finish(
    status: UnderstandingStatus,
    reason: &str,
    entities: Vec<Entity>,
    relationships: Vec<Relationship>,
    diagnostics: Vec<Diagnostic>,
) -> AnalysisResult {
    AnalysisResult {
        record: KnowledgeRecord {
            entities,
            relationships,
            diagnostics,
            status: Some(status),
            classification_reason: Some(reason.into()),
        },
        status,
        classification_reason: Some(reason.into()),
    }
}

fn note(message: &str) -> Diagnostic {
    Diagnostic {
        severity: DiagnosticSeverity::Info,
        message: message.into(),
    }
}

fn error_note(message: &str) -> Diagnostic {
    Diagnostic {
        severity: DiagnosticSeverity::Error,
        message: message.into(),
    }
}
