//! Slides in package order, plus speaker notes when a notes part is linked.

use super::opc::{
    attr_local, attr_rel_id, cap_text, local_is, local_name, OpcError, OpcPackage, MAX_XML_DEPTH,
};
use quick_xml::events::Event;
use quick_xml::Reader;

const SLIDE_CAP: usize = 32;

#[derive(Debug)]
pub struct Deck {
    pub part: String,
    pub slides: Vec<Slide>,
}

#[derive(Debug)]
pub struct Slide {
    pub id: String,
    pub index: u32,
    pub part: String,
    pub title: String,
    pub notes: Option<String>,
}

pub fn read_deck(package: &OpcPackage) -> Result<Deck, OpcError> {
    let (part, bytes) = super::part_named(package, "ppt/presentation.xml")
        .ok_or_else(|| OpcError::Xml("missing presentation part".into()))?;
    let rels = package.rels.get(&super::rels_path(part));
    let mut slides = Vec::new();
    for (offset, declared) in slide_ids(bytes)?.into_iter().enumerate() {
        if slides.len() >= SLIDE_CAP {
            break;
        }
        let slide_part = rels
            .and_then(|rels| rels.iter().find(|rel| rel.id == declared.rel))
            .map(|rel| rel.target.clone())
            .unwrap_or_default();
        let (title, notes) = if let Some(body) = package.parts.get(&slide_part) {
            let title = slide_title(body)?;
            let notes = notes_text(package, &slide_part)?;
            (title, notes)
        } else {
            (String::new(), None)
        };
        slides.push(Slide {
            id: declared.id,
            index: (offset as u32) + 1,
            part: slide_part,
            title,
            notes,
        });
    }
    Ok(Deck {
        part: part.to_string(),
        slides,
    })
}

struct SlideId {
    id: String,
    rel: String,
}

fn slide_ids(bytes: &[u8]) -> Result<Vec<SlideId>, OpcError> {
    let mut slides = Vec::new();
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Err(OpcError::Limit("xml depth".into()));
                }
                if local_name(event.name().as_ref()) == b"sldId" && slides.len() < SLIDE_CAP {
                    slides.push(slide_id(&event));
                }
            }
            Ok(Event::Empty(event)) => {
                if local_name(event.name().as_ref()) == b"sldId" && slides.len() < SLIDE_CAP {
                    slides.push(slide_id(&event));
                }
            }
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) => break,
            Err(err) => return Err(OpcError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    Ok(slides)
}

fn slide_id(event: &quick_xml::events::BytesStart<'_>) -> SlideId {
    SlideId {
        id: attr_local(event, "id").unwrap_or_default(),
        rel: attr_rel_id(event).unwrap_or_default(),
    }
}

fn slide_title(bytes: &[u8]) -> Result<String, OpcError> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut sp_depth = 0usize;
    let mut title_shape: Option<usize> = None;
    let mut title = String::new();
    let mut fallback = String::new();
    let mut capture_title = false;
    let mut capture_fallback = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Err(OpcError::Limit("xml depth".into()));
                }
                if local_is(event.name().as_ref(), b"sp") {
                    sp_depth += 1;
                } else if local_is(event.name().as_ref(), b"ph")
                    && is_title_placeholder(attr_local(&event, "type").as_deref())
                {
                    title_shape = Some(sp_depth);
                } else if local_is(event.name().as_ref(), b"t") {
                    if title_shape == Some(sp_depth) {
                        capture_title = true;
                    } else if fallback.is_empty() {
                        capture_fallback = true;
                    }
                }
            }
            Ok(Event::Empty(event)) => {
                if local_is(event.name().as_ref(), b"ph")
                    && is_title_placeholder(attr_local(&event, "type").as_deref())
                {
                    title_shape = Some(sp_depth);
                }
            }
            Ok(Event::Text(event)) => {
                if capture_title || capture_fallback {
                    let piece = event
                        .xml10_content()
                        .map_err(|err| OpcError::Xml(err.to_string()))?;
                    if capture_title {
                        title.push_str(&piece);
                    } else {
                        fallback.push_str(&piece);
                    }
                }
            }
            Ok(Event::End(event)) => {
                if local_is(event.name().as_ref(), b"t") {
                    capture_title = false;
                    capture_fallback = false;
                } else if local_is(event.name().as_ref(), b"sp") {
                    if title_shape == Some(sp_depth) {
                        title_shape = None;
                    }
                    sp_depth = sp_depth.saturating_sub(1);
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(OpcError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    let chosen = if title.trim().is_empty() {
        fallback
    } else {
        title
    };
    Ok(cap_text(&chosen))
}

fn notes_text(package: &OpcPackage, slide_part: &str) -> Result<Option<String>, OpcError> {
    let Some(rels) = package.rels.get(&super::rels_path(slide_part)) else {
        return Ok(None);
    };
    let Some(target) = rels
        .iter()
        .find(|rel| rel.target.contains("notesSlide"))
        .map(|rel| rel.target.clone())
    else {
        return Ok(None);
    };
    let Some(bytes) = package.parts.get(&target) else {
        return Ok(None);
    };
    let mut reader = Reader::from_reader(bytes.as_slice());
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut capture = false;
    let mut notes = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Err(OpcError::Limit("xml depth".into()));
                }
                if local_name(event.name().as_ref()) == b"t" {
                    if !notes.is_empty() {
                        notes.push(' ');
                    }
                    capture = true;
                }
            }
            Ok(Event::Text(event)) => {
                if capture {
                    let piece = event
                        .xml10_content()
                        .map_err(|err| OpcError::Xml(err.to_string()))?;
                    notes.push_str(&piece);
                }
            }
            Ok(Event::End(event)) => {
                if local_name(event.name().as_ref()) == b"t" {
                    capture = false;
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(OpcError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    let notes = cap_text(&notes);
    if notes.is_empty() {
        Ok(None)
    } else {
        Ok(Some(notes))
    }
}

fn is_title_placeholder(kind: Option<&str>) -> bool {
    matches!(kind, Some("title" | "ctrTitle"))
}
