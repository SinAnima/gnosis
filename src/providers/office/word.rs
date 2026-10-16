//! Headings and hyperlinks from a WordprocessingML document.

use super::opc::{
    attr_local, attr_rel_id, cap_text, local_name, OpcError, OpcPackage, MAX_XML_DEPTH,
};
use quick_xml::events::Event;
use quick_xml::Reader;

const HEADING_CAP: usize = 32;
const LINK_CAP: usize = 32;

#[derive(Debug)]
pub struct WordDoc {
    pub part: String,
    pub headings: Vec<Heading>,
    pub hyperlinks: Vec<String>,
}

#[derive(Debug)]
pub struct Heading {
    pub text: String,
    pub level: u8,
}

pub fn read_word(package: &OpcPackage) -> Result<WordDoc, OpcError> {
    let (part, bytes) = super::part_named(package, "word/document.xml")
        .ok_or_else(|| OpcError::Xml("missing document part".into()))?;
    let rels_path = super::rels_path(part);
    let rels = package.rels.get(&rels_path);
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut stack: Vec<String> = Vec::new();
    let mut in_del = 0usize;
    let mut run_hidden = false;
    let mut level: Option<u8> = None;
    let mut styled = false;
    let mut text = String::new();
    let mut capture = false;
    let mut headings = Vec::new();
    let mut hyperlinks = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Err(OpcError::Limit("xml depth".into()));
                }
                let name = local_name_owned(event.name().as_ref());
                if name == "p" {
                    level = None;
                    styled = false;
                    text.clear();
                } else if name == "del" {
                    in_del += 1;
                } else if name == "r" {
                    run_hidden = false;
                } else if name == "vanish"
                    && in_run(&stack)
                    && flag_on(attr_local(&event, "val").as_deref())
                {
                    run_hidden = true;
                } else if name == "pstyle" && in_pr(&stack) {
                    if let Some(parsed) =
                        heading_level(&attr_local(&event, "val").unwrap_or_default())
                    {
                        level = Some(parsed);
                        styled = true;
                    }
                } else if name == "outlinelvl" && in_pr(&stack) && !styled {
                    level = outline_level(attr_local(&event, "val").as_deref());
                } else if name == "hyperlink" {
                    push_link(&mut hyperlinks, rels, attr_rel_id(&event).as_deref());
                } else if (name == "t" || name == "deltext")
                    && in_del == 0
                    && !run_hidden
                    && level.is_some()
                {
                    capture = true;
                }
                stack.push(name);
            }
            Ok(Event::Empty(event)) => {
                let name = local_name_owned(event.name().as_ref());
                if name == "vanish"
                    && in_run(&stack)
                    && flag_on(attr_local(&event, "val").as_deref())
                {
                    run_hidden = true;
                } else if name == "pstyle" && in_pr(&stack) {
                    if let Some(parsed) =
                        heading_level(&attr_local(&event, "val").unwrap_or_default())
                    {
                        level = Some(parsed);
                        styled = true;
                    }
                } else if name == "outlinelvl" && in_pr(&stack) && !styled {
                    level = outline_level(attr_local(&event, "val").as_deref());
                } else if name == "hyperlink" {
                    push_link(&mut hyperlinks, rels, attr_rel_id(&event).as_deref());
                }
            }
            Ok(Event::Text(event)) => {
                if capture {
                    let piece = event
                        .xml10_content()
                        .map_err(|err| OpcError::Xml(err.to_string()))?;
                    text.push_str(&piece);
                }
            }
            Ok(Event::End(event)) => {
                let name = local_name_owned(event.name().as_ref());
                if name == "t" || name == "deltext" {
                    capture = false;
                } else if name == "p" {
                    let heading = cap_text(&text);
                    if let Some(level) = level.take() {
                        if !heading.is_empty() && headings.len() < HEADING_CAP {
                            headings.push(Heading {
                                text: heading,
                                level,
                            });
                        }
                    }
                    text.clear();
                    styled = false;
                } else if name == "del" {
                    in_del = in_del.saturating_sub(1);
                } else if name == "r" {
                    run_hidden = false;
                }
                stack.pop();
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(OpcError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    Ok(WordDoc {
        part: part.to_string(),
        headings,
        hyperlinks,
    })
}

fn push_link(links: &mut Vec<String>, rels: Option<&Vec<super::opc::Rel>>, id: Option<&str>) {
    let Some(id) = id else {
        return;
    };
    let Some(rels) = rels else {
        return;
    };
    if links.len() >= LINK_CAP {
        return;
    }
    if let Some(rel) = rels.iter().find(|rel| rel.id == id && rel.external) {
        if !rel.target.is_empty() && !links.iter().any(|existing| existing == &rel.target) {
            links.push(rel.target.clone());
        }
    }
}

fn in_pr(stack: &[String]) -> bool {
    stack.iter().any(|name| name == "ppr")
}

fn in_run(stack: &[String]) -> bool {
    stack.iter().rev().any(|name| name == "r" || name == "rpr")
}

fn heading_level(style: &str) -> Option<u8> {
    let style = style.trim();
    let rest = style
        .get(..7)
        .filter(|prefix| prefix.eq_ignore_ascii_case("heading"))
        .and_then(|_| style.get(7..))?;
    let digits: String = rest.chars().filter(|ch| ch.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn outline_level(value: Option<&str>) -> Option<u8> {
    let level: u8 = value?.parse().ok()?;
    Some(level.saturating_add(1))
}

fn flag_on(value: Option<&str>) -> bool {
    !matches!(
        value.map(|value| value.to_ascii_lowercase()).as_deref(),
        Some("false" | "0" | "off")
    )
}

fn local_name_owned(name: &[u8]) -> String {
    String::from_utf8_lossy(local_name(name)).to_ascii_lowercase()
}
