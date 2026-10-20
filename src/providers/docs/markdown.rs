use crate::providers::util::{attrs, entity, ext_is, relationship};
use crate::{
    AnalysisResult, Confidence, ContentReader, KnowledgeRecord, ObjectDescriptor, ProtoData,
    ProviderId, Result, Support, UnderstandingProvider, UnderstandingStatus,
};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

pub struct MarkdownProvider;

impl UnderstandingProvider for MarkdownProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("markdown")
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        if ext_is(&object.extension, &["md", "markdown", "mdx"]) {
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
        let source = String::from_utf8_lossy(&buf);
        let provider = self.id();
        let path = &object.relative_path;

        let mut title = None;
        let mut headings = Vec::new();
        let mut links = Vec::new();
        let mut code_langs = Vec::new();
        let mut front_matter = None;

        let body = if source.starts_with("---") {
            if let Some(end) = source[3..].find("---") {
                front_matter = Some(source[3..3 + end].trim().to_string());
                &source[3 + end + 3..]
            } else {
                source.as_ref()
            }
        } else {
            source.as_ref()
        };

        let parser = Parser::new_ext(body, Options::all());
        let mut heading_level = None;
        let mut heading_text = String::new();
        let mut in_heading = false;
        let mut link_dest = None;

        for event in parser {
            match event {
                Event::Start(Tag::Heading { level, .. }) => {
                    in_heading = true;
                    heading_level = Some(level);
                    heading_text.clear();
                }
                Event::End(TagEnd::Heading(_)) => {
                    if let Some(level) = heading_level.take() {
                        if title.is_none() {
                            title = Some(heading_text.clone());
                        }
                        headings.push((level as u8, heading_text.clone()));
                    }
                    in_heading = false;
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    link_dest = Some(dest_url.to_string());
                }
                Event::End(TagEnd::Link) => {
                    if let Some(dest) = link_dest.take() {
                        links.push(dest);
                    }
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    if let pulldown_cmark::CodeBlockKind::Fenced(lang) = kind {
                        if !lang.is_empty() {
                            code_langs.push(lang.to_string());
                        }
                    }
                }
                Event::Text(t) | Event::Code(t) => {
                    if in_heading {
                        heading_text.push_str(&t);
                    }
                }
                _ => {}
            }
        }

        let doc_name = title.clone().unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "document".into())
        });

        let path_owned = path.to_string_lossy().into_owned();
        let attrs_map = {
            let mut m = attrs(&[("path", &path_owned)]);
            if let Some(ref fm) = front_matter {
                m.insert("has_front_matter".into(), "true".into());
                if fm.len() < 500 {
                    m.insert("front_matter_preview".into(), fm.clone());
                }
            }
            m.insert("heading_count".into(), headings.len().to_string());
            m.insert("link_count".into(), links.len().to_string());
            m
        };

        let doc = entity(
            "document",
            &doc_name,
            path,
            &object.id,
            &provider,
            1,
            attrs_map,
            Confidence::High,
        );

        let mut entities = vec![doc.clone()];
        let mut relationships = Vec::new();

        for (i, (level, text)) in headings.iter().enumerate() {
            let section = entity(
                "section",
                text,
                path,
                &object.id,
                &provider,
                (i as u32) + 1,
                attrs(&[("level", &level.to_string())]),
                Confidence::High,
            );
            relationships.push(relationship(
                "contains",
                &doc.id,
                &section.id,
                &provider,
                path,
                (i as u32) + 1,
                Confidence::High,
            ));
            entities.push(section);
        }

        for (i, link) in links.iter().take(50).enumerate() {
            let target = entity(
                "link",
                link,
                path,
                &object.id,
                &provider,
                1,
                attrs(&[("href", link), ("placeholder", "true")]),
                Confidence::Medium,
            );
            relationships.push(relationship(
                "links_to",
                &doc.id,
                &target.id,
                &provider,
                path,
                1,
                Confidence::Medium,
            ));
            entities.push(target);
            let _ = i;
        }

        for lang in code_langs.iter().take(20) {
            let block = entity(
                "code_block",
                lang,
                path,
                &object.id,
                &provider,
                1,
                attrs(&[("language", lang)]),
                Confidence::High,
            );
            relationships.push(relationship(
                "contains",
                &doc.id,
                &block.id,
                &provider,
                path,
                1,
                Confidence::High,
            ));
            entities.push(block);
        }

        Ok(AnalysisResult {
            record: KnowledgeRecord {
                entities,
                relationships,
                diagnostics: Vec::new(),
                status: Some(UnderstandingStatus::Understood),
                classification_reason: Some("markdown structure extracted".into()),
            },
            status: UnderstandingStatus::Understood,
            classification_reason: Some("markdown structure extracted".into()),
        })
    }
}
