use gnosis::{
    BytesContentReader, Confidence, ContentReader, Entity, EntityId, Evidence, GnosisError,
    KnowledgeStore, ObjectDescriptor, ObjectId, PipelineEvent, ProtoData, ProviderId, QueryEngine,
    Relationship, RelationshipId, SourceSpan, StoredObject, UnderstandingStatus,
};
use gnosis::{LimitedReader, ScanConfig};
use std::io::Cursor;
use std::path::PathBuf;

fn sample_object(path: &str, status: UnderstandingStatus) -> StoredObject {
    let path = PathBuf::from(path);
    let id = ObjectId::new(format!("obj:{}", path.display()));
    StoredObject {
        descriptor: ObjectDescriptor {
            id: id.clone(),
            path: path.clone(),
            relative_path: path.clone(),
            is_dir: false,
            size: 10,
            modified: None,
            media_type: "text/plain".into(),
            extension: path.extension().map(|e| e.to_string_lossy().into_owned()),
            etag: None,
        },
        proto: ProtoData {
            filename: path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            ..ProtoData::default()
        },
        status,
        provider: Some(ProviderId::new("test")),
        classification_reason: Some("unit".into()),
        entity_ids: Vec::new(),
        diagnostics: Vec::new(),
    }
}

fn sample_entity(name: &str, kind: &str, source: &ObjectId) -> Entity {
    Entity {
        id: EntityId::generate(kind, name, "x"),
        kind: kind.into(),
        name: name.into(),
        attributes: Default::default(),
        evidence: vec![Evidence {
            summary: "test".into(),
            span: Some(SourceSpan::line(PathBuf::from("x"), 1)),
            provider: ProviderId::new("test"),
        }],
        confidence: Confidence::High,
        source_object: source.clone(),
    }
}

#[test]
fn query_summary_objects_unknown_find_explain_graph() {
    let mut store = KnowledgeStore::new();
    store.set_root(PathBuf::from("/tmp/repo"));
    store.set_git_branch(Some("main".into()));
    store.set_enabled_providers(vec!["test".into()]);

    let mut a = sample_object("src/a.rs", UnderstandingStatus::Understood);
    let b = sample_object("data/blob.bin", UnderstandingStatus::Unknown);
    let c = sample_object("docs/n.txt", UnderstandingStatus::PartiallyUnderstood);

    let ea = sample_entity("Alpha", "function", &a.descriptor.id);
    let eb = sample_entity("Beta", "struct", &a.descriptor.id);
    a.entity_ids.push(ea.id.clone());
    a.entity_ids.push(eb.id.clone());

    store.upsert_object(a);
    store.upsert_object(b);
    store.upsert_object(c);
    store.add_entity(ea.clone());
    store.add_entity(eb.clone());
    store.add_relationship(Relationship {
        id: RelationshipId::generate("defines", ea.id.as_str(), eb.id.as_str()),
        kind: "defines".into(),
        from: ea.id.clone(),
        to: eb.id.clone(),
        attributes: Default::default(),
        evidence: Vec::new(),
        confidence: Confidence::Medium,
    });

    let q = QueryEngine::new(&store);
    let summary = q.summary();
    assert!(summary.contains("Gnosis summary"));
    assert!(summary.contains("branch: main"));
    assert!(summary.contains("objects: 3"));

    assert_eq!(q.objects(None, None).len(), 3);
    assert_eq!(q.objects(Some(UnderstandingStatus::Unknown), None).len(), 1);
    assert_eq!(q.objects(None, Some("rs")).len(), 1);
    assert_eq!(q.unknown().len(), 2);
    assert_eq!(q.providers(), vec!["test".to_string()]);
    assert_eq!(q.stats().functions, 1);
    assert_eq!(q.stats().types, 1);

    let found = q.find("Alpha");
    assert_eq!(found.entities.len(), 1);

    match q.explain("Alpha").unwrap() {
        gnosis::ExplainResult::Entity {
            entity,
            neighborhood,
        } => {
            assert_eq!(entity.name, "Alpha");
            assert!(!neighborhood.edges.is_empty());
        }
        _ => panic!("expected entity"),
    }
    assert!(matches!(
        q.explain("data/blob.bin"),
        Some(gnosis::ExplainResult::Object { .. })
    ));
    assert!(q.explain("missing").is_none());

    let (entity, graph) = q.graph("Alpha", 1).unwrap();
    assert_eq!(entity.name, "Alpha");
    assert!(!graph.edges.is_empty());
}

#[test]
fn pipeline_event_summaries_cover_variants() {
    let id = ObjectId::new("obj:x");
    let provider = ProviderId::new("p");
    let entity = sample_entity("E", "function", &id);
    let events = vec![
        PipelineEvent::ScanStarted {
            root: PathBuf::from("."),
            scan_id: "scan:test".into(),
        },
        PipelineEvent::ScanCompleted {
            objects: 1,
            elapsed_ms: 2,
            scan_id: "scan:test".into(),
        },
        PipelineEvent::ObjectDiscovered {
            id: id.clone(),
            path: PathBuf::from("x"),
        },
        PipelineEvent::ObjectQueued {
            id: id.clone(),
            queue_depth: 1,
        },
        PipelineEvent::ProviderSelected {
            id: id.clone(),
            provider: provider.clone(),
            support: "full".into(),
        },
        PipelineEvent::AnalysisStarted {
            id: id.clone(),
            provider: provider.clone(),
        },
        PipelineEvent::AnalysisCompleted {
            id: id.clone(),
            provider: provider.clone(),
            status: UnderstandingStatus::Understood,
            entities: 1,
            relationships: 0,
        },
        PipelineEvent::EntityCreated {
            entity: entity.clone(),
        },
        PipelineEvent::RelationshipCreated {
            kind: "defines".into(),
            from: "a".into(),
            to: "b".into(),
        },
        PipelineEvent::ObjectClassified {
            id: id.clone(),
            status: UnderstandingStatus::Unknown,
            reason: Some("nope".into()),
        },
        PipelineEvent::Warning {
            message: "w".into(),
        },
        PipelineEvent::Failure {
            id: Some(id.clone()),
            message: "f".into(),
        },
        PipelineEvent::Failure {
            id: None,
            message: "f2".into(),
        },
        PipelineEvent::ExportStarted {
            path: PathBuf::from("out"),
        },
        PipelineEvent::ExportCompleted {
            path: PathBuf::from("out"),
        },
        PipelineEvent::MetricsSnapshot {
            metrics: Default::default(),
        },
    ];
    for ev in events {
        assert!(!ev.summary().is_empty());
    }
}

#[test]
fn content_readers_and_limited_reader() {
    let mut reader = BytesContentReader::from_slice(b"hello world");
    assert_eq!(reader.as_str_lossy(), "hello world");
    let mut buf = [0u8; 5];
    assert_eq!(reader.read(&mut buf).unwrap(), 5);
    assert_eq!(&buf, b"hello");
    let mut rest = Vec::new();
    assert_eq!(reader.read_to_end(&mut rest).unwrap(), 6);
    assert_eq!(rest, b" world");
    assert_eq!(reader.read(&mut buf).unwrap(), 0);

    let mut limited = LimitedReader::new(Cursor::new(b"abcdef"), 3);
    let mut out = Vec::new();
    limited.read_to_end(&mut out).unwrap();
    assert_eq!(out, b"abc");
    assert_eq!(limited.read(&mut buf).unwrap(), 0);
}

#[test]
fn error_display_and_provider_helper() {
    let err = GnosisError::provider("json", "boom");
    assert!(err.to_string().contains("json"));
    assert!(err.to_string().contains("boom"));
    assert!(GnosisError::Cancelled.to_string().contains("cancelled"));
}

#[test]
fn scan_config_defaults() {
    let cfg = ScanConfig::with_root("/tmp/x");
    assert_eq!(cfg.root, PathBuf::from("/tmp/x"));
    assert!(cfg.max_object_size > 0);
    assert!(cfg.concurrency >= 1);
    assert!(cfg.excluded_paths.iter().any(|p| p == "target"));
}

#[test]
fn status_and_confidence_strings() {
    assert_eq!(UnderstandingStatus::Understood.as_str(), "understood");
    assert_eq!(UnderstandingStatus::Failed.to_string(), "failed");
    assert_eq!(Confidence::Inferred.as_str(), "inferred");
}

#[test]
fn ids_helpers() {
    let oid = ObjectId::from_path(
        std::path::Path::new("/r"),
        std::path::Path::new("/r/a/b.rs"),
    );
    assert!(oid.as_str().contains("a/b.rs"));
    let _ = format!("{oid}");
    let eid = EntityId::generate("fn", "foo", "a.rs");
    assert!(eid.as_str().contains("foo"));
    let _ = format!("{eid}");
    let pid = ProviderId::new("x");
    assert_eq!(format!("{pid}"), "x");
    assert_eq!(gnosis::ids::random_suffix().len(), 8);
}
