//! Global cross-project memory backed by DuckDB + Parquet files.
//!
//! Architecture:
//! - `GlobalWriter` is the public API, holding an `mpsc::Sender<DbCommand>`
//! - `DbWorker` runs on `std::thread::spawn` (DuckDB is sync), owns the DuckDB connection
//! - Writes go to individual Parquet files (lock-free, no DuckDB file lock needed)
//! - Reads query DuckDB over parquet glob + compacted DB
//! - Periodic MERGE compacts Parquet files into the main DuckDB table
//!
//! All code is gated with `#[cfg(feature = "global-memory")]` at the module level.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::params;

use super::schema::{self, ModelEvent, ProjectInfo};
use crate::tools::review::ReviewModelResult;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Non-blocking handle for the global memory actor.
///
/// Sends commands to a background `DbWorker` via `std::sync::mpsc`.
/// All methods are non-blocking from async context (mpsc::Sender::send is
/// guaranteed non-blocking).
pub struct GlobalWriter {
    tx: mpsc::SyncSender<DbCommand>,
    events_dir: PathBuf,
    worker_handle: Option<std::thread::JoinHandle<()>>,
}

/// Recommendations returned from a global DuckDB query.
#[derive(Debug, Default)]
pub struct GlobalRecommendations {
    pub models: Vec<GlobalModelStats>,
}

/// Per-model stats from global aggregation.
#[derive(Debug)]
pub struct GlobalModelStats {
    pub model_key: String,
    pub success_rate: f64,
    pub sample_count: u64,
    pub avg_latency_ms: f64,
    pub p95_latency_ms: f64,
}

// ---------------------------------------------------------------------------
// Actor commands
// ---------------------------------------------------------------------------

enum DbCommand {
    /// Write model events as a Parquet file (lock-free, no DuckDB file needed).
    WriteParquet {
        events: Vec<ModelEvent>,
        project: ProjectInfo,
    },
    /// Query global recommendations (excluding a specific project).
    QueryRecommendations {
        exclude_project_id: Option<String>,
        reply: tokio::sync::oneshot::Sender<Result<GlobalRecommendations, String>>,
    },
    /// Merge pending Parquet files into the compacted DuckDB table.
    MergeParquet,
    /// Bootstrap: ingest local models.md events into DuckDB.
    Bootstrap {
        models_md_path: PathBuf,
        project_id: String,
        id_to_key: std::collections::HashMap<String, String>,
    },
    /// Shut down the worker thread.
    Shutdown,
}

// ---------------------------------------------------------------------------
// GlobalWriter implementation
// ---------------------------------------------------------------------------

impl GlobalWriter {
    /// Create a new GlobalWriter, spawning the background worker thread.
    ///
    /// Returns `None` if:
    /// - Directory creation fails
    /// - DuckDB fails to open
    /// - Worker thread fails to spawn
    pub fn new(db_path: PathBuf) -> Option<Self> {
        let events_dir = db_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("events");

        // Ensure directories exist
        if let Err(e) = std::fs::create_dir_all(&events_dir) {
            tracing::warn!("global memory: cannot create events dir: {e}");
            return None;
        }
        if let Some(parent) = db_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("global memory: cannot create db dir: {e}");
                return None;
            }
        }

        // Bounded channel: prevents unbounded memory growth if worker is slow/stuck.
        // 128 slots ≈ ~10 reviews worth of events buffered before backpressure.
        let (tx, rx) = mpsc::sync_channel(128);
        let worker_db_path = db_path.clone();
        let worker_events_dir = events_dir.clone();

        let builder = std::thread::Builder::new().name("squall-global-db".into());
        let handle = match builder.spawn(move || {
            DbWorker::run(rx, worker_db_path, worker_events_dir);
        }) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("global memory: failed to spawn worker thread: {e}");
                return None;
            }
        };

        Some(Self {
            tx,
            events_dir,
            worker_handle: Some(handle),
        })
    }

    /// Log model events from a completed review.
    ///
    /// Converts `ReviewModelResult` entries into `ModelEvent` structs and sends
    /// them to the worker as a `WriteParquet` command. Non-blocking.
    pub fn log_events(
        &self,
        results: &[ReviewModelResult],
        prompt_len: usize,
        project_id: &str,
        working_directory: Option<&str>,
        id_to_key: Option<&std::collections::HashMap<String, String>>,
    ) {
        let now_ms = epoch_ms();

        let events: Vec<ModelEvent> = results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let raw_model = &r.model;
                let model_key = id_to_key
                    .and_then(|map| map.get(raw_model.as_str()))
                    .cloned()
                    .unwrap_or_else(|| raw_model.clone());

                let status = format!("{:?}", r.status).to_lowercase();
                // Add index to timestamp for uniqueness when multiple events
                // share the same (ts, model, latency, status) tuple.
                let ts_with_idx = now_ms as i64 + i as i64;

                ModelEvent::new(
                    project_id.to_string(),
                    ts_with_idx,
                    model_key,
                    status,
                    r.partial,
                    r.reason.clone(),
                    r.latency_ms as i32,
                    Some(prompt_len as i32),
                )
            })
            .collect();

        if events.is_empty() {
            return;
        }

        let project = ProjectInfo {
            project_id: project_id.to_string(),
            root_path: working_directory.map(|s| s.to_string()),
            git_remote: None, // populated by context.rs compute_project_id
            language_primary: None,
            first_seen_ts: now_ms as i64,
            last_seen_ts: now_ms as i64,
        };

        if let Err(e) = self.tx.try_send(DbCommand::WriteParquet { events, project }) {
            tracing::warn!("global memory: failed to send WriteParquet command: {e}");
        }
    }

    /// Query global model recommendations, excluding the current project.
    ///
    /// Returns a `oneshot::Receiver` that the caller should `.await` in async context.
    /// The worker sends the result on the oneshot channel after processing the query.
    pub fn query_recommendations(
        &self,
        exclude_project_id: Option<&str>,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<GlobalRecommendations, String>>, String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(DbCommand::QueryRecommendations {
                exclude_project_id: exclude_project_id.map(|s| s.to_string()),
                reply: reply_tx,
            })
            .map_err(|e| format!("global memory: worker dead: {e}"))?;

        Ok(reply_rx)
    }

    /// Trigger a merge of pending Parquet files into the compacted DuckDB table.
    pub fn trigger_merge(&self) {
        if let Err(e) = self.tx.try_send(DbCommand::MergeParquet) {
            tracing::warn!("global memory: failed to send MergeParquet command: {e}");
        }
    }

    /// Send a bootstrap command to ingest local models.md into DuckDB.
    ///
    /// Fire-and-forget: uses `try_send` to avoid blocking the caller.
    pub fn send_bootstrap(
        &self,
        models_md_path: PathBuf,
        project_id: String,
        id_to_key: std::collections::HashMap<String, String>,
    ) {
        if let Err(e) = self.tx.try_send(DbCommand::Bootstrap {
            models_md_path,
            project_id,
            id_to_key,
        }) {
            tracing::warn!("global memory: failed to send Bootstrap command: {e}");
        }
    }

    /// Path to the events directory (for testing/diagnostics).
    pub fn events_dir(&self) -> &Path {
        &self.events_dir
    }
}

impl Drop for GlobalWriter {
    fn drop(&mut self) {
        let _ = self.tx.send(DbCommand::Shutdown);
        // Wait for the worker to finish its final merge before returning.
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Background worker
// ---------------------------------------------------------------------------

struct DbWorker {
    rx: mpsc::Receiver<DbCommand>,
    db_path: PathBuf,
    events_dir: PathBuf,
    /// DuckDB connection — opened lazily on first read/merge command.
    conn: Option<duckdb::Connection>,
    /// Counter for merge frequency: merge every N writes.
    write_count: u64,
}

/// How many WriteParquet commands between automatic merge attempts.
const MERGE_INTERVAL: u64 = 10;

impl DbWorker {
    fn run(rx: mpsc::Receiver<DbCommand>, db_path: PathBuf, events_dir: PathBuf) {
        let mut worker = DbWorker {
            rx,
            db_path,
            events_dir,
            conn: None,
            write_count: 0,
        };

        // On startup, attempt to merge any pending parquet files
        worker.ensure_connection();
        worker.do_merge();

        loop {
            match worker.rx.recv() {
                Ok(DbCommand::WriteParquet { events, project }) => {
                    worker.handle_write_parquet(events, project);
                }
                Ok(DbCommand::QueryRecommendations {
                    exclude_project_id,
                    reply,
                }) => {
                    let result = worker.handle_query_recommendations(exclude_project_id.as_deref());
                    let _ = reply.send(result);
                }
                Ok(DbCommand::MergeParquet) => {
                    worker.do_merge();
                }
                Ok(DbCommand::Bootstrap {
                    models_md_path,
                    project_id,
                    id_to_key,
                }) => {
                    worker.try_bootstrap_from_local(&models_md_path, &project_id, &id_to_key);
                }
                Ok(DbCommand::Shutdown) => {
                    tracing::debug!("global memory: worker shutting down");
                    // Final merge before exit
                    worker.do_merge();
                    break;
                }
                Err(_) => {
                    // Sender dropped — GlobalWriter was dropped
                    tracing::debug!("global memory: channel closed, worker exiting");
                    break;
                }
            }
        }
    }

    /// Ensure the DuckDB connection is open and migrations are applied.
    fn ensure_connection(&mut self) -> bool {
        if self.conn.is_some() {
            return true;
        }

        match duckdb::Connection::open(&self.db_path) {
            Ok(conn) => {
                // Apply schema migrations
                if let Err(e) = schema::apply_migrations(&conn) {
                    tracing::warn!("global memory: migration failed: {e}");
                    return false;
                }
                self.conn = Some(conn);
                true
            }
            Err(e) => {
                tracing::warn!("global memory: failed to open DuckDB: {e}");
                false
            }
        }
    }

    /// Write model events to a Parquet file using an in-memory DuckDB connection.
    ///
    /// This avoids locking the main DuckDB file — each parquet file is written
    /// independently using a throwaway in-memory connection.
    fn handle_write_parquet(&mut self, events: Vec<ModelEvent>, project: ProjectInfo) {
        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let pid = std::process::id();
        let safe_project_id = sanitize_filename(&project.project_id);
        let filename = format!("{safe_project_id}_{now_ns}_{pid}.parquet");
        let parquet_path = self.events_dir.join(&filename);

        if let Err(e) = write_parquet_file(&parquet_path, &events) {
            tracing::warn!("global memory: parquet write failed: {e}");
            return;
        }

        // Also persist project info for the merge step
        // (We'll upsert it during merge when we have a real connection)
        self.write_count += 1;

        // Auto-merge every MERGE_INTERVAL writes
        if self.write_count % MERGE_INTERVAL == 0 {
            self.do_merge();
        }
    }

    /// Query global recommendations from compacted DB + pending parquet files.
    fn handle_query_recommendations(
        &mut self,
        exclude_project_id: Option<&str>,
    ) -> Result<GlobalRecommendations, String> {
        if !self.ensure_connection() {
            return Err("global memory: no DuckDB connection".into());
        }
        let conn = self.conn.as_ref().unwrap();

        // 90-day lookback window
        let cutoff_ms = epoch_ms() as i64 - 90 * 86400 * 1000;

        let exclude_clause = if exclude_project_id.is_some() {
            "AND project_id != ?2"
        } else {
            ""
        };

        // Check if there are pending parquet files.
        // DuckDB's read_parquet errors on empty globs, so we only UNION ALL
        // the parquet source when files exist.
        let has_parquet = list_parquet_files(&self.events_dir)
            .map(|f| !f.is_empty())
            .unwrap_or(false);

        let sql = if has_parquet {
            let events_glob = self.events_dir.join("*.parquet");
            let events_glob_str = events_glob.to_string_lossy().replace('\'', "''");
            format!(
                r#"
                SELECT model_key,
                       COUNT(*) FILTER (WHERE status = 'success' AND reason IS NULL) AS successes,
                       COUNT(*) FILTER (WHERE reason IS NULL
                                        OR reason NOT IN ('auth_failed', 'rate_limited')) AS quality_n,
                       AVG(latency_ms) AS avg_latency,
                       APPROX_QUANTILE(latency_ms, 0.95) AS p95
                FROM (
                    SELECT event_uid, project_id, ts, model_key, status, partial,
                           reason, latency_ms, prompt_tokens
                    FROM model_events
                    WHERE ts > ?1
                      {exclude_clause}
                    UNION ALL
                    SELECT event_uid, project_id, ts, model_key, status, partial,
                           reason, latency_ms, prompt_tokens
                    FROM read_parquet('{events_glob}', union_by_name=true)
                    WHERE ts > ?1
                      {exclude_clause}
                ) combined
                GROUP BY model_key
                HAVING quality_n >= 5
                ORDER BY successes DESC, avg_latency ASC
                "#,
                events_glob = events_glob_str,
                exclude_clause = exclude_clause,
            )
        } else {
            format!(
                r#"
                SELECT model_key,
                       COUNT(*) FILTER (WHERE status = 'success' AND reason IS NULL) AS successes,
                       COUNT(*) FILTER (WHERE reason IS NULL
                                        OR reason NOT IN ('auth_failed', 'rate_limited')) AS quality_n,
                       AVG(latency_ms) AS avg_latency,
                       APPROX_QUANTILE(latency_ms, 0.95) AS p95
                FROM model_events
                WHERE ts > ?1
                  {exclude_clause}
                GROUP BY model_key
                HAVING quality_n >= 5
                ORDER BY successes DESC, avg_latency ASC
                "#,
                exclude_clause = exclude_clause,
            )
        };

        let result = if let Some(exclude_id) = exclude_project_id {
            query_recommendations_impl(conn, &sql, cutoff_ms, Some(exclude_id))
        } else {
            query_recommendations_impl(conn, &sql, cutoff_ms, None)
        };

        result.map_err(|e| format!("global memory: query failed: {e}"))
    }

    /// Merge pending Parquet files into the compacted DuckDB table.
    fn do_merge(&mut self) {
        // Check for pending parquet files first
        let parquet_files = match list_parquet_files(&self.events_dir) {
            Ok(files) => files,
            Err(e) => {
                tracing::warn!("global memory: failed to list parquet files: {e}");
                return;
            }
        };

        if parquet_files.is_empty() {
            return;
        }

        if !self.ensure_connection() {
            return;
        }
        let conn = self.conn.as_ref().unwrap();

        let events_glob = self.events_dir.join("*.parquet");
        let events_glob_str = events_glob.to_string_lossy().replace('\'', "''");

        // Upsert projects referenced by the parquet events
        // (required to satisfy the foreign key on model_events.project_id)
        let upsert_projects_sql = format!(
            r#"
            INSERT INTO projects (project_id, first_seen_ts, last_seen_ts)
            SELECT DISTINCT project_id, MIN(ts), MAX(ts)
            FROM read_parquet('{events_glob}', union_by_name=true)
            GROUP BY project_id
            ON CONFLICT (project_id) DO UPDATE SET
                last_seen_ts = GREATEST(projects.last_seen_ts, EXCLUDED.last_seen_ts)
            "#,
            events_glob = events_glob_str,
        );
        if let Err(e) = conn.execute_batch(&upsert_projects_sql) {
            tracing::warn!("global memory: project upsert during merge failed: {e}");
            return;
        }

        // Ingest pending Parquet files into main table
        let merge_sql = format!(
            r#"
            INSERT INTO model_events
            SELECT event_uid, project_id, ts, model_key, status, partial,
                   reason, latency_ms, prompt_tokens
            FROM read_parquet('{events_glob}', union_by_name=true)
            ON CONFLICT (event_uid) DO NOTHING
            "#,
            events_glob = events_glob_str,
        );

        match conn.execute_batch(&merge_sql) {
            Ok(_) => {
                tracing::debug!(
                    "global memory: merged {} parquet file(s)",
                    parquet_files.len()
                );
                // Delete successfully merged parquet files
                for path in &parquet_files {
                    if let Err(e) = std::fs::remove_file(path) {
                        tracing::warn!(
                            "global memory: failed to remove merged parquet file {}: {e}",
                            path.display()
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!("global memory: merge failed: {e}");
            }
        }
    }

    /// Ingest events from a local models.md file into DuckDB.
    ///
    /// Uses lenient parsing (skip lines with <8 columns), normalizes model keys
    /// via `id_to_key`, and inserts with `ON CONFLICT DO NOTHING` for idempotency.
    fn try_bootstrap_from_local(
        &mut self,
        models_md_path: &Path,
        project_id: &str,
        id_to_key: &std::collections::HashMap<String, String>,
    ) {
        use super::local::{parse_iso_to_epoch_ms, parse_models_file};

        // Read the file (sync I/O is fine on the worker thread)
        let content = match std::fs::read_to_string(models_md_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!("global memory: bootstrap skipped (can't read models.md): {e}");
                return;
            }
        };

        let (_summary, event_lines) = parse_models_file(&content);
        if event_lines.is_empty() {
            tracing::debug!("global memory: bootstrap skipped (no events in models.md)");
            return;
        }

        if !self.ensure_connection() {
            return;
        }

        let conn = self.conn.as_ref().unwrap();
        let em_dash = "\u{2014}";

        // Upsert the project (FK requirement)
        let now_ms = epoch_ms() as i64;
        if let Err(e) = conn.execute(
            "INSERT INTO projects (project_id, first_seen_ts, last_seen_ts) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT (project_id) DO UPDATE SET \
             last_seen_ts = GREATEST(projects.last_seen_ts, EXCLUDED.last_seen_ts)",
            duckdb::params![project_id, now_ms, now_ms],
        ) {
            tracing::warn!("global memory: bootstrap project upsert failed: {e}");
            return;
        }

        let mut inserted = 0u32;
        let mut skipped = 0u32;

        for (i, line) in event_lines.iter().enumerate() {
            let cols: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
            if cols.len() < 8 {
                skipped += 1;
                continue;
            }

            // Parse timestamp
            let ts = match parse_iso_to_epoch_ms(cols[1]) {
                Some(ms) => ms + i as i64, // index offset avoids UID collisions
                None => {
                    skipped += 1;
                    continue;
                }
            };

            // Normalize model key
            let raw_model = cols[2];
            let model_key = id_to_key
                .get(raw_model)
                .map(|s| s.as_str())
                .unwrap_or(raw_model);

            // Parse latency: "25.3s" → 25300
            let latency_ms = cols[3]
                .trim_end_matches('s')
                .parse::<f64>()
                .map(|secs| (secs * 1000.0) as i32)
                .unwrap_or(0);

            let status = cols[4];
            let partial = cols[5] == "yes";

            // Detect format: 10+ elements = new (has reason column)
            let reason = if cols.len() >= 10 {
                let r = cols[6];
                if r == em_dash { None } else { Some(r) }
            } else {
                // Old format: infer reason from error text (cols[6])
                let error = cols[6];
                if error.contains("auth") {
                    Some("auth_failed")
                } else if error.contains("rate") {
                    Some("rate_limited")
                } else {
                    None
                }
            };

            // Prompt tokens: last data column
            let prompt_col = if cols.len() >= 10 { cols[8] } else { cols[7] };
            let prompt_tokens: Option<i32> = if prompt_col == em_dash {
                None
            } else {
                prompt_col.parse().ok()
            };

            let event = ModelEvent::new(
                project_id.to_string(),
                ts,
                model_key.to_string(),
                status.to_string(),
                partial,
                reason.map(|s| s.to_string()),
                latency_ms,
                prompt_tokens,
            );

            if let Err(e) = conn.execute(
                "INSERT INTO model_events \
                 (event_uid, project_id, ts, model_key, status, partial, reason, latency_ms, prompt_tokens) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
                 ON CONFLICT (event_uid) DO NOTHING",
                duckdb::params![
                    event.event_uid,
                    event.project_id,
                    event.ts,
                    event.model_key,
                    event.status,
                    event.partial,
                    event.reason,
                    event.latency_ms,
                    event.prompt_tokens,
                ],
            ) {
                tracing::warn!("global memory: bootstrap insert failed for line {i}: {e}");
                skipped += 1;
                continue;
            }

            inserted += 1;
        }

        tracing::info!(
            "global memory: bootstrapped {inserted} events from local models.md ({skipped} skipped)"
        );
    }
}

// ---------------------------------------------------------------------------
// Parquet write (using in-memory DuckDB — no file lock on the main DB)
// ---------------------------------------------------------------------------

/// Write model events to a Parquet file using an ephemeral in-memory DuckDB.
///
/// This is lock-free with respect to the main DuckDB file: we create a
/// throwaway in-memory connection, insert events into a temp table, and
/// COPY to parquet.
fn write_parquet_file(path: &Path, events: &[ModelEvent]) -> Result<(), String> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|e| format!("in-memory DuckDB open failed: {e}"))?;

    // Create a temporary table matching the schema
    conn.execute_batch(
        "CREATE TABLE tmp_events (
            event_uid VARCHAR,
            project_id VARCHAR,
            ts BIGINT,
            model_key VARCHAR,
            status VARCHAR,
            partial BOOLEAN,
            reason VARCHAR,
            latency_ms INTEGER,
            prompt_tokens INTEGER
        )",
    )
    .map_err(|e| format!("create tmp table failed: {e}"))?;

    // Insert events
    let mut stmt = conn
        .prepare(
            "INSERT INTO tmp_events VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .map_err(|e| format!("prepare insert failed: {e}"))?;

    for ev in events {
        stmt.execute(params![
            ev.event_uid,
            ev.project_id,
            ev.ts,
            ev.model_key,
            ev.status,
            ev.partial,
            ev.reason,
            ev.latency_ms,
            ev.prompt_tokens,
        ])
        .map_err(|e| format!("insert failed: {e}"))?;
    }

    // Export to Parquet
    let path_str = path.to_string_lossy();
    conn.execute_batch(&format!(
        "COPY tmp_events TO '{}' (FORMAT PARQUET, COMPRESSION ZSTD)",
        path_str.replace('\'', "''")
    ))
    .map_err(|e| format!("COPY TO parquet failed: {e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

fn query_recommendations_impl(
    conn: &duckdb::Connection,
    sql: &str,
    cutoff_ms: i64,
    exclude_id: Option<&str>,
) -> Result<GlobalRecommendations, duckdb::Error> {
    let mut stmt = conn.prepare(sql)?;

    // Use query() with dynamic params to avoid closure type mismatch
    let mut rows = if let Some(eid) = exclude_id {
        stmt.query(params![cutoff_ms, eid])?
    } else {
        stmt.query(params![cutoff_ms])?
    };

    let mut models = Vec::new();
    while let Some(row) = rows.next()? {
        let successes: f64 = row.get(1)?;
        let quality_n: f64 = row.get(2)?;
        let success_rate = if quality_n > 0.0 {
            successes / quality_n
        } else {
            0.0
        };

        models.push(GlobalModelStats {
            model_key: row.get(0)?,
            success_rate,
            sample_count: quality_n as u64,
            avg_latency_ms: row.get(3)?,
            p95_latency_ms: row.get(4)?,
        });
    }

    Ok(GlobalRecommendations { models })
}

// ---------------------------------------------------------------------------
// Project identification
// ---------------------------------------------------------------------------

// compute_project_id and normalize_git_url live in context.rs (single source of truth).
// Use crate::context::compute_project_id (async) and crate::context::normalize_git_url.

// ---------------------------------------------------------------------------
// Filesystem helpers
// ---------------------------------------------------------------------------

/// List all .parquet files in the events directory.
fn list_parquet_files(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    if !dir.exists() {
        return Ok(files);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("parquet") {
            files.push(path);
        }
    }
    files.sort(); // deterministic order
    Ok(files)
}

/// Sanitize a string for use in filenames (replace non-alphanumeric with underscore).
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Current epoch time in milliseconds.
fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::review::ModelStatus;

    fn temp_dir(name: &str) -> PathBuf {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir()
            .join("squall-global-test")
            .join(format!("{name}_{}_{ts}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // normalize_git_url and compute_project_id tests live in context.rs (single source of truth).

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("git:abc123"), "git_abc123");
        assert_eq!(sanitize_filename("path:foo/bar"), "path_foo_bar");
        assert_eq!(sanitize_filename("simple-name_01"), "simple-name_01");
    }

    #[test]
    fn test_write_parquet_roundtrip() {
        let dir = temp_dir("parquet-roundtrip");
        let parquet_path = dir.join("test.parquet");

        let events = vec![ModelEvent {
            event_uid: "test_uid_001".into(),
            project_id: "git:abc123".into(),
            ts: 1700000000000,
            model_key: "grok".into(),
            status: "success".into(),
            partial: false,
            reason: None,
            latency_ms: 31000,
            prompt_tokens: Some(4200),
        }];

        write_parquet_file(&parquet_path, &events).unwrap();
        assert!(parquet_path.exists(), "parquet file should exist");

        // Verify we can read it back via DuckDB
        let conn = duckdb::Connection::open_in_memory().unwrap();
        let path_str = parquet_path.to_string_lossy();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT event_uid, model_key, latency_ms FROM read_parquet('{path_str}')"
            ))
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let uid: String = row.get(0).unwrap();
        let model: String = row.get(1).unwrap();
        let latency: i32 = row.get(2).unwrap();
        assert_eq!(uid, "test_uid_001");
        assert_eq!(model, "grok");
        assert_eq!(latency, 31000);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_global_writer_lifecycle() {
        let dir = temp_dir("writer-lifecycle");
        let db_path = dir.join("global.duckdb");

        let writer = GlobalWriter::new(db_path.clone());
        assert!(writer.is_some(), "GlobalWriter::new should succeed");

        let writer = writer.unwrap();

        // Log some events
        let results = vec![ReviewModelResult {
            model: "grok".to_string(),
            provider: "xai".to_string(),
            status: ModelStatus::Success,
            response: Some("review text".to_string()),
            error: None,
            reason: None,
            latency_ms: 25000,
            partial: false,
        }];

        writer.log_events(&results, 1000, "test:project", Some("/tmp/test"), None);

        // Give the worker time to process
        std::thread::sleep(std::time::Duration::from_millis(1000));

        // Check that a parquet file was created
        let parquet_files = list_parquet_files(&dir.join("events")).unwrap();
        assert!(
            !parquet_files.is_empty(),
            "should have created at least one parquet file"
        );

        // Drop triggers shutdown
        drop(writer);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_global_writer_query_after_merge() {
        let dir = temp_dir("writer-query");
        let db_path = dir.join("global.duckdb");

        let writer = GlobalWriter::new(db_path.clone()).unwrap();

        // Log events
        let results = vec![
            ReviewModelResult {
                model: "grok".to_string(),
                provider: "xai".to_string(),
                status: ModelStatus::Success,
                response: Some("ok".to_string()),
                error: None,
                reason: None,
                latency_ms: 25000,
                partial: false,
            },
            ReviewModelResult {
                model: "gemini".to_string(),
                provider: "google".to_string(),
                status: ModelStatus::Success,
                response: Some("ok".to_string()),
                error: None,
                reason: None,
                latency_ms: 50000,
                partial: false,
            },
        ];

        // Log enough events to pass HAVING quality_n >= 5 threshold
        for i in 0..5 {
            writer.log_events(
                &results,
                1000,
                &format!("test:project-{i}"),
                Some("/tmp/test"),
                None,
            );
        }

        // Give worker time to write parquet
        std::thread::sleep(std::time::Duration::from_millis(1000));

        // Trigger merge
        writer.trigger_merge();
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Query recommendations
        let rx = writer.query_recommendations(None).expect("send should succeed");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let recs = rt.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(10), rx)
                .await
                .expect("query should not timeout")
                .expect("worker should reply")
                .expect("query should succeed")
        });
        assert!(
            !recs.models.is_empty(),
            "should have model recommendations"
        );

        drop(writer);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_list_parquet_files_empty() {
        let dir = temp_dir("list-empty");
        let files = list_parquet_files(&dir).unwrap();
        assert!(files.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_list_parquet_files_filters() {
        let dir = temp_dir("list-filters");
        std::fs::write(dir.join("a.parquet"), b"").unwrap();
        std::fs::write(dir.join("b.parquet"), b"").unwrap();
        std::fs::write(dir.join("c.txt"), b"").unwrap();
        let files = list_parquet_files(&dir).unwrap();
        assert_eq!(files.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_epoch_ms_reasonable() {
        let ms = epoch_ms();
        // Should be after 2024-01-01 (1704067200000)
        assert!(ms > 1704067200000, "epoch_ms too small: {ms}");
        // Should be before 2030-01-01 (1893456000000)
        assert!(ms < 1893456000000, "epoch_ms too large: {ms}");
    }
}
