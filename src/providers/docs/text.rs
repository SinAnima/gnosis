use crate::providers::util::{attrs, entity, ext_is};
use crate::{
    AnalysisResult, Confidence, ContentReader, KnowledgeRecord, ObjectDescriptor, ProtoData,
    ProviderId, Result, Support, UnderstandingProvider, UnderstandingStatus,
};

pub struct PlainTextProvider;

fn is_web_asset(media: &str, extension: Option<&str>) -> bool {
    if matches!(
        extension,
        Some(
            "css"
                | "js"
                | "mjs"
                | "svg"
                | "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "woff"
                | "woff2"
        )
    ) {
        return true;
    }
    let media = media.split(';').next().unwrap_or(media).trim();
    media.eq_ignore_ascii_case("text/css")
        || media.eq_ignore_ascii_case("text/javascript")
        || media.eq_ignore_ascii_case("application/javascript")
        || media.starts_with("image/")
        || media.starts_with("font/")
}

impl UnderstandingProvider for PlainTextProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("plaintext")
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        if is_web_asset(&object.media_type, object.extension.as_deref()) {
            Support::None
        } else if ext_is(&object.extension, &["txt", "text", "log"]) {
            Support::Full
        } else if object.media_type.starts_with("text/")
            && !ext_is(
                &object.extension,
                &[
                    "md", "markdown", "json", "yaml", "yml", "toml", "csv", "rs", "cpp", "h", "ex",
                    "exs",
                ],
            )
        {
            Support::Partial
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
        let source = String::from_utf8_lossy(&buf);
        let provider = self.id();
        let path = &object.relative_path;

        let lines: Vec<&str> = source.lines().collect();
        let paragraphs = source
            .split("\n\n")
            .filter(|p| !p.trim().is_empty())
            .count();
        let preview: String = source.chars().take(240).collect();

        let mut headings = Vec::new();
        for (i, line) in lines.iter().enumerate().take(200) {
            let t = line.trim();
            if t.len() >= 3
                && t.len() < 80
                && t.bytes().all(|b| {
                    b.is_ascii_uppercase() || b.is_ascii_whitespace() || b == b'-' || b == b':'
                })
                && t.chars().any(|c| c.is_alphabetic())
            {
                headings.push((i + 1, t.to_string()));
            }
        }

        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "text".into());

        let mut attr = attrs(&[
            ("lines", &lines.len().to_string()),
            ("paragraphs", &paragraphs.to_string()),
        ]);
        attr.insert("preview".into(), preview);

        let doc = entity(
            "document",
            &name,
            path,
            &object.id,
            &provider,
            1,
            attr,
            Confidence::Medium,
        );

        let mut entities = vec![doc];
        for (line, text) in headings.into_iter().take(20) {
            entities.push(entity(
                "heading",
                &text,
                path,
                &object.id,
                &provider,
                line as u32,
                attrs(&[("detected", "uppercase")]),
                Confidence::Low,
            ));
        }

        Ok(AnalysisResult {
            record: KnowledgeRecord {
                entities,
                relationships: Vec::new(),
                diagnostics: Vec::new(),
                status: Some(UnderstandingStatus::PartiallyUnderstood),
                classification_reason: Some("plain text metadata and preview".into()),
            },
            status: UnderstandingStatus::PartiallyUnderstood,
            classification_reason: Some("plain text metadata and preview".into()),
        })
    }
}
