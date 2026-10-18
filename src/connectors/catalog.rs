//! Read-only database catalog. Relations become objects. Table rows are not read.

use crate::config::{CatalogLocation, CatalogOptions};
use crate::connectors::types::{CatalogColumn, CatalogForeignKey, CatalogProtoData};
use crate::error::{GnosisError, Result};
use crate::events::PipelineEvent;
use crate::ids::ObjectId;
use crossbeam_channel::Sender;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use url::Url;

pub const COMMENT_CAP: usize = 1024;
pub const VIEW_DEF_CAP: usize = 4096;
pub const MAX_COLUMNS: usize = 256;
pub const MAX_INDEXES: usize = 32;
pub const DEFAULT_MAX_RELATIONS: u64 = 200;
pub const DEFAULT_STATEMENT_TIMEOUT_MS: u64 = 5_000;

const RELATIONS_SQL: &str = "\
SELECT n.nspname::text, c.relname::text, c.relkind::text, \
obj_description(c.oid, 'pg_class')::text, \
CASE WHEN c.relkind IN ('v', 'm') THEN pg_catalog.pg_get_viewdef(c.oid, true) ELSE NULL END \
FROM pg_catalog.pg_class c \
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
WHERE n.nspname = $1 AND c.relkind IN ('r', 'v', 'm', 'p') AND NOT c.relispartition \
ORDER BY 1, 2";

const COLUMNS_SQL: &str = "\
SELECT n.nspname::text, c.relname::text, a.attname::text, \
pg_catalog.format_type(a.atttypid, a.atttypmod)::text, NOT a.attnotnull, a.attnum::int4, \
col_description(c.oid, a.attnum)::text \
FROM pg_catalog.pg_attribute a \
JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
WHERE n.nspname = $1 AND a.attnum > 0 AND NOT a.attisdropped \
AND c.relkind IN ('r', 'v', 'm', 'p') AND NOT c.relispartition \
ORDER BY n.nspname, c.relname, a.attnum";

const KEYS_SQL: &str = "\
SELECT n.nspname::text, c.relname::text, con.contype::text, \
ARRAY( \
SELECT a.attname::text \
FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) \
JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum = k.attnum \
ORDER BY k.ord) \
FROM pg_catalog.pg_constraint con \
JOIN pg_catalog.pg_class c ON c.oid = con.conrelid \
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
WHERE n.nspname = $1 AND con.contype IN ('p', 'u')";

const INDEXES_SQL: &str = "\
SELECT n.nspname::text, c.relname::text, i.relname::text \
FROM pg_catalog.pg_index ix \
JOIN pg_catalog.pg_class c ON c.oid = ix.indrelid \
JOIN pg_catalog.pg_class i ON i.oid = ix.indexrelid \
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
WHERE n.nspname = $1 AND NOT ix.indisprimary \
ORDER BY 1, 2, 3";

const FOREIGN_KEYS_SQL: &str = "\
SELECT src_ns.nspname::text, src.relname::text, dst_ns.nspname::text, dst.relname::text, \
ARRAY( \
SELECT a.attname::text \
FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) \
JOIN pg_catalog.pg_attribute a ON a.attrelid = src.oid AND a.attnum = k.attnum \
ORDER BY k.ord), \
ARRAY( \
SELECT a.attname::text \
FROM unnest(con.confkey) WITH ORDINALITY AS k(attnum, ord) \
JOIN pg_catalog.pg_attribute a ON a.attrelid = dst.oid AND a.attnum = k.attnum \
ORDER BY k.ord) \
FROM pg_catalog.pg_constraint con \
JOIN pg_catalog.pg_class src ON src.oid = con.conrelid \
JOIN pg_catalog.pg_namespace src_ns ON src_ns.oid = src.relnamespace \
JOIN pg_catalog.pg_class dst ON dst.oid = con.confrelid \
JOIN pg_catalog.pg_namespace dst_ns ON dst_ns.oid = dst.relnamespace \
WHERE src_ns.nspname = $1 AND con.contype = 'f'";

const DEPENDS_SQL: &str = "\
SELECT DISTINCT nv.nspname::text, cv.relname::text, nt.nspname::text, ct.relname::text \
FROM pg_catalog.pg_depend d \
JOIN pg_catalog.pg_rewrite rw ON rw.oid = d.objid \
JOIN pg_catalog.pg_class cv ON cv.oid = rw.ev_class \
JOIN pg_catalog.pg_namespace nv ON nv.oid = cv.relnamespace \
JOIN pg_catalog.pg_class ct ON ct.oid = d.refobjid \
JOIN pg_catalog.pg_namespace nt ON nt.oid = ct.relnamespace \
WHERE d.classid = 'pg_catalog.pg_rewrite'::regclass AND d.deptype = 'n' \
AND cv.relkind IN ('v', 'm') AND ct.relkind IN ('r', 'v', 'm', 'p') \
AND nv.nspname = $1 AND NOT (nt.nspname = nv.nspname AND ct.relname = cv.relname)";

/// Statements a catalog session is allowed to send, in order, for one schema.
pub fn catalog_statements(timeout_ms: u64) -> Vec<String> {
    vec![
        "BEGIN".into(),
        "SET TRANSACTION READ ONLY".into(),
        format!("SET statement_timeout = {timeout_ms}"),
        RELATIONS_SQL.into(),
        COLUMNS_SQL.into(),
        KEYS_SQL.into(),
        INDEXES_SQL.into(),
        FOREIGN_KEYS_SQL.into(),
        DEPENDS_SQL.into(),
        "COMMIT".into(),
    ]
}

/// Reject anything that is not a catalog read or the session's read-only setup.
pub fn guard_statement(sql: &str) -> Result<()> {
    let trimmed = sql.trim();
    let upper = trimmed.to_ascii_uppercase();
    if upper == "BEGIN" || upper == "COMMIT" || upper == "SET TRANSACTION READ ONLY" {
        return Ok(());
    }
    if let Some(rest) = upper.strip_prefix("SET STATEMENT_TIMEOUT = ") {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return Ok(());
        }
    }
    if !upper.starts_with("SELECT") {
        return Err(GnosisError::Pipeline(format!(
            "catalog statement is not a read: {trimmed}"
        )));
    }
    for keyword in [
        "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "TRUNCATE", "GRANT", "COPY",
    ] {
        if has_keyword(&upper, keyword) {
            return Err(GnosisError::Pipeline(format!(
                "catalog statement contains {keyword}"
            )));
        }
    }
    if !upper.contains("PG_CATALOG") && !upper.contains("INFORMATION_SCHEMA") {
        return Err(GnosisError::Pipeline(
            "catalog statement must read pg_catalog or information_schema".into(),
        ));
    }
    Ok(())
}

fn has_keyword(upper: &str, keyword: &str) -> bool {
    upper
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|word| word == keyword)
}

/// Double quotes inside an identifier. Used for stored SQL text, not for queries we send.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub fn schema_is_system(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "pg_catalog"
        || lower == "information_schema"
        || lower.starts_with("pg_temp")
        || lower.starts_with("pg_toast")
}

/// `public`, plus any `--schema` values. System schemas are removed.
pub fn schema_allowlist(requested: &[String]) -> (Vec<String>, Vec<String>) {
    let mut names = vec!["public".to_string()];
    for schema in requested {
        if !names.iter().any(|existing| existing == schema) {
            names.push(schema.clone());
        }
    }
    let mut warnings = Vec::new();
    names.retain(|name| {
        if schema_is_system(name) {
            warnings.push(format!("catalog skipped system schema {name}"));
            false
        } else {
            true
        }
    });
    (names, warnings)
}

pub fn redact_catalog_text(text: &str, secrets: &[String]) -> String {
    let mut out = crate::scrub::scrub_secrets(text);
    for secret in secrets {
        if secret.len() >= 4 {
            out = out.replace(secret, "REDACTED");
        }
    }
    out
}

pub fn catalog_secrets(password: Option<&str>) -> Vec<String> {
    let mut secrets = Vec::new();
    if let Some(password) = password.filter(|value| value.len() >= 4) {
        secrets.push(password.to_string());
    }
    if let Ok(password) = std::env::var("PGPASSWORD") {
        if password.len() >= 4 && !secrets.iter().any(|existing| existing == &password) {
            secrets.push(password);
        }
    }
    secrets
}

/// Parse `postgresql://` or `postgres://`. The password stays off the stored locator.
pub fn parse_catalog_uri(input: &str) -> Result<CatalogLocation> {
    let url = Url::parse(input)
        .map_err(|err| GnosisError::Pipeline(format!("invalid catalog URI: {err}")))?;
    let scheme = url.scheme().to_ascii_lowercase();
    if scheme != "postgresql" && scheme != "postgres" {
        return Err(GnosisError::Pipeline(format!("not a catalog URI: {input}")));
    }
    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| GnosisError::Pipeline(format!("catalog URI missing host: {input}")))?
        .to_string();
    let database = url.path().trim_matches('/');
    if database.is_empty() || database.contains('/') {
        return Err(GnosisError::Pipeline(format!(
            "catalog URI missing database name: {input}"
        )));
    }
    let user = if url.username().is_empty() {
        None
    } else {
        Some(url.username().to_string())
    };
    Ok(CatalogLocation {
        host,
        port: url.port().unwrap_or(5432),
        database: database.to_string(),
        user,
        password: url.password().map(str::to_string),
    })
}

pub fn relation_locator(database: &str, schema: &str, relation: &str) -> String {
    format!(
        "db://{}/{}/{}",
        encode_locator(database),
        encode_locator(schema),
        encode_locator(relation)
    )
}

pub fn column_locator(database: &str, schema: &str, relation: &str, column: &str) -> String {
    format!(
        "{}/{}",
        relation_locator(database, schema, relation),
        encode_locator(column)
    )
}

fn encode_locator(part: &str) -> String {
    part.replace('/', "_")
}

/// Relation supplied by a backend before caps and foreign-key resolution.
#[derive(Clone, Debug)]
pub struct InputRelation {
    pub schema: String,
    pub name: String,
    pub kind: String,
    pub comment: Option<String>,
    pub view_definition: Option<String>,
    pub columns: Vec<CatalogColumn>,
    pub primary_key: Vec<String>,
    pub unique_keys: Vec<Vec<String>>,
    pub indexes: Vec<String>,
    pub foreign_keys: Vec<CatalogForeignKey>,
    pub depends: Vec<(String, String)>,
}

pub struct CatalogSnapshot {
    pub relations: Vec<CatalogProtoData>,
    pub warnings: Vec<String>,
}

pub trait CatalogBackend: Send + Sync {
    fn load(&self, database: &str, options: &CatalogOptions) -> Result<CatalogSnapshot>;
}

/// Scripted catalog. CI uses this instead of a database service.
#[derive(Default)]
pub struct MemoryCatalogBackend {
    relations: Mutex<Vec<InputRelation>>,
    denied: Mutex<Vec<String>>,
}

impl MemoryCatalogBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, relation: InputRelation) {
        self.relations.lock().unwrap().push(relation);
    }

    pub fn deny_schema(&self, schema: &str) {
        self.denied.lock().unwrap().push(schema.to_string());
    }
}

impl CatalogBackend for MemoryCatalogBackend {
    fn load(&self, database: &str, options: &CatalogOptions) -> Result<CatalogSnapshot> {
        let (allow, mut warnings) = schema_allowlist(&options.schemas);
        let denied = self.denied.lock().unwrap().clone();
        let mut raw = Vec::new();
        for schema in &allow {
            if denied.iter().any(|name| name == schema) {
                warnings.push(format!("catalog privilege denied for schema {schema}"));
                continue;
            }
            for relation in self.relations.lock().unwrap().iter() {
                if &relation.schema == schema {
                    raw.push(relation.clone());
                }
            }
        }
        Ok(assemble(database, options, raw, warnings))
    }
}

pub struct PostgresCatalogBackend {
    location: CatalogLocation,
}

impl PostgresCatalogBackend {
    pub fn new(location: CatalogLocation) -> Self {
        Self { location }
    }
}

impl CatalogBackend for PostgresCatalogBackend {
    fn load(&self, _database: &str, options: &CatalogOptions) -> Result<CatalogSnapshot> {
        let timeout = statement_timeout(options);
        for statement in catalog_statements(timeout) {
            guard_statement(&statement)?;
        }
        let secrets = catalog_secrets(self.location.password.as_deref());
        let loaded = load_postgres(&self.location, options).map_err(|err| {
            GnosisError::Pipeline(redact_catalog_text(&err.to_string(), &secrets))
        })?;
        Ok(loaded)
    }
}

fn load_postgres(location: &CatalogLocation, options: &CatalogOptions) -> Result<CatalogSnapshot> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .thread_name("gnosis-catalog")
        .build()
        .map_err(|err| GnosisError::Pipeline(format!("tokio runtime: {err}")))?;
    runtime.block_on(load_postgres_async(location, options))
}

async fn load_postgres_async(
    location: &CatalogLocation,
    options: &CatalogOptions,
) -> Result<CatalogSnapshot> {
    let (allow, mut warnings) = schema_allowlist(&options.schemas);
    let timeout = statement_timeout(options);
    let mut config = tokio_postgres::Config::new();
    config.host(&location.host);
    config.port(location.port);
    config.dbname(&location.database);
    if let Some(user) = &location.user {
        config.user(user);
    }
    if let Some(password) = &location.password {
        config.password(password);
    } else if let Ok(password) = std::env::var("PGPASSWORD") {
        config.password(password);
    }
    let loopback = location.host == "localhost" || location.host == "127.0.0.1";
    let client = if loopback {
        config.ssl_mode(tokio_postgres::config::SslMode::Disable);
        let (client, connection) = config
            .connect(tokio_postgres::NoTls)
            .await
            .map_err(|err| GnosisError::Pipeline(format!("catalog connect: {err}")))?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client
    } else {
        let tls = rustls_connector()?;
        let (client, connection) = config
            .connect(tls)
            .await
            .map_err(|err| GnosisError::Pipeline(format!("catalog connect: {err}")))?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client
    };
    client
        .batch_execute("BEGIN")
        .await
        .map_err(|err| GnosisError::Pipeline(format!("catalog begin: {err}")))?;
    client
        .batch_execute("SET TRANSACTION READ ONLY")
        .await
        .map_err(|err| GnosisError::Pipeline(format!("catalog read only: {err}")))?;
    client
        .batch_execute(&format!("SET statement_timeout = {timeout}"))
        .await
        .map_err(|err| GnosisError::Pipeline(format!("catalog timeout: {err}")))?;

    let mut raw = Vec::new();
    for schema in &allow {
        match load_schema(&client, schema).await {
            Ok(relations) => raw.extend(relations),
            Err(err) if is_privilege(&err) => {
                warnings.push(format!("catalog privilege denied for schema {schema}"));
            }
            Err(err) => return Err(err),
        }
    }
    client
        .batch_execute("COMMIT")
        .await
        .map_err(|err| GnosisError::Pipeline(format!("catalog commit: {err}")))?;
    Ok(assemble(&location.database, options, raw, warnings))
}

fn rustls_connector() -> Result<tokio_postgres_rustls::MakeRustlsConnect> {
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        let _ = roots.add(cert);
    }
    let config = rustls::ClientConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()
        .map_err(|err| GnosisError::Pipeline(format!("rustls: {err}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(config))
}

fn is_privilege(err: &GnosisError) -> bool {
    let text = err.to_string().to_ascii_lowercase();
    text.contains("42501") || text.contains("permission denied") || text.contains("insufficient")
}

async fn load_schema(client: &tokio_postgres::Client, schema: &str) -> Result<Vec<InputRelation>> {
    let relations = client
        .query(RELATIONS_SQL, &[&schema])
        .await
        .map_err(|err| GnosisError::Pipeline(format!("catalog relations: {err}")))?;
    let mut by_key: BTreeMap<(String, String), InputRelation> = BTreeMap::new();
    for row in relations {
        let schema: String = row.get(0);
        let name: String = row.get(1);
        let relkind: String = row.get(2);
        let comment: Option<String> = row.get(3);
        let view_definition: Option<String> = row.get(4);
        by_key.insert(
            (schema.clone(), name.clone()),
            InputRelation {
                schema,
                name,
                kind: kind_from_relkind(&relkind).to_string(),
                comment,
                view_definition,
                columns: Vec::new(),
                primary_key: Vec::new(),
                unique_keys: Vec::new(),
                indexes: Vec::new(),
                foreign_keys: Vec::new(),
                depends: Vec::new(),
            },
        );
    }
    let columns = client
        .query(COLUMNS_SQL, &[&schema])
        .await
        .map_err(|err| GnosisError::Pipeline(format!("catalog columns: {err}")))?;
    for row in columns {
        let schema: String = row.get(0);
        let name: String = row.get(1);
        let Some(relation) = by_key.get_mut(&(schema, name)) else {
            continue;
        };
        relation.columns.push(CatalogColumn {
            name: row.get(2),
            data_type: row.get(3),
            nullable: row.get(4),
            ordinal: row.get(5),
            comment: row.get(6),
        });
    }
    let keys = client
        .query(KEYS_SQL, &[&schema])
        .await
        .map_err(|err| GnosisError::Pipeline(format!("catalog keys: {err}")))?;
    for row in keys {
        let schema: String = row.get(0);
        let name: String = row.get(1);
        let contype: String = row.get(2);
        let columns: Vec<String> = row.get(3);
        let Some(relation) = by_key.get_mut(&(schema, name)) else {
            continue;
        };
        if contype == "p" {
            relation.primary_key = columns;
        } else {
            relation.unique_keys.push(columns);
        }
    }
    let indexes = client
        .query(INDEXES_SQL, &[&schema])
        .await
        .map_err(|err| GnosisError::Pipeline(format!("catalog indexes: {err}")))?;
    for row in indexes {
        let schema: String = row.get(0);
        let name: String = row.get(1);
        let index: String = row.get(2);
        if let Some(relation) = by_key.get_mut(&(schema, name)) {
            relation.indexes.push(index);
        }
    }
    let foreign = client
        .query(FOREIGN_KEYS_SQL, &[&schema])
        .await
        .map_err(|err| GnosisError::Pipeline(format!("catalog foreign keys: {err}")))?;
    for row in foreign {
        let schema: String = row.get(0);
        let name: String = row.get(1);
        let Some(relation) = by_key.get_mut(&(schema, name)) else {
            continue;
        };
        relation.foreign_keys.push(CatalogForeignKey {
            columns: row.get(4),
            target_schema: row.get(2),
            target_relation: row.get(3),
            target_columns: row.get(5),
            target_discovered: false,
        });
    }
    let deps = client
        .query(DEPENDS_SQL, &[&schema])
        .await
        .map_err(|err| GnosisError::Pipeline(format!("catalog dependencies: {err}")))?;
    for row in deps {
        let schema: String = row.get(0);
        let name: String = row.get(1);
        let Some(relation) = by_key.get_mut(&(schema, name)) else {
            continue;
        };
        relation.depends.push((row.get(2), row.get(3)));
    }
    Ok(by_key.into_values().collect())
}

fn kind_from_relkind(relkind: &str) -> &'static str {
    match relkind {
        "v" => "view",
        "m" => "materialized_view",
        _ => "table",
    }
}

fn statement_timeout(options: &CatalogOptions) -> u64 {
    if options.statement_timeout_ms == 0 {
        DEFAULT_STATEMENT_TIMEOUT_MS
    } else {
        options.statement_timeout_ms
    }
}

fn max_relations(options: &CatalogOptions) -> usize {
    let cap = if options.max_objects == 0 {
        DEFAULT_MAX_RELATIONS
    } else {
        options.max_objects
    };
    usize::try_from(cap).unwrap_or(usize::MAX)
}

fn cap_text(value: &str, cap: usize) -> String {
    value.chars().take(cap).collect()
}

fn assemble(
    database: &str,
    options: &CatalogOptions,
    mut raw: Vec<InputRelation>,
    mut warnings: Vec<String>,
) -> CatalogSnapshot {
    raw.sort_by(|a, b| (&a.schema, &a.name).cmp(&(&b.schema, &b.name)));
    let cap = max_relations(options);
    if raw.len() > cap {
        warnings.push(format!(
            "catalog relation cap {cap} reached; further relations were skipped"
        ));
        raw.truncate(cap);
    }
    let discovered: BTreeSet<(String, String)> = raw
        .iter()
        .map(|relation| (relation.schema.clone(), relation.name.clone()))
        .collect();
    let (allow, _) = schema_allowlist(&options.schemas);
    let allow: BTreeSet<String> = allow.into_iter().collect();
    let relations = raw
        .into_iter()
        .map(|relation| {
            let mut columns = relation.columns;
            if columns.len() > MAX_COLUMNS {
                columns.truncate(MAX_COLUMNS);
            }
            for column in &mut columns {
                if let Some(comment) = column.comment.as_mut() {
                    *comment = cap_text(comment, COMMENT_CAP);
                }
            }
            let mut indexes = relation.indexes;
            indexes.sort();
            indexes.dedup();
            if indexes.len() > MAX_INDEXES {
                indexes.truncate(MAX_INDEXES);
            }
            let mut foreign_keys = relation.foreign_keys;
            for key in &mut foreign_keys {
                key.target_discovered =
                    discovered.contains(&(key.target_schema.clone(), key.target_relation.clone()));
            }
            let mut unresolved = Vec::new();
            for (schema, name) in relation.depends {
                if !allow.contains(&schema) {
                    unresolved.push(format!("{schema}.{name}"));
                }
            }
            unresolved.sort();
            unresolved.dedup();
            CatalogProtoData {
                database: database.to_string(),
                schema: relation.schema,
                relation: relation.name,
                kind: relation.kind,
                comment: relation.comment.map(|text| cap_text(&text, COMMENT_CAP)),
                view_definition: relation
                    .view_definition
                    .map(|text| cap_text(&text, VIEW_DEF_CAP)),
                columns,
                primary_key: relation.primary_key,
                unique_keys: relation.unique_keys,
                indexes,
                foreign_keys,
                unresolved,
            }
        })
        .collect();
    CatalogSnapshot {
        relations,
        warnings,
    }
}

pub fn projection_etag(proto: &CatalogProtoData) -> String {
    let mut text = format!(
        "{}|{}|{}|{}|{}|{}",
        proto.database,
        proto.schema,
        proto.relation,
        proto.kind,
        proto.comment.as_deref().unwrap_or(""),
        proto.view_definition.as_deref().unwrap_or("")
    );
    for column in &proto.columns {
        text.push_str(&format!(
            "|col:{}:{}:{}:{}:{}",
            column.name,
            column.data_type,
            column.nullable,
            column.ordinal,
            column.comment.as_deref().unwrap_or("")
        ));
    }
    text.push_str(&format!("|pk:{}", proto.primary_key.join(",")));
    for key in &proto.unique_keys {
        text.push_str(&format!("|uq:{}", key.join(",")));
    }
    text.push_str(&format!("|ix:{}", proto.indexes.join(",")));
    for key in &proto.foreign_keys {
        text.push_str(&format!(
            "|fk:{}->{}:{}:{}:{}",
            key.columns.join(","),
            key.target_schema,
            key.target_relation,
            key.target_columns.join(","),
            key.target_discovered
        ));
    }
    text.push_str(&format!("|un:{}", proto.unresolved.join(",")));
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

pub struct CatalogConnector {
    location: CatalogLocation,
    options: CatalogOptions,
    backend: Arc<dyn CatalogBackend>,
    protos: Mutex<BTreeMap<String, CatalogProtoData>>,
    warnings: Mutex<Vec<String>>,
}

impl CatalogConnector {
    pub fn new(
        location: CatalogLocation,
        options: CatalogOptions,
        backend: Arc<dyn CatalogBackend>,
    ) -> Self {
        Self {
            location,
            options,
            backend,
            protos: Mutex::new(BTreeMap::new()),
            warnings: Mutex::new(Vec::new()),
        }
    }

    pub fn proto_for(&self, id: &ObjectId) -> Option<CatalogProtoData> {
        self.protos.lock().unwrap().get(id.as_str()).cloned()
    }

    pub fn warnings(&self) -> Vec<String> {
        self.warnings.lock().unwrap().clone()
    }

    pub fn discover(
        &self,
        events: &Sender<PipelineEvent>,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Vec<crate::connectors::types::ObjectDescriptor>> {
        let snapshot = self.backend.load(&self.location.database, &self.options)?;
        let mut warnings = self.warnings.lock().unwrap();
        for warning in snapshot.warnings {
            if !warnings.iter().any(|existing| existing == &warning) {
                let _ = events.send(PipelineEvent::Warning {
                    message: warning.clone(),
                });
                warnings.push(warning);
            }
        }
        drop(warnings);
        let mut objects = Vec::new();
        for proto in snapshot.relations {
            if cancel.load(Ordering::Relaxed) {
                return Err(GnosisError::Cancelled);
            }
            let object = descriptor_for(&proto);
            let _ = events.send(PipelineEvent::ObjectDiscovered {
                id: object.id.clone(),
                path: object.relative_path.clone(),
            });
            self.protos
                .lock()
                .unwrap()
                .insert(object.id.as_str().to_string(), proto);
            objects.push(object);
        }
        Ok(objects)
    }
}

fn descriptor_for(proto: &CatalogProtoData) -> crate::connectors::types::ObjectDescriptor {
    let locator = relation_locator(&proto.database, &proto.schema, &proto.relation);
    let relative = PathBuf::from(format!("{}/{}", proto.schema, proto.name_safe()));
    crate::connectors::types::ObjectDescriptor {
        id: ObjectId::new(format!(
            "obj:db:{}/{}/{}",
            proto.database, proto.schema, proto.relation
        )),
        path: PathBuf::from(&locator),
        relative_path: relative,
        is_dir: false,
        size: 0,
        modified: None,
        media_type: "application/x-gnosis-catalog".into(),
        extension: None,
        etag: Some(projection_etag(proto)),
    }
}

trait NameSafe {
    fn name_safe(&self) -> String;
}

impl NameSafe for CatalogProtoData {
    fn name_safe(&self) -> String {
        self.relation.replace('/', "_")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CatalogLocation, CatalogOptions, ScanConfig, ScanMetrics, ScanSource};
    use crate::connectors::types::ProtoData;
    use crate::intrinsic::{extract_intrinsic, FactValue};
    use crate::knowledge_db::KnowledgeDb;
    use crate::pipeline::Pipeline;
    use crate::providers::default_registry;
    use crate::providers::CatalogProvider;
    use crate::status::UnderstandingStatus;
    use crate::BytesContentReader;
    use crate::UnderstandingProvider;
    use crossbeam_channel::unbounded;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    fn relation(schema: &str, name: &str) -> InputRelation {
        InputRelation {
            schema: schema.into(),
            name: name.into(),
            kind: "table".into(),
            comment: None,
            view_definition: None,
            columns: Vec::new(),
            primary_key: Vec::new(),
            unique_keys: Vec::new(),
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
            depends: Vec::new(),
        }
    }

    fn location() -> CatalogLocation {
        parse_catalog_uri("postgresql://user:s3cret@localhost:5432/trading").unwrap()
    }

    fn options(schemas: &[&str], max_objects: u64) -> CatalogOptions {
        CatalogOptions {
            schemas: schemas.iter().map(|schema| (*schema).to_string()).collect(),
            max_objects,
            statement_timeout_ms: 5000,
        }
    }

    #[test]
    fn uri_drops_userinfo_and_quote_doubles_quotes() {
        let loc = location();
        assert_eq!(loc.database, "trading");
        assert_eq!(loc.password.as_deref(), Some("s3cret"));
        assert_eq!(loc.display_root(), PathBuf::from("db://trading"));
        let rendered = format!("{loc:?}");
        assert!(!rendered.contains("s3cret"));
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(
            relation_locator("trading", "public", "Order"),
            "db://trading/public/Order"
        );
    }

    #[test]
    fn statements_are_read_only_and_never_name_a_user_table() {
        let statements = catalog_statements(5000);
        for statement in &statements {
            guard_statement(statement).unwrap();
            assert!(!statement.to_ascii_lowercase().contains("orders"));
            assert!(!statement.contains("trading.positions"));
        }
        assert!(guard_statement("SELECT * FROM orders").is_err());
        assert!(guard_statement("DELETE FROM pg_catalog.pg_class").is_err());
    }

    #[test]
    fn secrets_in_errors_are_redacted() {
        let text = redact_catalog_text(
            "connect failed for user password s3cret AKIAIOSFODNN7EXAMPLE",
            &catalog_secrets(Some("s3cret")),
        );
        assert!(!text.contains("s3cret"));
        assert!(!text.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(text.contains("REDACTED"));
    }

    #[test]
    fn public_does_not_include_secret_and_schema_flag_includes_risk() {
        let backend = MemoryCatalogBackend::new();
        backend.insert(relation("public", "orders"));
        backend.insert(relation("secret", "positions"));
        backend.insert(relation("risk", "limits"));
        let snapshot = backend.load("trading", &options(&[], 200)).unwrap();
        let names: Vec<_> = snapshot
            .relations
            .iter()
            .map(|relation| relation.schema.as_str())
            .collect();
        assert_eq!(names, vec!["public"]);
        let snapshot = backend.load("trading", &options(&["risk"], 200)).unwrap();
        let mut names: Vec<_> = snapshot
            .relations
            .iter()
            .map(|relation| format!("{}.{}", relation.schema, relation.relation))
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["public.orders".to_string(), "risk.limits".to_string()]
        );
        assert!(!names.iter().any(|name| name.contains("secret")));
        let (allow, warnings) = schema_allowlist(&["pg_catalog".into(), "pg_temp_1".into()]);
        assert!(!allow.iter().any(|name| schema_is_system(name)));
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("pg_catalog")));
    }

    #[test]
    fn privilege_warning_skips_one_schema() {
        let backend = MemoryCatalogBackend::new();
        backend.insert(relation("public", "orders"));
        backend.insert(relation("audit", "events"));
        backend.deny_schema("audit");
        let snapshot = backend.load("trading", &options(&["audit"], 200)).unwrap();
        assert_eq!(snapshot.relations.len(), 1);
        assert_eq!(snapshot.relations[0].relation, "orders");
        assert!(snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("audit")));
    }

    #[test]
    fn caps_comment_and_relation_count() {
        let backend = MemoryCatalogBackend::new();
        let mut wide = relation("public", "aaa");
        wide.comment = Some("x".repeat(COMMENT_CAP + 1));
        backend.insert(wide);
        backend.insert(relation("public", "second"));
        let snapshot = backend.load("trading", &options(&[], 1)).unwrap();
        assert_eq!(snapshot.relations.len(), 1);
        assert_eq!(
            snapshot.relations[0].comment.as_ref().map(String::len),
            Some(COMMENT_CAP)
        );
        assert!(snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("relation cap")));
        let mut heavy = relation("public", "heavy");
        heavy.columns = (0..MAX_COLUMNS + 1)
            .map(|index| CatalogColumn {
                name: format!("c{index}"),
                data_type: "text".into(),
                nullable: true,
                ordinal: index as i32,
                comment: None,
            })
            .collect();
        heavy.indexes = (0..MAX_INDEXES + 1)
            .map(|index| format!("ix{index}"))
            .collect();
        let backend = MemoryCatalogBackend::new();
        backend.insert(heavy);
        let snapshot = backend.load("trading", &options(&[], 200)).unwrap();
        assert_eq!(snapshot.relations[0].columns.len(), MAX_COLUMNS);
        assert_eq!(snapshot.relations[0].indexes.len(), MAX_INDEXES);
    }

    #[test]
    fn comment_stays_literal_and_composite_key_lists_every_column() {
        let backend = MemoryCatalogBackend::new();
        let mut orders = relation("public", "a\"b");
        orders.comment = Some("not a synonym for risk".into());
        orders.columns = vec![
            CatalogColumn {
                name: "desk".into(),
                data_type: "text".into(),
                nullable: false,
                ordinal: 1,
                comment: Some("desk code".into()),
            },
            CatalogColumn {
                name: "book".into(),
                data_type: "text".into(),
                nullable: false,
                ordinal: 2,
                comment: None,
            },
        ];
        orders.primary_key = vec!["desk".into(), "book".into()];
        orders.foreign_keys = vec![
            CatalogForeignKey {
                columns: vec!["book".into()],
                target_schema: "secret".into(),
                target_relation: "books".into(),
                target_columns: vec!["id".into()],
                target_discovered: false,
            },
            CatalogForeignKey {
                columns: vec!["desk".into()],
                target_schema: "public".into(),
                target_relation: "desks".into(),
                target_columns: vec!["id".into()],
                target_discovered: false,
            },
        ];
        let mut view = relation("public", "open_orders");
        view.kind = "view".into();
        view.view_definition = Some("select 1".into());
        view.depends = vec![("secret".into(), "positions".into())];
        backend.insert(orders);
        backend.insert(view);
        let mut desks = relation("public", "desks");
        desks.columns = vec![CatalogColumn {
            name: "id".into(),
            data_type: "text".into(),
            nullable: false,
            ordinal: 1,
            comment: None,
        }];
        backend.insert(desks);
        let snapshot = backend.load("trading", &options(&[], 200)).unwrap();
        let quoted = snapshot
            .relations
            .iter()
            .find(|relation| relation.relation == "a\"b")
            .unwrap();
        assert_eq!(quoted.comment.as_deref(), Some("not a synonym for risk"));
        assert_eq!(quoted.primary_key, vec!["desk", "book"]);
        assert!(!quoted.foreign_keys[0].target_discovered);
        assert!(quoted.foreign_keys[1].target_discovered);
        let view = snapshot
            .relations
            .iter()
            .find(|relation| relation.relation == "open_orders")
            .unwrap();
        assert_eq!(view.unresolved, vec!["secret.positions".to_string()]);

        let object = descriptor_for(quoted);
        let proto = ProtoData {
            catalog: Some(quoted.clone()),
            ..ProtoData::default()
        };
        let evidence = extract_intrinsic(&object, &proto, b"");
        let comment = evidence
            .intrinsic
            .facts
            .iter()
            .find(|fact| fact.key == "db:comment")
            .unwrap();
        assert_eq!(
            comment.value,
            FactValue::String("not a synonym for risk".into())
        );
        assert!(evidence.intrinsic.facts.iter().any(|fact| {
            fact.key == "db:column:desk:comment"
                && fact.value == FactValue::String("desk code".into())
        }));
        let mut reader = BytesContentReader::new(Vec::new());
        let analyzed = CatalogProvider
            .analyze(&object, &proto, &mut reader)
            .unwrap();
        assert_eq!(analyzed.status, UnderstandingStatus::PartiallyUnderstood);
        assert_ne!(analyzed.status, UnderstandingStatus::Understood);
        let relation_entity = analyzed
            .record
            .entities
            .iter()
            .find(|entity| entity.kind == "relation")
            .unwrap();
        assert!(relation_entity
            .attributes
            .get("primary_key")
            .unwrap()
            .contains("desk"));
        assert!(relation_entity
            .attributes
            .get("primary_key")
            .unwrap()
            .contains("book"));
        assert!(analyzed
            .record
            .relationships
            .iter()
            .any(|edge| edge.kind == "contains"));
        assert!(analyzed
            .record
            .relationships
            .iter()
            .any(|edge| edge.kind == "references"));
    }

    fn catalog_config(dir: &std::path::Path, max_objects: u64) -> ScanConfig {
        let mut config = ScanConfig::with_source(ScanSource::Catalog {
            location: CatalogLocation {
                host: "localhost".into(),
                port: 5432,
                database: "trading".into(),
                user: None,
                password: None,
            },
            options: options(&[], max_objects),
        });
        config.concurrency = 1;
        config.job_db_path = dir.join("jobs.redb");
        config.knowledge_db_path = dir.join("knowledge.redb");
        config
    }

    fn run_catalog(
        config: ScanConfig,
        backend: Arc<MemoryCatalogBackend>,
        metrics: Arc<ScanMetrics>,
    ) {
        let pipeline = Pipeline::with_knowledge_and_metrics(
            config.clone(),
            default_registry(),
            Arc::new(crate::jobs::RedbJobStore::open(&config.job_db_path).unwrap()),
            metrics,
            Arc::new(KnowledgeDb::open(&config.knowledge_db_path).unwrap()),
        )
        .with_catalog_backend(backend);
        let (tx, rx) = unbounded();
        let drain = std::thread::spawn(move || while rx.recv().is_ok() {});
        pipeline.run(tx).unwrap();
        drain.join().unwrap();
    }

    #[test]
    fn second_scan_skips_and_a_dropped_relation_disappears() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"workbook beside a table").unwrap();
        let mut files = ScanConfig::with_root(dir.path());
        files.concurrency = 1;
        files.job_db_path = dir.path().join("jobs.redb");
        files.knowledge_db_path = dir.path().join("knowledge.redb");
        let pipeline = Pipeline::new(files, default_registry()).unwrap();
        let (tx, rx) = unbounded();
        let drain = std::thread::spawn(move || while rx.recv().is_ok() {});
        pipeline.run(tx).unwrap();
        drain.join().unwrap();

        let backend = Arc::new(MemoryCatalogBackend::new());
        backend.insert(relation("public", "orders"));
        backend.insert(relation("public", "books"));
        let metrics = Arc::new(ScanMetrics::new());
        run_catalog(
            catalog_config(dir.path(), 200),
            backend.clone(),
            Arc::clone(&metrics),
        );
        let metrics = Arc::new(ScanMetrics::new());
        run_catalog(
            catalog_config(dir.path(), 200),
            backend.clone(),
            Arc::clone(&metrics),
        );
        assert!(metrics.objects_skipped.load(Ordering::Relaxed) >= 2);

        backend
            .relations
            .lock()
            .unwrap()
            .retain(|relation| relation.name != "books");
        run_catalog(
            catalog_config(dir.path(), 200),
            backend,
            Arc::new(ScanMetrics::new()),
        );
        let db = KnowledgeDb::open(dir.path().join("knowledge.redb")).unwrap();
        let ids: Vec<_> = db
            .load_all()
            .unwrap()
            .into_iter()
            .map(|row| row.object.descriptor.id.as_str().to_string())
            .collect();
        assert!(ids.iter().any(|id| id.contains("notes.txt")));
        assert!(ids.iter().any(|id| id == "obj:db:trading/public/orders"));
        assert!(!ids.iter().any(|id| id.contains("books")));
    }
}
