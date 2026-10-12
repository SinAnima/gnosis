//! List and accept LLM review marks.
//!
//! The knowledge database is the source of truth. `gnosis review accept` updates
//! that store and, when an OKF bundle is present, the same mark in the bundle.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewRow {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub review: String,
}

pub fn format_reviews(rows: &[ReviewRow]) -> String {
    let pending: Vec<_> = rows.iter().filter(|r| r.review == "pending").collect();
    if pending.is_empty() {
        return "no pending LLM reviews".into();
    }
    let mut lines = vec![format!("{} pending LLM review(s):", pending.len())];
    for row in pending {
        lines.push(format!(
            "{}  {}  {}  {}",
            row.review, row.kind, row.label, row.id
        ));
    }
    lines.join("\n")
}

pub fn list_okf(dir: &Path) -> Result<Vec<ReviewRow>, String> {
    load_docs(dir).map(|docs| docs.into_iter().map(Doc::into_row).collect())
}

pub fn resolve_review_id(rows: &[ReviewRow], query: &str) -> Result<String, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("review id required".into());
    }
    let mut matches: Vec<&str> = rows
        .iter()
        .map(|row| row.id.as_str())
        .filter(|id| *id == q || id.ends_with(q) || id.contains(q))
        .collect();
    matches.sort();
    matches.dedup();
    match matches.len() {
        0 => Err(format!("review target not found: {q}")),
        1 => Ok(matches[0].to_string()),
        n => Err(format!("ambiguous review id '{q}' ({n} matches)")),
    }
}

pub fn accept_okf(dir: &Path, query: &str) -> Result<String, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("review id required".into());
    }
    let docs = load_docs(dir)?;
    let mut matches: Vec<&Doc> = docs
        .iter()
        .filter(|d| d.id == q || d.id.ends_with(q) || d.id.contains(q))
        .collect();
    matches.sort_by(|a, b| a.id.cmp(&b.id));
    matches.dedup_by(|a, b| a.id == b.id);
    let doc = match matches.len() {
        0 => return Err(format!("review target not found: {q}")),
        1 => matches[0],
        n => return Err(format!("ambiguous review id '{q}' ({n} matches)")),
    };
    let text = fs::read_to_string(&doc.path).map_err(|e| e.to_string())?;
    let needle = "- `review`: pending";
    if !text.contains(needle) {
        return Err(format!("{} is not pending", doc.id));
    }
    let updated = text.replacen(needle, "- `review`: accepted", 1);
    fs::write(&doc.path, updated).map_err(|e| e.to_string())?;
    Ok(doc.id.clone())
}

struct Doc {
    path: PathBuf,
    id: String,
    kind: String,
    label: String,
    review: String,
}

impl Doc {
    fn into_row(self) -> ReviewRow {
        ReviewRow {
            id: self.id,
            kind: self.kind,
            label: self.label,
            review: self.review,
        }
    }
}

fn load_docs(dir: &Path) -> Result<Vec<Doc>, String> {
    let mut docs = Vec::new();
    for sub in ["entities", "relationships"] {
        let folder = dir.join(sub);
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("read {}: {e}", folder.display())),
        };
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let Some(review) = attr_value(&text, "review") else {
                continue;
            };
            let Some(id) = frontmatter_value(&text, "id") else {
                continue;
            };
            docs.push(Doc {
                path,
                id,
                kind: frontmatter_value(&text, "gnosis_kind").unwrap_or_else(|| sub.into()),
                label: frontmatter_value(&text, "title").unwrap_or_default(),
                review,
            });
        }
    }
    docs.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(docs)
}

fn frontmatter_value(text: &str, key: &str) -> Option<String> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    let block = &rest[..end];
    let prefix = format!("{key}: ");
    block.lines().find_map(|line| {
        let value = line.trim().strip_prefix(&prefix)?;
        let value = value.trim().trim_matches('"');
        Some(value.to_string())
    })
}

fn attr_value(text: &str, key: &str) -> Option<String> {
    let prefix = format!("- `{key}`: ");
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .map(|v| v.trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_rewrites_pending_mark() {
        let dir = tempfile::tempdir().unwrap();
        let entities = dir.path().join("entities");
        fs::create_dir(&entities).unwrap();
        fs::write(
            entities.join("module_auth.md"),
            "---\nid: ent:module:auth@a.rs\ntitle: auth\ngnosis_kind: module\n---\n\n- `review`: pending\n- `origin`: llm\n",
        )
        .unwrap();
        let rows = list_okf(dir.path()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].review, "pending");
        let id = accept_okf(dir.path(), "auth@a.rs").unwrap();
        assert!(id.contains("auth"));
        let rows = list_okf(dir.path()).unwrap();
        assert_eq!(rows[0].review, "accepted");
        assert!(format_reviews(&rows).contains("no pending"));
    }
}
