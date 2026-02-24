mod local;

#[cfg(feature = "global-memory")]
pub mod global;
#[cfg(feature = "global-memory")]
pub mod schema;

// Re-export public items from local (excluding MemoryStore, which is aliased below).
pub use local::{
    CONFIRMED_THRESHOLD, MAX_MEMORIZE_CONTENT_LEN, MAX_PATTERN_ENTRIES, MAX_TACTICS_BYTES,
    ModelGateStats, VALID_CATEGORIES, content_hash_pub, extract_evidence_count_pub,
    generate_recommendations_pub, iso_date_pub,
};

use std::collections::HashMap;
use std::path::PathBuf;

use crate::tools::review::ReviewModelResult;

/// Composite memory store wrapping local (per-project) and optional global (cross-project) storage.
///
/// In the default build (no `global-memory` feature), this delegates entirely to the local store.
/// When `global-memory` is enabled, write operations fan out to both local and global, and
/// read operations compose results from both sources.
pub struct CompositeMemoryStore {
    local: local::MemoryStore,
    #[cfg(feature = "global-memory")]
    global: Option<global::GlobalWriter>,
    /// Cached (working_directory, project_id) from `log_model_metrics`.
    /// Avoids spawning `git remote get-url origin` on every review call.
    /// Also used by `compose_recommendations` to exclude the current project from global stats.
    #[cfg(feature = "global-memory")]
    cached_project: std::sync::Mutex<Option<(String, String)>>,
    /// Guard: only attempt bootstrap once per process lifetime.
    #[cfg(feature = "global-memory")]
    bootstrapped: std::sync::atomic::AtomicBool,
}

/// Type alias preserving all existing imports (`use crate::memory::MemoryStore`).
pub type MemoryStore = CompositeMemoryStore;

impl Default for CompositeMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositeMemoryStore {
    pub fn new() -> Self {
        Self {
            local: local::MemoryStore::new(),
            #[cfg(feature = "global-memory")]
            global: None,
            #[cfg(feature = "global-memory")]
            cached_project: std::sync::Mutex::new(None),
            #[cfg(feature = "global-memory")]
            bootstrapped: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Create a CompositeMemoryStore with a custom base directory (for testing).
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self {
            local: local::MemoryStore::with_base_dir(base_dir),
            #[cfg(feature = "global-memory")]
            global: None,
            #[cfg(feature = "global-memory")]
            cached_project: std::sync::Mutex::new(None),
            #[cfg(feature = "global-memory")]
            bootstrapped: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Set the model_id -> config_key normalization map.
    pub fn with_id_to_key(mut self, map: HashMap<String, String>) -> Self {
        self.local = self.local.with_id_to_key(map);
        self
    }

    /// Set the global writer for cross-project memory.
    #[cfg(feature = "global-memory")]
    pub fn with_global(mut self, writer: global::GlobalWriter) -> Self {
        self.global = Some(writer);
        self
    }

    /// Set the cached project ID (for testing compose_recommendations exclusion).
    #[cfg(feature = "global-memory")]
    pub fn set_project_id(&self, id: String) {
        if let Ok(mut cached) = self.cached_project.lock() {
            *cached = Some(("__test__".to_string(), id));
        }
    }

    /// Log model metrics from a completed review.
    ///
    /// When `working_directory` is `Some` and a global writer is configured,
    /// events are also forwarded to the global cross-project store.
    pub async fn log_model_metrics(
        &self,
        results: &[ReviewModelResult],
        prompt_len: usize,
        id_to_key: Option<&HashMap<String, String>>,
        #[cfg_attr(not(feature = "global-memory"), allow(unused_variables))]
        working_directory: Option<&str>,
    ) {
        self.local
            .log_model_metrics(results, prompt_len, id_to_key)
            .await;

        #[cfg(feature = "global-memory")]
        if let (Some(writer), Some(wd)) = (&self.global, working_directory) {
            // Check cache: reuse project_id if working_directory matches
            let project_id = {
                let cached = self.cached_project.lock().ok().and_then(|g| g.clone());
                if let Some((cached_wd, cached_id)) = cached {
                    if cached_wd == wd {
                        cached_id
                    } else {
                        crate::context::compute_project_id(std::path::Path::new(wd)).await
                    }
                } else {
                    crate::context::compute_project_id(std::path::Path::new(wd)).await
                }
            };
            // Update cache
            if let Ok(mut cached) = self.cached_project.lock() {
                *cached = Some((wd.to_string(), project_id.clone()));
            }
            writer.log_events(results, prompt_len, &project_id, Some(wd), id_to_key);

            // Lazy bootstrap: on first call with a real project_id, ingest local
            // models.md history into DuckDB. Runs at most once per process.
            if !self.bootstrapped.load(std::sync::atomic::Ordering::Relaxed) {
                let models_path = self.local.models_path();
                if models_path.exists() {
                    let id_map = id_to_key.cloned().unwrap_or_default();
                    writer.send_bootstrap(models_path, project_id.clone(), id_map);
                }
                self.bootstrapped
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    /// Write an explicit memorize entry to patterns.md or tactics.md.
    pub async fn memorize(
        &self,
        category: &str,
        content: &str,
        model: Option<&str>,
        tags: Option<&[String]>,
        scope: Option<&str>,
        metadata: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<String, String> {
        self.local
            .memorize(category, content, model, tags, scope, metadata)
            .await
    }

    /// Read memory files for the read path.
    ///
    /// When `category` is `"recommend"` and a global writer is configured,
    /// composes local and global recommendations into a unified view with
    /// confidence scoring.
    pub async fn read_memory(
        &self,
        category: Option<&str>,
        model: Option<&str>,
        max_chars: usize,
        scope: Option<&str>,
    ) -> Result<String, String> {
        #[cfg(feature = "global-memory")]
        if category == Some("recommend")
            && let Some(writer) = &self.global
        {
            return self.compose_recommendations(writer, max_chars).await;
        }

        self.local
            .read_memory(category, model, max_chars, scope)
            .await
    }

    /// Returns per-model stats parsed from models.md event log.
    pub async fn get_model_stats(
        &self,
        id_to_key: Option<&HashMap<String, String>>,
    ) -> Option<HashMap<String, ModelGateStats>> {
        self.local.get_model_stats(id_to_key).await
    }

    /// Flush branch-scoped memory after PR merge.
    pub async fn flush_branch(&self, branch: &str) -> Result<String, String> {
        self.local.flush_branch(branch).await
    }

    /// Compose local + global recommendations into a unified view.
    ///
    /// Confidence levels:
    /// - **H** (High): both local and global agree (>80% success in both)
    /// - **M** (Medium): only one source has data
    /// - **L** (Low): local and global disagree (>20% success rate delta)
    ///
    /// Includes an exploration slot for models with <5 global samples.
    #[cfg(feature = "global-memory")]
    async fn compose_recommendations(
        &self,
        writer: &global::GlobalWriter,
        max_chars: usize,
    ) -> Result<String, String> {
        // 1. Get local stats (structured)
        let local_stats = self.local.get_model_stats(None).await;

        // 2. Query global stats, excluding the current project to avoid double-counting
        let exclude_id = self
            .cached_project
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|(_, id)| id.clone()));
        let global_recs = match writer.query_recommendations(exclude_id.as_deref()) {
            Ok(reply_rx) => {
                // 15s allows for a queued merge (~2s) + the query itself (~1s) with margin.
                match tokio::time::timeout(std::time::Duration::from_secs(15), reply_rx).await {
                    Ok(Ok(Ok(recs))) => recs,
                    Ok(Ok(Err(e))) => {
                        tracing::warn!("global memory: query failed: {e}");
                        global::GlobalRecommendations::default()
                    }
                    Ok(Err(_)) => {
                        tracing::warn!("global memory: worker dropped reply channel");
                        global::GlobalRecommendations::default()
                    }
                    Err(_) => {
                        tracing::warn!("global memory: query timed out after 15s");
                        global::GlobalRecommendations::default()
                    }
                }
            }
            Err(e) => {
                tracing::warn!("global memory: {e}");
                global::GlobalRecommendations::default()
            }
        };

        // 3. Compose
        let local_map = local_stats.unwrap_or_default();

        // Build unified model set
        let mut all_models: Vec<String> = local_map.keys().cloned().collect();
        for gs in &global_recs.models {
            if !all_models.contains(&gs.model_key) {
                all_models.push(gs.model_key.clone());
            }
        }
        all_models.sort();

        if all_models.is_empty() {
            return Ok(
                "No model data yet. Run a `review` first to populate model metrics.".to_string(),
            );
        }

        // Build global lookup
        let global_map: HashMap<&str, &global::GlobalModelStats> = global_recs
            .models
            .iter()
            .map(|gs| (gs.model_key.as_str(), gs))
            .collect();

        // Score and rank models
        struct CompositeRow {
            model: String,
            confidence: char,       // H, M, L
            local_summary: String,  // "95% (14) 31s" or "—"
            global_summary: String, // "91% (5k) 30s" or "—"
            sort_score: f64,        // for ranking
            is_exploration: bool,
        }

        let mut rows: Vec<CompositeRow> = Vec::new();
        let mut exploration_candidate: Option<CompositeRow> = None;

        for model in &all_models {
            let local = local_map.get(model.as_str());
            let global = global_map.get(model.as_str());

            let local_summary = if let Some(ls) = local {
                format!(
                    "{:.0}% ({}) {:.0}s",
                    ls.success_rate * 100.0,
                    ls.sample_count,
                    ls.avg_latency_secs
                )
            } else {
                "\u{2014}".to_string()
            };

            let global_summary = if let Some(gs) = global {
                let count_str = if gs.sample_count >= 1000 {
                    format!("{:.0}k", gs.sample_count as f64 / 1000.0)
                } else {
                    gs.sample_count.to_string()
                };
                format!(
                    "{:.0}% ({}) {:.0}s",
                    gs.success_rate * 100.0,
                    count_str,
                    gs.avg_latency_ms / 1000.0
                )
            } else {
                "\u{2014}".to_string()
            };

            // Confidence scoring
            let confidence = match (local, global) {
                (Some(ls), Some(gs)) => {
                    let delta = (ls.success_rate - gs.success_rate).abs();
                    if delta <= 0.20 && ls.success_rate > 0.8 && gs.success_rate > 0.8 {
                        'H'
                    } else if delta > 0.20 {
                        'L'
                    } else {
                        'M'
                    }
                }
                _ => 'M', // only one source
            };

            // Sort score: prefer high confidence, high success rate, low latency
            let conf_weight = match confidence {
                'H' => 3.0,
                'M' => 2.0,
                'L' => 1.0,
                _ => 0.0,
            };
            let success = local
                .map(|ls| ls.success_rate)
                .or(global.map(|gs| gs.success_rate))
                .unwrap_or(0.0);
            let sort_score = conf_weight + success;

            // Exploration slot: model with <5 global samples
            if (global.is_none_or(|gs| gs.sample_count < 5) && local.is_some())
                && (exploration_candidate.is_none()
                    || local.is_some_and(|ls| ls.success_rate > 0.5))
            {
                exploration_candidate = Some(CompositeRow {
                    model: model.clone(),
                    confidence: 'M',
                    local_summary: local_summary.clone(),
                    global_summary: global_summary.clone(),
                    sort_score: 0.5, // low priority
                    is_exploration: true,
                });
            }

            rows.push(CompositeRow {
                model: model.clone(),
                confidence,
                local_summary,
                global_summary,
                sort_score,
                is_exploration: false,
            });
        }

        // Sort by score descending
        rows.sort_by(|a, b| {
            b.sort_score
                .partial_cmp(&a.sort_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Format output
        let mut output = String::from("# Model Recommendations\n\n");

        // Top pick
        if let Some(top) = rows.first() {
            let conf_label = match top.confidence {
                'H' => "High Confidence: Local & Global align",
                'M' => "Medium Confidence: single source",
                _ => "Low Confidence: sources disagree",
            };
            output.push_str(&format!("**Top Pick:** {} ({conf_label})\n\n", top.model));
        }

        // Table
        output.push_str("| Model | Conf | Local | Global |\n");
        output.push_str("|-------|------|-------|--------|\n");
        for row in &rows {
            let marker = if row.is_exploration { " *" } else { "" };
            output.push_str(&format!(
                "| {}{marker} | {} | {} | {} |\n",
                row.model, row.confidence, row.local_summary, row.global_summary
            ));
        }

        // Exploration slot note
        if let Some(exp) = &exploration_candidate
            && !rows
                .iter()
                .any(|r| r.is_exploration && r.model == exp.model)
        {
            output.push_str(&format!(
                "\n*Exploration: {} has limited global data — try it to build confidence.*\n",
                exp.model
            ));
        }

        // Progressive truncation: trim to max_chars
        if output.len() > max_chars {
            let suffix = "\n\n[truncated]";
            if max_chars > suffix.len() {
                let boundary = local::floor_char_boundary(&output, max_chars - suffix.len());
                output.truncate(boundary);
                output.push_str(suffix);
            } else {
                // max_chars is too small for the suffix — just hard-truncate
                let boundary = local::floor_char_boundary(&output, max_chars);
                output.truncate(boundary);
            }
        }

        Ok(output)
    }
}
