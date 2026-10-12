use gnosis::{AnalysisResult, ContentReader, KnowledgeRecord, Result};
use gnosis::{
    KnowledgeStore, ObjectDescriptor, ObjectId, ProtoData, ProviderId, ProviderRegistry, Support,
    UnderstandingProvider, UnderstandingStatus,
};
use std::path::PathBuf;

struct AlwaysFull;

impl UnderstandingProvider for AlwaysFull {
    fn id(&self) -> ProviderId {
        ProviderId::new("always-full")
    }
    fn probe(&self, _: &ObjectDescriptor, _: &ProtoData) -> Support {
        Support::Full
    }
    fn analyze(
        &self,
        _: &ObjectDescriptor,
        _: &ProtoData,
        _: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        Ok(AnalysisResult {
            record: KnowledgeRecord::default(),
            status: UnderstandingStatus::Understood,
            classification_reason: Some("ok".into()),
        })
    }
}

struct AlwaysWeak;

impl UnderstandingProvider for AlwaysWeak {
    fn id(&self) -> ProviderId {
        ProviderId::new("always-weak")
    }
    fn probe(&self, _: &ObjectDescriptor, _: &ProtoData) -> Support {
        Support::Weak
    }
    fn analyze(
        &self,
        _: &ObjectDescriptor,
        _: &ProtoData,
        _: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        Ok(AnalysisResult {
            record: KnowledgeRecord::default(),
            status: UnderstandingStatus::Unknown,
            classification_reason: Some("weak".into()),
        })
    }
}

#[test]
fn provider_selection_prefers_full_over_weak() {
    let mut reg = ProviderRegistry::new();
    reg.register(Box::new(AlwaysWeak));
    reg.register(Box::new(AlwaysFull));
    let obj = ObjectDescriptor {
        id: ObjectId::new("obj:x"),
        path: PathBuf::from("x.rs"),
        relative_path: PathBuf::from("x.rs"),
        is_dir: false,
        size: 1,
        modified: None,
        media_type: "text/plain".into(),
        extension: Some("rs".into()),
        etag: None,
    };
    let proto = ProtoData::default();
    let (p, support) = reg.select(&obj, &proto).unwrap();
    assert_eq!(p.id().as_str(), "always-full");
    assert_eq!(support, Support::Full);
}

#[test]
fn store_find_and_inventory() {
    let mut store = KnowledgeStore::new();
    let obj = ObjectDescriptor {
        id: ObjectId::new("obj:a"),
        path: PathBuf::from("a.rs"),
        relative_path: PathBuf::from("a.rs"),
        is_dir: false,
        size: 10,
        modified: None,
        media_type: "text/x-rust".into(),
        extension: Some("rs".into()),
        etag: None,
    };
    store.upsert_object(gnosis::StoredObject {
        descriptor: obj.clone(),
        proto: ProtoData::default(),
        status: UnderstandingStatus::Understood,
        provider: Some(ProviderId::new("test")),
        classification_reason: None,
        entity_ids: Vec::new(),
        diagnostics: Vec::new(),
    });
    let entity = gnosis::Entity {
        id: gnosis::EntityId::new("ent:fn:foo"),
        kind: "function".into(),
        name: "foo".into(),
        attributes: Default::default(),
        evidence: Vec::new(),
        confidence: gnosis::Confidence::High,
        source_object: obj.id.clone(),
    };
    store.add_entity(entity);
    let found = store.find("foo");
    assert_eq!(found.entities.len(), 1);
    let inv = store.inventory();
    assert_eq!(inv.source_objects, 1);
    assert_eq!(inv.functions, 1);
    assert_eq!(inv.understood, 1);
}
