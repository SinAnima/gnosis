use crate::connectors::types::{ObjectDescriptor, ProtoData};
use crate::ids::{EntityId, ObjectId, ProviderId, RelationshipId};
use crate::knowledge::{Entity, Relationship};
use crate::status::UnderstandingStatus;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct StoredObject {
    pub descriptor: ObjectDescriptor,
    pub proto: ProtoData,
    pub status: UnderstandingStatus,
    pub provider: Option<ProviderId>,
    pub classification_reason: Option<String>,
    pub entity_ids: Vec<EntityId>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InventoryCounts {
    pub source_objects: u64,
    pub modules: u64,
    pub types: u64,
    pub functions: u64,
    pub documents: u64,
    pub datasets: u64,
    pub relationships: u64,
    pub understood: u64,
    pub partial: u64,
    pub unknown: u64,
    pub failed: u64,
}

#[derive(Debug, Default)]
pub struct KnowledgeStore {
    objects: HashMap<ObjectId, StoredObject>,
    entities: HashMap<EntityId, Entity>,
    relationships: HashMap<RelationshipId, Relationship>,
    by_name: HashMap<String, HashSet<EntityId>>,
    by_kind: HashMap<String, HashSet<EntityId>>,
    by_path: HashMap<String, ObjectId>,
    reverse_rels: HashMap<EntityId, HashSet<RelationshipId>>,
    forward_rels: HashMap<EntityId, HashSet<RelationshipId>>,
    root: Option<std::path::PathBuf>,
    git_branch: Option<String>,
    connector: Option<String>,
    scan_id: Option<String>,
    enabled_providers: Vec<String>,
}

impl KnowledgeStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_root(&mut self, root: std::path::PathBuf) {
        self.root = Some(root);
    }

    pub fn root(&self) -> Option<&std::path::PathBuf> {
        self.root.as_ref()
    }

    pub fn set_git_branch(&mut self, branch: Option<String>) {
        self.git_branch = branch;
    }

    pub fn git_branch(&self) -> Option<&str> {
        self.git_branch.as_deref()
    }

    pub fn set_connector(&mut self, connector: impl Into<String>) {
        self.connector = Some(connector.into());
    }

    pub fn connector(&self) -> Option<&str> {
        self.connector.as_deref()
    }

    pub fn set_scan_id(&mut self, scan_id: impl Into<String>) {
        self.scan_id = Some(scan_id.into());
    }

    pub fn scan_id(&self) -> Option<&str> {
        self.scan_id.as_deref()
    }

    pub fn set_enabled_providers(&mut self, providers: Vec<String>) {
        self.enabled_providers = providers;
    }

    pub fn enabled_providers(&self) -> &[String] {
        &self.enabled_providers
    }

    pub fn object_status(&self, id: &ObjectId) -> Option<UnderstandingStatus> {
        self.objects.get(id).map(|o| o.status)
    }

    pub fn review_rows(&self) -> Vec<crate::review::ReviewRow> {
        let mut rows = Vec::new();
        for entity in self.entities.values() {
            if entity.attributes.get("origin").map(String::as_str) != Some("llm") {
                continue;
            }
            rows.push(crate::review::ReviewRow {
                id: entity.id.to_string(),
                kind: entity.kind.clone(),
                label: entity.name.clone(),
                review: entity
                    .attributes
                    .get("review")
                    .cloned()
                    .unwrap_or_else(|| "pending".into()),
            });
        }
        for rel in self.relationships.values() {
            if rel.attributes.get("origin").map(String::as_str) != Some("llm") {
                continue;
            }
            rows.push(crate::review::ReviewRow {
                id: rel.id.as_str().to_string(),
                kind: rel.kind.clone(),
                label: format!("{} -> {}", rel.from, rel.to),
                review: rel
                    .attributes
                    .get("review")
                    .cloned()
                    .unwrap_or_else(|| "pending".into()),
            });
        }
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        rows
    }

    pub fn accept_review(&mut self, query: &str) -> std::result::Result<String, String> {
        let q = query.trim();
        if q.is_empty() {
            return Err("review id required".into());
        }
        let mut matches: Vec<String> = self
            .review_rows()
            .into_iter()
            .map(|r| r.id)
            .filter(|id| id == q || id.ends_with(q) || id.contains(q))
            .collect();
        matches.sort();
        matches.dedup();
        let id = match matches.len() {
            0 => return Err(format!("review target not found: {q}")),
            1 => matches.remove(0),
            n => return Err(format!("ambiguous review id '{q}' ({n} matches)")),
        };
        if let Some(entity) = self.entities.values_mut().find(|e| e.id.as_str() == id) {
            entity.attributes.insert("review".into(), "accepted".into());
            return Ok(id);
        }
        if let Some(rel) = self
            .relationships
            .values_mut()
            .find(|r| r.id.as_str() == id)
        {
            rel.attributes.insert("review".into(), "accepted".into());
            return Ok(id);
        }
        Err(format!("review target not found: {q}"))
    }

    pub fn upsert_object(&mut self, object: StoredObject) {
        let path_key = object
            .descriptor
            .relative_path
            .to_string_lossy()
            .to_string();
        self.by_path.insert(path_key, object.descriptor.id.clone());
        self.objects.insert(object.descriptor.id.clone(), object);
    }

    pub fn add_entity(&mut self, entity: Entity) {
        let id = entity.id.clone();
        self.by_name
            .entry(entity.name.to_lowercase())
            .or_default()
            .insert(id.clone());
        self.by_kind
            .entry(entity.kind.clone())
            .or_default()
            .insert(id.clone());
        if let Some(obj) = self.objects.get_mut(&entity.source_object) {
            if !obj.entity_ids.contains(&id) {
                obj.entity_ids.push(id.clone());
            }
        }
        self.entities.insert(id, entity);
    }

    pub fn add_relationship(&mut self, rel: Relationship) {
        let id = rel.id.clone();
        self.forward_rels
            .entry(rel.from.clone())
            .or_default()
            .insert(id.clone());
        self.reverse_rels
            .entry(rel.to.clone())
            .or_default()
            .insert(id.clone());
        self.relationships.insert(id, rel);
    }

    pub fn get_object(&self, id: &ObjectId) -> Option<&StoredObject> {
        self.objects.get(id)
    }

    pub fn get_object_by_path(&self, path: &str) -> Option<&StoredObject> {
        self.by_path.get(path).and_then(|id| self.objects.get(id))
    }

    pub fn objects(&self) -> impl Iterator<Item = &StoredObject> {
        self.objects.values()
    }

    pub fn objects_by_status(&self, status: UnderstandingStatus) -> Vec<&StoredObject> {
        self.objects
            .values()
            .filter(|o| o.status == status)
            .collect()
    }

    pub fn get_entity(&self, id: &EntityId) -> Option<&Entity> {
        self.entities.get(id)
    }

    pub fn entities(&self) -> impl Iterator<Item = &Entity> {
        self.entities.values()
    }

    pub fn relationships(&self) -> impl Iterator<Item = &Relationship> {
        self.relationships.values()
    }

    pub fn find_entities_by_name(&self, name: &str) -> Vec<&Entity> {
        let key = name.to_lowercase();
        self.by_name
            .get(&key)
            .into_iter()
            .flatten()
            .filter_map(|id| self.entities.get(id))
            .collect()
    }

    pub fn find(&self, text: &str) -> FindResults {
        let q = text.to_lowercase();
        let mut entities = Vec::new();
        for e in self.entities.values() {
            if e.name.to_lowercase().contains(&q)
                || e.kind.to_lowercase().contains(&q)
                || e.attributes.values().any(|v| v.to_lowercase().contains(&q))
            {
                entities.push(e.clone());
            }
        }
        let mut objects = Vec::new();
        for o in self.objects.values() {
            let path = o.descriptor.relative_path.to_string_lossy().to_lowercase();
            if path.contains(&q)
                || o.descriptor.media_type.to_lowercase().contains(&q)
                || o.classification_reason
                    .as_ref()
                    .map(|r| r.to_lowercase().contains(&q))
                    .unwrap_or(false)
            {
                objects.push(o.clone());
            }
        }
        entities.sort_by(|a, b| a.name.cmp(&b.name));
        objects.sort_by(|a, b| a.descriptor.relative_path.cmp(&b.descriptor.relative_path));
        FindResults { entities, objects }
    }

    pub fn neighborhood(&self, entity_id: &EntityId, depth: usize) -> GraphNeighborhood {
        let mut nodes = BTreeMap::new();
        let mut edges = Vec::new();
        let mut frontier = vec![entity_id.clone()];
        let mut seen = HashSet::new();
        seen.insert(entity_id.clone());

        if let Some(e) = self.entities.get(entity_id) {
            nodes.insert(entity_id.clone(), e.clone());
        }

        for _ in 0..depth {
            let mut next = Vec::new();
            for id in &frontier {
                let rel_ids: Vec<_> = self
                    .forward_rels
                    .get(id)
                    .into_iter()
                    .flatten()
                    .chain(self.reverse_rels.get(id).into_iter().flatten())
                    .cloned()
                    .collect();
                for rid in rel_ids {
                    if let Some(rel) = self.relationships.get(&rid) {
                        edges.push(rel.clone());
                        for neighbor in [&rel.from, &rel.to] {
                            if seen.insert(neighbor.clone()) {
                                if let Some(e) = self.entities.get(neighbor) {
                                    nodes.insert(neighbor.clone(), e.clone());
                                    next.push(neighbor.clone());
                                }
                            }
                        }
                    }
                }
            }
            frontier = next;
            if frontier.is_empty() {
                break;
            }
        }

        GraphNeighborhood { nodes, edges }
    }

    pub fn inventory(&self) -> InventoryCounts {
        let mut counts = InventoryCounts {
            source_objects: self.objects.len() as u64,
            relationships: self.relationships.len() as u64,
            ..Default::default()
        };
        for o in self.objects.values() {
            match o.status {
                UnderstandingStatus::Understood => counts.understood += 1,
                UnderstandingStatus::PartiallyUnderstood => counts.partial += 1,
                UnderstandingStatus::Unknown => counts.unknown += 1,
                UnderstandingStatus::Failed => counts.failed += 1,
            }
        }
        for e in self.entities.values() {
            match e.kind.as_str() {
                "module" | "namespace" | "package" => counts.modules += 1,
                "class" | "struct" | "enum" | "trait" | "protocol" | "type" => counts.types += 1,
                "function" | "method" => counts.functions += 1,
                "document" | "section" | "heading" => counts.documents += 1,
                "dataset" | "column" => counts.datasets += 1,
                _ => {}
            }
        }
        counts
    }
}

#[derive(Clone, Debug, Default)]
pub struct FindResults {
    pub entities: Vec<Entity>,
    pub objects: Vec<StoredObject>,
}

#[derive(Clone, Debug, Default)]
pub struct GraphNeighborhood {
    pub nodes: BTreeMap<EntityId, Entity>,
    pub edges: Vec<Relationship>,
}
