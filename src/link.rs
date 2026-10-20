//! One pass that turns path-shaped references into exact, unresolved, or ambiguous edges.
//!
//! The pass does not recurse, so a cycle between two files is two edges and then it stops.
//! A missing target stays on the edge. It is not a guess.

use crate::ids::{EntityId, ObjectId, ProviderId, RelationshipId};
use crate::knowledge::{Confidence, Entity, Evidence, Relationship, SourceSpan};
use crate::knowledge_db::dep_matches;
use crate::store::KnowledgeStore;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const CONTAINS: &str = "contains";
pub const DEFINES: &str = "defines";
pub const IMPORTS: &str = "imports";
pub const INCLUDES: &str = "includes";
pub const REFERENCES: &str = "references";
pub const HYPERLINKS_TO: &str = "hyperlinks_to";
pub const HAS_PROPERTY: &str = "has_property";
pub const DERIVED_FROM: &str = "derived_from";
pub const FOREIGN_KEY_TO: &str = "foreign_key_to";
pub const DECLARES_FORMULA: &str = "declares_formula";
pub const FORMULA_REFERENCES: &str = "formula_references";
pub const BELONGS_TO: &str = "belongs_to";

/// Canonical kinds. `defines` and `belongs_to` are named here and unused by this pass.
pub const VOCABULARY: &[&str] = &[
    CONTAINS,
    DEFINES,
    IMPORTS,
    INCLUDES,
    REFERENCES,
    HYPERLINKS_TO,
    HAS_PROPERTY,
    DERIVED_FROM,
    FOREIGN_KEY_TO,
    DECLARES_FORMULA,
    FORMULA_REFERENCES,
    BELONGS_TO,
];

pub const RESOLUTION_EXACT: &str = "exact";
pub const RESOLUTION_UNRESOLVED: &str = "unresolved";
pub const RESOLUTION_AMBIGUOUS: &str = "ambiguous";

struct Draft {
    entities: Vec<Entity>,
    rels: Vec<Relationship>,
    dirty: bool,
}

/// Link the store in place. Returns the object ids whose graphs changed.
pub fn link_store(store: &mut KnowledgeStore) -> Vec<ObjectId> {
    let ids = store.object_ids();
    let mut drafts: HashMap<ObjectId, Draft> = HashMap::new();
    let mut paths: Vec<(String, ObjectId)> = Vec::new();
    for id in &ids {
        let Some(object) = store.get_object(id) else {
            continue;
        };
        let path = object
            .descriptor
            .relative_path
            .to_string_lossy()
            .replace('\\', "/");
        paths.push((path, id.clone()));
        drafts.insert(
            id.clone(),
            Draft {
                entities: store.entities_of(id),
                rels: store.relationships_of(id),
                dirty: false,
            },
        );
    }
    paths.sort_by(|a, b| a.0.cmp(&b.0));

    for draft in drafts.values_mut() {
        mark_placeholders(draft);
    }

    let roots = roots_of(&drafts);
    for (id, draft) in drafts.iter_mut() {
        let provider = store
            .get_object(id)
            .and_then(|object| object.provider.clone())
            .unwrap_or_else(|| ProviderId::new("link"));
        let path = store
            .get_object(id)
            .map(|object| {
                object
                    .descriptor
                    .relative_path
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .unwrap_or_default();
        link_object(draft, &path, &provider, &paths, &roots);
        if let Some(object) = store.get_object(id) {
            if let Some(catalog) = object.proto.catalog.clone() {
                link_missing_foreign_keys(draft, &path, &provider, &catalog);
            }
        }
    }

    let removed = store.take_removed_roots();
    link_renames(&mut drafts, &roots, &removed, store);

    drop_stale_edges(&mut drafts);

    let mut changed = Vec::new();
    for (id, draft) in drafts {
        if !draft.dirty {
            continue;
        }
        store.replace_object_graph(&id, draft.entities, draft.rels);
        changed.push(id);
    }
    changed.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    changed
}

fn mark_placeholders(draft: &mut Draft) {
    let mut stand_ins = BTreeSet::new();
    for rel in &draft.rels {
        if rel.kind == "links_to" || rel.kind == IMPORTS || rel.kind == INCLUDES {
            stand_ins.insert(rel.to.clone());
        }
    }
    for entity in &mut draft.entities {
        if !stand_ins.contains(&entity.id) {
            continue;
        }
        if entity.kind == "link"
            || entity.kind == "web_resource"
            || entity.kind == "module"
            || entity.attributes.contains_key("import")
            || entity.attributes.contains_key("include")
            || entity.attributes.contains_key("href")
        {
            set_placeholder(entity, &mut draft.dirty);
        }
    }
}

fn set_placeholder(entity: &mut Entity, dirty: &mut bool) {
    if entity.attributes.get("placeholder").map(String::as_str) == Some("true") {
        return;
    }
    entity
        .attributes
        .insert("placeholder".into(), "true".into());
    *dirty = true;
}

fn roots_of(drafts: &HashMap<ObjectId, Draft>) -> HashMap<ObjectId, EntityId> {
    let mut roots = HashMap::new();
    for (id, draft) in drafts {
        if let Some(root) = draft_root(draft) {
            roots.insert(id.clone(), root);
        }
    }
    roots
}

fn draft_root(draft: &Draft) -> Option<EntityId> {
    if let Some(entity) = draft
        .entities
        .iter()
        .find(|entity| is_root_kind(&entity.kind) && !is_placeholder(entity))
    {
        return Some(entity.id.clone());
    }
    draft
        .entities
        .iter()
        .find(|entity| !is_placeholder(entity))
        .map(|entity| entity.id.clone())
}

fn is_placeholder(entity: &Entity) -> bool {
    entity.attributes.get("placeholder").map(String::as_str) == Some("true")
}

fn is_root_kind(kind: &str) -> bool {
    matches!(
        kind,
        "document" | "workbook" | "presentation" | "relation" | "module"
    )
}

fn link_object(
    draft: &mut Draft,
    path: &str,
    provider: &ProviderId,
    paths: &[(String, ObjectId)],
    roots: &HashMap<ObjectId, EntityId>,
) {
    let rels = draft.rels.clone();
    for rel in &rels {
        if rel.kind == "links_to" {
            link_href(draft, rel, path, provider, paths, roots);
        } else if rel.kind == IMPORTS || rel.kind == INCLUDES {
            link_import(draft, rel, path, provider, paths, roots);
        } else if rel.kind == CONTAINS {
            link_formula_declaration(draft, rel, path, provider);
        } else if rel.kind == REFERENCES {
            link_formula_reference(draft, rel, path, provider);
            link_discovered_foreign_key(draft, rel, path, provider);
        }
    }
    link_hyperlink_attribute(draft, path, provider, paths, roots);
}

fn link_href(
    draft: &mut Draft,
    rel: &Relationship,
    path: &str,
    provider: &ProviderId,
    paths: &[(String, ObjectId)],
    roots: &HashMap<ObjectId, EntityId>,
) {
    let Some(target_entity) = draft.entities.iter().find(|entity| entity.id == rel.to) else {
        return;
    };
    let written = target_entity
        .attributes
        .get("href")
        .or_else(|| target_entity.attributes.get("url"))
        .cloned()
        .unwrap_or_else(|| target_entity.name.clone());
    let from = rel.from.clone();
    let placeholder = rel.to.clone();
    resolve_hyperlink(
        draft,
        &from,
        &placeholder,
        &written,
        &LinkCtx {
            path,
            provider,
            paths,
            roots,
        },
    );
}

fn link_hyperlink_attribute(
    draft: &mut Draft,
    path: &str,
    provider: &ProviderId,
    paths: &[(String, ObjectId)],
    roots: &HashMap<ObjectId, EntityId>,
) {
    let written: Vec<(EntityId, String)> = draft
        .entities
        .iter()
        .filter_map(|entity| {
            entity
                .attributes
                .get("hyperlinks")
                .map(|value| (entity.id.clone(), value.clone()))
        })
        .collect();
    for (from, block) in written {
        for line in block.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let placeholder = placeholder_link(draft, path, provider, line);
            resolve_hyperlink(
                draft,
                &from,
                &placeholder,
                line,
                &LinkCtx {
                    path,
                    provider,
                    paths,
                    roots,
                },
            );
        }
    }
}

fn placeholder_link(
    draft: &mut Draft,
    path: &str,
    provider: &ProviderId,
    written: &str,
) -> EntityId {
    let id = EntityId::generate("link", written, path);
    if draft.entities.iter().any(|entity| entity.id == id) {
        return id;
    }
    let mut entity = Entity {
        id: id.clone(),
        kind: "link".into(),
        name: written.to_string(),
        attributes: BTreeMap::from([
            ("href".into(), written.to_string()),
            ("placeholder".into(), "true".into()),
        ]),
        evidence: vec![evidence(provider, "hyperlink target", path)],
        confidence: Confidence::Low,
        source_object: draft
            .entities
            .first()
            .map(|entity| entity.source_object.clone())
            .unwrap_or_else(|| ObjectId::new("obj:")),
    };
    if let Some(source) = draft
        .entities
        .first()
        .map(|item| item.source_object.clone())
    {
        entity.source_object = source;
    }
    draft.entities.push(entity);
    draft.dirty = true;
    id
}

struct LinkCtx<'a> {
    path: &'a str,
    provider: &'a ProviderId,
    paths: &'a [(String, ObjectId)],
    roots: &'a HashMap<ObjectId, EntityId>,
}

fn resolve_hyperlink(
    draft: &mut Draft,
    from: &EntityId,
    placeholder: &EntityId,
    written: &str,
    ctx: &LinkCtx<'_>,
) {
    if written.contains("://") {
        put_edge(
            draft,
            &EdgeSpec {
                kind: HYPERLINKS_TO,
                from,
                to: placeholder,
                resolution: RESOLUTION_UNRESOLVED,
                target: Some(written),
                candidates: None,
                confidence: Confidence::Low,
                path: ctx.path,
                provider: ctx.provider,
                summary: "unresolved hyperlink",
            },
        );
        return;
    }
    let dep = href_path(written);
    let hits = path_hits(&dep, ctx.paths);
    match hits.len() {
        1 => {
            let Some(root) = ctx.roots.get(&hits[0].1) else {
                put_edge(
                    draft,
                    &EdgeSpec {
                        kind: HYPERLINKS_TO,
                        from,
                        to: placeholder,
                        resolution: RESOLUTION_UNRESOLVED,
                        target: Some(written),
                        candidates: None,
                        confidence: Confidence::Low,
                        path: ctx.path,
                        provider: ctx.provider,
                        summary: "unresolved hyperlink",
                    },
                );
                return;
            };
            put_edge(
                draft,
                &EdgeSpec {
                    kind: HYPERLINKS_TO,
                    from,
                    to: root,
                    resolution: RESOLUTION_EXACT,
                    target: None,
                    candidates: None,
                    confidence: Confidence::High,
                    path: ctx.path,
                    provider: ctx.provider,
                    summary: "exact path match",
                },
            );
        }
        0 => put_edge(
            draft,
            &EdgeSpec {
                kind: HYPERLINKS_TO,
                from,
                to: placeholder,
                resolution: RESOLUTION_UNRESOLVED,
                target: Some(written),
                candidates: None,
                confidence: Confidence::Low,
                path: ctx.path,
                provider: ctx.provider,
                summary: "unresolved hyperlink",
            },
        ),
        _ => {
            let candidates = hits
                .iter()
                .map(|(hit_path, _)| hit_path.clone())
                .collect::<Vec<_>>()
                .join("\n");
            put_edge(
                draft,
                &EdgeSpec {
                    kind: HYPERLINKS_TO,
                    from,
                    to: placeholder,
                    resolution: RESOLUTION_AMBIGUOUS,
                    target: Some(written),
                    candidates: Some(&candidates),
                    confidence: Confidence::Low,
                    path: ctx.path,
                    provider: ctx.provider,
                    summary: "ambiguous path match",
                },
            );
        }
    }
}

fn link_import(
    draft: &mut Draft,
    rel: &Relationship,
    path: &str,
    provider: &ProviderId,
    paths: &[(String, ObjectId)],
    roots: &HashMap<ObjectId, EntityId>,
) {
    let Some(index) = draft.rels.iter().position(|item| item.id == rel.id) else {
        return;
    };
    let dep = draft
        .entities
        .iter()
        .find(|entity| entity.id == rel.to)
        .map(|entity| {
            entity
                .attributes
                .get("import")
                .or_else(|| entity.attributes.get("include"))
                .cloned()
                .unwrap_or_else(|| entity.name.clone())
        })
        .unwrap_or_default();
    let hits = path_hits(&dep, paths);
    match hits.len() {
        1 => {
            let Some(root) = roots.get(&hits[0].1).cloned() else {
                set_resolution(
                    draft,
                    index,
                    RESOLUTION_UNRESOLVED,
                    Some(&dep),
                    None,
                    Confidence::Low,
                );
                return;
            };
            retarget(draft, index, &root, path, provider);
        }
        0 => set_resolution(
            draft,
            index,
            RESOLUTION_UNRESOLVED,
            Some(&dep),
            None,
            Confidence::Low,
        ),
        _ => {
            let candidates = hits
                .iter()
                .map(|(hit_path, _)| hit_path.clone())
                .collect::<Vec<_>>()
                .join("\n");
            set_resolution(
                draft,
                index,
                RESOLUTION_AMBIGUOUS,
                Some(&dep),
                Some(&candidates),
                Confidence::Low,
            );
        }
    }
}

fn retarget(draft: &mut Draft, index: usize, root: &EntityId, path: &str, provider: &ProviderId) {
    let current = &draft.rels[index];
    if &current.to == root
        && current.attributes.get("resolution").map(String::as_str) == Some(RESOLUTION_EXACT)
    {
        return;
    }
    let kind = current.kind.clone();
    let from = current.from.clone();
    let id = RelationshipId::generate(&kind, from.as_str(), root.as_str());
    let mut rel = current.clone();
    rel.id = id;
    rel.to = root.clone();
    rel.confidence = Confidence::High;
    rel.attributes
        .insert("resolution".into(), RESOLUTION_EXACT.into());
    rel.attributes.remove("target");
    rel.attributes.remove("candidates");
    if rel.evidence.is_empty() {
        rel.evidence = vec![evidence(provider, "exact path match", path)];
    }
    draft.rels[index] = rel;
    draft.dirty = true;
}

fn set_resolution(
    draft: &mut Draft,
    index: usize,
    resolution: &str,
    target: Option<&str>,
    candidates: Option<&str>,
    confidence: Confidence,
) {
    let rel = &mut draft.rels[index];
    let mut changed = false;
    if rel.attributes.get("resolution").map(String::as_str) != Some(resolution) {
        rel.attributes
            .insert("resolution".into(), resolution.to_string());
        changed = true;
    }
    if let Some(target) = target {
        if rel.attributes.get("target").map(String::as_str) != Some(target) {
            rel.attributes.insert("target".into(), target.to_string());
            changed = true;
        }
    }
    if let Some(candidates) = candidates {
        if rel.attributes.get("candidates").map(String::as_str) != Some(candidates) {
            rel.attributes
                .insert("candidates".into(), candidates.to_string());
            changed = true;
        }
    }
    if rel.confidence != confidence {
        rel.confidence = confidence;
        changed = true;
    }
    if changed {
        draft.dirty = true;
    }
}

fn link_formula_declaration(
    draft: &mut Draft,
    rel: &Relationship,
    path: &str,
    provider: &ProviderId,
) {
    let Some(target) = draft.entities.iter().find(|entity| entity.id == rel.to) else {
        return;
    };
    if target.kind != "formula" {
        return;
    }
    put_edge(
        draft,
        &EdgeSpec {
            kind: DECLARES_FORMULA,
            from: &rel.from,
            to: &rel.to,
            resolution: RESOLUTION_EXACT,
            target: None,
            candidates: None,
            confidence: Confidence::High,
            path,
            provider,
            summary: "formula declared on worksheet",
        },
    );
}

fn link_formula_reference(
    draft: &mut Draft,
    rel: &Relationship,
    path: &str,
    provider: &ProviderId,
) {
    let Some(source) = draft.entities.iter().find(|entity| entity.id == rel.from) else {
        return;
    };
    if source.kind != "formula" {
        return;
    }
    let external = source.attributes.get("external").map(String::as_str) == Some("true");
    if external {
        let target = draft
            .entities
            .iter()
            .find(|entity| entity.id == rel.to)
            .map(|entity| entity.name.clone());
        put_edge(
            draft,
            &EdgeSpec {
                kind: FORMULA_REFERENCES,
                from: &rel.from,
                to: &rel.to,
                resolution: RESOLUTION_UNRESOLVED,
                target: target.as_deref(),
                candidates: None,
                confidence: Confidence::Low,
                path,
                provider,
                summary: "external formula reference",
            },
        );
    } else {
        put_edge(
            draft,
            &EdgeSpec {
                kind: FORMULA_REFERENCES,
                from: &rel.from,
                to: &rel.to,
                resolution: RESOLUTION_EXACT,
                target: None,
                candidates: None,
                confidence: Confidence::High,
                path,
                provider,
                summary: "in-workbook formula reference",
            },
        );
    }
}

fn link_discovered_foreign_key(
    draft: &mut Draft,
    rel: &Relationship,
    path: &str,
    provider: &ProviderId,
) {
    let from_column = draft
        .entities
        .iter()
        .any(|entity| entity.id == rel.from && entity.kind == "column");
    if !from_column {
        return;
    }
    put_edge(
        draft,
        &EdgeSpec {
            kind: FOREIGN_KEY_TO,
            from: &rel.from,
            to: &rel.to,
            resolution: RESOLUTION_EXACT,
            target: None,
            candidates: None,
            confidence: Confidence::High,
            path,
            provider,
            summary: "foreign key to a discovered column",
        },
    );
}

fn link_missing_foreign_keys(
    draft: &mut Draft,
    path: &str,
    provider: &ProviderId,
    catalog: &crate::connectors::types::CatalogProtoData,
) {
    for key in &catalog.foreign_keys {
        if key.target_discovered {
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
            let target = format!("{}.{}", key.target_schema, key.target_relation);
            put_edge(
                draft,
                &EdgeSpec {
                    kind: FOREIGN_KEY_TO,
                    from: &from,
                    to: &to,
                    resolution: RESOLUTION_UNRESOLVED,
                    target: Some(&target),
                    candidates: None,
                    confidence: Confidence::Low,
                    path,
                    provider,
                    summary: "foreign key target was not discovered",
                },
            );
        }
    }
}

fn link_renames(
    drafts: &mut HashMap<ObjectId, Draft>,
    roots: &HashMap<ObjectId, EntityId>,
    removed: &[crate::store::RemovedRoot],
    store: &KnowledgeStore,
) {
    let mut removed_by_fp: BTreeMap<String, Vec<&crate::store::RemovedRoot>> = BTreeMap::new();
    for item in removed {
        if let Some(fingerprint) = &item.fingerprint {
            removed_by_fp
                .entry(fingerprint.clone())
                .or_default()
                .push(item);
        }
    }
    let mut current_by_fp: BTreeMap<String, Vec<ObjectId>> = BTreeMap::new();
    for id in drafts.keys() {
        if let Some(fingerprint) = store
            .get_object(id)
            .and_then(|object| object.proto.fingerprint.clone())
        {
            current_by_fp
                .entry(fingerprint)
                .or_default()
                .push(id.clone());
        }
    }
    for (fingerprint, removed_items) in &removed_by_fp {
        let Some(current) = current_by_fp.get(fingerprint) else {
            continue;
        };
        if removed_items.len() != 1 || current.len() != 1 {
            continue;
        }
        let old = removed_items[0];
        let new_id = &current[0];
        let Some(root) = roots.get(new_id) else {
            continue;
        };
        let Some(draft) = drafts.get_mut(new_id) else {
            continue;
        };
        let path = store
            .get_object(new_id)
            .map(|object| {
                object
                    .descriptor
                    .relative_path
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .unwrap_or_default();
        let provider = store
            .get_object(new_id)
            .and_then(|object| object.provider.clone())
            .unwrap_or_else(|| ProviderId::new("link"));
        let id = RelationshipId::generate(DERIVED_FROM, root.as_str(), old.root.as_str());
        if draft.rels.iter().any(|rel| rel.id == id) {
            continue;
        }
        let mut rel = Relationship {
            id,
            kind: DERIVED_FROM.into(),
            from: root.clone(),
            to: old.root.clone(),
            attributes: BTreeMap::from([
                ("resolution".into(), RESOLUTION_EXACT.into()),
                ("previous_path".into(), old.path.clone()),
            ]),
            evidence: vec![evidence(&provider, "same content fingerprint", &path)],
            confidence: Confidence::High,
        };
        rel.attributes
            .insert("resolution".into(), RESOLUTION_EXACT.into());
        draft.rels.push(rel);
        draft.dirty = true;
    }
}

fn drop_stale_edges(drafts: &mut HashMap<ObjectId, Draft>) {
    let mut live = BTreeSet::new();
    for draft in drafts.values() {
        for entity in &draft.entities {
            live.insert(entity.id.clone());
        }
    }
    for draft in drafts.values_mut() {
        let before = draft.rels.len();
        draft.rels.retain(|rel| edge_is_kept(rel, &live));
        if draft.rels.len() != before {
            draft.dirty = true;
        }
    }
}

fn edge_is_kept(rel: &Relationship, live: &BTreeSet<EntityId>) -> bool {
    if live.contains(&rel.from) && live.contains(&rel.to) {
        return true;
    }
    if rel.kind == DERIVED_FROM {
        return true;
    }
    rel.kind == FOREIGN_KEY_TO
        && rel.attributes.get("resolution").map(String::as_str) == Some(RESOLUTION_UNRESOLVED)
}

struct EdgeSpec<'a> {
    kind: &'a str,
    from: &'a EntityId,
    to: &'a EntityId,
    resolution: &'a str,
    target: Option<&'a str>,
    candidates: Option<&'a str>,
    confidence: Confidence,
    path: &'a str,
    provider: &'a ProviderId,
    summary: &'a str,
}

fn put_edge(draft: &mut Draft, spec: &EdgeSpec<'_>) {
    let id = RelationshipId::generate(spec.kind, spec.from.as_str(), spec.to.as_str());
    if let Some(index) = draft.rels.iter().position(|rel| rel.id == id) {
        let mut changed = false;
        {
            let existing = &mut draft.rels[index];
            changed |= assign_attr(existing, "resolution", spec.resolution);
            if let Some(target) = spec.target {
                changed |= assign_attr(existing, "target", target);
            }
            if let Some(candidates) = spec.candidates {
                changed |= assign_attr(existing, "candidates", candidates);
            }
        }
        if changed {
            draft.dirty = true;
        }
        return;
    }
    let mut attributes = BTreeMap::from([("resolution".into(), spec.resolution.to_string())]);
    if let Some(target) = spec.target {
        attributes.insert("target".into(), target.to_string());
    }
    if let Some(candidates) = spec.candidates {
        attributes.insert("candidates".into(), candidates.to_string());
    }
    draft.rels.push(Relationship {
        id,
        kind: spec.kind.to_string(),
        from: spec.from.clone(),
        to: spec.to.clone(),
        attributes,
        evidence: vec![evidence(spec.provider, spec.summary, spec.path)],
        confidence: spec.confidence,
    });
    draft.dirty = true;
}

fn assign_attr(rel: &mut Relationship, key: &str, value: &str) -> bool {
    if rel.attributes.get(key).map(String::as_str) == Some(value) {
        return false;
    }
    rel.attributes.insert(key.into(), value.to_string());
    true
}

fn path_hits(dep: &str, paths: &[(String, ObjectId)]) -> Vec<(String, ObjectId)> {
    paths
        .iter()
        .filter(|(path, _)| dep_matches(dep, path))
        .cloned()
        .collect()
}

fn href_path(raw: &str) -> String {
    let raw = raw.trim();
    let cut = raw.split(['#', '?']).next().unwrap_or(raw);
    cut.trim_start_matches("./").to_string()
}

fn evidence(provider: &ProviderId, summary: &str, path: &str) -> Evidence {
    Evidence {
        summary: summary.to_string(),
        span: Some(SourceSpan::line(std::path::PathBuf::from(path), 1)),
        provider: provider.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::types::{
        CatalogForeignKey, CatalogProtoData, ObjectDescriptor, ProtoData,
    };
    use crate::ids::ProviderId;
    use crate::knowledge::{AttributeMap, Confidence};
    use crate::status::UnderstandingStatus;
    use crate::store::{KnowledgeStore, StoredObject};
    use std::path::Path;

    fn attrs(pairs: &[(&str, &str)]) -> AttributeMap {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn entity(
        kind: &str,
        name: &str,
        path: &Path,
        source: &ObjectId,
        provider: &ProviderId,
        _line: u32,
        attrs: AttributeMap,
        confidence: Confidence,
    ) -> Entity {
        Entity {
            id: EntityId::generate(kind, name, &path.to_string_lossy()),
            kind: kind.into(),
            name: name.into(),
            attributes: attrs,
            evidence: vec![evidence(provider, kind, &path.to_string_lossy())],
            confidence,
            source_object: source.clone(),
        }
    }

    fn relationship(
        kind: &str,
        from: &EntityId,
        to: &EntityId,
        provider: &ProviderId,
        path: &Path,
        _line: u32,
        confidence: Confidence,
    ) -> Relationship {
        Relationship {
            id: RelationshipId::generate(kind, from.as_str(), to.as_str()),
            kind: kind.into(),
            from: from.clone(),
            to: to.clone(),
            attributes: BTreeMap::new(),
            evidence: vec![evidence(provider, kind, &path.to_string_lossy())],
            confidence,
        }
    }

    fn object(root: &Path, rel: &str) -> StoredObject {
        let path = root.join(rel);
        StoredObject {
            descriptor: ObjectDescriptor::from_path(root, &path, false, 1, None),
            proto: ProtoData::default(),
            status: UnderstandingStatus::Understood,
            provider: Some(ProviderId::new("markdown")),
            classification_reason: None,
            entity_ids: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn document(stored: &StoredObject, name: &str) -> Entity {
        entity(
            "document",
            name,
            &stored.descriptor.relative_path,
            &stored.descriptor.id,
            &ProviderId::new("markdown"),
            1,
            attrs(&[]),
            Confidence::High,
        )
    }

    #[test]
    fn exact_hyperlink_and_false_positive_word() {
        let root = Path::new("/repo");
        let mut source = object(root, "docs/a.md");
        let mut target = object(root, "docs/other.md");
        let mut overview = object(root, "docs/overview.md");
        let doc = document(&source, "A");
        let other = document(&target, "Other");
        let overview_doc = document(&overview, "Overview");
        let link = entity(
            "link",
            "other.md",
            &source.descriptor.relative_path,
            &source.descriptor.id,
            &ProviderId::new("markdown"),
            1,
            attrs(&[("href", "other.md")]),
            Confidence::Medium,
        );
        let bare = entity(
            "link",
            "overview",
            &source.descriptor.relative_path,
            &source.descriptor.id,
            &ProviderId::new("markdown"),
            1,
            attrs(&[("href", "overview")]),
            Confidence::Medium,
        );
        let rels = vec![
            relationship(
                "links_to",
                &doc.id,
                &link.id,
                &ProviderId::new("markdown"),
                &source.descriptor.relative_path,
                1,
                Confidence::Medium,
            ),
            relationship(
                "links_to",
                &doc.id,
                &bare.id,
                &ProviderId::new("markdown"),
                &source.descriptor.relative_path,
                1,
                Confidence::Medium,
            ),
        ];
        source.entity_ids = vec![doc.id.clone(), link.id.clone(), bare.id.clone()];
        target.entity_ids = vec![other.id.clone()];
        overview.entity_ids = vec![overview_doc.id.clone()];
        let mut store = KnowledgeStore::new();
        store.install_record(source, vec![doc, link, bare], rels);
        store.install_record(target, vec![other.clone()], Vec::new());
        store.install_record(overview, vec![overview_doc], Vec::new());
        link_store(&mut store);
        let edges: Vec<_> = store
            .relationships()
            .filter(|rel| rel.kind == HYPERLINKS_TO)
            .collect();
        assert!(edges.iter().any(|rel| {
            rel.attributes.get("resolution").map(String::as_str) == Some(RESOLUTION_EXACT)
                && rel.to == other.id
        }));
        assert!(edges.iter().any(|rel| {
            rel.attributes.get("resolution").map(String::as_str) == Some(RESOLUTION_UNRESOLVED)
                && rel.attributes.get("target").map(String::as_str) == Some("overview")
        }));
        let placeholder = store
            .entities()
            .find(|entity| entity.name == "overview")
            .unwrap();
        assert_eq!(
            placeholder
                .attributes
                .get("placeholder")
                .map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn ambiguous_suffix_does_not_pick_a_target() {
        let root = Path::new("/repo");
        let mut source = object(root, "docs/a.md");
        let mut left = object(root, "a/other.md");
        let mut right = object(root, "b/other.md");
        let doc = document(&source, "A");
        let link = entity(
            "link",
            "other.md",
            &source.descriptor.relative_path,
            &source.descriptor.id,
            &ProviderId::new("markdown"),
            1,
            attrs(&[("href", "other.md")]),
            Confidence::Medium,
        );
        let rel = relationship(
            "links_to",
            &doc.id,
            &link.id,
            &ProviderId::new("markdown"),
            &source.descriptor.relative_path,
            1,
            Confidence::Medium,
        );
        source.entity_ids = vec![doc.id.clone(), link.id.clone()];
        let left_doc = document(&left, "L");
        let right_doc = document(&right, "R");
        left.entity_ids = vec![left_doc.id.clone()];
        right.entity_ids = vec![right_doc.id.clone()];
        let mut store = KnowledgeStore::new();
        store.install_record(source, vec![doc, link.clone()], vec![rel]);
        store.install_record(left, vec![left_doc], Vec::new());
        store.install_record(right, vec![right_doc], Vec::new());
        link_store(&mut store);
        let edge = store
            .relationships()
            .find(|rel| rel.kind == HYPERLINKS_TO)
            .unwrap();
        assert_eq!(
            edge.attributes.get("resolution").map(String::as_str),
            Some(RESOLUTION_AMBIGUOUS)
        );
        assert_eq!(edge.to, link.id);
        let candidates = edge.attributes.get("candidates").unwrap();
        assert!(candidates.contains("a/other.md"));
        assert!(candidates.contains("b/other.md"));
    }

    #[test]
    fn mutual_links_finish_in_one_pass() {
        let root = Path::new("/repo");
        let mut left = object(root, "docs/left.md");
        let mut right = object(root, "docs/right.md");
        let left_doc = document(&left, "Left");
        let right_doc = document(&right, "Right");
        let left_link = entity(
            "link",
            "right.md",
            &left.descriptor.relative_path,
            &left.descriptor.id,
            &ProviderId::new("markdown"),
            1,
            attrs(&[("href", "right.md")]),
            Confidence::Medium,
        );
        let right_link = entity(
            "link",
            "left.md",
            &right.descriptor.relative_path,
            &right.descriptor.id,
            &ProviderId::new("markdown"),
            1,
            attrs(&[("href", "left.md")]),
            Confidence::Medium,
        );
        let left_rel = relationship(
            "links_to",
            &left_doc.id,
            &left_link.id,
            &ProviderId::new("markdown"),
            &left.descriptor.relative_path,
            1,
            Confidence::Medium,
        );
        let right_rel = relationship(
            "links_to",
            &right_doc.id,
            &right_link.id,
            &ProviderId::new("markdown"),
            &right.descriptor.relative_path,
            1,
            Confidence::Medium,
        );
        left.entity_ids = vec![left_doc.id.clone(), left_link.id.clone()];
        right.entity_ids = vec![right_doc.id.clone(), right_link.id.clone()];
        let mut store = KnowledgeStore::new();
        store.install_record(left, vec![left_doc.clone(), left_link], vec![left_rel]);
        store.install_record(right, vec![right_doc.clone(), right_link], vec![right_rel]);
        link_store(&mut store);
        let exact = store
            .relationships()
            .filter(|rel| {
                rel.kind == HYPERLINKS_TO
                    && rel.attributes.get("resolution").map(String::as_str)
                        == Some(RESOLUTION_EXACT)
            })
            .count();
        assert_eq!(exact, 2);
        assert!(store.relationships().any(|rel| rel.to == right_doc.id));
        assert!(store.relationships().any(|rel| rel.to == left_doc.id));
    }

    #[test]
    fn deleting_the_target_drops_the_exact_edge() {
        let root = Path::new("/repo");
        let mut source = object(root, "docs/a.md");
        let mut target = object(root, "docs/b.md");
        let doc = document(&source, "A");
        let other = document(&target, "B");
        let link = entity(
            "link",
            "b.md",
            &source.descriptor.relative_path,
            &source.descriptor.id,
            &ProviderId::new("markdown"),
            1,
            attrs(&[("href", "b.md")]),
            Confidence::Medium,
        );
        let rel = relationship(
            "links_to",
            &doc.id,
            &link.id,
            &ProviderId::new("markdown"),
            &source.descriptor.relative_path,
            1,
            Confidence::Medium,
        );
        source.entity_ids = vec![doc.id.clone(), link.id.clone()];
        target.entity_ids = vec![other.id.clone()];
        let target_id = target.descriptor.id.clone();
        let mut store = KnowledgeStore::new();
        store.install_record(source, vec![doc, link], vec![rel]);
        store.install_record(target, vec![other], Vec::new());
        link_store(&mut store);
        store.remove_object(&target_id);
        link_store(&mut store);
        assert!(store.relationships().all(|rel| {
            rel.kind != HYPERLINKS_TO
                || rel.attributes.get("resolution").map(String::as_str) != Some(RESOLUTION_EXACT)
        }));
    }

    #[test]
    fn one_fingerprint_match_emits_derived_from_and_two_do_not() {
        let root = Path::new("/repo");
        let mut old = object(root, "docs/old.md");
        old.proto.fingerprint = Some("abc".into());
        let doc = document(&old, "Old");
        old.entity_ids = vec![doc.id.clone()];
        let old_id = old.descriptor.id.clone();
        let mut store = KnowledgeStore::new();
        store.install_record(old, vec![doc], Vec::new());
        store.remove_object(&old_id);

        let mut renamed = object(root, "docs/new.md");
        renamed.proto.fingerprint = Some("abc".into());
        let next = document(&renamed, "New");
        renamed.entity_ids = vec![next.id.clone()];
        store.install_record(renamed, vec![next.clone()], Vec::new());
        link_store(&mut store);
        let edge = store
            .relationships()
            .find(|rel| rel.kind == DERIVED_FROM)
            .unwrap();
        assert_eq!(edge.from, next.id);
        assert_eq!(
            edge.attributes.get("previous_path").map(String::as_str),
            Some("docs/old.md")
        );

        let mut extra = object(root, "docs/also.md");
        extra.proto.fingerprint = Some("abc".into());
        let also = document(&extra, "Also");
        extra.entity_ids = vec![also.id.clone()];
        store.install_record(extra, vec![also], Vec::new());
        let mut second_old = object(root, "docs/gone.md");
        second_old.proto.fingerprint = Some("zzz".into());
        let gone = document(&second_old, "Gone");
        second_old.entity_ids = vec![gone.id.clone()];
        let gone_object = second_old.descriptor.id.clone();
        store.install_record(second_old, vec![gone], Vec::new());
        store.remove_object(&gone_object);
        let mut twin_a = object(root, "docs/twin-a.md");
        twin_a.proto.fingerprint = Some("zzz".into());
        let a = document(&twin_a, "TwinA");
        twin_a.entity_ids = vec![a.id.clone()];
        let mut twin_b = object(root, "docs/twin-b.md");
        twin_b.proto.fingerprint = Some("zzz".into());
        let b = document(&twin_b, "TwinB");
        twin_b.entity_ids = vec![b.id.clone()];
        store.install_record(twin_a, vec![a], Vec::new());
        store.install_record(twin_b, vec![b], Vec::new());
        link_store(&mut store);
        let derived = store
            .relationships()
            .filter(|rel| rel.kind == DERIVED_FROM)
            .count();
        assert_eq!(derived, 1);
    }

    #[test]
    fn unresolved_foreign_key_stays_when_the_target_entity_is_absent() {
        let root = Path::new("/repo");
        let mut stored = object(root, "db://app/public/orders");
        stored.provider = Some(ProviderId::new("catalog"));
        stored.proto.catalog = Some(CatalogProtoData {
            database: "app".into(),
            schema: "public".into(),
            relation: "orders".into(),
            kind: "table".into(),
            foreign_keys: vec![CatalogForeignKey {
                columns: vec!["account_id".into()],
                target_schema: "public".into(),
                target_relation: "accounts".into(),
                target_columns: vec!["id".into()],
                target_discovered: false,
            }],
            ..CatalogProtoData::default()
        });
        let relation = entity(
            "relation",
            "orders",
            &stored.descriptor.relative_path,
            &stored.descriptor.id,
            &ProviderId::new("catalog"),
            1,
            attrs(&[]),
            Confidence::High,
        );
        let column_path =
            crate::connectors::catalog::column_locator("app", "public", "orders", "account_id");
        let column = entity(
            "column",
            "account_id",
            Path::new(&column_path),
            &stored.descriptor.id,
            &ProviderId::new("catalog"),
            2,
            attrs(&[]),
            Confidence::High,
        );
        stored.entity_ids = vec![relation.id.clone(), column.id.clone()];
        let mut store = KnowledgeStore::new();
        store.install_record(stored, vec![relation, column], Vec::new());
        link_store(&mut store);
        let edge = store
            .relationships()
            .find(|rel| rel.kind == FOREIGN_KEY_TO)
            .unwrap();
        assert_eq!(
            edge.attributes.get("resolution").map(String::as_str),
            Some(RESOLUTION_UNRESOLVED)
        );
        assert_eq!(
            edge.attributes.get("target").map(String::as_str),
            Some("public.accounts")
        );
        assert!(store.get_entity(&edge.to).is_none());
        link_store(&mut store);
        assert_eq!(
            store
                .relationships()
                .filter(|rel| rel.kind == FOREIGN_KEY_TO)
                .count(),
            1
        );
    }

    #[test]
    fn discovered_foreign_key_is_exact() {
        let root = Path::new("/repo");
        let mut orders = object(root, "db://app/public/orders");
        orders.provider = Some(ProviderId::new("catalog"));
        let mut accounts = object(root, "db://app/public/accounts");
        accounts.provider = Some(ProviderId::new("catalog"));
        let order_col =
            crate::connectors::catalog::column_locator("app", "public", "orders", "account_id");
        let account_col =
            crate::connectors::catalog::column_locator("app", "public", "accounts", "id");
        let from = entity(
            "column",
            "account_id",
            Path::new(&order_col),
            &orders.descriptor.id,
            &ProviderId::new("catalog"),
            1,
            attrs(&[]),
            Confidence::High,
        );
        let to = entity(
            "column",
            "id",
            Path::new(&account_col),
            &accounts.descriptor.id,
            &ProviderId::new("catalog"),
            1,
            attrs(&[]),
            Confidence::High,
        );
        let rel = relationship(
            "references",
            &from.id,
            &to.id,
            &ProviderId::new("catalog"),
            &orders.descriptor.relative_path,
            1,
            Confidence::High,
        );
        orders.entity_ids = vec![from.id.clone()];
        accounts.entity_ids = vec![to.id.clone()];
        let mut store = KnowledgeStore::new();
        store.install_record(orders, vec![from], vec![rel]);
        store.install_record(accounts, vec![to.clone()], Vec::new());
        link_store(&mut store);
        let edge = store
            .relationships()
            .find(|rel| rel.kind == FOREIGN_KEY_TO)
            .unwrap();
        assert_eq!(edge.to, to.id);
        assert_eq!(
            edge.attributes.get("resolution").map(String::as_str),
            Some(RESOLUTION_EXACT)
        );
        assert!(store.get_entity(&edge.to).is_some());
    }

    #[test]
    fn in_workbook_formula_is_exact_and_external_is_unresolved() {
        let root = Path::new("/repo");
        let mut stored = object(root, "office/book.xlsx");
        stored.provider = Some(ProviderId::new("office"));
        let sheet = entity(
            "worksheet",
            "Sheet1",
            &stored.descriptor.relative_path,
            &stored.descriptor.id,
            &ProviderId::new("office"),
            1,
            attrs(&[]),
            Confidence::High,
        );
        let local = entity(
            "formula",
            "Sheet1!A1",
            &stored.descriptor.relative_path,
            &stored.descriptor.id,
            &ProviderId::new("office"),
            1,
            attrs(&[("source", "=B1")]),
            Confidence::High,
        );
        let remote = entity(
            "formula",
            "Sheet1!A2",
            &stored.descriptor.relative_path,
            &stored.descriptor.id,
            &ProviderId::new("office"),
            2,
            attrs(&[("source", "=[other.xlsx]Sheet1!A1"), ("external", "true")]),
            Confidence::High,
        );
        let range = entity(
            "range",
            "B1",
            &stored.descriptor.relative_path,
            &stored.descriptor.id,
            &ProviderId::new("office"),
            1,
            attrs(&[]),
            Confidence::High,
        );
        let outside = entity(
            "range",
            "[other.xlsx]Sheet1!A1",
            &stored.descriptor.relative_path,
            &stored.descriptor.id,
            &ProviderId::new("office"),
            2,
            attrs(&[]),
            Confidence::High,
        );
        let provider = ProviderId::new("office");
        let rels = vec![
            relationship(
                "contains",
                &sheet.id,
                &local.id,
                &provider,
                &stored.descriptor.relative_path,
                1,
                Confidence::High,
            ),
            relationship(
                "contains",
                &sheet.id,
                &remote.id,
                &provider,
                &stored.descriptor.relative_path,
                2,
                Confidence::High,
            ),
            relationship(
                "references",
                &local.id,
                &range.id,
                &provider,
                &stored.descriptor.relative_path,
                1,
                Confidence::High,
            ),
            relationship(
                "references",
                &remote.id,
                &outside.id,
                &provider,
                &stored.descriptor.relative_path,
                2,
                Confidence::High,
            ),
        ];
        stored.entity_ids = vec![
            sheet.id.clone(),
            local.id.clone(),
            remote.id.clone(),
            range.id.clone(),
            outside.id.clone(),
        ];
        let mut store = KnowledgeStore::new();
        store.install_record(
            stored,
            vec![sheet, local.clone(), remote.clone(), range.clone(), outside],
            rels,
        );
        link_store(&mut store);
        assert!(store.relationships().any(|rel| {
            rel.kind == DECLARES_FORMULA
                && rel.to == local.id
                && rel.attributes.get("resolution").map(String::as_str) == Some(RESOLUTION_EXACT)
        }));
        assert!(store.relationships().any(|rel| {
            rel.kind == FORMULA_REFERENCES
                && rel.from == local.id
                && rel.to == range.id
                && rel.attributes.get("resolution").map(String::as_str) == Some(RESOLUTION_EXACT)
        }));
        assert!(store.relationships().any(|rel| {
            rel.kind == FORMULA_REFERENCES
                && rel.from == remote.id
                && rel.attributes.get("resolution").map(String::as_str)
                    == Some(RESOLUTION_UNRESOLVED)
        }));
    }

    #[test]
    fn https_hyperlink_stays_unresolved_without_a_second_object() {
        let root = Path::new("/repo");
        let mut stored = object(root, "office/brief.docx");
        stored.provider = Some(ProviderId::new("office"));
        let doc = document(&stored, "Brief");
        let mut doc = doc;
        doc.attributes
            .insert("hyperlinks".into(), "https://example.com/note".into());
        stored.entity_ids = vec![doc.id.clone()];
        let mut store = KnowledgeStore::new();
        store.install_record(stored, vec![doc], Vec::new());
        link_store(&mut store);
        assert_eq!(store.objects().count(), 1);
        let edge = store
            .relationships()
            .find(|rel| rel.kind == HYPERLINKS_TO)
            .unwrap();
        assert_eq!(
            edge.attributes.get("resolution").map(String::as_str),
            Some(RESOLUTION_UNRESOLVED)
        );
        assert_eq!(
            edge.attributes.get("target").map(String::as_str),
            Some("https://example.com/note")
        );
    }

    #[test]
    fn import_points_at_the_matched_root() {
        let root = Path::new("/repo");
        let mut source = object(root, "cpp/rule.cpp");
        source.provider = Some(ProviderId::new("tree-sitter-cpp"));
        let mut header = object(root, "cpp/rule.hpp");
        header.provider = Some(ProviderId::new("tree-sitter-cpp"));
        let file = entity(
            "module",
            "rule.cpp",
            &source.descriptor.relative_path,
            &source.descriptor.id,
            &ProviderId::new("tree-sitter-cpp"),
            1,
            attrs(&[]),
            Confidence::High,
        );
        let stand_in = entity(
            "module",
            "rule.hpp",
            &source.descriptor.relative_path,
            &source.descriptor.id,
            &ProviderId::new("tree-sitter-cpp"),
            1,
            attrs(&[("include", "rule.hpp")]),
            Confidence::Medium,
        );
        let header_mod = entity(
            "module",
            "rule",
            &header.descriptor.relative_path,
            &header.descriptor.id,
            &ProviderId::new("tree-sitter-cpp"),
            1,
            attrs(&[]),
            Confidence::High,
        );
        let rel = relationship(
            "includes",
            &file.id,
            &stand_in.id,
            &ProviderId::new("tree-sitter-cpp"),
            &source.descriptor.relative_path,
            1,
            Confidence::High,
        );
        source.entity_ids = vec![file.id.clone(), stand_in.id.clone()];
        header.entity_ids = vec![header_mod.id.clone()];
        let mut store = KnowledgeStore::new();
        store.install_record(source, vec![file, stand_in], vec![rel]);
        store.install_record(header, vec![header_mod.clone()], Vec::new());
        link_store(&mut store);
        let edge = store
            .relationships()
            .find(|rel| rel.kind == INCLUDES)
            .unwrap();
        assert_eq!(edge.to, header_mod.id);
        assert_eq!(
            edge.attributes.get("resolution").map(String::as_str),
            Some(RESOLUTION_EXACT)
        );
        assert_eq!(
            store
                .relationships()
                .filter(|rel| rel.kind == INCLUDES)
                .count(),
            1
        );
    }
}
