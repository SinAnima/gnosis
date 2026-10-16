//! Bounded OPC/ZIP reader. Parts that escape the package are rejected.
//! `vbaProject.bin` is never loaded.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};

use quick_xml::events::Event;
use quick_xml::Reader;

pub const MAX_ENTRIES: usize = 256;
pub const MAX_PART: u64 = 4 * 1024 * 1024;
pub const MAX_TOTAL: u64 = 16 * 1024 * 1024;
pub const MAX_XML_DEPTH: usize = 32;
const MAX_TEXT: usize = 1024;

#[derive(Debug)]
pub enum OpcError {
    NotZip,
    Encrypted,
    UnsafePath(String),
    Duplicate(String),
    Limit(String),
    Xml(String),
}

impl std::fmt::Display for OpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotZip => write!(f, "opc package unreadable"),
            Self::Encrypted => write!(f, "encrypted"),
            Self::UnsafePath(path) => write!(f, "unsafe path {path}"),
            Self::Duplicate(path) => write!(f, "duplicate part {path}"),
            Self::Limit(what) => write!(f, "opc limit {what}"),
            Self::Xml(what) => write!(f, "opc xml {what}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Rel {
    pub id: String,
    pub target: String,
    pub external: bool,
}

#[derive(Clone, Debug, Default)]
pub struct CoreProps {
    pub title: Option<String>,
    pub creator: Option<String>,
    pub subject: Option<String>,
    pub modified: Option<String>,
    pub revision: Option<String>,
    pub application: Option<String>,
}

#[derive(Debug)]
pub struct OpcPackage {
    pub parts: BTreeMap<String, Vec<u8>>,
    pub rels: BTreeMap<String, Vec<Rel>>,
    pub skipped_vba: bool,
}

pub fn open_opc(bytes: &[u8]) -> Result<OpcPackage, OpcError> {
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        return Err(OpcError::Encrypted);
    }
    if bytes.len() < 4 || !bytes.starts_with(b"PK") {
        return Err(OpcError::NotZip);
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| OpcError::NotZip)?;
    if archive.len() > MAX_ENTRIES {
        return Err(OpcError::Limit("entry count".into()));
    }
    let mut planned: Vec<(usize, String, u64)> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut skipped_vba = false;
    let mut total = 0u64;
    for index in 0..archive.len() {
        let file = archive
            .by_index_raw(index)
            .map_err(|err| OpcError::Xml(err.to_string()))?;
        if file.encrypted() || file.name().eq_ignore_ascii_case("EncryptedPackage") {
            return Err(OpcError::Encrypted);
        }
        let name = safe_zip_name(file.name())?;
        if name.is_empty() {
            continue;
        }
        if !seen.insert(name.clone()) {
            return Err(OpcError::Duplicate(name));
        }
        if name.to_ascii_lowercase().ends_with("vbaproject.bin") {
            skipped_vba = true;
            continue;
        }
        let declared = file.size();
        if declared > MAX_PART {
            return Err(OpcError::Limit("uncompressed part".into()));
        }
        total = total.saturating_add(declared);
        if total > MAX_TOTAL {
            return Err(OpcError::Limit("uncompressed total".into()));
        }
        planned.push((index, name, declared));
    }
    let mut parts = BTreeMap::new();
    for (index, name, declared) in planned {
        let mut file = archive
            .by_index(index)
            .map_err(|err| OpcError::Xml(err.to_string()))?;
        let mut buf = Vec::new();
        file.by_ref()
            .take(declared.saturating_add(1))
            .read_to_end(&mut buf)
            .map_err(|err| OpcError::Xml(err.to_string()))?;
        if buf.len() as u64 > MAX_PART {
            return Err(OpcError::Limit("uncompressed part".into()));
        }
        parts.insert(name, buf);
    }
    if let Some(bytes) = parts.get("[Content_Types].xml") {
        let _ = element_texts(bytes)?;
        if bytes
            .windows(b"EncryptedPackage".len())
            .any(|window| window == b"EncryptedPackage")
        {
            return Err(OpcError::Encrypted);
        }
    }
    let rels = parse_all_rels(&parts)?;
    Ok(OpcPackage {
        parts,
        rels,
        skipped_vba,
    })
}

pub fn core_props(package: &OpcPackage) -> Result<CoreProps, OpcError> {
    let mut props = CoreProps::default();
    if let Some(bytes) = package.parts.get("docProps/core.xml") {
        let texts = element_texts(bytes)?;
        props.title = texts.get("title").cloned();
        props.creator = texts.get("creator").cloned();
        props.subject = texts.get("subject").cloned();
        props.modified = texts.get("modified").cloned();
        props.revision = texts.get("revision").cloned();
    }
    if let Some(bytes) = package.parts.get("docProps/app.xml") {
        let texts = element_texts(bytes)?;
        props.application = texts.get("application").cloned();
    }
    Ok(props)
}

pub fn resolve_target(rels_path: &str, target: &str) -> Result<String, OpcError> {
    if target.contains("://") {
        return Ok(target.to_string());
    }
    if target.starts_with('/') || target.starts_with('\\') {
        return normalize_path("", &target[1..]);
    }
    normalize_path(&rels_base(rels_path), target)
}

fn rels_base(rels_path: &str) -> String {
    if let Some(index) = rels_path.find("/_rels/") {
        rels_path[..index].to_string()
    } else {
        String::new()
    }
}

fn normalize_path(base: &str, target: &str) -> Result<String, OpcError> {
    let mut parts: Vec<&str> = Vec::new();
    if !base.is_empty() {
        parts.extend(base.split('/').filter(|part| !part.is_empty()));
    }
    for part in target.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if parts.pop().is_none() {
                return Err(OpcError::UnsafePath(target.into()));
            }
            continue;
        }
        if part.contains(':') || part.contains('\0') {
            return Err(OpcError::UnsafePath(target.into()));
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

fn safe_zip_name(name: &str) -> Result<String, OpcError> {
    if name.ends_with('/') {
        return Ok(String::new());
    }
    normalize_path("", name)
}

fn parse_all_rels(
    parts: &BTreeMap<String, Vec<u8>>,
) -> Result<BTreeMap<String, Vec<Rel>>, OpcError> {
    let mut rels = BTreeMap::new();
    for (name, bytes) in parts {
        if !name.ends_with(".rels") {
            continue;
        }
        rels.insert(name.clone(), parse_rels(name, bytes)?);
    }
    Ok(rels)
}

fn parse_rels(rels_path: &str, bytes: &[u8]) -> Result<Vec<Rel>, OpcError> {
    let mut out = Vec::new();
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
                if local_name(event.name().as_ref()) == b"Relationship" {
                    out.push(relationship(rels_path, &event)?);
                }
            }
            Ok(Event::Empty(event)) => {
                if local_name(event.name().as_ref()) == b"Relationship" {
                    out.push(relationship(rels_path, &event)?);
                }
            }
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) => break,
            Err(err) => return Err(OpcError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

fn relationship(
    rels_path: &str,
    event: &quick_xml::events::BytesStart<'_>,
) -> Result<Rel, OpcError> {
    let id = attr_local(event, "Id").unwrap_or_default();
    let target = attr_local(event, "Target").unwrap_or_default();
    let mode = attr_local(event, "TargetMode").unwrap_or_default();
    let external = mode.eq_ignore_ascii_case("External") || target.contains("://");
    let stored = if external {
        target
    } else {
        resolve_target(rels_path, &target)?
    };
    Ok(Rel {
        id,
        target: cap_text(&stored),
        external,
    })
}

pub fn element_texts(bytes: &[u8]) -> Result<BTreeMap<String, String>, OpcError> {
    let mut texts = BTreeMap::new();
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut current = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                if stack.len() >= MAX_XML_DEPTH {
                    return Err(OpcError::Limit("xml depth".into()));
                }
                stack.push(local_name_string(event.name().as_ref()));
                current.clear();
            }
            Ok(Event::Empty(event)) => {
                let name = local_name_string(event.name().as_ref());
                texts.entry(name).or_default();
            }
            Ok(Event::Text(event)) => {
                let text = event
                    .xml10_content()
                    .map_err(|err| OpcError::Xml(err.to_string()))?;
                current.push_str(&text);
            }
            Ok(Event::End(_)) => {
                if let Some(name) = stack.pop() {
                    let value = cap_text(current.trim());
                    if !value.is_empty() {
                        texts.entry(name).or_insert(value);
                    }
                    current.clear();
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(OpcError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    Ok(texts)
}

pub fn local_is(name: &[u8], expected: &[u8]) -> bool {
    local_name(name) == expected
}

pub fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':' || *byte == b'}')
        .next()
        .unwrap_or(name)
}

fn local_name_string(name: &[u8]) -> String {
    String::from_utf8_lossy(local_name(name)).to_ascii_lowercase()
}

pub fn attr_rel_id(event: &quick_xml::events::BytesStart<'_>) -> Option<String> {
    for attribute in event.attributes().flatten() {
        let key = attribute.key.as_ref();
        if key.ends_with(b":id") {
            return attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(|value| cap_text(&value));
        }
    }
    None
}

pub fn attr_local(event: &quick_xml::events::BytesStart<'_>, local: &str) -> Option<String> {
    let mut namespaced = None;
    for attribute in event.attributes().flatten() {
        let key = attribute.key.as_ref();
        if local_name(key) != local.as_bytes() {
            continue;
        }
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .ok()
            .map(|value| cap_text(&value));
        if !key.contains(&b':') {
            return value;
        }
        if namespaced.is_none() {
            namespaced = value;
        }
    }
    namespaced
}

pub fn cap_text(value: &str) -> String {
    let mut text = value.trim().to_string();
    if text.len() > MAX_TEXT {
        text.truncate(MAX_TEXT);
    }
    text
}
