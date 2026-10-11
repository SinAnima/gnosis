use gnosis::OkfExporter;
use gnosis::{
    Confidence, Entity, EntityId, Exporter, KnowledgeStore, ObjectDescriptor, ObjectId, ProtoData,
    ProviderId, Relationship, RelationshipId, StoredObject, UnderstandingStatus,
};
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn okf_export_writes_bundle_layout() {
    let mut store = KnowledgeStore::new();
    store.set_root(PathBuf::from("/repo"));
    store.set_enabled_providers(vec!["test".into()]);

    let obj_id = ObjectId::new("obj:a.rs");
    let ent = Entity {
        id: EntityId::new("ent:fn:foo"),
        kind: "function".into(),
        name: "foo".into(),
        attributes: [("lang".into(), "rust".into())].into_iter().collect(),
        evidence: Vec::new(),
        confidence: Confidence::High,
        source_object: obj_id.clone(),
    };
    let ent2 = Entity {
        id: EntityId::new("ent:struct:Bar"),
        kind: "struct".into(),
        name: "Bar".into(),
        attributes: Default::default(),
        evidence: Vec::new(),
        confidence: Confidence::Medium,
        source_object: obj_id.clone(),
    };
    store.upsert_object(StoredObject {
        descriptor: ObjectDescriptor {
            id: obj_id.clone(),
            path: PathBuf::from("/repo/a.rs"),
            relative_path: PathBuf::from("a.rs"),
            is_dir: false,
            size: 1,
            modified: None,
            media_type: "text/x-rust".into(),
            extension: Some("rs".into()),
            etag: None,
        },
        proto: ProtoData::default(),
        status: UnderstandingStatus::Understood,
        provider: Some(ProviderId::new("test")),
        classification_reason: Some("ok".into()),
        entity_ids: vec![ent.id.clone(), ent2.id.clone()],
        diagnostics: Vec::new(),
    });
    store.add_entity(ent.clone());
    store.add_entity(ent2.clone());
    store.add_relationship(Relationship {
        id: RelationshipId::generate("defines", ent.id.as_str(), ent2.id.as_str()),
        kind: "defines".into(),
        from: ent.id,
        to: ent2.id,
        attributes: Default::default(),
        evidence: Vec::new(),
        confidence: Confidence::High,
    });

    let dir = TempDir::new().unwrap();
    let out = dir.path().join("knowledge.okf");
    OkfExporter::new().export(&store, &out).unwrap();

    assert!(out.join("index.md").exists());
    assert!(out.join("sidecar.json").exists());
    assert!(out.join("log.md").exists());
    assert!(out.join("entities").is_dir());
    assert!(out.join("objects").is_dir());
    assert!(out.join("relationships").is_dir());
    assert!(std::fs::read_dir(out.join("entities")).unwrap().count() >= 2);
    assert_eq!(OkfExporter::new().name(), "okf");
}

#[test]
fn okf_export_rewrites_changed_pages_and_drops_deleted_ones() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("knowledge.okf");
    let mut store = two_objects();
    OkfExporter::new().export(&store, &out).unwrap();

    let kept = out.join("objects").join("a.rs.md");
    let rewritten = out.join("objects").join("b.rs.md");
    std::fs::write(&kept, "---\nid: obj:a.rs\n---\n\nKEEP\n").unwrap();
    std::fs::write(&rewritten, "---\nid: obj:b.rs\n---\n\nOLD\n").unwrap();

    store.set_reason(&ObjectId::new("obj:a.rs"), "unchanged");
    let mut changed = store
        .get_object(&ObjectId::new("obj:b.rs"))
        .unwrap()
        .clone();
    changed.classification_reason = Some("edited".into());
    store.replace_analysis(changed, Vec::new(), Vec::new());
    OkfExporter::new().export(&store, &out).unwrap();

    let kept_text = std::fs::read_to_string(&kept).unwrap();
    assert!(kept_text.contains("KEEP"), "{kept_text}");
    let rewritten_text = std::fs::read_to_string(&rewritten).unwrap();
    assert!(rewritten_text.contains("edited"), "{rewritten_text}");
    assert!(out.join("index.md").is_file());

    store.remove_object(&ObjectId::new("obj:b.rs"));
    OkfExporter::new().export(&store, &out).unwrap();
    assert!(!rewritten.exists());
    assert!(kept.exists());
}

fn two_objects() -> KnowledgeStore {
    let mut store = KnowledgeStore::new();
    for name in ["a.rs", "b.rs"] {
        let id = ObjectId::new(format!("obj:{name}"));
        store.replace_analysis(
            StoredObject {
                descriptor: ObjectDescriptor {
                    id,
                    path: PathBuf::from(format!("/repo/{name}")),
                    relative_path: PathBuf::from(name),
                    is_dir: false,
                    size: 1,
                    modified: None,
                    media_type: "text/x-rust".into(),
                    extension: Some("rs".into()),
                    etag: None,
                },
                proto: ProtoData::default(),
                status: UnderstandingStatus::Understood,
                provider: Some(ProviderId::new("test")),
                classification_reason: Some("ok".into()),
                entity_ids: Vec::new(),
                diagnostics: Vec::new(),
            },
            Vec::new(),
            Vec::new(),
        );
    }
    store
}
