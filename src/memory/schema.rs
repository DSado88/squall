//! DuckDB schema definitions, migration support, and global memory types.
//!
//! All items are gated behind `#[cfg(feature = "global-memory")]`.

use std::fmt;

// ---------------------------------------------------------------------------
// DDL constants
// ---------------------------------------------------------------------------

pub const DDL_SCHEMA_VERSION: &str = "\
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    applied_at BIGINT NOT NULL
);";

pub const DDL_PROJECTS: &str = "\
CREATE TABLE IF NOT EXISTS projects (
    project_id TEXT PRIMARY KEY,
    root_path TEXT,
    git_remote TEXT,
    language_primary TEXT,
    first_seen_ts BIGINT NOT NULL,
    last_seen_ts BIGINT NOT NULL
);";

pub const DDL_MODEL_EVENTS: &str = "\
CREATE TABLE IF NOT EXISTS model_events (
    event_uid TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    ts BIGINT NOT NULL,
    model_key TEXT NOT NULL,
    status TEXT NOT NULL,
    partial BOOLEAN NOT NULL DEFAULT FALSE,
    reason TEXT,
    latency_ms INTEGER NOT NULL,
    prompt_tokens INTEGER,
    FOREIGN KEY (project_id) REFERENCES projects(project_id)
);";

pub const DDL_INDEX_EVENTS_TS: &str = "\
CREATE INDEX IF NOT EXISTS idx_events_ts ON model_events(ts);";

pub const DDL_INDEX_EVENTS_MODEL: &str = "\
CREATE INDEX IF NOT EXISTS idx_events_model ON model_events(model_key);";

/// All DDL statements for schema version 1, in order.
pub const SCHEMA_V1: &[&str] = &[
    DDL_SCHEMA_VERSION,
    DDL_PROJECTS,
    DDL_MODEL_EVENTS,
    DDL_INDEX_EVENTS_TS,
    DDL_INDEX_EVENTS_MODEL,
];

/// Current schema version.
pub const CURRENT_VERSION: i32 = 1;

// ---------------------------------------------------------------------------
// Migration support
// ---------------------------------------------------------------------------

/// Apply schema migrations up to `CURRENT_VERSION`.
///
/// Uses `schema_version` table for idempotent version tracking.
/// Returns the version that was applied (or the already-current version).
pub fn apply_migrations(conn: &duckdb::Connection) -> Result<i32, MigrationError> {
    // Bootstrap: create schema_version table first (idempotent).
    conn.execute_batch(DDL_SCHEMA_VERSION)
        .map_err(MigrationError::Duckdb)?;

    let current = get_current_version(conn)?;

    if current >= CURRENT_VERSION {
        return Ok(current);
    }

    // Apply version 1
    if current < 1 {
        for ddl in SCHEMA_V1 {
            conn.execute_batch(ddl).map_err(MigrationError::Duckdb)?;
        }
        record_version(conn, 1)?;
    }

    // Future migrations go here:
    // if current < 2 { ... record_version(conn, 2)?; }

    Ok(CURRENT_VERSION)
}

fn get_current_version(conn: &duckdb::Connection) -> Result<i32, MigrationError> {
    let mut stmt = conn
        .prepare("SELECT COALESCE(MAX(version), 0) FROM schema_version")
        .map_err(MigrationError::Duckdb)?;
    let version: i32 = stmt
        .query_row([], |row| row.get(0))
        .map_err(MigrationError::Duckdb)?;
    Ok(version)
}

fn record_version(conn: &duckdb::Connection, version: i32) -> Result<(), MigrationError> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    conn.execute(
        "INSERT INTO schema_version (version, applied_at) VALUES (?, ?)",
        duckdb::params![version, now_ms],
    )
    .map_err(MigrationError::Duckdb)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum MigrationError {
    Duckdb(duckdb::Error),
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MigrationError::Duckdb(e) => write!(f, "DuckDB migration error: {e}"),
        }
    }
}

impl std::error::Error for MigrationError {}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single model invocation event for global cross-project tracking.
#[derive(Debug, Clone)]
pub struct ModelEvent {
    /// Unique ID: sha256(ts + model_key + latency_ms + status)[:16].
    pub event_uid: String,
    /// Project identifier (e.g. "git:a1b2c3d4e5f6g7h8" or "path:...").
    pub project_id: String,
    /// Unix timestamp in milliseconds.
    pub ts: i64,
    /// Normalized model config key (e.g. "grok", "gemini").
    pub model_key: String,
    /// "success", "error", "timeout".
    pub status: String,
    /// Whether the response was partial/truncated.
    pub partial: bool,
    /// Infrastructure failure reason (e.g. "auth_failed", "rate_limited"), or None.
    pub reason: Option<String>,
    /// Wall-clock latency in milliseconds.
    pub latency_ms: i32,
    /// Prompt token count, if available.
    pub prompt_tokens: Option<i32>,
}

impl ModelEvent {
    /// Generate a deterministic event UID from event fields.
    ///
    /// Uses sha256(project_id + timestamp + model + latency + status), takes first 16 hex chars.
    /// Includes project_id so events from different projects with the same
    /// (ts, model, latency, status) don't collide on UID.
    pub fn compute_uid(ts: i64, model_key: &str, latency_ms: i32, status: &str) -> String {
        Self::compute_uid_with_project("", ts, model_key, latency_ms, status)
    }

    /// Generate a deterministic event UID including project_id for cross-project uniqueness.
    pub fn compute_uid_with_project(
        project_id: &str,
        ts: i64,
        model_key: &str,
        latency_ms: i32,
        status: &str,
    ) -> String {
        use sha2::{Digest, Sha256};
        let input = format!("{project_id}{ts}{model_key}{latency_ms}{status}");
        let hash = Sha256::digest(input.as_bytes());
        hex::encode(&hash[..8]) // 8 bytes = 16 hex chars
    }

    /// Create a new ModelEvent, automatically computing the event_uid.
    pub fn new(
        project_id: String,
        ts: i64,
        model_key: String,
        status: String,
        partial: bool,
        reason: Option<String>,
        latency_ms: i32,
        prompt_tokens: Option<i32>,
    ) -> Self {
        let event_uid =
            Self::compute_uid_with_project(&project_id, ts, &model_key, latency_ms, &status);
        Self {
            event_uid,
            project_id,
            ts,
            model_key,
            status,
            partial,
            reason,
            latency_ms,
            prompt_tokens,
        }
    }
}

/// Project metadata for the `projects` table.
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    /// Unique project identifier (sha256-based).
    pub project_id: String,
    /// Filesystem root path of the project.
    pub root_path: Option<String>,
    /// Git remote URL (normalized).
    pub git_remote: Option<String>,
    /// Primary programming language.
    pub language_primary: Option<String>,
    /// First time this project was seen (Unix ms).
    pub first_seen_ts: i64,
    /// Last time this project was seen (Unix ms).
    pub last_seen_ts: i64,
}

// ---------------------------------------------------------------------------
// Parquet column definitions (for arrow/parquet write path)
// ---------------------------------------------------------------------------

/// Column names for the model_events Parquet schema, in canonical order.
/// Used by the write path to construct Parquet files that DuckDB can query.
pub const PARQUET_COLUMNS: &[&str] = &[
    "event_uid",
    "project_id",
    "ts",
    "model_key",
    "status",
    "partial",
    "reason",
    "latency_ms",
    "prompt_tokens",
];

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ----- ModelEvent / UID tests -----

    #[test]
    fn event_uid_is_deterministic() {
        let uid1 = ModelEvent::compute_uid(1708000000000, "grok", 25000, "success");
        let uid2 = ModelEvent::compute_uid(1708000000000, "grok", 25000, "success");
        assert_eq!(uid1, uid2);
        assert_eq!(uid1.len(), 16);
    }

    #[test]
    fn event_uid_is_hex() {
        let uid = ModelEvent::compute_uid(1708000000000, "grok", 25000, "success");
        assert!(
            uid.chars().all(|c| c.is_ascii_hexdigit()),
            "uid should be hex: {uid}"
        );
    }

    #[test]
    fn event_uid_differs_on_different_timestamp() {
        let uid1 = ModelEvent::compute_uid(1708000000000, "grok", 25000, "success");
        let uid2 = ModelEvent::compute_uid(1708000000001, "grok", 25000, "success");
        assert_ne!(uid1, uid2);
    }

    #[test]
    fn event_uid_differs_on_different_model() {
        let uid1 = ModelEvent::compute_uid(1708000000000, "grok", 25000, "success");
        let uid2 = ModelEvent::compute_uid(1708000000000, "gemini", 25000, "success");
        assert_ne!(uid1, uid2);
    }

    #[test]
    fn event_uid_differs_on_different_latency() {
        let uid1 = ModelEvent::compute_uid(1708000000000, "grok", 25000, "success");
        let uid2 = ModelEvent::compute_uid(1708000000000, "grok", 30000, "success");
        assert_ne!(uid1, uid2);
    }

    #[test]
    fn event_uid_differs_on_different_status() {
        let uid1 = ModelEvent::compute_uid(1708000000000, "grok", 25000, "success");
        let uid2 = ModelEvent::compute_uid(1708000000000, "grok", 25000, "error");
        assert_ne!(uid1, uid2);
    }

    #[test]
    fn model_event_new_populates_all_fields() {
        let event = ModelEvent::new(
            "git:abc123".to_string(),
            1708000000000,
            "grok".to_string(),
            "success".to_string(),
            true,
            Some("rate_limited".to_string()),
            25000,
            Some(4200),
        );
        assert_eq!(event.event_uid.len(), 16);
        assert_eq!(event.project_id, "git:abc123");
        assert_eq!(event.ts, 1708000000000);
        assert_eq!(event.model_key, "grok");
        assert_eq!(event.status, "success");
        assert!(event.partial);
        assert_eq!(event.reason.as_deref(), Some("rate_limited"));
        assert_eq!(event.latency_ms, 25000);
        assert_eq!(event.prompt_tokens, Some(4200));
    }

    #[test]
    fn model_event_new_with_none_optionals() {
        let event = ModelEvent::new(
            "path:deadbeef".to_string(),
            1708000000000,
            "codex".to_string(),
            "error".to_string(),
            false,
            None,
            60000,
            None,
        );
        assert_eq!(event.event_uid.len(), 16);
        assert!(event.reason.is_none());
        assert!(event.prompt_tokens.is_none());
        assert!(!event.partial);
    }

    #[test]
    fn model_event_new_uid_matches_compute_uid() {
        let ts = 1708000000000_i64;
        let model = "grok";
        let latency = 25000;
        let status = "success";
        let event = ModelEvent::new(
            "git:abc".to_string(),
            ts,
            model.to_string(),
            status.to_string(),
            false,
            None,
            latency,
            None,
        );
        let expected = ModelEvent::compute_uid_with_project("git:abc", ts, model, latency, status);
        assert_eq!(event.event_uid, expected);
    }

    // ----- DDL / schema constants tests -----

    #[test]
    fn schema_v1_has_all_ddl() {
        assert_eq!(SCHEMA_V1.len(), 5);
        assert!(SCHEMA_V1[0].contains("schema_version"));
        assert!(SCHEMA_V1[1].contains("projects"));
        assert!(SCHEMA_V1[2].contains("model_events"));
        assert!(SCHEMA_V1[3].contains("idx_events_ts"));
        assert!(SCHEMA_V1[4].contains("idx_events_model"));
    }

    #[test]
    fn ddl_constants_are_valid_sql() {
        // Parse-check: DuckDB should accept all DDL without error
        let conn = duckdb::Connection::open_in_memory().unwrap();
        for (i, ddl) in SCHEMA_V1.iter().enumerate() {
            conn.execute_batch(ddl)
                .unwrap_or_else(|e| panic!("SCHEMA_V1[{i}] is invalid SQL: {e}"));
        }
    }

    #[test]
    fn ddl_model_events_has_foreign_key() {
        assert!(
            DDL_MODEL_EVENTS.contains("FOREIGN KEY"),
            "model_events should have FK to projects"
        );
        assert!(DDL_MODEL_EVENTS.contains("project_id"));
    }

    #[test]
    fn parquet_columns_match_schema() {
        assert_eq!(PARQUET_COLUMNS.len(), 9);
        for col in PARQUET_COLUMNS {
            assert!(
                DDL_MODEL_EVENTS.contains(col),
                "column '{col}' not found in DDL_MODEL_EVENTS"
            );
        }
    }

    #[test]
    fn current_version_is_positive() {
        assert!(CURRENT_VERSION >= 1);
    }

    // ----- DuckDB migration integration tests -----

    #[test]
    fn apply_migrations_creates_tables() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        let version = apply_migrations(&conn).unwrap();
        assert_eq!(version, CURRENT_VERSION);

        // Verify tables exist by querying them
        conn.execute_batch("SELECT COUNT(*) FROM schema_version")
            .unwrap();
        conn.execute_batch("SELECT COUNT(*) FROM projects")
            .unwrap();
        conn.execute_batch("SELECT COUNT(*) FROM model_events")
            .unwrap();
    }

    #[test]
    fn apply_migrations_is_idempotent() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        let v1 = apply_migrations(&conn).unwrap();
        let v2 = apply_migrations(&conn).unwrap();
        assert_eq!(v1, v2);

        // Only one version record
        let mut stmt = conn
            .prepare("SELECT COUNT(*) FROM schema_version")
            .unwrap();
        let count: i32 = stmt.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn apply_migrations_records_version_with_timestamp() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();

        let mut stmt = conn
            .prepare("SELECT version, applied_at FROM schema_version WHERE version = 1")
            .unwrap();
        let (version, applied_at): (i32, i64) = stmt
            .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        assert_eq!(version, 1);
        // applied_at should be a reasonable epoch-ms (after 2024-01-01)
        assert!(
            applied_at > 1_704_067_200_000,
            "applied_at should be a recent timestamp in ms: {applied_at}"
        );
    }

    #[test]
    fn schema_supports_insert_and_query() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();

        // Insert a project
        conn.execute(
            "INSERT INTO projects (project_id, first_seen_ts, last_seen_ts) VALUES (?, ?, ?)",
            duckdb::params!["git:test123", 1708000000000_i64, 1708000000000_i64],
        )
        .unwrap();

        // Insert a model event
        let uid = ModelEvent::compute_uid(1708000000000, "grok", 25000, "success");
        conn.execute(
            "INSERT INTO model_events (event_uid, project_id, ts, model_key, status, partial, latency_ms, prompt_tokens) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            duckdb::params![uid, "git:test123", 1708000000000_i64, "grok", "success", false, 25000, 4200],
        )
        .unwrap();

        // Query it back
        let mut stmt = conn
            .prepare("SELECT model_key, latency_ms FROM model_events WHERE event_uid = ?")
            .unwrap();
        let (model, latency): (String, i32) = stmt
            .query_row(duckdb::params![uid], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        assert_eq!(model, "grok");
        assert_eq!(latency, 25000);
    }

    #[test]
    fn schema_enforces_unique_event_uid() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        apply_migrations(&conn).unwrap();

        conn.execute(
            "INSERT INTO projects (project_id, first_seen_ts, last_seen_ts) VALUES (?, ?, ?)",
            duckdb::params!["git:test", 1708000000000_i64, 1708000000000_i64],
        )
        .unwrap();

        let uid = ModelEvent::compute_uid(1708000000000, "grok", 25000, "success");
        conn.execute(
            "INSERT INTO model_events (event_uid, project_id, ts, model_key, status, partial, latency_ms) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            duckdb::params![uid, "git:test", 1708000000000_i64, "grok", "success", false, 25000],
        )
        .unwrap();

        // Duplicate insert should fail
        let result = conn.execute(
            "INSERT INTO model_events (event_uid, project_id, ts, model_key, status, partial, latency_ms) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            duckdb::params![uid, "git:test", 1708000000000_i64, "grok", "success", false, 25000],
        );
        assert!(result.is_err(), "duplicate event_uid should be rejected");
    }
}
