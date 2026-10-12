use crate::providers::util::{attrs, entity, ext_is};
use crate::{
    AnalysisResult, Confidence, ContentReader, Diagnostic, DiagnosticSeverity, KnowledgeRecord,
    ObjectDescriptor, ProtoData, ProviderId, Result, Support, UnderstandingProvider,
    UnderstandingStatus,
};

const MAX_KEYS: usize = 64;
const MAX_DEPTH: usize = 4;

pub struct YamlProvider;

impl UnderstandingProvider for YamlProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("yaml")
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        if ext_is(&object.extension, &["yaml", "yml"]) {
            Support::Full
        } else {
            Support::None
        }
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        _proto: &ProtoData,
        content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        let mut buf = Vec::new();
        content.read_to_end(&mut buf)?;
        let provider = self.id();
        let path = &object.relative_path;
        let mut diagnostics = Vec::new();

        let text = match std::str::from_utf8(&buf) {
            Ok(text) => text,
            Err(e) => {
                diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    message: format!("yaml parse error: {e}"),
                });
                return Ok(failed_doc(object, &provider, "yaml", diagnostics));
            }
        };
        let value = match yaml_rust2::YamlLoader::load_from_str(text) {
            Ok(mut docs) => {
                if docs.is_empty() {
                    yaml_rust2::Yaml::Null
                } else {
                    docs.remove(0)
                }
            }
            Err(e) => {
                diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    message: format!("yaml parse error: {e}"),
                });
                return Ok(failed_doc(object, &provider, "yaml", diagnostics));
            }
        };

        let mut keys = Vec::new();
        collect_yaml_keys(&value, "", 0, &mut keys);

        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "yaml".into());

        let mut attr = attrs(&[("format", "yaml"), ("key_count", &keys.len().to_string())]);
        if let yaml_rust2::Yaml::Hash(map) = &value {
            let top: Vec<String> = map
                .keys()
                .filter_map(|k| k.as_str().map(|s| s.to_string()))
                .take(32)
                .collect();
            attr.insert("top_level_keys".into(), top.join(","));
        }

        let mut entities = vec![entity(
            "document",
            &name,
            path,
            &object.id,
            &provider,
            1,
            attr,
            Confidence::High,
        )];
        for key in keys.into_iter().take(MAX_KEYS) {
            entities.push(entity(
                "key_path",
                &key,
                path,
                &object.id,
                &provider,
                1,
                attrs(&[("path", &key)]),
                Confidence::Medium,
            ));
        }

        Ok(AnalysisResult {
            record: KnowledgeRecord {
                entities,
                relationships: Vec::new(),
                diagnostics,
                status: Some(UnderstandingStatus::Understood),
                classification_reason: Some("yaml structure extracted".into()),
            },
            status: UnderstandingStatus::Understood,
            classification_reason: Some("yaml structure extracted".into()),
        })
    }
}

fn collect_yaml_keys(value: &yaml_rust2::Yaml, prefix: &str, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_DEPTH || out.len() >= MAX_KEYS {
        return;
    }
    match value {
        yaml_rust2::Yaml::Hash(map) => {
            for (k, v) in map {
                let key = k.as_str().unwrap_or("?").to_string();
                let path = if prefix.is_empty() {
                    key
                } else {
                    format!("{prefix}.{key}")
                };
                out.push(path.clone());
                collect_yaml_keys(v, &path, depth + 1, out);
            }
        }
        yaml_rust2::Yaml::Array(seq) => {
            if let Some(first) = seq.first() {
                let path = format!("{prefix}[]");
                out.push(path.clone());
                collect_yaml_keys(first, &path, depth + 1, out);
            }
        }
        _ => {}
    }
}

fn failed_doc(
    object: &ObjectDescriptor,
    provider: &ProviderId,
    format: &str,
    diagnostics: Vec<Diagnostic>,
) -> AnalysisResult {
    let path = &object.relative_path;
    let doc = entity(
        "document",
        &path.display().to_string(),
        path,
        &object.id,
        provider,
        1,
        attrs(&[("format", format), ("valid", "false")]),
        Confidence::Low,
    );
    AnalysisResult {
        record: KnowledgeRecord {
            entities: vec![doc],
            relationships: Vec::new(),
            diagnostics,
            status: Some(UnderstandingStatus::Failed),
            classification_reason: Some(format!("{format} parse failed")),
        },
        status: UnderstandingStatus::Failed,
        classification_reason: Some(format!("{format} parse failed")),
    }
}
