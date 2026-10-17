//! Bounded classic-xref PDF reader.
//!
//! `lopdf` 0.42 pulls unmaintained `ttf-parser` (RUSTSEC-2026-0192), so this
//! parser stays in-tree. It reads a classic xref table, the Info dictionary,
//! and literal `Tj` / `TJ` strings. FlateDecode is inflated under the stream
//! cap. Xref streams, object streams, encryption, and actions stay unread.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;

const MAX_STREAM: u64 = 4 * 1024 * 1024;
const MAX_TEXT: usize = 1024;
const MAX_PAGES: usize = 32;
const MAX_BOOKMARKS: usize = 32;
const MAX_LINKS: usize = 32;
const MAX_FIELDS: usize = 16;
const MAX_ATTACHMENTS: usize = 8;
const MAX_FONTS: usize = 16;
const MAX_DEPTH: u32 = 32;
const MAX_COLLECTION: usize = 4096;
const MAX_XREF: usize = 8192;
const MAX_STRING: usize = 8192;

/// High half of PDFDocEncoding (bytes 0x80..=0xFF). Undefined slots are U+FFFD.
const PDF_DOC_HIGH: [u32; 128] = [
    0x2022, 0x2020, 0x2021, 0x2026, 0x2014, 0x2013, 0x0192, 0x2044, 0x2039, 0x203A, 0x2212, 0x2030,
    0x201E, 0x201C, 0x201D, 0x2018, 0x2019, 0x201A, 0x2122, 0xFB01, 0xFB02, 0x0141, 0x0152, 0x0160,
    0x0178, 0x017D, 0x0131, 0x0142, 0x0153, 0x0161, 0x017E, 0xFFFD, 0x20AC, 0x00A1, 0x00A2, 0x00A3,
    0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0xFFFD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x00BA, 0x00BB,
    0x00BC, 0x00BD, 0x00BE, 0x00BF, 0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF, 0x00D0, 0x00D1, 0x00D2, 0x00D3,
    0x00D4, 0x00D5, 0x00D6, 0x00D7, 0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB,
    0x00EC, 0x00ED, 0x00EE, 0x00EF, 0x00F0, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x00FE, 0x00FF,
];

#[derive(Debug)]
pub(crate) struct PdfDoc {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    pub creation_date: Option<String>,
    pub mod_date: Option<String>,
    pub page_count: Option<i64>,
    pub encrypted: bool,
    pub signature_present: bool,
    pub xmp_present: bool,
    pub pages: Vec<PdfPage>,
    pub bookmarks: Vec<String>,
    pub links: Vec<String>,
    pub fields: Vec<String>,
    pub attachments: Vec<String>,
    pub fonts: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct PdfPage {
    pub width: String,
    pub height: String,
    pub rotate: String,
    pub embedded_text: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PdfError {
    NotPdf,
    Truncated,
    Limit,
    XrefStream,
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PdfError::NotPdf => "pdf unreadable",
            PdfError::Truncated => "pdf truncated",
            PdfError::Limit => "pdf limit uncompressed stream",
            PdfError::XrefStream => "xref stream unread",
        })
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
enum Obj {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Name(String),
    Str(String),
    Array(Vec<Obj>),
    Dict(Dict),
    Stream { dict: Dict, data: Vec<u8> },
    Ref(u32, u16),
}

type Dict = BTreeMap<String, Obj>;

#[derive(Clone)]
struct Inherited {
    width: String,
    height: String,
    rotate: String,
    fonts: Vec<String>,
}

impl Default for Inherited {
    fn default() -> Self {
        Self {
            width: "0".into(),
            height: "0".into(),
            rotate: "0".into(),
            fonts: Vec::new(),
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    i: usize,
    xref: HashMap<u32, usize>,
    cache: HashMap<u32, Obj>,
    loading: HashSet<u32>,
    depth: u32,
    signature_present: bool,
    links: Vec<String>,
    fields: Vec<String>,
    attachments: Vec<String>,
    fonts: Vec<String>,
}

pub(crate) fn parse(bytes: &[u8]) -> Result<PdfDoc, PdfError> {
    if !has_header(bytes) {
        return Err(PdfError::NotPdf);
    }
    let mut parser = Parser {
        bytes,
        i: 0,
        xref: HashMap::new(),
        cache: HashMap::new(),
        loading: HashSet::new(),
        depth: 0,
        signature_present: false,
        links: Vec::new(),
        fields: Vec::new(),
        attachments: Vec::new(),
        fonts: Vec::new(),
    };
    let xref_at = parser.startxref_offset()?;
    let trailer = parser.parse_xref(xref_at)?;
    if trailer.contains_key("Encrypt") {
        return Ok(encrypted_doc());
    }
    parser.finish(trailer)
}

fn encrypted_doc() -> PdfDoc {
    PdfDoc {
        title: None,
        author: None,
        subject: None,
        creator: None,
        producer: None,
        creation_date: None,
        mod_date: None,
        page_count: None,
        encrypted: true,
        signature_present: false,
        xmp_present: false,
        pages: Vec::new(),
        bookmarks: Vec::new(),
        links: Vec::new(),
        fields: Vec::new(),
        attachments: Vec::new(),
        fonts: Vec::new(),
    }
}

fn has_header(bytes: &[u8]) -> bool {
    let end = bytes.len().min(1024);
    bytes[..end].windows(4).any(|window| window == b"%PDF")
}

impl<'a> Parser<'a> {
    fn finish(&mut self, trailer: Dict) -> Result<PdfDoc, PdfError> {
        let mut doc = encrypted_doc();
        doc.encrypted = false;
        if let Some(info) = trailer.get("Info").cloned() {
            let resolved = self.resolve(&info)?;
            if let Some(dict) = dict_owned(&resolved) {
                doc.title = self.string_of(&dict, "Title")?;
                doc.author = self.string_of(&dict, "Author")?;
                doc.subject = self.string_of(&dict, "Subject")?;
                doc.creator = self.string_of(&dict, "Creator")?;
                doc.producer = self.string_of(&dict, "Producer")?;
                doc.creation_date = self.string_of(&dict, "CreationDate")?;
                doc.mod_date = self.string_of(&dict, "ModDate")?;
            }
        }
        let root = trailer.get("Root").cloned().ok_or(PdfError::Truncated)?;
        let root_obj = self.resolve(&root)?;
        let catalog = dict_owned(&root_obj).ok_or(PdfError::Truncated)?;
        if let Some(meta) = catalog.get("Metadata").cloned() {
            match self.resolve(&meta)? {
                Obj::Stream { .. } | Obj::Dict(_) => doc.xmp_present = true,
                _ => {}
            }
        }
        let pages_ref = catalog.get("Pages").cloned().ok_or(PdfError::Truncated)?;
        let pages_obj = self.resolve(&pages_ref)?;
        let pages_dict = dict_owned(&pages_obj).ok_or(PdfError::Truncated)?;
        let declared = self.int_of(&pages_dict, "Count")?;
        self.walk_pages(
            pages_ref,
            Inherited::default(),
            &mut doc.pages,
            &mut HashSet::new(),
        )?;
        doc.page_count = Some(declared.unwrap_or(doc.pages.len() as i64));
        if let Some(outlines) = catalog.get("Outlines").cloned() {
            let resolved = self.resolve(&outlines)?;
            if let Some(dict) = dict_owned(&resolved) {
                if let Some(first) = dict.get("First").cloned() {
                    self.walk_outline(first, &mut HashSet::new(), &mut doc.bookmarks)?;
                }
            }
        }
        if let Some(form) = catalog.get("AcroForm").cloned() {
            let resolved = self.resolve(&form)?;
            if let Some(dict) = dict_owned(&resolved) {
                let fields = self.array_items(&dict, "Fields")?;
                let mut seen = HashSet::new();
                let mut scanned = 0u32;
                for field in fields {
                    self.walk_fields(field, &mut seen, &mut scanned)?;
                }
            }
        }
        if let Some(names) = catalog.get("Names").cloned() {
            let resolved = self.resolve(&names)?;
            if let Some(dict) = dict_owned(&resolved) {
                if let Some(embedded) = dict.get("EmbeddedFiles").cloned() {
                    self.walk_names(embedded, &mut HashSet::new(), 0)?;
                }
            }
        }
        doc.signature_present = self.signature_present;
        doc.links = std::mem::take(&mut self.links);
        doc.fields = std::mem::take(&mut self.fields);
        doc.attachments = std::mem::take(&mut self.attachments);
        doc.fonts = std::mem::take(&mut self.fonts);
        Ok(doc)
    }

    fn walk_pages(
        &mut self,
        obj: Obj,
        inherited: Inherited,
        pages: &mut Vec<PdfPage>,
        seen: &mut HashSet<u32>,
    ) -> Result<(), PdfError> {
        if pages.len() >= MAX_PAGES {
            return Ok(());
        }
        if let Obj::Ref(number, _) = &obj {
            if !seen.insert(*number) {
                return Ok(());
            }
        }
        let resolved = self.resolve(&obj)?;
        let dict = dict_owned(&resolved).ok_or(PdfError::Truncated)?;
        let kind = match dict.get("Type") {
            Some(Obj::Name(name)) => Some(name.clone()),
            _ => None,
        };
        let is_tree = kind.as_deref() == Some("Pages")
            || (kind.is_none() && dict.contains_key("Kids") && !dict.contains_key("Contents"));
        if is_tree {
            let next = self.inherit(&dict, inherited)?;
            let kids = self.array_items(&dict, "Kids")?;
            for kid in kids {
                if pages.len() >= MAX_PAGES {
                    break;
                }
                self.walk_pages(kid, next.clone(), pages, seen)?;
            }
            return Ok(());
        }
        let here = self.inherit(&dict, inherited)?;
        let text = self.page_text(&dict)?;
        self.remember_fonts(&here.fonts);
        self.collect_links(&dict)?;
        pages.push(PdfPage {
            width: here.width,
            height: here.height,
            rotate: here.rotate,
            embedded_text: text,
        });
        Ok(())
    }

    fn inherit(&mut self, dict: &Dict, parent: Inherited) -> Result<Inherited, PdfError> {
        let mut next = parent;
        if let Some((width, height)) = self.media_size(dict)? {
            next.width = fmt_num(width);
            next.height = fmt_num(height);
        }
        if let Some(rotate) = self.int_of(dict, "Rotate")? {
            next.rotate = rotate.to_string();
        }
        let fonts = self.font_names(dict)?;
        if !fonts.is_empty() {
            next.fonts = fonts;
        }
        Ok(next)
    }

    fn media_size(&mut self, dict: &Dict) -> Result<Option<(f64, f64)>, PdfError> {
        let Some(obj) = dict.get("MediaBox").cloned() else {
            return Ok(None);
        };
        let Obj::Array(items) = self.resolve(&obj)? else {
            return Ok(None);
        };
        if items.len() < 4 {
            return Ok(None);
        }
        let Some(x0) = self.number_of(&items[0])? else {
            return Ok(None);
        };
        let Some(y0) = self.number_of(&items[1])? else {
            return Ok(None);
        };
        let Some(x1) = self.number_of(&items[2])? else {
            return Ok(None);
        };
        let Some(y1) = self.number_of(&items[3])? else {
            return Ok(None);
        };
        Ok(Some(((x1 - x0).abs(), (y1 - y0).abs())))
    }

    fn page_text(&mut self, dict: &Dict) -> Result<Option<String>, PdfError> {
        let Some(obj) = dict.get("Contents").cloned() else {
            return Ok(None);
        };
        let mut combined = String::new();
        match self.resolve(&obj)? {
            Obj::Array(items) => {
                for item in items {
                    if combined.chars().count() >= MAX_TEXT {
                        break;
                    }
                    self.append_content(&item, &mut combined)?;
                }
            }
            other => self.append_content(&other, &mut combined)?,
        }
        if combined.is_empty() {
            Ok(None)
        } else {
            Ok(Some(combined))
        }
    }

    fn append_content(&mut self, obj: &Obj, out: &mut String) -> Result<(), PdfError> {
        if let Obj::Stream { dict, data } = self.resolve(obj)? {
            let decoded = self.decode_stream(&dict, &data)?;
            push_capped(out, &scan_text(&decoded));
        }
        Ok(())
    }

    fn decode_stream(&mut self, dict: &Dict, data: &[u8]) -> Result<Vec<u8>, PdfError> {
        if self.predicted(dict)? {
            return Ok(Vec::new());
        }
        let filters = self.filter_names(dict)?;
        match filters.as_slice() {
            [] => Ok(data.to_vec()),
            [name] if name == "FlateDecode" || name == "Fl" => inflate(data),
            _ => Ok(Vec::new()),
        }
    }

    fn predicted(&mut self, dict: &Dict) -> Result<bool, PdfError> {
        let Some(obj) = dict.get("DecodeParms").or_else(|| dict.get("DP")).cloned() else {
            return Ok(false);
        };
        match self.resolve(&obj)? {
            Obj::Dict(parms) => self.predictor_set(&parms),
            Obj::Array(items) => {
                for item in items {
                    if let Obj::Dict(parms) = self.resolve(&item)? {
                        if self.predictor_set(&parms)? {
                            return Ok(true);
                        }
                    }
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    fn predictor_set(&mut self, dict: &Dict) -> Result<bool, PdfError> {
        Ok(self.int_of(dict, "Predictor")?.unwrap_or(1) > 1)
    }

    fn filter_names(&mut self, dict: &Dict) -> Result<Vec<String>, PdfError> {
        let Some(obj) = dict.get("Filter").cloned() else {
            return Ok(Vec::new());
        };
        match self.resolve(&obj)? {
            Obj::Name(name) => Ok(vec![name]),
            Obj::Array(items) => {
                let mut names = Vec::new();
                for item in items {
                    if let Obj::Name(name) = self.resolve(&item)? {
                        names.push(name);
                    }
                }
                Ok(names)
            }
            _ => Ok(Vec::new()),
        }
    }

    fn font_names(&mut self, dict: &Dict) -> Result<Vec<String>, PdfError> {
        let Some(resources) = dict.get("Resources").cloned() else {
            return Ok(Vec::new());
        };
        let Some(resource_dict) = dict_owned(&self.resolve(&resources)?) else {
            return Ok(Vec::new());
        };
        let Some(font) = resource_dict.get("Font").cloned() else {
            return Ok(Vec::new());
        };
        let Some(font_dict) = dict_owned(&self.resolve(&font)?) else {
            return Ok(Vec::new());
        };
        let mut names = Vec::new();
        for value in font_dict.values() {
            if names.len() >= MAX_FONTS {
                break;
            }
            let Some(one) = dict_owned(&self.resolve(value)?) else {
                continue;
            };
            if let Some(base) = one.get("BaseFont").cloned() {
                if let Obj::Name(name) = self.resolve(&base)? {
                    if !name.is_empty() {
                        names.push(name);
                    }
                }
            }
        }
        Ok(names)
    }

    fn remember_fonts(&mut self, names: &[String]) {
        for name in names {
            if self.fonts.len() >= MAX_FONTS {
                return;
            }
            if !name.is_empty() && !self.fonts.iter().any(|have| have == name) {
                self.fonts.push(name.clone());
            }
        }
    }

    fn collect_links(&mut self, dict: &Dict) -> Result<(), PdfError> {
        let Some(obj) = dict.get("Annots").cloned() else {
            return Ok(());
        };
        let Obj::Array(items) = self.resolve(&obj)? else {
            return Err(PdfError::Truncated);
        };
        for item in items {
            let Some(annot) = dict_owned(&self.resolve(&item)?) else {
                continue;
            };
            self.note_signature(&annot)?;
            if self.links.len() >= MAX_LINKS {
                continue;
            }
            if self.name_is(&annot, "Subtype", "Link")? {
                if let Some(uri) = self.link_uri(&annot)? {
                    self.links.push(uri);
                }
            }
        }
        Ok(())
    }

    fn link_uri(&mut self, dict: &Dict) -> Result<Option<String>, PdfError> {
        if let Some(action) = dict.get("A").cloned() {
            if let Some(action_dict) = dict_owned(&self.resolve(&action)?) {
                if self.name_is(&action_dict, "S", "URI")? {
                    return self.string_of(&action_dict, "URI");
                }
                return Ok(None);
            }
        }
        self.string_of(dict, "URI")
    }

    fn walk_outline(
        &mut self,
        obj: Obj,
        seen: &mut HashSet<u32>,
        out: &mut Vec<String>,
    ) -> Result<(), PdfError> {
        if out.len() >= MAX_BOOKMARKS {
            return Ok(());
        }
        if let Obj::Ref(number, _) = &obj {
            if !seen.insert(*number) {
                return Ok(());
            }
        }
        let Some(dict) = dict_owned(&self.resolve(&obj)?) else {
            return Ok(());
        };
        if let Some(title) = self.string_of(&dict, "Title")? {
            if out.len() < MAX_BOOKMARKS {
                out.push(title);
            }
        }
        if let Some(first) = dict.get("First").cloned() {
            self.walk_outline(first, seen, out)?;
        }
        if let Some(next) = dict.get("Next").cloned() {
            self.walk_outline(next, seen, out)?;
        }
        Ok(())
    }

    fn walk_fields(
        &mut self,
        obj: Obj,
        seen: &mut HashSet<u32>,
        scanned: &mut u32,
    ) -> Result<(), PdfError> {
        if *scanned >= 64 {
            return Ok(());
        }
        *scanned += 1;
        if let Obj::Ref(number, _) = &obj {
            if !seen.insert(*number) {
                return Ok(());
            }
        }
        let Some(dict) = dict_owned(&self.resolve(&obj)?) else {
            return Ok(());
        };
        self.note_signature(&dict)?;
        if self.fields.len() < MAX_FIELDS {
            if let Some(name) = self.string_of(&dict, "T")? {
                self.fields.push(name);
            }
        }
        for kid in self.array_items(&dict, "Kids")? {
            self.walk_fields(kid, seen, scanned)?;
        }
        Ok(())
    }

    fn walk_names(
        &mut self,
        obj: Obj,
        seen: &mut HashSet<u32>,
        depth: u32,
    ) -> Result<(), PdfError> {
        if self.attachments.len() >= MAX_ATTACHMENTS || depth > 8 {
            return Ok(());
        }
        if let Obj::Ref(number, _) = &obj {
            if !seen.insert(*number) {
                return Ok(());
            }
        }
        let Some(dict) = dict_owned(&self.resolve(&obj)?) else {
            return Ok(());
        };
        if let Some(names) = dict.get("Names").cloned() {
            if let Obj::Array(items) = self.resolve(&names)? {
                let mut index = 0;
                while index < items.len() {
                    if self.attachments.len() >= MAX_ATTACHMENTS {
                        break;
                    }
                    if let Obj::Str(name) = &items[index] {
                        if !name.is_empty() {
                            self.attachments.push(name.clone());
                        }
                    }
                    index = index.saturating_add(2);
                }
            }
        }
        if let Some(kids) = dict.get("Kids").cloned() {
            if let Obj::Array(items) = self.resolve(&kids)? {
                for kid in items {
                    self.walk_names(kid, seen, depth + 1)?;
                }
            }
        }
        Ok(())
    }

    fn note_signature(&mut self, dict: &Dict) -> Result<(), PdfError> {
        if self.name_is(dict, "FT", "Sig")? || self.name_is(dict, "Type", "Sig")? {
            self.signature_present = true;
        }
        if let Some(value) = dict.get("V").cloned() {
            if let Some(signed) = dict_owned(&self.resolve(&value)?) {
                if self.name_is(&signed, "Type", "Sig")? || self.name_is(&signed, "FT", "Sig")? {
                    self.signature_present = true;
                }
            }
        }
        Ok(())
    }

    fn string_of(&mut self, dict: &Dict, key: &str) -> Result<Option<String>, PdfError> {
        let Some(obj) = dict.get(key).cloned() else {
            return Ok(None);
        };
        match self.resolve(&obj)? {
            Obj::Str(text) if !text.is_empty() => Ok(Some(text)),
            _ => Ok(None),
        }
    }

    fn name_is(&mut self, dict: &Dict, key: &str, expected: &str) -> Result<bool, PdfError> {
        let Some(obj) = dict.get(key).cloned() else {
            return Ok(false);
        };
        Ok(matches!(self.resolve(&obj)?, Obj::Name(name) if name == expected))
    }

    fn int_of(&mut self, dict: &Dict, key: &str) -> Result<Option<i64>, PdfError> {
        let Some(obj) = dict.get(key).cloned() else {
            return Ok(None);
        };
        match self.resolve(&obj)? {
            Obj::Int(value) => Ok(Some(value)),
            Obj::Real(value) if value.fract() == 0.0 && value.abs() < 1.0e15 => {
                Ok(Some(value as i64))
            }
            _ => Ok(None),
        }
    }

    fn number_of(&mut self, obj: &Obj) -> Result<Option<f64>, PdfError> {
        match self.resolve(obj)? {
            Obj::Int(value) => Ok(Some(value as f64)),
            Obj::Real(value) if value.is_finite() => Ok(Some(value)),
            _ => Ok(None),
        }
    }

    fn array_items(&mut self, dict: &Dict, key: &str) -> Result<Vec<Obj>, PdfError> {
        let Some(obj) = dict.get(key).cloned() else {
            return Ok(Vec::new());
        };
        match self.resolve(&obj)? {
            Obj::Array(items) => Ok(items),
            _ => Err(PdfError::Truncated),
        }
    }

    fn resolve(&mut self, obj: &Obj) -> Result<Obj, PdfError> {
        match obj {
            Obj::Ref(number, _) => self.load(*number),
            other => Ok(other.clone()),
        }
    }

    fn load(&mut self, number: u32) -> Result<Obj, PdfError> {
        if let Some(hit) = self.cache.get(&number) {
            return Ok(hit.clone());
        }
        if self.depth >= MAX_DEPTH {
            return Err(PdfError::Limit);
        }
        if !self.loading.insert(number) {
            return Err(PdfError::Truncated);
        }
        let saved = self.i;
        self.depth += 1;
        let parsed = self.read_object(number);
        self.depth -= 1;
        self.i = saved;
        self.loading.remove(&number);
        let obj = parsed?;
        self.cache.insert(number, obj.clone());
        Ok(obj)
    }

    fn read_object(&mut self, _number: u32) -> Result<Obj, PdfError> {
        let offset = *self.xref.get(&_number).ok_or(PdfError::Truncated)?;
        if offset > self.bytes.len() {
            return Err(PdfError::Truncated);
        }
        self.i = offset;
        self.skip_ws();
        let _id = self.parse_int()?;
        self.skip_ws();
        let _gen = self.parse_int()?;
        self.skip_ws();
        if !self.keyword_here(b"obj") {
            return Err(PdfError::Truncated);
        }
        self.i += 3;
        self.parse_value()
    }

    fn startxref_offset(&mut self) -> Result<usize, PdfError> {
        let marker = b"startxref";
        let at = self
            .bytes
            .windows(marker.len())
            .rposition(|window| window == marker)
            .ok_or(PdfError::Truncated)?;
        self.i = at + marker.len();
        let offset = self.parse_int()?;
        if offset < 0 {
            return Err(PdfError::Truncated);
        }
        let offset = usize::try_from(offset).map_err(|_| PdfError::Limit)?;
        if offset >= self.bytes.len() {
            return Err(PdfError::Truncated);
        }
        Ok(offset)
    }

    fn parse_xref(&mut self, at: usize) -> Result<Dict, PdfError> {
        self.i = at;
        self.skip_ws();
        if !self.keyword_here(b"xref") {
            return Err(PdfError::XrefStream);
        }
        self.i += 4;
        loop {
            self.skip_ws();
            if self.keyword_here(b"trailer") {
                self.i += b"trailer".len();
                self.skip_ws();
                if self.bytes.get(self.i) != Some(&b'<')
                    || self.bytes.get(self.i + 1) != Some(&b'<')
                {
                    return Err(PdfError::Truncated);
                }
                self.i += 2;
                return self.parse_dict_body();
            }
            let start = self.parse_int()?;
            let count = self.parse_int()?;
            if count < 0 || count > MAX_XREF as i64 {
                return Err(PdfError::Limit);
            }
            if start < 0 {
                return Err(PdfError::Truncated);
            }
            for step in 0..count {
                let offset = self.parse_int()?;
                let _gen = self.parse_int()?;
                self.skip_ws();
                let flag = self.bytes.get(self.i).copied().ok_or(PdfError::Truncated)?;
                self.i += 1;
                if flag != b'n' && flag != b'f' {
                    return Err(PdfError::Truncated);
                }
                let number = start + step;
                if flag == b'n' && number > 0 && offset >= 0 {
                    if self.xref.len() >= MAX_XREF {
                        return Err(PdfError::Limit);
                    }
                    let offset = usize::try_from(offset).map_err(|_| PdfError::Limit)?;
                    if offset <= self.bytes.len() {
                        self.xref.insert(number as u32, offset);
                    }
                }
            }
        }
    }

    fn parse_value(&mut self) -> Result<Obj, PdfError> {
        self.skip_ws();
        let byte = self.peek().ok_or(PdfError::Truncated)?;
        if byte == b'/' {
            return self.parse_name();
        }
        if byte == b'(' {
            let (raw, next) = read_literal(self.bytes, self.i)?;
            self.i = next;
            return Ok(Obj::Str(decode_pdf_string(&raw)));
        }
        if byte == b'<' {
            if self.bytes.get(self.i + 1) == Some(&b'<') {
                self.i += 2;
                let dict = self.parse_dict_body()?;
                self.skip_ws();
                if self.keyword_here(b"stream") {
                    let data = self.read_stream(&dict)?;
                    return Ok(Obj::Stream { dict, data });
                }
                return Ok(Obj::Dict(dict));
            }
            let (raw, next) = read_hex(self.bytes, self.i)?;
            self.i = next;
            return Ok(Obj::Str(decode_pdf_string(&raw)));
        }
        if byte == b'[' {
            return self.parse_array();
        }
        if self.keyword_here(b"true") {
            self.i += 4;
            return Ok(Obj::Bool(true));
        }
        if self.keyword_here(b"false") {
            self.i += 5;
            return Ok(Obj::Bool(false));
        }
        if self.keyword_here(b"null") {
            self.i += 4;
            return Ok(Obj::Null);
        }
        if byte == b'+' || byte == b'-' || byte == b'.' || byte.is_ascii_digit() {
            return self.parse_number_or_ref();
        }
        Err(PdfError::Truncated)
    }

    fn parse_number_or_ref(&mut self) -> Result<Obj, PdfError> {
        let number = self.parse_number()?;
        if !matches!(number, Obj::Int(_)) {
            return Ok(number);
        }
        let save = self.i;
        self.skip_ws();
        if let Ok(gen) = self.parse_number() {
            if let Obj::Int(generation) = gen {
                self.skip_ws();
                if self.keyword_here(b"R") {
                    self.i += 1;
                    if let Obj::Int(id) = number {
                        if id >= 0 && generation >= 0 && generation <= i64::from(u16::MAX) {
                            return Ok(Obj::Ref(id as u32, generation as u16));
                        }
                    }
                }
            }
        }
        self.i = save;
        Ok(number)
    }

    fn parse_array(&mut self) -> Result<Obj, PdfError> {
        self.i += 1;
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b']') {
                self.i += 1;
                return Ok(Obj::Array(items));
            }
            if items.len() >= MAX_COLLECTION {
                return Err(PdfError::Limit);
            }
            items.push(self.parse_value()?);
        }
    }

    fn parse_dict_body(&mut self) -> Result<Dict, PdfError> {
        let mut dict = Dict::new();
        loop {
            self.skip_ws();
            if self.bytes.get(self.i) == Some(&b'>') && self.bytes.get(self.i + 1) == Some(&b'>') {
                self.i += 2;
                return Ok(dict);
            }
            if dict.len() >= MAX_COLLECTION {
                return Err(PdfError::Limit);
            }
            let Obj::Name(key) = self.parse_value()? else {
                return Err(PdfError::Truncated);
            };
            let value = self.parse_value()?;
            dict.insert(key, value);
        }
    }

    fn parse_name(&mut self) -> Result<Obj, PdfError> {
        self.i += 1;
        let mut out = String::new();
        while self.i < self.bytes.len() {
            let byte = self.bytes[self.i];
            if byte.is_ascii_whitespace() || is_delim(byte) {
                break;
            }
            self.i += 1;
            let decoded = if byte == b'#' {
                let high = self.bytes.get(self.i).copied().ok_or(PdfError::Truncated)?;
                let low = self
                    .bytes
                    .get(self.i + 1)
                    .copied()
                    .ok_or(PdfError::Truncated)?;
                self.i += 2;
                (hex_val(high).ok_or(PdfError::Truncated)? << 4)
                    | hex_val(low).ok_or(PdfError::Truncated)?
            } else {
                byte
            };
            if out.len() < 256 {
                out.push(decoded as char);
            }
        }
        Ok(Obj::Name(out))
    }

    fn parse_number(&mut self) -> Result<Obj, PdfError> {
        let start = self.i;
        if matches!(self.peek(), Some(b'+' | b'-')) {
            self.i += 1;
        }
        let mut saw_digit = false;
        let mut saw_dot = false;
        loop {
            match self.peek() {
                Some(b'0'..=b'9') => {
                    saw_digit = true;
                    self.i += 1;
                }
                Some(b'.') if !saw_dot => {
                    saw_dot = true;
                    self.i += 1;
                }
                _ => break,
            }
        }
        if !saw_digit {
            self.i = start;
            return Err(PdfError::Truncated);
        }
        let text =
            std::str::from_utf8(&self.bytes[start..self.i]).map_err(|_| PdfError::Truncated)?;
        if saw_dot {
            let value = text.parse::<f64>().map_err(|_| PdfError::Truncated)?;
            Ok(Obj::Real(value))
        } else {
            let value = text.parse::<i64>().map_err(|_| PdfError::Limit)?;
            Ok(Obj::Int(value))
        }
    }

    fn parse_int(&mut self) -> Result<i64, PdfError> {
        let save = self.i;
        self.skip_ws();
        let start = self.i;
        if matches!(self.peek(), Some(b'+' | b'-')) {
            self.i += 1;
        }
        let digits = self.i;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        if self.i == digits {
            self.i = save;
            return Err(PdfError::Truncated);
        }
        let text =
            std::str::from_utf8(&self.bytes[start..self.i]).map_err(|_| PdfError::Truncated)?;
        text.parse::<i64>().map_err(|_| PdfError::Limit)
    }

    fn read_stream(&mut self, dict: &Dict) -> Result<Vec<u8>, PdfError> {
        if !self.keyword_here(b"stream") {
            return Err(PdfError::Truncated);
        }
        self.i += b"stream".len();
        if self.bytes.get(self.i) == Some(&b'\r') {
            self.i += 1;
        }
        if self.bytes.get(self.i) == Some(&b'\n') {
            self.i += 1;
        }
        let len = self.stream_length(dict)?;
        if len > MAX_STREAM {
            return Err(PdfError::Limit);
        }
        let len = usize::try_from(len).map_err(|_| PdfError::Limit)?;
        let end = self.i.checked_add(len).ok_or(PdfError::Limit)?;
        if end > self.bytes.len() {
            return Err(PdfError::Truncated);
        }
        let data = self.bytes[self.i..end].to_vec();
        self.i = end;
        Ok(data)
    }

    fn stream_length(&mut self, dict: &Dict) -> Result<u64, PdfError> {
        let Some(obj) = dict.get("Length").cloned() else {
            return Err(PdfError::Truncated);
        };
        match self.resolve(&obj)? {
            Obj::Int(value) if value >= 0 => Ok(value as u64),
            _ => Err(PdfError::Truncated),
        }
    }

    fn skip_ws(&mut self) {
        loop {
            if self.i >= self.bytes.len() {
                return;
            }
            let byte = self.bytes[self.i];
            if byte == b'%' {
                self.i += 1;
                while self.i < self.bytes.len() && self.bytes[self.i] != b'\n' {
                    self.i += 1;
                }
                continue;
            }
            if byte.is_ascii_whitespace() {
                self.i += 1;
                continue;
            }
            return;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.i).copied()
    }

    fn keyword_here(&self, word: &[u8]) -> bool {
        let end = self.i + word.len();
        if self.bytes.get(self.i..end) != Some(word) {
            return false;
        }
        match self.bytes.get(end) {
            None => true,
            Some(byte) => byte.is_ascii_whitespace() || is_delim(*byte),
        }
    }
}

fn dict_owned(obj: &Obj) -> Option<Dict> {
    match obj {
        Obj::Dict(dict) => Some(dict.clone()),
        Obj::Stream { dict, .. } => Some(dict.clone()),
        _ => None,
    }
}

fn inflate(data: &[u8]) -> Result<Vec<u8>, PdfError> {
    let decoder = flate2::read::ZlibDecoder::new(data);
    let mut limited = decoder.take(MAX_STREAM + 1);
    let mut out = Vec::new();
    match limited.read_to_end(&mut out) {
        Ok(_) if out.len() as u64 > MAX_STREAM => Err(PdfError::Limit),
        Ok(_) => Ok(out),
        Err(_) => Ok(Vec::new()),
    }
}

fn fmt_num(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1.0e15 {
        format!("{}", value as i64)
    } else if value.is_finite() {
        let text = format!("{value:.2}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        "0".into()
    }
}

fn push_capped(out: &mut String, extra: &str) {
    for ch in extra.chars() {
        if out.chars().count() >= MAX_TEXT {
            return;
        }
        out.push(ch);
    }
}

fn decode_pdf_string(raw: &[u8]) -> String {
    if raw.len() >= 2 && raw[0] == 0xFE && raw[1] == 0xFF {
        return decode_utf16(&raw[2..], true);
    }
    if raw.len() >= 2 && raw[0] == 0xFF && raw[1] == 0xFE {
        return decode_utf16(&raw[2..], false);
    }
    raw.iter().copied().map(decode_byte).collect()
}

fn decode_utf16(raw: &[u8], big: bool) -> String {
    let mut units = Vec::new();
    let mut index = 0;
    while index + 1 < raw.len() {
        let unit = if big {
            u16::from_be_bytes([raw[index], raw[index + 1]])
        } else {
            u16::from_le_bytes([raw[index], raw[index + 1]])
        };
        units.push(unit);
        index += 2;
    }
    String::from_utf16_lossy(&units)
}

fn decode_byte(byte: u8) -> char {
    let code = match byte {
        0x18 => 0x02D8,
        0x19 => 0x02C7,
        0x1A => 0x02C6,
        0x1B => 0x02D9,
        0x1C => 0x02DD,
        0x1D => 0x02DB,
        0x1E => 0x02DA,
        0x1F => 0x02DC,
        0x7F => 0xFFFD,
        0x80..=0xFF => PDF_DOC_HIGH[usize::from(byte) - 0x80],
        other => u32::from(other),
    };
    char::from_u32(code).unwrap_or('\u{FFFD}')
}

fn read_literal(data: &[u8], mut index: usize) -> Result<(Vec<u8>, usize), PdfError> {
    if data.get(index) != Some(&b'(') {
        return Err(PdfError::Truncated);
    }
    index += 1;
    let mut raw = Vec::new();
    let mut depth = 1i32;
    while index < data.len() {
        let byte = data[index];
        index += 1;
        if byte == b'\\' {
            if index >= data.len() {
                return Err(PdfError::Truncated);
            }
            let escaped = data[index];
            index += 1;
            match escaped {
                b'n' => push_raw(&mut raw, b'\n'),
                b'r' => push_raw(&mut raw, b'\r'),
                b't' => push_raw(&mut raw, b'\t'),
                b'b' => push_raw(&mut raw, 0x08),
                b'f' => push_raw(&mut raw, 0x0C),
                b'(' | b')' | b'\\' => push_raw(&mut raw, escaped),
                b'\n' => {}
                b'\r' => {
                    if data.get(index) == Some(&b'\n') {
                        index += 1;
                    }
                }
                b'0'..=b'7' => {
                    let mut value = u16::from(escaped - b'0');
                    let mut count = 1;
                    while count < 3 {
                        let Some(digit) = data.get(index).copied() else {
                            break;
                        };
                        if !digit.is_ascii_digit() || digit > b'7' {
                            break;
                        }
                        value = value * 8 + u16::from(digit - b'0');
                        index += 1;
                        count += 1;
                    }
                    push_raw(&mut raw, value as u8);
                }
                other => push_raw(&mut raw, other),
            }
            continue;
        }
        if byte == b'(' {
            depth += 1;
            push_raw(&mut raw, byte);
            continue;
        }
        if byte == b')' {
            depth -= 1;
            if depth == 0 {
                return Ok((raw, index));
            }
            push_raw(&mut raw, byte);
            continue;
        }
        push_raw(&mut raw, byte);
    }
    Err(PdfError::Truncated)
}

fn read_hex(data: &[u8], mut index: usize) -> Result<(Vec<u8>, usize), PdfError> {
    if data.get(index) != Some(&b'<') {
        return Err(PdfError::Truncated);
    }
    index += 1;
    let mut nibbles = Vec::new();
    while index < data.len() {
        let byte = data[index];
        index += 1;
        if byte == b'>' {
            if nibbles.len() % 2 == 1 {
                nibbles.push(0);
            }
            let mut raw = Vec::new();
            let mut pair = 0;
            while pair + 1 < nibbles.len() && raw.len() < MAX_STRING {
                raw.push((nibbles[pair] << 4) | nibbles[pair + 1]);
                pair += 2;
            }
            return Ok((raw, index));
        }
        if byte.is_ascii_whitespace() {
            continue;
        }
        let Some(nibble) = hex_val(byte) else {
            return Err(PdfError::Truncated);
        };
        if nibbles.len() < MAX_STRING * 2 {
            nibbles.push(nibble);
        }
    }
    Err(PdfError::Truncated)
}

fn push_raw(raw: &mut Vec<u8>, byte: u8) {
    if raw.len() < MAX_STRING {
        raw.push(byte);
    }
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_delim(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn scan_text(data: &[u8]) -> String {
    let mut index = 0;
    let mut out = String::new();
    let mut pending = String::new();
    let mut array = Vec::new();
    let mut depth = 0i32;
    while index < data.len() && out.chars().count() < MAX_TEXT {
        let byte = data[index];
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if byte == b'%' {
            while index < data.len() && data[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if byte == b'(' {
            match read_literal(data, index) {
                Ok((raw, next)) => {
                    index = next;
                    let text = decode_pdf_string(&raw);
                    if depth > 0 {
                        array.push(text);
                    } else {
                        pending = text;
                    }
                }
                Err(_) => break,
            }
            continue;
        }
        if byte == b'<' && data.get(index + 1) == Some(&b'<') {
            index = skip_dict(data, index);
            pending.clear();
            continue;
        }
        if byte == b'<' {
            match read_hex(data, index) {
                Ok((raw, next)) => {
                    index = next;
                    let text = decode_pdf_string(&raw);
                    if depth > 0 {
                        array.push(text);
                    } else {
                        pending = text;
                    }
                }
                Err(_) => break,
            }
            continue;
        }
        if byte == b'[' {
            depth += 1;
            if depth == 1 {
                array.clear();
            }
            index += 1;
            continue;
        }
        if byte == b']' {
            if depth > 0 {
                depth -= 1;
            }
            index += 1;
            continue;
        }
        if byte == b'/' {
            index += 1;
            while index < data.len() && !data[index].is_ascii_whitespace() && !is_delim(data[index])
            {
                index += 1;
            }
            continue;
        }
        let start = index;
        while index < data.len() && !data[index].is_ascii_whitespace() && !is_delim(data[index]) {
            index += 1;
        }
        if start == index {
            index += 1;
            continue;
        }
        let word = &data[start..index];
        if word == b"Tj" && depth == 0 {
            push_capped(&mut out, &pending);
            pending.clear();
        } else if word == b"TJ" {
            for text in array.drain(..) {
                push_capped(&mut out, &text);
            }
            pending.clear();
        } else if depth == 0 && !is_number_token(word) {
            pending.clear();
        }
    }
    out
}

fn skip_dict(data: &[u8], mut index: usize) -> usize {
    let mut depth = 0i32;
    while index < data.len() {
        if data[index] == b'<' && data.get(index + 1) == Some(&b'<') {
            depth += 1;
            index += 2;
            continue;
        }
        if data[index] == b'>' && data.get(index + 1) == Some(&b'>') {
            depth -= 1;
            index += 2;
            if depth <= 0 {
                return index;
            }
            continue;
        }
        if data[index] == b'(' {
            if let Ok((_, next)) = read_literal(data, index) {
                index = next;
            } else {
                return data.len();
            }
            continue;
        }
        index += 1;
    }
    data.len()
}

fn is_number_token(word: &[u8]) -> bool {
    if word.is_empty() {
        return false;
    }
    let mut index = 0;
    if word[0] == b'+' || word[0] == b'-' {
        index = 1;
    }
    if index >= word.len() {
        return false;
    }
    let mut saw_digit = false;
    let mut saw_dot = false;
    while index < word.len() {
        let byte = word[index];
        if byte.is_ascii_digit() {
            saw_digit = true;
        } else if byte == b'.' && !saw_dot {
            saw_dot = true;
        } else {
            return false;
        }
        index += 1;
    }
    saw_digit
}
