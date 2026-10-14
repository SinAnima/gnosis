//! Durable knowledge store.
//!
//! Separate from the job queue so `jobs purge` cannot delete compiled knowledge.
//! Each object is one row: the stored object, the entities and relationships it
//! produced, and the fingerprint / provider version used to skip unchanged work.

use crate::error::{GnosisError, Result};
use crate::ids::ObjectId;
use crate::knowledge::{Entity, Relationship};
use crate::status::UnderstandingStatus;
use crate::store::StoredObject;
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const OBJECTS: TableDefinition<&str, &[u8]> = TableDefinition::new("objects");
const SNAPSHOTS: TableDefinition<&str, &[u8]> = TableDefinition::new("snapshots");
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("kmeta");

const META_SNAPSHOT_IDS: &str = "snapshot_ids";

/// Why a scan kept or redid an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReanalyzeReason {
    New,
    Content,
    Provider,
    Dependency,
    Unchanged,
    Deleted,
    /// Content and the structural provider are unchanged. Refresh evidence only.
    Intrinsic,
}

impl ReanalyzeReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Content => "content",
            Self::Provider => "provider",
            Self::Dependency => "dependency",
            Self::Unchanged => "unchanged",
            Self::Deleted => "deleted",
            Self::Intrinsic => "intrinsic",
        }
    }
}

/// Outcome of comparing a discovered object with the stored record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Classify {
    /// Keep the stored analysis. `touch_stat` updates mtime/etag without reanalyzing.
    Keep {
        touch_stat: bool,
    },
    /// Fast path did not apply; the caller should hash the bytes and classify again.
    NeedBytes,
    Analyze(ReanalyzeReason),
}

/// Inputs for [`classify_object`]. `fingerprint` is `None` until the bytes are read.
#[derive(Clone, Debug)]
pub struct ClassifyInput {
    pub reanalyze: bool,
    pub has_stored: bool,
    pub stored_fingerprint: Option<String>,
    pub stored_provider_id: Option<String>,
    pub stored_provider_version: Option<String>,
    pub stored_size: u64,
    pub stored_modified_unix_ms: Option<u64>,
    pub stored_etag: Option<String>,
    pub size: u64,
    pub modified_unix_ms: Option<u64>,
    pub etag: Option<String>,
    pub provider_id: Option<String>,
    pub provider_version: Option<String>,
    pub fingerprint: Option<String>,
    pub is_s3: bool,
}

pub fn classify_object(input: &ClassifyInput) -> Classify {
    if !input.has_stored {
        return Classify::Analyze(ReanalyzeReason::New);
    }
    if input.reanalyze {
        return Classify::Analyze(ReanalyzeReason::Content);
    }
    if input.stored_provider_id != input.provider_id
        || input.stored_provider_version != input.provider_version
    {
        return Classify::Analyze(ReanalyzeReason::Provider);
    }
    let etags_match = matches!(
        (&input.stored_etag, &input.etag),
        (Some(stored), Some(live)) if stored == live
    );
    let etags_differ = matches!(
        (&input.stored_etag, &input.etag),
        (Some(stored), Some(live)) if stored != live
    );
    if etags_match
        && input.stored_size == input.size
        && (input.is_s3 || input.stored_modified_unix_ms == input.modified_unix_ms)
    {
        return Classify::Keep { touch_stat: false };
    }
    if !input.is_s3
        && !etags_differ
        && input.stored_size == input.size
        && input.stored_modified_unix_ms.is_some()
        && input.stored_modified_unix_ms == input.modified_unix_ms
    {
        return Classify::Keep { touch_stat: false };
    }
    let Some(fingerprint) = input.fingerprint.as_deref() else {
        return Classify::NeedBytes;
    };
    if input.stored_fingerprint.as_deref() == Some(fingerprint) {
        Classify::Keep { touch_stat: true }
    } else {
        Classify::Analyze(ReanalyzeReason::Content)
    }
}

/// True when an include/import string refers to `relative_path`.
pub fn dep_matches(dep: &str, relative_path: &str) -> bool {
    let dep = dep
        .trim()
        .trim_matches(|c| c == '"' || c == '<' || c == '>')
        .replace('\\', "/");
    let rel = relative_path.trim().replace('\\', "/");
    if dep.is_empty() || rel.is_empty() {
        return false;
    }
    rel == dep || rel.ends_with(&format!("/{dep}"))
}

pub fn collect_deps(entities: &[Entity], relationships: &[Relationship]) -> Vec<String> {
    let mut deps = Vec::new();
    for entity in entities {
        if let Some(value) = entity
            .attributes
            .get("include")
            .or_else(|| entity.attributes.get("import"))
        {
            if !value.is_empty() {
                deps.push(value.clone());
            }
        }
    }
    for rel in relationships {
        if rel.kind == "includes" || rel.kind == "imports" {
            if let Some(value) = rel
                .attributes
                .get("include")
                .or_else(|| rel.attributes.get("import"))
            {
                if !value.is_empty() {
                    deps.push(value.clone());
                }
            }
        }
    }
    deps.sort();
    deps.dedup();
    deps
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedObject {
    pub object: StoredObject,
    pub entities: Vec<Entity>,
    pub relationships: Vec<Relationship>,
    pub fingerprint: Option<String>,
    pub provider_id: Option<String>,
    pub provider_version: Option<String>,
    pub size: u64,
    pub modified_unix_ms: Option<u64>,
    pub etag: Option<String>,
    pub deps: Vec<String>,
    pub reason: ReanalyzeReason,
    /// Intrinsic evidence. Absent on rows written before v0.5.0.
    #[serde(default)]
    pub evidence: Option<crate::intrinsic::ArtifactEvidenceRecord>,
    /// Stamp of the intrinsic extractors that produced `evidence`.
    #[serde(default)]
    pub intrinsic_version: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotObject {
    pub id: String,
    pub relative_path: String,
    pub fingerprint: Option<String>,
    pub status: String,
    pub provider_id: Option<String>,
    pub provider_version: Option<String>,
    pub reason: ReanalyzeReason,
    pub entity_ids: Vec<String>,
    pub relationship_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub scan_id: String,
    pub created_unix_ms: u64,
    pub objects: Vec<SnapshotObject>,
}

pub struct KnowledgeDb {
    db: Database,
    write_lock: Mutex<()>,
    path: Option<std::path::PathBuf>,
}

const EVIDENCE_SCHEMA: &str = "2";
const META_SCHEMA: &str = "schema_version";

impl KnowledgeDb {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let db = Database::create(path).map_err(|e| {
            GnosisError::Pipeline(format!("open knowledge db {}: {e}", path.display()))
        })?;
        let store = Self {
            db,
            write_lock: Mutex::new(()),
            path: Some(path.to_path_buf()),
        };
        store.init_schema()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self> {
        let db = Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .map_err(|e| GnosisError::Pipeline(format!("open in-memory knowledge db: {e}")))?;
        let store = Self {
            db,
            write_lock: Mutex::new(()),
            path: None,
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        let _guard = self.write_lock.lock().unwrap();
        let txn = self.begin_write()?;
        {
            let _objects = txn
                .open_table(OBJECTS)
                .map_err(|e| GnosisError::Pipeline(format!("open objects table: {e}")))?;
            let _snapshots = txn
                .open_table(SNAPSHOTS)
                .map_err(|e| GnosisError::Pipeline(format!("open snapshots table: {e}")))?;
            let _meta = txn
                .open_table(META)
                .map_err(|e| GnosisError::Pipeline(format!("open knowledge meta: {e}")))?;
        }
        txn.commit()
            .map_err(|e| GnosisError::Pipeline(format!("knowledge db commit: {e}")))?;
        Ok(())
    }

    fn begin_write(&self) -> Result<redb::WriteTransaction> {
        self.db
            .begin_write()
            .map_err(|e| GnosisError::Pipeline(format!("knowledge begin_write: {e}")))
    }

    pub fn put(&self, record: &PersistedObject) -> Result<()> {
        let id = record.object.descriptor.id.as_str().to_string();
        let bytes = serde_json::to_vec(record)
            .map_err(|e| GnosisError::Pipeline(format!("encode object {id}: {e}")))?;
        let _guard = self.write_lock.lock().unwrap();
        self.ensure_evidence_schema()?;
        let txn = self.begin_write()?;
        {
            let mut table = txn
                .open_table(OBJECTS)
                .map_err(|e| GnosisError::Pipeline(format!("open objects: {e}")))?;
            table
                .insert(id.as_str(), bytes.as_slice())
                .map_err(|e| GnosisError::Pipeline(format!("insert object {id}: {e}")))?;
        }
        txn.commit()
            .map_err(|e| GnosisError::Pipeline(format!("knowledge db commit: {e}")))?;
        Ok(())
    }

    /// Copy a populated pre-v0.5 database beside itself before the first new write.
    /// A decode failure in `get` returns an error and does not replace the file.
    fn ensure_evidence_schema(&self) -> Result<()> {
        if self.meta_value(META_SCHEMA)?.as_deref() == Some(EVIDENCE_SCHEMA) {
            return Ok(());
        }
        if self.object_count()? > 0 {
            if let Some(path) = &self.path {
                let backup = path.with_extension("redb.bak");
                if !backup.exists() {
                    std::fs::copy(path, &backup).map_err(|err| {
                        GnosisError::Pipeline(format!(
                            "backup {} to {}: {err}",
                            path.display(),
                            backup.display()
                        ))
                    })?;
                }
            }
        }
        self.write_meta(META_SCHEMA, EVIDENCE_SCHEMA.as_bytes())
    }

    fn object_count(&self) -> Result<u64> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| GnosisError::Pipeline(format!("knowledge begin_read: {e}")))?;
        let table = txn
            .open_table(OBJECTS)
            .map_err(|e| GnosisError::Pipeline(format!("open objects: {e}")))?;
        let mut count = 0u64;
        let iter = table
            .iter()
            .map_err(|e| GnosisError::Pipeline(format!("scan objects: {e}")))?;
        for item in iter {
            item.map_err(|e| GnosisError::Pipeline(format!("scan objects: {e}")))?;
            count += 1;
        }
        Ok(count)
    }

    fn meta_value(&self, key: &str) -> Result<Option<String>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| GnosisError::Pipeline(format!("knowledge begin_read: {e}")))?;
        let table = txn
            .open_table(META)
            .map_err(|e| GnosisError::Pipeline(format!("open knowledge meta: {e}")))?;
        let Some(value) = table
            .get(key)
            .map_err(|e| GnosisError::Pipeline(format!("read knowledge meta: {e}")))?
        else {
            return Ok(None);
        };
        Ok(Some(String::from_utf8_lossy(value.value()).into_owned()))
    }

    fn write_meta(&self, key: &str, value: &[u8]) -> Result<()> {
        let txn = self.begin_write()?;
        {
            let mut table = txn
                .open_table(META)
                .map_err(|e| GnosisError::Pipeline(format!("open knowledge meta: {e}")))?;
            table
                .insert(key, value)
                .map_err(|e| GnosisError::Pipeline(format!("write knowledge meta: {e}")))?;
        }
        txn.commit()
            .map_err(|e| GnosisError::Pipeline(format!("knowledge db commit: {e}")))?;
        Ok(())
    }

    /// Insert a pre-v0.5 row without bumping the schema. Tests use this to
    /// check that the first new write copies the file aside.
    #[cfg(test)]
    pub fn insert_legacy_for_test(&self, id: &str, bytes: &[u8]) -> Result<()> {
        let _guard = self.write_lock.lock().unwrap();
        let txn = self.begin_write()?;
        {
            let mut table = txn
                .open_table(OBJECTS)
                .map_err(|e| GnosisError::Pipeline(format!("open objects: {e}")))?;
            table
                .insert(id, bytes)
                .map_err(|e| GnosisError::Pipeline(format!("insert legacy {id}: {e}")))?;
        }
        txn.commit()
            .map_err(|e| GnosisError::Pipeline(format!("knowledge db commit: {e}")))?;
        Ok(())
    }

    pub fn get(&self, id: &ObjectId) -> Result<Option<PersistedObject>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| GnosisError::Pipeline(format!("knowledge begin_read: {e}")))?;
        let table = txn
            .open_table(OBJECTS)
            .map_err(|e| GnosisError::Pipeline(format!("open objects: {e}")))?;
        let Some(value) = table
            .get(id.as_str())
            .map_err(|e| GnosisError::Pipeline(format!("get object: {e}")))?
        else {
            return Ok(None);
        };
        let record = serde_json::from_slice(value.value())
            .map_err(|e| GnosisError::Pipeline(format!("decode object {}: {e}", id.as_str())))?;
        Ok(Some(record))
    }

    pub fn delete(&self, id: &ObjectId) -> Result<()> {
        let _guard = self.write_lock.lock().unwrap();
        let txn = self.begin_write()?;
        {
            let mut table = txn
                .open_table(OBJECTS)
                .map_err(|e| GnosisError::Pipeline(format!("open objects: {e}")))?;
            table
                .remove(id.as_str())
                .map_err(|e| GnosisError::Pipeline(format!("delete object: {e}")))?;
        }
        txn.commit()
            .map_err(|e| GnosisError::Pipeline(format!("knowledge db commit: {e}")))?;
        Ok(())
    }

    pub fn load_all(&self) -> Result<Vec<PersistedObject>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| GnosisError::Pipeline(format!("knowledge begin_read: {e}")))?;
        let table = txn
            .open_table(OBJECTS)
            .map_err(|e| GnosisError::Pipeline(format!("open objects: {e}")))?;
        let mut out = Vec::new();
        let iter = table
            .iter()
            .map_err(|e| GnosisError::Pipeline(format!("iterate objects: {e}")))?;
        for item in iter {
            let (_key, value) =
                item.map_err(|e| GnosisError::Pipeline(format!("read object row: {e}")))?;
            let record = serde_json::from_slice(value.value())
                .map_err(|e| GnosisError::Pipeline(format!("decode object: {e}")))?;
            out.push(record);
        }
        Ok(out)
    }

    pub fn update_stat(
        &self,
        id: &ObjectId,
        size: u64,
        modified_unix_ms: Option<u64>,
        etag: Option<String>,
    ) -> Result<()> {
        let Some(mut record) = self.get(id)? else {
            return Ok(());
        };
        record.size = size;
        record.modified_unix_ms = modified_unix_ms;
        record.etag = etag;
        record.object.descriptor.size = size;
        record.object.proto.size = size;
        record.reason = ReanalyzeReason::Unchanged;
        self.put(&record)
    }

    pub fn put_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        let bytes = serde_json::to_vec(snapshot)
            .map_err(|e| GnosisError::Pipeline(format!("encode snapshot: {e}")))?;
        let _guard = self.write_lock.lock().unwrap();
        let txn = self.begin_write()?;
        {
            let mut table = txn
                .open_table(SNAPSHOTS)
                .map_err(|e| GnosisError::Pipeline(format!("open snapshots: {e}")))?;
            table
                .insert(snapshot.id.as_str(), bytes.as_slice())
                .map_err(|e| GnosisError::Pipeline(format!("insert snapshot: {e}")))?;
            let mut meta = txn
                .open_table(META)
                .map_err(|e| GnosisError::Pipeline(format!("open knowledge meta: {e}")))?;
            let existing = meta
                .get(META_SNAPSHOT_IDS)
                .map_err(|e| GnosisError::Pipeline(format!("read snapshot ids: {e}")))?
                .map(|v| v.value().to_vec());
            let mut ids = decode_ids(existing.as_deref())?;
            ids.push(snapshot.id.clone());
            let encoded = serde_json::to_vec(&ids)
                .map_err(|e| GnosisError::Pipeline(format!("encode snapshot ids: {e}")))?;
            meta.insert(META_SNAPSHOT_IDS, encoded.as_slice())
                .map_err(|e| GnosisError::Pipeline(format!("write snapshot ids: {e}")))?;
        }
        txn.commit()
            .map_err(|e| GnosisError::Pipeline(format!("knowledge db commit: {e}")))?;
        Ok(())
    }

    pub fn snapshot_ids(&self) -> Result<Vec<String>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| GnosisError::Pipeline(format!("knowledge begin_read: {e}")))?;
        let meta = txn
            .open_table(META)
            .map_err(|e| GnosisError::Pipeline(format!("open knowledge meta: {e}")))?;
        let existing = meta
            .get(META_SNAPSHOT_IDS)
            .map_err(|e| GnosisError::Pipeline(format!("read snapshot ids: {e}")))?
            .map(|v| v.value().to_vec());
        decode_ids(existing.as_deref())
    }

    pub fn snapshot(&self, id: &str) -> Result<Option<Snapshot>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| GnosisError::Pipeline(format!("knowledge begin_read: {e}")))?;
        let table = txn
            .open_table(SNAPSHOTS)
            .map_err(|e| GnosisError::Pipeline(format!("open snapshots: {e}")))?;
        let Some(value) = table
            .get(id)
            .map_err(|e| GnosisError::Pipeline(format!("get snapshot: {e}")))?
        else {
            return Ok(None);
        };
        let snapshot = serde_json::from_slice(value.value())
            .map_err(|e| GnosisError::Pipeline(format!("decode snapshot {id}: {e}")))?;
        Ok(Some(snapshot))
    }

    /// Latest two snapshots, older first. `None` when fewer than two exist.
    pub fn latest_pair(&self) -> Result<Option<(Snapshot, Snapshot)>> {
        let ids = self.snapshot_ids()?;
        if ids.len() < 2 {
            return Ok(None);
        }
        let older = self
            .snapshot(&ids[ids.len() - 2])?
            .ok_or_else(|| GnosisError::Pipeline("missing snapshot".into()))?;
        let newer = self
            .snapshot(&ids[ids.len() - 1])?
            .ok_or_else(|| GnosisError::Pipeline("missing snapshot".into()))?;
        Ok(Some((older, newer)))
    }

    pub fn resolve_snapshot(&self, query: &str) -> Result<Snapshot> {
        let ids = self.snapshot_ids()?;
        let q = query.trim();
        let matches: Vec<_> = ids
            .into_iter()
            .filter(|id| id == q || id.ends_with(q) || id.contains(q))
            .collect();
        let id = match matches.len() {
            0 => {
                return Err(GnosisError::Pipeline(format!(
                    "snapshot not found: {query}"
                )))
            }
            1 => matches.into_iter().next().unwrap(),
            n => {
                return Err(GnosisError::Pipeline(format!(
                    "ambiguous snapshot '{query}' ({n} matches)"
                )))
            }
        };
        self.snapshot(&id)?
            .ok_or_else(|| GnosisError::Pipeline(format!("snapshot not found: {id}")))
    }

    pub fn persist_review(&self, id: &str, review: &str) -> Result<bool> {
        let records = self.load_all()?;
        for mut record in records {
            let mut hit = false;
            for entity in &mut record.entities {
                if entity.id.as_str() == id {
                    entity
                        .attributes
                        .insert("review".into(), review.to_string());
                    hit = true;
                }
            }
            for rel in &mut record.relationships {
                if rel.id.as_str() == id {
                    rel.attributes.insert("review".into(), review.to_string());
                    hit = true;
                }
            }
            if hit {
                self.put(&record)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn review_rows(&self) -> Result<Vec<crate::review::ReviewRow>> {
        let mut rows = Vec::new();
        for record in self.load_all()? {
            for entity in &record.entities {
                if entity.attributes.get("origin").map(String::as_str) != Some("llm") {
                    continue;
                }
                rows.push(crate::review::ReviewRow {
                    id: entity.id.to_string(),
                    kind: entity.kind.clone(),
                    label: entity.name.clone(),
                    review: entity
                        .attributes
                        .get("review")
                        .cloned()
                        .unwrap_or_else(|| "pending".into()),
                });
            }
            for rel in &record.relationships {
                if rel.attributes.get("origin").map(String::as_str) != Some("llm") {
                    continue;
                }
                rows.push(crate::review::ReviewRow {
                    id: rel.id.as_str().to_string(),
                    kind: rel.kind.clone(),
                    label: format!("{} -> {}", rel.from, rel.to),
                    review: rel
                        .attributes
                        .get("review")
                        .cloned()
                        .unwrap_or_else(|| "pending".into()),
                });
            }
        }
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(rows)
    }
}

fn decode_ids(bytes: Option<&[u8]>) -> Result<Vec<String>> {
    match bytes {
        Some(bytes) => serde_json::from_slice(bytes)
            .map_err(|e| GnosisError::Pipeline(format!("decode snapshot ids: {e}"))),
        None => Ok(Vec::new()),
    }
}

pub fn unix_ms(time: Option<SystemTime>) -> Option<u64> {
    time.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
}

pub fn format_diff(from: &Snapshot, to: &Snapshot) -> String {
    let older: BTreeMap<&str, &SnapshotObject> =
        from.objects.iter().map(|o| (o.id.as_str(), o)).collect();
    let newer: BTreeMap<&str, &SnapshotObject> =
        to.objects.iter().map(|o| (o.id.as_str(), o)).collect();
    let mut lines = vec![format!(
        "diff {} -> {} ({} -> {} objects)",
        from.id,
        to.id,
        from.objects.len(),
        to.objects.len()
    )];
    let mut ids: Vec<&str> = older.keys().copied().chain(newer.keys().copied()).collect();
    ids.sort();
    ids.dedup();
    for id in ids {
        match (older.get(id), newer.get(id)) {
            (None, Some(obj)) => lines.push(format!(
                "+ {}  {}  {}",
                obj.relative_path,
                obj.reason.as_str(),
                obj.status
            )),
            (Some(obj), None) => lines.push(format!("- {}  deleted", obj.relative_path)),
            (Some(before), Some(after)) => {
                let entities_changed = before.entity_ids != after.entity_ids;
                let rels_changed = before.relationship_ids != after.relationship_ids;
                let status_changed = before.status != after.status;
                if !entities_changed
                    && !rels_changed
                    && !status_changed
                    && before.reason == after.reason
                {
                    continue;
                }
                lines.push(format!(
                    "~ {}  {} -> {}  ({})",
                    after.relative_path,
                    before.status,
                    after.status,
                    after.reason.as_str()
                ));
                if status_changed || entities_changed {
                    push_id_delta(
                        &mut lines,
                        "entities",
                        &before.entity_ids,
                        &after.entity_ids,
                    );
                }
                if rels_changed {
                    push_id_delta(
                        &mut lines,
                        "relationships",
                        &before.relationship_ids,
                        &after.relationship_ids,
                    );
                }
            }
            (None, None) => {}
        }
    }
    if lines.len() == 1 {
        lines.push("no object, status, entity, or relationship changes".into());
    }
    lines.join("\n")
}

fn push_id_delta(lines: &mut Vec<String>, label: &str, before: &[String], after: &[String]) {
    let added: Vec<_> = after.iter().filter(|id| !before.contains(id)).collect();
    let removed: Vec<_> = before.iter().filter(|id| !after.contains(id)).collect();
    if added.is_empty() && removed.is_empty() {
        return;
    }
    lines.push(format!("  {label}:"));
    for id in added {
        lines.push(format!("    + {id}"));
    }
    for id in removed {
        lines.push(format!("    - {id}"));
    }
}

pub fn status_name(status: UnderstandingStatus) -> String {
    status.as_str().to_string()
}

impl crate::store::ReviewSink for KnowledgeDb {
    fn set_review(&self, id: &str, review: &str) -> std::result::Result<(), String> {
        self.persist_review(id, review)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::types::{ObjectDescriptor, ProtoData};
    use crate::ids::{EntityId, ObjectId, ProviderId, RelationshipId};
    use crate::knowledge::{AttributeMap, Confidence, Entity, Relationship};
    use std::path::PathBuf;

    fn input(has_stored: bool) -> ClassifyInput {
        ClassifyInput {
            reanalyze: false,
            has_stored,
            stored_fingerprint: Some("abc".into()),
            stored_provider_id: Some("rust".into()),
            stored_provider_version: Some("0.1.0".into()),
            stored_size: 4,
            stored_modified_unix_ms: Some(10),
            stored_etag: Some("etag-1".into()),
            size: 4,
            modified_unix_ms: Some(10),
            etag: Some("etag-1".into()),
            provider_id: Some("rust".into()),
            provider_version: Some("0.1.0".into()),
            fingerprint: None,
            is_s3: false,
        }
    }

    #[test]
    fn classify_skips_matching_mtime_and_reanalyzes_on_version() {
        assert!(matches!(
            classify_object(&input(false)),
            Classify::Analyze(ReanalyzeReason::New)
        ));
        assert!(matches!(
            classify_object(&input(true)),
            Classify::Keep { touch_stat: false }
        ));
        let mut changed = input(true);
        changed.modified_unix_ms = Some(11);
        assert!(matches!(classify_object(&changed), Classify::NeedBytes));
        changed.fingerprint = Some("abc".into());
        assert!(matches!(
            classify_object(&changed),
            Classify::Keep { touch_stat: true }
        ));
        changed.fingerprint = Some("def".into());
        assert!(matches!(
            classify_object(&changed),
            Classify::Analyze(ReanalyzeReason::Content)
        ));
        let mut version = input(true);
        version.provider_version = Some("0.2.0".into());
        assert!(matches!(
            classify_object(&version),
            Classify::Analyze(ReanalyzeReason::Provider)
        ));
        let mut s3 = input(true);
        s3.is_s3 = true;
        s3.modified_unix_ms = None;
        s3.stored_modified_unix_ms = None;
        assert!(matches!(
            classify_object(&s3),
            Classify::Keep { touch_stat: false }
        ));
        s3.etag = Some("etag-2".into());
        assert!(matches!(classify_object(&s3), Classify::NeedBytes));
    }

    #[test]
    fn dep_match_is_a_path_suffix() {
        assert!(dep_matches("a.h", "src/a.h"));
        assert!(dep_matches("\"a.h\"", "src/a.h"));
        assert!(!dep_matches("h", "src/a.h"));
        assert!(!dep_matches("a.h", "src/ba.h"));
        assert!(dep_matches("src/a.h", "src/a.h"));
    }

    #[test]
    fn round_trip_and_review_persist() {
        let db = KnowledgeDb::in_memory().unwrap();
        let object_id = ObjectId::new("obj:a.rs");
        let entity = Entity {
            id: EntityId::new("ent:fn:ping@a.rs"),
            kind: "function".into(),
            name: "ping".into(),
            attributes: {
                let mut map = AttributeMap::new();
                map.insert("origin".into(), "llm".into());
                map.insert("review".into(), "pending".into());
                map.insert("include".into(), "a.h".into());
                map
            },
            evidence: Vec::new(),
            confidence: Confidence::Inferred,
            source_object: object_id.clone(),
        };
        let rel = Relationship {
            id: RelationshipId::new("rel:includes:ent:fn:ping@a.rs->ent:module:a.h@a.rs"),
            kind: "includes".into(),
            from: entity.id.clone(),
            to: EntityId::new("ent:module:a.h@a.rs"),
            attributes: AttributeMap::new(),
            evidence: Vec::new(),
            confidence: Confidence::Medium,
        };
        let root = PathBuf::from(".");
        let path = PathBuf::from("a.rs");
        let mut descriptor = ObjectDescriptor::from_path(&root, &path, false, 3, None);
        descriptor.id = object_id.clone();
        let record = PersistedObject {
            object: StoredObject {
                descriptor,
                proto: ProtoData::default(),
                status: UnderstandingStatus::Understood,
                provider: Some(ProviderId::new("rust")),
                classification_reason: None,
                entity_ids: vec![entity.id.clone()],
                diagnostics: Vec::new(),
            },
            entities: vec![entity],
            relationships: vec![rel],
            fingerprint: Some("abc".into()),
            provider_id: Some("rust".into()),
            provider_version: Some("0.1.0".into()),
            size: 3,
            modified_unix_ms: Some(1),
            etag: None,
            deps: vec!["a.h".into()],
            reason: ReanalyzeReason::New,
            evidence: None,
            intrinsic_version: None,
        };
        db.put(&record).unwrap();
        assert!(db.persist_review("ent:fn:ping@a.rs", "accepted").unwrap());
        let loaded = db.get(&object_id).unwrap().unwrap();
        assert_eq!(
            loaded.entities[0]
                .attributes
                .get("review")
                .map(String::as_str),
            Some("accepted")
        );
        let rows = db.review_rows().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].review, "accepted");
    }

    #[test]
    fn diff_reports_added_and_changed_ids() {
        let from = Snapshot {
            id: "snap:1".into(),
            scan_id: "scan:1".into(),
            created_unix_ms: 1,
            objects: vec![SnapshotObject {
                id: "obj:a.rs".into(),
                relative_path: "a.rs".into(),
                fingerprint: Some("a".into()),
                status: "understood".into(),
                provider_id: Some("rust".into()),
                provider_version: Some("0.1.0".into()),
                reason: ReanalyzeReason::New,
                entity_ids: vec!["ent:fn:old@a.rs".into()],
                relationship_ids: Vec::new(),
            }],
        };
        let mut to = from.clone();
        to.id = "snap:2".into();
        to.objects[0].entity_ids = vec!["ent:fn:new@a.rs".into()];
        to.objects[0].reason = ReanalyzeReason::Content;
        to.objects.push(SnapshotObject {
            id: "obj:b.rs".into(),
            relative_path: "b.rs".into(),
            fingerprint: None,
            status: "unknown".into(),
            provider_id: None,
            provider_version: None,
            reason: ReanalyzeReason::New,
            entity_ids: Vec::new(),
            relationship_ids: Vec::new(),
        });
        let text = format_diff(&from, &to);
        assert!(text.contains("~ a.rs"));
        assert!(text.contains("+ ent:fn:new@a.rs"));
        assert!(text.contains("- ent:fn:old@a.rs"));
        assert!(text.contains("+ b.rs"));
    }

    #[test]
    fn legacy_row_without_evidence_loads_and_first_write_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("knowledge.redb");
        let db = KnowledgeDb::open(&path).unwrap();
        let object_id = ObjectId::new("obj:a.rs");
        let root = PathBuf::from(".");
        let file = PathBuf::from("a.rs");
        let descriptor = ObjectDescriptor::from_path(&root, &file, false, 3, None);
        let record = PersistedObject {
            object: StoredObject {
                descriptor,
                proto: ProtoData::default(),
                status: UnderstandingStatus::Understood,
                provider: Some(ProviderId::new("rust")),
                classification_reason: None,
                entity_ids: Vec::new(),
                diagnostics: Vec::new(),
            },
            entities: Vec::new(),
            relationships: Vec::new(),
            fingerprint: Some("abc".into()),
            provider_id: Some("rust".into()),
            provider_version: Some("0.1.0".into()),
            size: 3,
            modified_unix_ms: Some(1),
            etag: None,
            deps: Vec::new(),
            reason: ReanalyzeReason::New,
            evidence: None,
            intrinsic_version: None,
        };
        let mut legacy = serde_json::to_value(&record).unwrap();
        let object = legacy.as_object_mut().unwrap();
        object.remove("evidence");
        object.remove("intrinsic_version");
        let bytes = serde_json::to_vec(&legacy).unwrap();
        db.insert_legacy_for_test(object_id.as_str(), &bytes)
            .unwrap();
        db.insert_legacy_for_test("obj:bad", b"not-json").unwrap();
        drop(db);

        let db = KnowledgeDb::open(&path).unwrap();
        let loaded = db.get(&object_id).unwrap().unwrap();
        assert!(loaded.evidence.is_none());
        assert!(loaded.intrinsic_version.is_none());
        let err = db.get(&ObjectId::new("obj:bad")).unwrap_err();
        assert!(err.to_string().contains("decode"));
        assert!(path.is_file());
        db.put(&loaded).unwrap();
        let backup = path.with_extension("redb.bak");
        assert!(backup.is_file(), "missing {}", backup.display());
        let round = db.get(&object_id).unwrap().unwrap();
        let encoded = serde_json::to_vec(&round).unwrap();
        let decoded: PersistedObject = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.provider_id.as_deref(), Some("rust"));
    }
}
