use gnosis::{
    default_registry, CppProvider, CsvProvider, ElixirProvider, GenericMetadataProvider,
    JsonProvider, MarkdownProvider, PlainTextProvider, RustProvider, TomlProvider, YamlProvider,
};
use gnosis::{BytesContentReader, ObjectDescriptor, ProtoData, Support, UnderstandingProvider};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn descriptor(root: &Path, rel: &str) -> ObjectDescriptor {
    let path = root.join(rel);
    let meta = fs::metadata(&path).unwrap();
    ObjectDescriptor::from_path(root, &path, false, meta.len(), meta.modified().ok())
}

fn analyze_file(
    provider: &dyn UnderstandingProvider,
    root: &Path,
    rel: &str,
) -> gnosis::AnalysisResult {
    let object = descriptor(root, rel);
    let bytes = fs::read(&object.path).unwrap();
    let proto = ProtoData {
        filename: object
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        extension: object.extension.clone(),
        media_type: object.media_type.clone(),
        size: object.size,
        ..ProtoData::default()
    };
    let mut reader = BytesContentReader::new(bytes);
    assert_ne!(provider.probe(&object, &proto), Support::None);
    provider.analyze(&object, &proto, &mut reader).unwrap()
}

fn analyze_bytes(
    provider: &dyn UnderstandingProvider,
    name: &str,
    bytes: &[u8],
) -> gnosis::AnalysisResult {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(name);
    fs::write(&path, bytes).unwrap();
    analyze_file(provider, dir.path(), name)
}

#[test]
fn default_registry_covers_fixture_extensions() {
    let reg = default_registry();
    assert!(reg.providers().len() >= 10);
    assert!(!reg.coverage_summary().is_empty());
}

#[test]
fn tree_sitter_providers_on_fixture_sources() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/mixed-repo");
    let root = root.canonicalize().unwrap();

    let cpp = analyze_file(&CppProvider, &root, "cpp/pricing.cpp");
    assert!(cpp.record.entities.iter().any(|e| {
        e.name.contains("Pricing")
            || e.kind == "function"
            || e.kind == "class"
            || e.kind == "namespace"
    }));

    let rust = analyze_file(&RustProvider, &root, "src/catalog.rs");
    assert!(rust
        .record
        .entities
        .iter()
        .any(|e| e.name == "Catalog" || e.kind == "struct"));

    let elixir = analyze_file(&ElixirProvider, &root, "elixir/engine.ex");
    assert!(elixir
        .record
        .entities
        .iter()
        .any(|e| { e.name.contains("Pricing") || e.kind == "module" || e.kind == "function" }));
}

#[test]
fn noncode_providers_on_fixtures() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/mixed-repo");
    let root = root.canonicalize().unwrap();

    let md = analyze_file(&MarkdownProvider, &root, "docs/pricing.md");
    assert!(md.record.entities.iter().any(|e| e.kind == "document"));

    let txt = analyze_file(&PlainTextProvider, &root, "docs/notes.txt");
    assert_eq!(txt.status, gnosis::UnderstandingStatus::PartiallyUnderstood);

    let json = analyze_file(&JsonProvider, &root, "data/package.json");
    assert!(json.record.entities.iter().any(|e| e.kind == "document"));

    let yaml = analyze_file(&YamlProvider, &root, "data/config.yaml");
    assert!(yaml.record.entities.iter().any(|e| e.kind == "document"));

    let toml = analyze_file(&TomlProvider, &root, "data/demo.toml");
    assert!(toml.record.entities.iter().any(|e| e.kind == "document"));

    let csv = analyze_file(&CsvProvider, &root, "data/prices.csv");
    assert!(csv.record.entities.iter().any(|e| e.kind == "dataset"));
    assert!(csv.record.entities.iter().any(|e| e.kind == "column"));

    let binary = descriptor(&root, "data/blob.bin");
    let proto = ProtoData {
        filename: "blob.bin".into(),
        extension: Some("bin".into()),
        media_type: binary.media_type.clone(),
        fingerprint: Some("abc".into()),
        ..ProtoData::default()
    };
    let mut reader = BytesContentReader::new(vec![0, 1, 2]);
    let generic = GenericMetadataProvider
        .analyze(&binary, &proto, &mut reader)
        .unwrap();
    assert!(matches!(
        generic.status,
        gnosis::UnderstandingStatus::Unknown | gnosis::UnderstandingStatus::PartiallyUnderstood
    ));
}

#[test]
fn malformed_structured_inputs_are_failed_or_diagnosed() {
    let json = analyze_bytes(&JsonProvider, "bad.json", b"{not json");
    assert_eq!(json.status, gnosis::UnderstandingStatus::Failed);
    assert!(!json.record.diagnostics.is_empty());

    let yaml = analyze_bytes(&YamlProvider, "bad.yaml", b"[[[[");
    assert_eq!(yaml.status, gnosis::UnderstandingStatus::Failed);

    let toml = analyze_bytes(&TomlProvider, "bad.toml", b"[[[");
    assert_eq!(toml.status, gnosis::UnderstandingStatus::Failed);

    let csv = analyze_bytes(&CsvProvider, "empty.csv", b"");
    assert!(matches!(
        csv.status,
        gnosis::UnderstandingStatus::PartiallyUnderstood
            | gnosis::UnderstandingStatus::Understood
            | gnosis::UnderstandingStatus::Failed
    ));
}

#[test]
fn provider_probe_rejects_wrong_extensions() {
    let obj = ObjectDescriptor {
        id: gnosis::ObjectId::new("obj:x"),
        path: PathBuf::from("x.bin"),
        relative_path: PathBuf::from("x.bin"),
        is_dir: false,
        size: 1,
        modified: None,
        media_type: "application/octet-stream".into(),
        extension: Some("bin".into()),
        etag: None,
    };
    let proto = ProtoData::default();
    assert_eq!(CppProvider.probe(&obj, &proto), Support::None);
    assert_eq!(RustProvider.probe(&obj, &proto), Support::None);
    assert_eq!(MarkdownProvider.probe(&obj, &proto), Support::None);
    assert_eq!(JsonProvider.probe(&obj, &proto), Support::None);
    assert_eq!(GenericMetadataProvider.probe(&obj, &proto), Support::Weak);
}
