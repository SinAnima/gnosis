use crate::providers::util::{attrs, entity};
use crate::{
    AnalysisResult, Confidence, ContentReader, KnowledgeRecord, ObjectDescriptor, ProtoData,
    ProviderId, Result, Support, UnderstandingProvider, UnderstandingStatus,
};

/// Fallback provider: always weakly supports any object; extracts ProtoData-backed metadata only.
pub struct GenericMetadataProvider;

impl UnderstandingProvider for GenericMetadataProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("generic-metadata")
    }

    fn probe(&self, _object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        Support::Weak
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        proto: &ProtoData,
        _content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        let provider = self.id();
        let path = &object.relative_path;
        let name = proto.filename.clone();

        let candidate = candidate_category(object, proto);
        let mut attr = attrs(&[
            ("media_type", &object.media_type),
            ("size", &object.size.to_string()),
            ("candidate_provider", &candidate),
        ]);
        if let Some(fp) = &proto.fingerprint {
            attr.insert("fingerprint".into(), fp.clone());
        }
        if let Some(ext) = &object.extension {
            attr.insert("extension".into(), ext.clone());
        }

        let reason =
            format!("no specialized provider; recorded metadata only (candidate: {candidate})");

        let e = entity(
            "unknown_object",
            &name,
            path,
            &object.id,
            &provider,
            1,
            attr,
            Confidence::Low,
        );

        let status = if object.media_type == "application/octet-stream"
            || object.extension.is_none()
            || is_unparsed_asset(object)
        {
            UnderstandingStatus::Unknown
        } else {
            UnderstandingStatus::PartiallyUnderstood
        };

        Ok(AnalysisResult {
            record: KnowledgeRecord {
                entities: vec![e],
                relationships: Vec::new(),
                diagnostics: Vec::new(),
                status: Some(status),
                classification_reason: Some(reason.clone()),
            },
            status,
            classification_reason: Some(reason),
        })
    }
}

fn is_unparsed_asset(object: &ObjectDescriptor) -> bool {
    matches!(
        object.extension.as_deref(),
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
                | "avif"
                | "psd"
                | "woff"
                | "woff2"
                | "ico"
        )
    ) || {
        let media = object.media_type.split(';').next().unwrap_or("").trim();
        media.eq_ignore_ascii_case("text/css")
            || media.eq_ignore_ascii_case("text/javascript")
            || media.eq_ignore_ascii_case("application/javascript")
            || media.starts_with("image/")
            || media.starts_with("font/")
    }
}

fn candidate_category(object: &ObjectDescriptor, _proto: &ProtoData) -> String {
    let ext = object.extension.as_deref().unwrap_or("");
    match ext {
        "pdf" => "pdf-provider".into(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "psd" | "svg" => "image-provider".into(),
        "mp3" | "wav" | "flac" => "audio-provider".into(),
        "mp4" | "mov" | "webm" => "video-provider".into(),
        "zip" | "tar" | "gz" | "tgz" | "7z" => "archive-provider".into(),
        "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" => "office-provider".into(),
        "go" => "future-treesitter".into(),
        _ if object.media_type.starts_with("text/") => "text-provider".into(),
        _ => "binary-or-unknown".into(),
    }
}
