use crate::ids::EntityId;
use crate::knowledge::Entity;
use crate::status::UnderstandingStatus;
use crate::store::{FindResults, GraphNeighborhood, InventoryCounts, KnowledgeStore, StoredObject};

pub struct QueryEngine<'a> {
    store: &'a KnowledgeStore,
}

impl<'a> QueryEngine<'a> {
    pub fn new(store: &'a KnowledgeStore) -> Self {
        Self { store }
    }

    pub fn summary(&self) -> String {
        let inv = self.store.inventory();
        let root = self
            .store
            .root()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(unknown)".into());
        let branch = self
            .store
            .git_branch()
            .map(|b| format!("branch: {b}"))
            .unwrap_or_else(|| "not a git repo".into());
        let scan = self
            .store
            .scan_id()
            .map(|s| format!("scan_id: {s}"))
            .unwrap_or_else(|| "scan_id: (none)".into());
        format!(
            "Gnosis summary\n\
             root: {root}\n\
             {branch}\n\
             {scan}\n\
             objects: {}\n\
             understood: {}  partial: {}  unknown: {}  failed: {}\n\
             modules: {}  types: {}  functions: {}\n\
             documents: {}  datasets: {}\n\
             relationships: {}\n\
             providers: {}\n",
            inv.source_objects,
            inv.understood,
            inv.partial,
            inv.unknown,
            inv.failed,
            inv.modules,
            inv.types,
            inv.functions,
            inv.documents,
            inv.datasets,
            inv.relationships,
            self.store.enabled_providers().join(", ")
        )
    }

    pub fn objects(
        &self,
        status: Option<UnderstandingStatus>,
        kind_substr: Option<&str>,
    ) -> Vec<&'a StoredObject> {
        let mut objs: Vec<_> = self.store.objects().collect();
        if let Some(status) = status {
            objs.retain(|o| o.status == status);
        }
        if let Some(k) = kind_substr {
            let k = k.to_lowercase();
            objs.retain(|o| {
                o.descriptor.media_type.to_lowercase().contains(&k)
                    || o.descriptor
                        .extension
                        .as_ref()
                        .map(|e| e.to_lowercase().contains(&k))
                        .unwrap_or(false)
            });
        }
        objs.sort_by(|a, b| a.descriptor.relative_path.cmp(&b.descriptor.relative_path));
        objs
    }

    pub fn unknown(&self) -> Vec<&'a StoredObject> {
        let mut objs: Vec<_> = self
            .store
            .objects()
            .filter(|o| {
                matches!(
                    o.status,
                    UnderstandingStatus::Unknown | UnderstandingStatus::PartiallyUnderstood
                )
            })
            .collect();
        objs.sort_by(|a, b| a.descriptor.relative_path.cmp(&b.descriptor.relative_path));
        objs
    }

    pub fn providers(&self) -> Vec<String> {
        self.store.enabled_providers().to_vec()
    }

    pub fn stats(&self) -> InventoryCounts {
        self.store.inventory()
    }

    pub fn find(&self, text: &str) -> FindResults {
        self.store.find(text)
    }

    pub fn explain(&self, query: &str) -> Option<ExplainResult<'a>> {
        if let Some(entity) = self.store.get_entity(&EntityId::new(query)) {
            let neighborhood = self.store.neighborhood(&entity.id, 1);
            return Some(ExplainResult::Entity {
                entity,
                neighborhood,
            });
        }
        let by_name = self.named(query);
        if by_name.len() > 1 {
            return Some(ExplainResult::Candidates(by_name));
        }
        if let Some(entity) = by_name.first() {
            let id = entity.id.clone();
            let neighborhood = self.store.neighborhood(&id, 1);
            if let Some(entity) = self.store.get_entity(&id) {
                return Some(ExplainResult::Entity {
                    entity,
                    neighborhood,
                });
            }
        }
        if let Some(obj) = self.store.get_object_by_path(query) {
            return Some(ExplainResult::Object { object: obj });
        }
        for obj in self.store.objects() {
            if obj.descriptor.id.as_str() == query
                || obj
                    .descriptor
                    .relative_path
                    .to_string_lossy()
                    .ends_with(query)
            {
                return Some(ExplainResult::Object { object: obj });
            }
        }
        None
    }

    pub fn graph(&self, query: &str, depth: usize) -> Option<GraphResult<'a>> {
        if let Some(entity) = self.store.get_entity(&EntityId::new(query)) {
            let neighborhood = self.store.neighborhood(&entity.id, depth);
            return Some(GraphResult::Graph {
                entity,
                neighborhood,
            });
        }
        let by_name = self.named(query);
        if by_name.len() > 1 {
            return Some(GraphResult::Candidates(by_name));
        }
        let entity = *by_name.first()?;
        let neighborhood = self.store.neighborhood(&entity.id, depth);
        Some(GraphResult::Graph {
            entity,
            neighborhood,
        })
    }

    /// Kind, endpoints, resolution, and evidence for one relationship id.
    pub fn why(&self, id: &str) -> String {
        let Some(rel) = self.store.relationships().find(|rel| rel.id.as_str() == id) else {
            return "not found".into();
        };
        let resolution = rel
            .attributes
            .get("resolution")
            .map(String::as_str)
            .unwrap_or("(none)");
        let evidence = rel
            .evidence
            .first()
            .map(|item| item.summary.as_str())
            .unwrap_or("(none)");
        let span = rel
            .evidence
            .first()
            .and_then(|item| item.span.as_ref())
            .map(|span| span.path.display().to_string())
            .unwrap_or_else(|| "(none)".into());
        let provider = rel
            .evidence
            .first()
            .map(|item| item.provider.to_string())
            .unwrap_or_else(|| "(none)".into());
        let mut text = format!(
            "kind: {}\nfrom: {}\nto: {}\nresolution: {resolution}\nevidence: {evidence}\nspan: {span}\nprovider: {provider}",
            rel.kind, rel.from, rel.to
        );
        if self.store.get_entity(&rel.to).is_none() {
            text.push_str(&format!("\n{} target entity not in store", rel.to));
        }
        text
    }

    /// Source object for one entity id or one name. Several names stay a list.
    pub fn where_is(&self, query: &str) -> String {
        if let Some(entity) = self.store.get_entity(&EntityId::new(query)) {
            return self.where_entity(entity);
        }
        let by_name = self.named(query);
        if by_name.is_empty() {
            return "not found".into();
        }
        if by_name.len() > 1 {
            return candidate_lines(&by_name);
        }
        self.where_entity(by_name[0])
    }

    /// Entities and intrinsic facts that carry `key`. At most 40 rows.
    pub fn meta(&self, key: &str) -> String {
        let mut lines = Vec::new();
        for entity in self.store.entities() {
            if let Some(value) = entity.attributes.get(key) {
                let path = self
                    .store
                    .get_object(&entity.source_object)
                    .map(|object| {
                        object
                            .descriptor
                            .relative_path
                            .to_string_lossy()
                            .to_string()
                    })
                    .unwrap_or_default();
                lines.push(format!("entity {} {path} {key}={value}", entity.id));
            }
        }
        for fact in self.store.facts() {
            if fact.key == key {
                lines.push(format!("fact {} {} {}", fact.path, fact.key, fact.value));
            }
        }
        lines.sort();
        lines.truncate(40);
        if lines.is_empty() {
            "not found".into()
        } else {
            lines.join("\n")
        }
    }

    /// Declared deps, plus exact import, include, and hyperlink targets.
    pub fn deps(&self, query: &str) -> String {
        let Some(object) = self.object_for(query) else {
            return "not found".into();
        };
        let entities = self.store.entities_of(&object.descriptor.id);
        let relationships = self.store.relationships_of(&object.descriptor.id);
        let mut declared = crate::knowledge_db::collect_deps(&entities, &relationships);
        declared.sort();
        declared.dedup();
        let mut exact = Vec::new();
        let mut unresolved = Vec::new();
        for rel in &relationships {
            if rel.kind != "imports" && rel.kind != "includes" && rel.kind != "hyperlinks_to" {
                continue;
            }
            let resolution = rel.attributes.get("resolution").map(String::as_str);
            if resolution == Some("exact") {
                if let Some(target) = self.store.get_entity(&rel.to) {
                    if let Some(path) = self.store.get_object(&target.source_object) {
                        exact.push(path.descriptor.relative_path.to_string_lossy().to_string());
                    }
                }
            } else if let Some(target) = rel.attributes.get("target") {
                unresolved.push(target.clone());
            }
        }
        exact.sort();
        exact.dedup();
        unresolved.sort();
        unresolved.dedup();
        format!(
            "deps:\n{}\nexact:\n{}\nunresolved:\n{}",
            join_lines(&declared),
            join_lines(&exact),
            join_lines(&unresolved)
        )
    }

    fn named(&self, query: &str) -> Vec<&'a Entity> {
        let mut by_name = self.store.find_entities_by_name(query);
        by_name.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        by_name
    }

    fn where_entity(&self, entity: &Entity) -> String {
        let path = self
            .store
            .get_object(&entity.source_object)
            .map(|object| {
                object
                    .descriptor
                    .relative_path
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_default();
        let mut text = format!("object: {}\npath: {path}", entity.source_object);
        if entity.attributes.get("placeholder").map(String::as_str) == Some("true") {
            text.push_str("\nplaceholder");
        }
        text
    }

    fn object_for(&self, query: &str) -> Option<&'a StoredObject> {
        if let Some(object) = self.store.get_object(&crate::ids::ObjectId::new(query)) {
            return Some(object);
        }
        if let Some(object) = self.store.get_object_by_path(query) {
            return Some(object);
        }
        let mut hits: Vec<_> = self
            .store
            .objects()
            .filter(|object| {
                object
                    .descriptor
                    .relative_path
                    .to_string_lossy()
                    .ends_with(query)
            })
            .collect();
        if hits.len() == 1 {
            hits.pop()
        } else {
            None
        }
    }
}

fn candidate_lines(entities: &[&Entity]) -> String {
    let mut lines = vec!["candidates:".to_string()];
    for entity in entities {
        lines.push(format!(
            "{} {} {} {}",
            entity.kind, entity.name, entity.id, entity.source_object
        ));
    }
    lines.join("\n")
}

fn join_lines(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".into()
    } else {
        items.join("\n")
    }
}

#[derive(Debug)]
pub enum ExplainResult<'a> {
    Entity {
        entity: &'a Entity,
        neighborhood: GraphNeighborhood,
    },
    Object {
        object: &'a StoredObject,
    },
    Candidates(Vec<&'a Entity>),
}

#[derive(Debug)]
pub enum GraphResult<'a> {
    Graph {
        entity: &'a Entity,
        neighborhood: GraphNeighborhood,
    },
    Candidates(Vec<&'a Entity>),
}
