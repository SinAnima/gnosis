//! Workbook sheets and formula source text. Cached cell values are not stored.

use super::formula::{formula_is_external, formula_refs, FormulaRef};
use super::opc::{
    attr_local, attr_rel_id, cap_text, local_is, OpcError, OpcPackage, MAX_XML_DEPTH,
};
use quick_xml::events::Event;
use quick_xml::Reader;

const SHEET_CAP: usize = 16;
const FORMULA_CAP: usize = 32;

#[derive(Debug)]
pub struct Workbook {
    pub part: String,
    pub sheets: Vec<Sheet>,
}

#[derive(Debug)]
pub struct Sheet {
    pub name: String,
    pub visibility: String,
    pub part: String,
    pub index: u32,
    pub formulas: Vec<FormulaCell>,
}

#[derive(Debug)]
pub struct FormulaCell {
    pub cell: String,
    pub source: String,
    pub refs: Vec<FormulaRef>,
    pub external: bool,
}

pub fn read_workbook(package: &OpcPackage) -> Result<Workbook, OpcError> {
    let (part, bytes) = super::part_named(package, "xl/workbook.xml")
        .ok_or_else(|| OpcError::Xml("missing workbook part".into()))?;
    let rels = package.rels.get(&super::rels_path(part));
    let declared = sheet_decls(bytes)?;
    let mut sheets = Vec::new();
    let mut remaining = FORMULA_CAP;
    for (offset, decl) in declared.into_iter().enumerate() {
        if sheets.len() >= SHEET_CAP {
            break;
        }
        let sheet_part = rels
            .and_then(|rels| rels.iter().find(|rel| rel.id == decl.id))
            .map(|rel| rel.target.clone())
            .unwrap_or_default();
        let formulas = if remaining > 0 {
            if let Some(body) = package.parts.get(&sheet_part) {
                let found = sheet_formulas(body, remaining)?;
                remaining = remaining.saturating_sub(found.len());
                found
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        sheets.push(Sheet {
            name: decl.name,
            visibility: decl.visibility,
            part: sheet_part,
            index: (offset as u32) + 1,
            formulas,
        });
    }
    Ok(Workbook {
        part: part.to_string(),
        sheets,
    })
}

struct SheetDecl {
    name: String,
    id: String,
    visibility: String,
}

fn sheet_decls(bytes: &[u8]) -> Result<Vec<SheetDecl>, OpcError> {
    let mut sheets = Vec::new();
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
                if local_eq(event.name().as_ref(), "sheet") && sheets.len() < SHEET_CAP {
                    sheets.push(decl_from(&event));
                }
            }
            Ok(Event::Empty(event)) => {
                if local_eq(event.name().as_ref(), "sheet") && sheets.len() < SHEET_CAP {
                    sheets.push(decl_from(&event));
                }
            }
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) => break,
            Err(err) => return Err(OpcError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    Ok(sheets)
}

fn decl_from(event: &quick_xml::events::BytesStart<'_>) -> SheetDecl {
    SheetDecl {
        name: cap_text(&attr_local(event, "name").unwrap_or_default()),
        id: attr_rel_id(event).unwrap_or_default(),
        visibility: visibility(attr_local(event, "state").as_deref()),
    }
}

fn visibility(state: Option<&str>) -> String {
    match state.unwrap_or("visible") {
        value if value.eq_ignore_ascii_case("hidden") => "hidden".into(),
        value
            if value.eq_ignore_ascii_case("veryhidden")
                || value.eq_ignore_ascii_case("very_hidden") =>
        {
            "very_hidden".into()
        }
        _ => "visible".into(),
    }
}

fn sheet_formulas(bytes: &[u8], cap: usize) -> Result<Vec<FormulaCell>, OpcError> {
    let mut formulas = Vec::new();
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut cell = String::new();
    let mut source = String::new();
    let mut in_formula = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Err(OpcError::Limit("xml depth".into()));
                }
                if local_is(event.name().as_ref(), b"c") {
                    cell = attr_local(&event, "r").unwrap_or_default();
                    source.clear();
                } else if local_is(event.name().as_ref(), b"f") && formulas.len() < cap {
                    in_formula = true;
                    source.clear();
                }
            }
            Ok(Event::Empty(event)) => {
                if local_is(event.name().as_ref(), b"c") {
                    cell = attr_local(&event, "r").unwrap_or_default();
                    source.clear();
                }
            }
            Ok(Event::Text(event)) => {
                if in_formula {
                    let piece = event
                        .xml10_content()
                        .map_err(|err| OpcError::Xml(err.to_string()))?;
                    source.push_str(&piece);
                }
            }
            Ok(Event::End(event)) => {
                if local_is(event.name().as_ref(), b"f") {
                    in_formula = false;
                } else if local_is(event.name().as_ref(), b"c")
                    && !source.trim().is_empty()
                    && formulas.len() < cap
                {
                    let text = cap_text(&source);
                    let refs = formula_refs(&text);
                    let external = formula_is_external(&text, &refs);
                    let cell_name = if cell.is_empty() {
                        format!("{}", formulas.len() + 1)
                    } else {
                        cell.clone()
                    };
                    formulas.push(FormulaCell {
                        cell: cell_name,
                        source: text,
                        refs,
                        external,
                    });
                    source.clear();
                    cell.clear();
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(OpcError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    Ok(formulas)
}

fn local_eq(name: &[u8], expected: &str) -> bool {
    local_is(name, expected.as_bytes())
}
