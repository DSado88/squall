use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::Mutex;

use crate::tools::review::ReviewModelResult;

/// Per-model performance stats for hard gate decisions.
#[derive(Debug, Clone)]
pub struct ModelGateStats {
    pub success_rate: f64,
    pub avg_latency_secs: f64,
    pub sample_count: usize,
    /// Auth failures, rate limits, etc. excluded from success_rate.
    pub infrastructure_failures: usize,
    pub last_seen: String,
}

/// Maximum entries in the models.md event log before compaction.
const MAX_EVENT_LOG_ENTRIES: usize = 100;

/// Recompute summary every N writes.
const COMPACTION_INTERVAL: u64 = 10;

/// Maximum entries in patterns.md.
pub const MAX_PATTERN_ENTRIES: usize = 50;

/// Maximum size of tactics.md in bytes.
pub const MAX_TACTICS_BYTES: usize = 10 * 1024;

/// Maximum length of a memorize content string.
pub const MAX_MEMORIZE_CONTENT_LEN: usize = 500;

/// Valid categories for the memorize tool.
pub const VALID_CATEGORIES: &[&str] = &["pattern", "tactic", "recommend"];

/// Evidence threshold for [confirmed] status.
pub const CONFIRMED_THRESHOLD: usize = 5;

/// Default base directory for memory files.
const DEFAULT_MEMORY_DIR: &str = ".squall/memory";

/// Manages Squall's persistent memory files.
///
/// Thread-safe: all file writes go through an internal Mutex to prevent
/// concurrent writes from interleaving. Reads are lock-free (atomic file
/// reads via temp+rename ensure no partial reads).
pub struct MemoryStore {
    base_dir: PathBuf,
    write_lock: Mutex<()>,
    write_counter: AtomicU64,
    /// Maps provider model_ids to config keys for display normalization.
    id_to_key: HashMap<String, String>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            base_dir: PathBuf::from(DEFAULT_MEMORY_DIR),
            write_lock: Mutex::new(()),
            write_counter: AtomicU64::new(0),
            id_to_key: HashMap::new(),
        }
    }

    /// Create a MemoryStore with a custom base directory.
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            write_lock: Mutex::new(()),
            write_counter: AtomicU64::new(0),
            id_to_key: HashMap::new(),
        }
    }

    /// Set the model_id → config_key normalization map.
    /// Used by `compute_summary()` and `generate_recommendations()` to
    /// normalize legacy event log entries that used provider model_ids.
    pub fn with_id_to_key(mut self, map: HashMap<String, String>) -> Self {
        self.id_to_key = map;
        self
    }

    pub(crate) fn models_path(&self) -> PathBuf {
        self.base_dir.join("models.md")
    }

    fn patterns_path(&self) -> PathBuf {
        self.base_dir.join("patterns.md")
    }

    fn tactics_path(&self) -> PathBuf {
        self.base_dir.join("tactics.md")
    }

    fn archive_path(&self) -> PathBuf {
        self.base_dir.join("archive.md")
    }

    fn index_path(&self) -> PathBuf {
        self.base_dir.join("index.md")
    }

    /// The display path for this store's directory (for returning in tool responses).
    fn display_dir(&self) -> String {
        self.base_dir.display().to_string()
    }

    /// Ensure the memory directory and index.md exist.
    async fn ensure_dir(&self) -> Result<(), std::io::Error> {
        tokio::fs::create_dir_all(&self.base_dir).await?;

        let index = self.index_path();
        if !tokio::fs::try_exists(&index).await.unwrap_or(false) {
            atomic_write(&index, INDEX_CONTENT).await?;
        }
        Ok(())
    }

    /// Log model metrics from a completed review. Called after persist_results().
    ///
    /// Extracts latency, status, and error info from each model result and
    /// appends to the event log in models.md. Every COMPACTION_INTERVAL writes,
    /// recomputes the summary table and truncates to MAX_EVENT_LOG_ENTRIES.
    pub async fn log_model_metrics(
        &self,
        results: &[ReviewModelResult],
        prompt_len: usize,
        id_to_key: Option<&HashMap<String, String>>,
    ) {
        let _lock = self.write_lock.lock().await;

        if let Err(e) = self.ensure_dir().await {
            tracing::warn!("memory: failed to create directory: {e}");
            return;
        }

        let path = self.models_path();
        let existing = match read_to_string_lossy(&path).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("memory: failed to read models.md: {e}");
                return;
            }
        };

        let timestamp = iso_timestamp();
        let mut new_events = Vec::new();
        for r in results {
            let latency_s = format!("{:.1}s", r.latency_ms as f64 / 1000.0);
            let status = format!("{:?}", r.status).to_lowercase();
            let partial = if r.partial { "yes" } else { "no" };
            let reason = escape_pipes(r.reason.as_deref().unwrap_or("\u{2014}"));
            let error = escape_pipes(r.error.as_deref().unwrap_or("\u{2014}"));
            // Normalize model_id → config key if map provided
            let raw_model = &r.model;
            let normalized = id_to_key
                .and_then(|map| map.get(raw_model.as_str()))
                .map(|s| s.as_str())
                .unwrap_or(raw_model.as_str());
            let model = escape_pipes(normalized);
            new_events.push(format!(
                "| {timestamp} | {model} | {latency_s} | {status} | {partial} | {reason} | {error} | {prompt_len} |",
            ));
        }

        let (summary_section, event_lines) = parse_models_file(&existing);
        let mut all_events: Vec<String> = event_lines;
        all_events.extend(new_events);

        let count = self.write_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let should_compact = count.is_multiple_of(COMPACTION_INTERVAL);

        // Truncate to last MAX_EVENT_LOG_ENTRIES
        let truncated = all_events.len() > MAX_EVENT_LOG_ENTRIES;
        if truncated {
            let start = all_events.len() - MAX_EVENT_LOG_ENTRIES;
            all_events = all_events[start..].to_vec();
        }

        let id_map = id_to_key.cloned().unwrap_or_default();
        let new_summary = if should_compact || truncated || summary_section.is_empty() {
            compute_summary(&all_events, &id_map)
        } else {
            summary_section
        };

        let output = format_models_file(&new_summary, &all_events);
        if let Err(e) = atomic_write(&path, &output).await {
            tracing::warn!("memory: failed to write models.md: {e}");
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
        if !VALID_CATEGORIES.contains(&category) {
            return Err(format!(
                "invalid category: {category}. Must be one of: {}",
                VALID_CATEGORIES.join(", ")
            ));
        }
        if content.len() > MAX_MEMORIZE_CONTENT_LEN {
            return Err(format!(
                "content too long: {} chars (max {MAX_MEMORIZE_CONTENT_LEN})",
                content.len()
            ));
        }
        if content.trim().is_empty() {
            return Err("content must not be empty".to_string());
        }

        let _lock = self.write_lock.lock().await;

        if let Err(e) = self.ensure_dir().await {
            return Err(format!("failed to create memory directory: {e}"));
        }

        let timestamp = iso_date();

        let display_dir = self.display_dir();

        // Sanitize all user-provided strings to prevent markdown structure injection.
        // Any newline in tags/metadata/model/scope could inject fake `- Scope:` or `## [...]` lines.
        let content = content.replace(['\n', '\r'], " ");
        let content = content.trim();

        let tag_line = tags
            .filter(|t| !t.is_empty())
            .map(|t| {
                let sanitized: Vec<String> =
                    t.iter().map(|tag| tag.replace(['\n', '\r'], " ")).collect();
                format!("- Tags: {}", sanitized.join(", "))
            })
            .unwrap_or_default();
        let model_line = model
            .filter(|m| !m.is_empty())
            .map(|m| {
                let m = m.replace(['\n', '\r'], " ");
                format!("- Model: {m}")
            })
            .unwrap_or_default();
        let metadata_lines: Vec<String> = metadata
            .iter()
            .flat_map(|m| {
                let mut pairs: Vec<_> = m.iter().collect();
                pairs.sort_by_key(|(k, _)| (*k).clone());
                pairs
                    .into_iter()
                    .map(|(k, v)| {
                        let k = k.replace(['\n', '\r'], " ");
                        let v = v.replace(['\n', '\r'], " ");
                        format!("- {k}: {v}")
                    })
                    .collect::<Vec<_>>()
            })
            .collect();

        match category {
            "pattern" => {
                let path = self.patterns_path();
                let existing = read_to_string_lossy(&path)
                    .await
                    .map_err(|e| format!("failed to read patterns.md: {e}"))?;
                let mut entries = parse_pattern_entries(&existing);

                let hash = content_hash(content, scope);

                // Check for existing entry with same hash (evidence counting)
                let existing_idx = entries
                    .iter()
                    .position(|e| extract_entry_hash(e) == Some(&hash));

                if let Some(idx) = existing_idx {
                    // Merge: increment evidence count, update date (new request values take precedence)
                    let old = &entries[idx];
                    let old_count = extract_evidence_count(old);
                    let new_count = old_count + 1;
                    let first_seen = extract_first_seen(old).unwrap_or(&timestamp).to_string();

                    let confirmed = if new_count >= CONFIRMED_THRESHOLD {
                        " [confirmed]"
                    } else {
                        ""
                    };

                    let scope_line = scope
                        .filter(|s| !s.is_empty())
                        .map(|s| {
                            let s = s.replace(['\n', '\r'], " ");
                            format!("- Scope: {s}")
                        })
                        .unwrap_or_else(|| {
                            // Preserve existing scope (already sanitized on disk)
                            extract_entry_scope(old)
                                .map(|s| format!("- Scope: {s}"))
                                .unwrap_or_default()
                        });

                    let mut entry = format!(
                        "## [{timestamp}] {content} [x{new_count}]{confirmed}\n\
                         <!-- hash:{hash} -->\n\
                         - Evidence: {new_count} occurrences ({first_seen} to {timestamp})\n"
                    );
                    if !scope_line.is_empty() {
                        entry.push_str(&format!("{scope_line}\n"));
                    }

                    // Preserve prior model/tags/metadata when new request omits them
                    let effective_model_line = if !model_line.is_empty() {
                        model_line.clone()
                    } else {
                        extract_entry_model(old)
                            .map(|m| format!("- Model: {m}"))
                            .unwrap_or_default()
                    };
                    if !effective_model_line.is_empty() {
                        entry.push_str(&format!("{effective_model_line}\n"));
                    }

                    let effective_tag_line = if !tag_line.is_empty() {
                        tag_line.clone()
                    } else {
                        extract_entry_tags(old)
                            .map(|t| format!("- Tags: {t}"))
                            .unwrap_or_default()
                    };
                    if !effective_tag_line.is_empty() {
                        entry.push_str(&format!("{effective_tag_line}\n"));
                    }

                    if !metadata_lines.is_empty() {
                        for ml in &metadata_lines {
                            entry.push_str(&format!("{ml}\n"));
                        }
                    } else {
                        for ml in extract_entry_metadata_lines(old) {
                            entry.push_str(&format!("{ml}\n"));
                        }
                    }

                    entries[idx] = entry;
                } else {
                    // New entry
                    let scope_line = scope
                        .filter(|s| !s.is_empty())
                        .map(|s| {
                            let s = s.replace(['\n', '\r'], " ");
                            format!("- Scope: {s}")
                        })
                        .unwrap_or_default();

                    let mut entry = format!(
                        "## [{timestamp}] {content} [x1]\n\
                         <!-- hash:{hash} -->\n"
                    );
                    if !scope_line.is_empty() {
                        entry.push_str(&format!("{scope_line}\n"));
                    }
                    if !model_line.is_empty() {
                        entry.push_str(&format!("{model_line}\n"));
                    }
                    if !tag_line.is_empty() {
                        entry.push_str(&format!("{tag_line}\n"));
                    }
                    for ml in &metadata_lines {
                        entry.push_str(&format!("{ml}\n"));
                    }

                    entries.push(entry);
                }

                // Prune oldest if over cap
                while entries.len() > MAX_PATTERN_ENTRIES {
                    entries.remove(0);
                }

                let output = format!("# Recurring Patterns\n\n{}", entries.join("\n"));
                atomic_write(&path, &output)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(format!("{display_dir}/patterns.md"))
            }
            "tactic" | "recommend" => {
                let path = self.tactics_path();
                let existing = read_to_string_lossy(&path)
                    .await
                    .map_err(|e| format!("failed to read tactics.md: {e}"))?;

                let new_line = if let Some(m) = model.filter(|m| !m.is_empty()) {
                    let m = m.replace(['\n', '\r'], " ");
                    format!("- [{m}] {content}")
                } else {
                    format!("- {content}")
                };

                let mut output = if existing.is_empty() {
                    format!("# Prompt Tactics\n\n{new_line}\n")
                } else {
                    format!("{existing}\n{new_line}\n")
                };

                // Auto-prune oldest entries if over size cap
                while output.len() > MAX_TACTICS_BYTES {
                    // Find the first `- ` entry line and remove it
                    if let Some(pos) = output.find("\n- ") {
                        let end = output[pos + 1..]
                            .find('\n')
                            .map_or(output.len(), |e| pos + 1 + e);
                        output.replace_range(pos..end, "");
                    } else {
                        break;
                    }
                }

                // Trim trailing whitespace buildup
                while output.ends_with("\n\n\n") {
                    output.pop();
                }

                atomic_write(&path, &output)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(format!("{display_dir}/tactics.md"))
            }
            _ => unreachable!(), // validated above
        }
    }

    /// Read memory files for the read path.
    /// Returns the content of the requested category.
    pub async fn read_memory(
        &self,
        category: Option<&str>,
        model: Option<&str>,
        max_chars: usize,
        scope: Option<&str>,
    ) -> Result<String, String> {
        let category = category.unwrap_or("all");
        let mut sections = Vec::new();

        if category == "recommend" {
            let path = self.models_path();
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => {
                    let recommendation = generate_recommendations(&content, &self.id_to_key);
                    if !recommendation.is_empty() {
                        return Ok(recommendation);
                    }
                    return Ok(
                        "No model data yet. Run a `review` first to populate model metrics."
                            .to_string(),
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(
                        "No model data yet. Run a `review` first to populate model metrics."
                            .to_string(),
                    );
                }
                Err(e) => return Err(format!("failed to read models.md: {e}")),
            }
        }

        if category == "all" || category == "models" {
            let path = self.models_path();
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => {
                    // Return only the summary section, not the full event log
                    let (summary, _) = parse_models_file(&content);
                    if !summary.is_empty() {
                        sections.push(format!("# Model Performance\n\n{summary}"));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("failed to read models.md: {e}")),
            }
        }

        if category == "all" || category == "patterns" {
            let path = self.patterns_path();
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => {
                    if let Some(filter_scope) = scope {
                        // Filter entries by exact scope match
                        let entries = parse_pattern_entries(&content);
                        let filtered: Vec<&str> = entries
                            .iter()
                            .filter(|entry| {
                                entry.lines().any(|line| {
                                    line.starts_with("- Scope: ")
                                        && line.trim_start_matches("- Scope: ").trim()
                                            == filter_scope.trim()
                                })
                            })
                            .map(|s| s.as_str())
                            .collect();
                        if !filtered.is_empty() {
                            sections
                                .push(format!("# Recurring Patterns\n\n{}", filtered.join("\n")));
                        }
                    } else {
                        sections.push(content);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("failed to read patterns.md: {e}")),
            }
        }

        if category == "all" || category == "tactics" {
            let path = self.tactics_path();
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => {
                    if let Some(m) = model.filter(|m| !m.is_empty()) {
                        // Filter to only the lines mentioning this model
                        let filtered: Vec<&str> = content
                            .lines()
                            .filter(|line| {
                                line.starts_with('#')
                                    || line.contains(&format!("[{m}]"))
                                    || line.trim().is_empty()
                            })
                            .collect();
                        if filtered.iter().any(|l| l.contains(&format!("[{m}]"))) {
                            sections.push(filtered.join("\n"));
                        }
                    } else {
                        sections.push(content);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("failed to read tactics.md: {e}")),
            }
        }

        if sections.is_empty() {
            return Ok("No memory found. Use the `memorize` tool to save learnings, or run a `review` to auto-populate model metrics.".to_string());
        }

        let mut result = sections.join("\n\n---\n\n");
        const TRUNCATION_SUFFIX: &str = "\n\n[truncated]";
        if result.len() > max_chars {
            // Reserve space for the suffix so total output stays within max_chars
            let target = max_chars.saturating_sub(TRUNCATION_SUFFIX.len());
            let boundary = floor_char_boundary(&result, target);
            result.truncate(boundary);
            result.push_str(TRUNCATION_SUFFIX);
        }

        Ok(result)
    }

    /// Returns per-model stats parsed from models.md event log.
    /// Used by hard gates in ReviewExecutor to exclude underperforming models.
    /// Returns None if models.md doesn't exist or has no events.
    /// If `id_to_key` is provided, normalizes model_ids to config keys.
    pub async fn get_model_stats(
        &self,
        id_to_key: Option<&HashMap<String, String>>,
    ) -> Option<HashMap<String, ModelGateStats>> {
        let content = tokio::fs::read_to_string(self.models_path()).await.ok()?;
        let (_, events) = parse_models_file(&content);
        if events.is_empty() {
            return None;
        }

        // (total_latency, quality_count, successes, infra_failures, last_seen)
        let mut stats: HashMap<String, (f64, usize, usize, usize, String)> = HashMap::new();
        for line in &events {
            let cols: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
            if cols.len() < 8 {
                continue;
            }
            let raw_model = cols[2];
            // Normalize model_id → config key if map provided
            let model = id_to_key
                .and_then(|map| map.get(raw_model))
                .cloned()
                .unwrap_or_else(|| raw_model.to_string());
            let latency: f64 = cols[3].trim_end_matches('s').parse().unwrap_or(0.0);
            let status = cols[4];
            let partial = cols[5];
            // Detect format: 10+ elements = new (has reason), 9 = old (no reason)
            let reason = if cols.len() >= 10 {
                cols[6]
            } else {
                "\u{2014}"
            };
            let event_date = cols[1].get(..10).unwrap_or("").to_string();

            let entry = stats.entry(model).or_insert((0.0, 0, 0, 0, String::new()));

            // Exclude infrastructure failures from quality stats
            let is_infra = matches!(reason, "auth_failed" | "rate_limited");
            if is_infra {
                entry.3 += 1; // infrastructure_failures
            } else {
                entry.0 += latency;
                entry.1 += 1; // quality_count
                if status == "success" && partial != "yes" {
                    entry.2 += 1; // successes
                }
            }
            if event_date > entry.4 {
                entry.4 = event_date;
            }
        }

        let result: HashMap<String, ModelGateStats> = stats
            .into_iter()
            .map(
                |(model, (total_lat, quality_count, successes, infra_failures, last_seen))| {
                    (
                        model,
                        ModelGateStats {
                            success_rate: if quality_count > 0 {
                                successes as f64 / quality_count as f64
                            } else {
                                0.0
                            },
                            avg_latency_secs: if quality_count > 0 {
                                total_lat / quality_count as f64
                            } else {
                                0.0
                            },
                            sample_count: quality_count,
                            infrastructure_failures: infra_failures,
                            last_seen,
                        },
                    )
                },
            )
            .collect();

        Some(result)
    }

    /// Flush branch-scoped memory after PR merge.
    /// - Patterns with evidence >= 3 scoped to this branch: graduate to "codebase"
    /// - Patterns with evidence < 3 scoped to this branch: move to archive.md
    /// - Model events older than 30 days: pruned
    ///
    /// Returns a graduation report string.
    pub async fn flush_branch(&self, branch: &str) -> Result<String, String> {
        let _lock = self.write_lock.lock().await;

        let branch_scope = format!("branch:{branch}");
        let mut graduated = 0usize;
        let mut archived = 0usize;

        // Process patterns
        let patterns_path = self.patterns_path();
        let existing = read_to_string_lossy(&patterns_path)
            .await
            .map_err(|e| format!("failed to read patterns.md: {e}"))?;
        let entries = parse_pattern_entries(&existing);

        let mut kept = Vec::new();
        let mut archive_entries = Vec::new();

        for entry in &entries {
            let entry_scope = extract_entry_scope(entry);
            if entry_scope == Some(&branch_scope) {
                let evidence = extract_evidence_count(entry);
                if evidence >= 3 {
                    // Graduate: change scope to codebase
                    let updated =
                        entry.replace(&format!("- Scope: {branch_scope}"), "- Scope: codebase");
                    kept.push(updated);
                    graduated += 1;
                } else {
                    // Archive: move to archive.md
                    archive_entries.push(entry.clone());
                    archived += 1;
                }
            } else {
                kept.push(entry.clone());
            }
        }

        // Write archive FIRST — if this fails, patterns.md is untouched and
        // no data is lost. Previously, patterns were rewritten first, so an
        // archive write failure would lose the archived entries permanently.
        if !archive_entries.is_empty() {
            let archive_path = self.archive_path();
            let mut archive = read_to_string_lossy(&archive_path)
                .await
                .map_err(|e| format!("failed to read archive.md: {e}"))?;
            if archive.is_empty() {
                archive = "# Archived Patterns\n\n".to_string();
            }
            for entry in &archive_entries {
                archive.push_str(entry);
                archive.push('\n');
            }
            atomic_write(&archive_path, &archive)
                .await
                .map_err(|e| e.to_string())?;
        }

        // Write updated patterns (safe now — archived entries are persisted).
        if graduated > 0 || archived > 0 {
            let output = format!("# Recurring Patterns\n\n{}", kept.join("\n"));
            atomic_write(&patterns_path, &output)
                .await
                .map_err(|e| e.to_string())?;
        }

        // Prune model events older than 30 days
        let pruned_events = self.prune_old_model_events(30).await;

        Ok(format!(
            "Flush complete for branch '{branch}': \
             {graduated} patterns graduated to codebase, \
             {archived} patterns archived, \
             {pruned_events} old model events pruned"
        ))
    }

    /// Prune model events older than `max_age_days` from models.md.
    /// Returns the number of events pruned.
    async fn prune_old_model_events(&self, max_age_days: u64) -> usize {
        let path = self.models_path();
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(_) => return 0,
        };

        let (_, events) = parse_models_file(&content);
        if events.is_empty() {
            return 0;
        }

        let cutoff_date = {
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let cutoff_secs = now_secs.saturating_sub(max_age_days * 86400);
            let days = cutoff_secs / 86400;
            let (year, month, day) = days_to_ymd(days);
            format!("{year:04}-{month:02}-{day:02}")
        };

        let old_count = events.len();
        let kept_events: Vec<String> = events
            .iter()
            .filter(|event| {
                // Extract timestamp from event row: | YYYY-MM-DD... |
                event
                    .strip_prefix("| ")
                    .and_then(|s| s.get(..10))
                    .is_none_or(|date| date >= cutoff_date.as_str())
            })
            .cloned()
            .collect();
        let pruned = old_count - kept_events.len();

        if pruned > 0 {
            let summary = compute_summary(&kept_events, &self.id_to_key);
            let new_content = format_models_file(&summary, &kept_events);
            let _ = atomic_write(&path, &new_content).await;
        }

        pruned
    }
}

/// Atomic write: write to temp file, then rename.
/// Temp filename includes PID to avoid cross-process collisions.
async fn atomic_write(path: &PathBuf, content: &str) -> Result<(), std::io::Error> {
    let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));
    tokio::fs::write(&tmp_path, content.as_bytes()).await?;
    if let Err(e) = tokio::fs::rename(&tmp_path, path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }
    Ok(())
}

/// Parse models.md into (summary_section, event_log_lines).
pub(crate) fn parse_models_file(content: &str) -> (String, Vec<String>) {
    if content.is_empty() {
        return (String::new(), Vec::new());
    }

    let mut summary = String::new();
    let mut events = Vec::new();
    let mut in_events = false;
    let mut in_summary = false;
    let mut past_event_header = false;

    for line in content.lines() {
        if line.starts_with("## Summary") {
            in_summary = true;
            in_events = false;
            continue;
        }
        if line.starts_with("## Recent Events") {
            in_events = true;
            in_summary = false;
            continue;
        }
        if in_events && line.starts_with("| Timestamp") {
            // Skip the header row
            past_event_header = true;
            continue;
        }
        if in_events && line.starts_with("|---") {
            // Skip separator
            continue;
        }
        if in_summary {
            summary.push_str(line);
            summary.push('\n');
        }
        if in_events && past_event_header && line.starts_with('|') {
            events.push(line.to_string());
        }
    }

    (summary.trim().to_string(), events)
}

/// Compute summary table from event log lines.
/// If `id_to_key` is non-empty, normalizes model names from provider model_ids to config keys.
fn compute_summary(events: &[String], id_to_key: &HashMap<String, String>) -> String {
    struct ModelStats {
        total_latency: f64,
        count: usize,
        successes: usize,
        latencies: Vec<f64>,
        common_errors: HashMap<String, usize>,
    }

    let mut stats: HashMap<String, ModelStats> = HashMap::new();

    for line in events {
        let cols: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
        // New format (10 elements): | timestamp | model | latency | status | partial | reason | error | prompt_len |
        // Old format (9 elements):  | timestamp | model | latency | status | partial | error | prompt_len |
        if cols.len() < 8 {
            continue;
        }
        let raw_model = cols[2];
        let model = id_to_key
            .get(raw_model)
            .cloned()
            .unwrap_or_else(|| raw_model.to_string());
        let latency_str = cols[3].trim_end_matches('s');
        let latency: f64 = latency_str.parse().unwrap_or(0.0);
        let status = cols[4];
        let partial = cols[5];
        // Detect format by column count: 10+ = new (has reason), 9 = old (no reason)
        let (reason, error) = if cols.len() >= 10 {
            (cols[6], cols[7])
        } else {
            ("\u{2014}", cols[6])
        };

        let entry = stats.entry(model).or_insert_with(|| ModelStats {
            total_latency: 0.0,
            count: 0,
            successes: 0,
            latencies: Vec::new(),
            common_errors: HashMap::new(),
        });

        // Exclude infrastructure failures from quality stats
        let is_infra = matches!(reason, "auth_failed" | "rate_limited");
        if !is_infra {
            entry.total_latency += latency;
            entry.count += 1;
            entry.latencies.push(latency);
            if status == "success" && partial != "yes" {
                entry.successes += 1;
            }
        }
        if error != "\u{2014}" && !error.is_empty() {
            *entry.common_errors.entry(error.to_string()).or_insert(0) += 1;
        }
    }

    let mut rows: Vec<(String, String)> = Vec::new();
    let mut sorted_models: Vec<&String> = stats.keys().collect();
    sorted_models.sort();

    for model in sorted_models {
        let s = &stats[model];
        let avg = if s.count > 0 {
            format!("{:.0}s", s.total_latency / s.count as f64)
        } else {
            "\u{2014}".to_string()
        };

        let mut sorted_latencies = s.latencies.clone();
        sorted_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p95 = if !sorted_latencies.is_empty() {
            let idx = ((sorted_latencies.len() as f64) * 0.95).ceil() as usize;
            let idx = idx.min(sorted_latencies.len()) - 1;
            format!("{:.0}s", sorted_latencies[idx])
        } else {
            "\u{2014}".to_string()
        };

        let rate = if s.count > 0 {
            let pct = (s.successes as f64 / s.count as f64 * 100.0).round() as u64;
            format!("{pct}%")
        } else {
            "\u{2014}".to_string()
        };

        let top_error = s
            .common_errors
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(err, _)| err.clone())
            .unwrap_or_else(|| "\u{2014}".to_string());

        let today = iso_date();
        rows.push((
            model.clone(),
            format!("| {model} | {avg} | {p95} | {rate} | {top_error} | {today} |"),
        ));
    }

    let mut table = String::from(
        "| Model | Avg Latency | P95 Latency | Success Rate | Common Failures | Last Updated |\n",
    );
    table.push_str(
        "|-------|-------------|-------------|--------------|-----------------|--------------|",
    );
    for (_, row) in &rows {
        table.push('\n');
        table.push_str(row);
    }
    table
}

/// Generate model recommendations with recency-weighted confidence.
///
/// Parses the event log to compute per-model stats, applies a 90-day
/// decay to confidence, and generates actionable recommendations for
/// model selection.
/// If `id_to_key` is non-empty, normalizes model names from provider model_ids to config keys.
fn generate_recommendations(models_content: &str, id_to_key: &HashMap<String, String>) -> String {
    let (_, events) = parse_models_file(models_content);
    if events.is_empty() {
        return String::new();
    }

    struct ModelRec {
        avg_latency: f64,
        success_rate: f64,
        count: usize,
        last_seen: String, // YYYY-MM-DD
        confidence: f64,
    }

    let today = iso_date();
    let today_days = date_to_days(&today).unwrap_or(0);

    let mut stats: HashMap<String, (f64, usize, usize, String)> = HashMap::new(); // (total_lat, count, successes, last_seen)

    for line in &events {
        let cols: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
        if cols.len() < 8 {
            continue;
        }
        let raw_model = cols[2];
        let model = id_to_key
            .get(raw_model)
            .cloned()
            .unwrap_or_else(|| raw_model.to_string());
        let latency: f64 = cols[3].trim_end_matches('s').parse().unwrap_or(0.0);
        let status = cols[4];
        let partial = cols[5];
        let reason = if cols.len() >= 10 {
            cols[6]
        } else {
            "\u{2014}"
        };
        let event_date = cols[1].get(..10).unwrap_or("").to_string();

        let entry = stats.entry(model).or_insert((0.0, 0, 0, String::new()));

        // Exclude infrastructure failures from quality stats
        let is_infra = matches!(reason, "auth_failed" | "rate_limited");
        if !is_infra {
            entry.0 += latency;
            entry.1 += 1;
            if status == "success" && partial != "yes" {
                entry.2 += 1;
            }
        }
        if event_date > entry.3 {
            entry.3 = event_date;
        }
    }

    let mut recs: Vec<(String, ModelRec)> = stats
        .into_iter()
        .map(|(model, (total_lat, count, successes, last_seen))| {
            let avg_latency = if count > 0 {
                total_lat / count as f64
            } else {
                0.0
            };
            let success_rate = if count > 0 {
                successes as f64 / count as f64
            } else {
                0.0
            };
            let days_since = date_to_days(&last_seen)
                .map(|d| today_days.saturating_sub(d))
                .unwrap_or(90);
            let confidence = (1.0 - days_since as f64 / 90.0).max(0.1);

            (
                model,
                ModelRec {
                    avg_latency,
                    success_rate,
                    count,
                    last_seen,
                    confidence,
                },
            )
        })
        .collect();

    // Sort by Bayesian-smoothed score: confidence * smoothed_success_rate.
    // Bayesian smoothing: (successes + prior_successes) / (count + prior_count)
    // This prevents a model with 1/1 from outranking one with 95/100.
    const PRIOR_COUNT: f64 = 5.0;
    const PRIOR_RATE: f64 = 0.5;
    recs.sort_by(|a, b| {
        let smoothed_a = (a.1.success_rate * a.1.count as f64 + PRIOR_RATE * PRIOR_COUNT)
            / (a.1.count as f64 + PRIOR_COUNT);
        let smoothed_b = (b.1.success_rate * b.1.count as f64 + PRIOR_RATE * PRIOR_COUNT)
            / (b.1.count as f64 + PRIOR_COUNT);
        let score_a = a.1.confidence * smoothed_a;
        let score_b = b.1.confidence * smoothed_b;
        score_b
            .partial_cmp(&score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut output = String::from("# Model Recommendations\n\n");

    // Quick triage: fastest model with >80% success
    if let Some((name, r)) = recs
        .iter()
        .filter(|(_, r)| r.success_rate > 0.8 && r.confidence > 0.3)
        .min_by(|a, b| {
            a.1.avg_latency
                .partial_cmp(&b.1.avg_latency)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    {
        output.push_str(&format!(
            "**Quick triage**: {} ({:.0}s avg, {:.0}% success, confidence {:.0}%)\n\n",
            name,
            r.avg_latency,
            r.success_rate * 100.0,
            r.confidence * 100.0,
        ));
    }

    // Thorough: highest success rate models
    let thorough: Vec<&str> = recs
        .iter()
        .filter(|(_, r)| r.success_rate > 0.9 && r.confidence > 0.3)
        .take(2)
        .map(|(name, _)| name.as_str())
        .collect();
    if !thorough.is_empty() {
        output.push_str(&format!(
            "**Thorough review**: {}\n\n",
            thorough.join(" + ")
        ));
    }

    // Full table
    output.push_str("| Model | Avg Latency | Success Rate | Confidence | Last Seen | Samples |\n");
    output.push_str("|-------|-------------|--------------|------------|-----------|---------|");
    for (name, r) in &recs {
        output.push_str(&format!(
            "\n| {} | {:.0}s | {:.0}% | {:.0}% | {} | {} |",
            name,
            r.avg_latency,
            r.success_rate * 100.0,
            r.confidence * 100.0,
            r.last_seen,
            r.count,
        ));
    }

    output
}

/// Parse a YYYY-MM-DD date string to days since Unix epoch.
/// Returns None for malformed or out-of-range dates (year < 1970, month/day = 0).
fn date_to_days(date: &str) -> Option<u64> {
    if date.len() < 10 {
        return None;
    }
    let year: u64 = date[..4].parse().ok()?;
    let month: u64 = date[5..7].parse().ok()?;
    let day: u64 = date[8..10].parse().ok()?;
    // Guard against underflow in ymd_to_days (u64 arithmetic wraps for pre-epoch dates)
    if year < 1970 || month == 0 || day == 0 || month > 12 || day > 31 {
        return None;
    }
    Some(ymd_to_days(year, month, day))
}

/// Convert (year, month, day) to days since Unix epoch.
/// Inverse of days_to_ymd (Howard Hinnant civil_from_days).
/// Caller must ensure year >= 1, month >= 1, day >= 1.
fn ymd_to_days(year: u64, month: u64, day: u64) -> u64 {
    let y = if month <= 2 { year - 1 } else { year };
    let m = if month <= 2 { month + 9 } else { month - 3 };
    let era = y / 400;
    let yoe = y % 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Parse an ISO 8601 timestamp like `"2026-02-23T15:14:14Z"` into Unix epoch milliseconds.
///
/// Uses the existing `ymd_to_days` for the date portion and manually parses HMS.
/// Returns `None` for malformed input.
#[cfg_attr(not(feature = "global-memory"), allow(dead_code))]
pub(crate) fn parse_iso_to_epoch_ms(s: &str) -> Option<i64> {
    // Minimum: "YYYY-MM-DDTHH:MM:SSZ" = 20 chars
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }

    let year: u64 = s.get(..4)?.parse().ok()?;
    let month: u64 = s.get(5..7)?.parse().ok()?;
    let day: u64 = s.get(8..10)?.parse().ok()?;

    // Validate date parts (year < 1970 would underflow u64 in ymd_to_days)
    if year < 1970 || month == 0 || day == 0 || month > 12 || day > 31 {
        return None;
    }

    // Check separator
    if s.as_bytes().get(10)? != &b'T' {
        return None;
    }

    let hour: u64 = s.get(11..13)?.parse().ok()?;
    let min: u64 = s.get(14..16)?.parse().ok()?;
    let sec: u64 = s.get(17..19)?.parse().ok()?;

    if hour > 23 || min > 59 || sec > 59 {
        return None;
    }

    let days = ymd_to_days(year, month, day);
    let epoch_secs = days * 86400 + hour * 3600 + min * 60 + sec;
    Some(epoch_secs as i64 * 1000)
}

/// Format the full models.md file.
fn format_models_file(summary: &str, events: &[String]) -> String {
    let mut output = String::from("# Model Performance Profiles\n\n");
    output.push_str("## Summary (auto-generated)\n");
    output.push_str(summary);
    output.push_str("\n\n## Recent Events (last 100)\n");
    output.push_str(
        "| Timestamp | Model | Latency | Status | Partial | Reason | Error | Prompt Len |\n",
    );
    output.push_str(
        "|-----------|-------|---------|--------|---------|--------|-------|------------|",
    );
    for event in events {
        output.push('\n');
        output.push_str(event);
    }
    output.push('\n');
    output
}

/// Parse pattern entries from patterns.md.
fn parse_pattern_entries(content: &str) -> Vec<String> {
    if content.is_empty() {
        return Vec::new();
    }

    let mut entries = Vec::new();
    let mut current_entry = String::new();

    for line in content.lines() {
        if line.starts_with("## [") {
            if !current_entry.is_empty() {
                entries.push(current_entry.trim().to_string());
            }
            current_entry = format!("{line}\n");
        } else if line.starts_with("# ") {
            // Skip the top-level heading
            continue;
        } else if !current_entry.is_empty() {
            current_entry.push_str(line);
            current_entry.push('\n');
        }
    }
    if !current_entry.is_empty() {
        entries.push(current_entry.trim().to_string());
    }
    entries
}

/// Compute a stable content hash for dedup.
/// Normalizes content: lowercase, collapse whitespace. Includes scope
/// so that identical content under different scopes remains separate.
/// Returns hex string (16 chars from DefaultHasher).
fn content_hash(content: &str, scope: Option<&str>) -> String {
    let normalized: String = content
        .chars()
        .flat_map(|c| c.to_lowercase())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut hasher = DefaultHasher::new();
    normalized.hash(&mut hasher);
    // Include scope in hash so same content under different scopes stays separate
    scope.unwrap_or("").hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Extract the hash from a pattern entry's `<!-- hash:xxx -->` comment.
fn extract_entry_hash(entry: &str) -> Option<&str> {
    for line in entry.lines() {
        if let Some(rest) = line.strip_prefix("<!-- hash:")
            && let Some(hash) = rest.strip_suffix(" -->")
        {
            return Some(hash);
        }
    }
    None
}

/// Extract the evidence count from a pattern entry heading like `## [...] content [x3]`.
/// Returns the parsed count, or 1 if no `[xN]` marker is present.
/// Returns 0 if a `[x...]` marker is present but contains an invalid number (e.g. `[xNaN]`).
fn extract_evidence_count(entry: &str) -> usize {
    let first_line = entry.lines().next().unwrap_or("");
    if let Some(bracket_start) = first_line.rfind("[x")
        && let Some(end) = first_line[bracket_start + 2..].find(']')
    {
        let num_str = &first_line[bracket_start + 2..][..end];
        return num_str.parse::<usize>().unwrap_or(0);
    }
    // No [xN] marker at all → genuinely first occurrence.
    1
}

/// Extract the first-seen date from an entry's `- Evidence:` line.
fn extract_first_seen(entry: &str) -> Option<&str> {
    for line in entry.lines() {
        if let Some(rest) = line.strip_prefix("- Evidence: ") {
            // Format: "N occurrences (YYYY-MM-DD to YYYY-MM-DD)"
            if let Some(paren_start) = rest.find('(') {
                let after = &rest[paren_start + 1..];
                if after.len() >= 10 {
                    return Some(&after[..10]);
                }
            }
        }
    }
    None
}

/// Extract the scope from a pattern entry.
fn extract_entry_scope(entry: &str) -> Option<&str> {
    for line in entry.lines() {
        if let Some(rest) = line.strip_prefix("- Scope: ") {
            return Some(rest.trim());
        }
    }
    None
}

/// Extract the `- Model: ...` line from an existing entry.
fn extract_entry_model(entry: &str) -> Option<&str> {
    for line in entry.lines() {
        if let Some(rest) = line.strip_prefix("- Model: ") {
            return Some(rest.trim());
        }
    }
    None
}

/// Extract the `- Tags: ...` line from an existing entry.
fn extract_entry_tags(entry: &str) -> Option<&str> {
    for line in entry.lines() {
        if let Some(rest) = line.strip_prefix("- Tags: ") {
            return Some(rest.trim());
        }
    }
    None
}

/// Extract metadata lines (lines starting with `- ` that aren't known fields).
fn extract_entry_metadata_lines(entry: &str) -> Vec<&str> {
    entry
        .lines()
        .filter(|line| {
            line.starts_with("- ")
                && !line.starts_with("- Evidence:")
                && !line.starts_with("- Scope:")
                && !line.starts_with("- Model:")
                && !line.starts_with("- Tags:")
        })
        .collect()
}

/// ISO date string (YYYY-MM-DD).
fn iso_date() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = now / 86400;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// ISO timestamp string (YYYY-MM-DDTHH:MM:SSZ).
fn iso_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = now / 86400;
    let day_secs = now % 86400;
    let (year, month, day) = days_to_ymd(days);
    let hour = day_secs / 3600;
    let min = (day_secs % 3600) / 60;
    let sec = day_secs % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Algorithm: civil_from_days (Howard Hinnant)
    days += 719468;
    let era = days / 146097;
    let doe = days % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Replace pipe characters in markdown table cell values to prevent column corruption.
fn escape_pipes(s: &str) -> String {
    s.replace('|', "\u{00a6}") // broken bar (¦) — visually similar, won't break table parsing
}

/// Find the largest valid char boundary <= `max`.
pub(crate) fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Read a file as a string, replacing invalid UTF-8 with the replacement character.
/// Returns empty string if the file does not exist.
/// Returns Err for other I/O errors (permissions, locks) to prevent data loss
/// in read-modify-write callers.
async fn read_to_string_lossy(path: &std::path::Path) -> Result<String, std::io::Error> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// Index file content.
const INDEX_CONTENT: &str = "# Squall Memory

This directory contains Squall's learning from past interactions.

## For AI Callers (Claude Code, Cursor)
- Read `models.md` to choose which models to use for a task
- Read `patterns.md` to see known patterns in the codebase under review
- Read `tactics.md` to craft better system_prompt for each model
- Use the `memorize` tool to save learnings after a review
- Use the `memory` tool to retrieve relevant memory for injection

## Files
- `models.md` \u{2014} Auto-updated model performance stats
- `patterns.md` \u{2014} Human/AI-curated recurring findings
- `tactics.md` \u{2014} What works for each model
";

// --- Public wrappers for integration testing (phase4_defects) ---

/// Public wrapper for `content_hash` (testing cross-version stability).
pub fn content_hash_pub(content: &str, scope: Option<&str>) -> String {
    content_hash(content, scope)
}

/// Public wrapper for `generate_recommendations` (testing sample-count weighting).
pub fn generate_recommendations_pub(models_content: &str) -> String {
    generate_recommendations(models_content, &HashMap::new())
}

/// Public wrapper for `iso_date` (needed by tests to build model events).
pub fn iso_date_pub() -> String {
    iso_date()
}

/// Public wrapper for `extract_evidence_count` (testing NaN handling).
pub fn extract_evidence_count_pub(entry: &str) -> usize {
    extract_evidence_count(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::review::ModelStatus;

    /// Create an isolated MemoryStore in a unique temp directory.
    async fn test_store(name: &str) -> (MemoryStore, PathBuf) {
        let tmp = std::env::temp_dir()
            .join("squall-test")
            .join(name)
            .join("memory");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        let store = MemoryStore::with_base_dir(tmp.clone());
        (store, tmp)
    }

    #[test]
    fn iso_date_format() {
        let d = iso_date();
        assert!(d.len() == 10, "expected YYYY-MM-DD, got: {d}");
        assert!(d.contains('-'));
    }

    #[test]
    fn iso_timestamp_format() {
        let ts = iso_timestamp();
        assert!(ts.contains('T'));
        assert!(ts.ends_with('Z'));
    }

    #[test]
    fn parse_empty_models_file() {
        let (summary, events) = parse_models_file("");
        assert!(summary.is_empty());
        assert!(events.is_empty());
    }

    #[test]
    fn parse_models_file_roundtrip() {
        let events = vec![
            "| 2026-02-21T14:00:00Z | grok | 22.0s | success | no | \u{2014} | 4200 |".to_string(),
            "| 2026-02-21T14:00:00Z | gemini | 145.0s | success | no | \u{2014} | 4200 |"
                .to_string(),
        ];
        let summary = compute_summary(&events, &HashMap::new());
        let output = format_models_file(&summary, &events);

        let (parsed_summary, parsed_events) = parse_models_file(&output);
        assert_eq!(parsed_events.len(), 2);
        assert!(!parsed_summary.is_empty());
    }

    #[test]
    fn compute_summary_basic() {
        let events = vec![
            "| 2026-02-21T14:00:00Z | grok | 22.0s | success | no | \u{2014} | 4200 |".to_string(),
            "| 2026-02-21T14:00:01Z | grok | 30.0s | success | no | \u{2014} | 4200 |".to_string(),
            "| 2026-02-21T14:00:02Z | grok | 65.0s | error | no | timeout | 4200 |".to_string(),
        ];
        let summary = compute_summary(&events, &HashMap::new());
        assert!(summary.contains("grok"));
        // 2/3 = 66.67% rounds to 67%
        assert!(summary.contains("67%"), "summary: {summary}");
    }

    #[test]
    fn parse_pattern_entries_basic() {
        let content = "# Recurring Patterns\n\n\
            ## [2026-02-21] First pattern\n\
            - Tags: foo, bar\n\n\
            ## [2026-02-20] Second pattern\n\
            - Tags: baz\n";
        let entries = parse_pattern_entries(content);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].contains("First pattern"));
        assert!(entries[1].contains("Second pattern"));
    }

    #[test]
    fn parse_pattern_entries_empty() {
        assert!(parse_pattern_entries("").is_empty());
    }

    #[test]
    fn valid_categories() {
        assert!(VALID_CATEGORIES.contains(&"pattern"));
        assert!(VALID_CATEGORIES.contains(&"tactic"));
        assert!(VALID_CATEGORIES.contains(&"recommend"));
        assert!(!VALID_CATEGORIES.contains(&"model_note"));
    }

    #[tokio::test]
    async fn memory_store_log_metrics() {
        let (store, tmp) = test_store("log-metrics").await;
        let results = vec![ReviewModelResult {
            model: "test-model".to_string(),
            provider: "test".to_string(),
            status: ModelStatus::Success,
            response: Some("ok".to_string()),
            error: None,
            reason: None,
            latency_ms: 5000,
            partial: false,
        }];

        store.log_model_metrics(&results, 1000, None).await;

        let content = tokio::fs::read_to_string(tmp.join("models.md"))
            .await
            .unwrap();
        assert!(content.contains("test-model"));
        assert!(content.contains("5.0s"));
        assert!(content.contains("success"));

        // Also verify index.md was created
        assert!(tokio::fs::try_exists(tmp.join("index.md")).await.unwrap());

        let _ = tokio::fs::remove_dir_all(tmp.parent().unwrap()).await;
    }

    #[tokio::test]
    async fn memory_store_memorize_pattern() {
        let (store, tmp) = test_store("memorize-pattern").await;
        let result = store
            .memorize(
                "pattern",
                "Race condition in session middleware",
                Some("gemini"),
                Some(&["concurrency".to_string(), "async".to_string()]),
                None,
                None,
            )
            .await;

        assert!(result.is_ok(), "error: {:?}", result.err());
        let path = result.unwrap();
        assert!(path.contains("patterns.md"));

        let content = tokio::fs::read_to_string(tmp.join("patterns.md"))
            .await
            .unwrap();
        assert!(content.contains("Race condition"));
        assert!(content.contains("gemini"));
        assert!(content.contains("concurrency"));

        let _ = tokio::fs::remove_dir_all(tmp.parent().unwrap()).await;
    }

    #[tokio::test]
    async fn memory_store_memorize_tactic() {
        let (store, tmp) = test_store("memorize-tactic").await;
        let result = store
            .memorize(
                "tactic",
                "Step-by-step reduces FP",
                Some("grok"),
                None,
                None,
                None,
            )
            .await;

        assert!(result.is_ok(), "error: {:?}", result.err());
        let content = tokio::fs::read_to_string(tmp.join("tactics.md"))
            .await
            .unwrap();
        assert!(content.contains("[grok] Step-by-step reduces FP"));

        let _ = tokio::fs::remove_dir_all(tmp.parent().unwrap()).await;
    }

    #[tokio::test]
    async fn memorize_rejects_invalid_category() {
        let store = MemoryStore::new();
        let result = store
            .memorize("invalid", "test", None, None, None, None)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid category"));
    }

    #[tokio::test]
    async fn memorize_rejects_empty_content() {
        let store = MemoryStore::new();
        let result = store
            .memorize("pattern", "   ", None, None, None, None)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must not be empty"));
    }

    #[tokio::test]
    async fn memorize_rejects_too_long_content() {
        let store = MemoryStore::new();
        let long_content = "x".repeat(MAX_MEMORIZE_CONTENT_LEN + 1);
        let result = store
            .memorize("pattern", &long_content, None, None, None, None)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too long"));
    }

    #[tokio::test]
    async fn memory_store_read_empty() {
        let (store, tmp) = test_store("read-empty").await;
        let result = store.read_memory(None, None, 4000, None).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("No memory found"));

        let _ = tokio::fs::remove_dir_all(tmp.parent().unwrap()).await;
    }

    #[tokio::test]
    async fn memory_store_read_after_write() {
        let (store, tmp) = test_store("read-after-write").await;

        // Write some data
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
        store.log_model_metrics(&results, 1000, None).await;
        store
            .memorize("pattern", "Found race condition", None, None, None, None)
            .await
            .unwrap();
        store
            .memorize(
                "tactic",
                "Use chain-of-thought",
                Some("grok"),
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Read all
        let all = store.read_memory(None, None, 10000, None).await.unwrap();
        assert!(all.contains("grok"), "all: {all}");
        assert!(all.contains("race condition"), "all: {all}");
        assert!(all.contains("chain-of-thought"), "all: {all}");

        // Read only models
        let models = store
            .read_memory(Some("models"), None, 10000, None)
            .await
            .unwrap();
        assert!(models.contains("grok"));
        assert!(!models.contains("race condition"));

        // Read tactics filtered by model
        let tactics = store
            .read_memory(Some("tactics"), Some("grok"), 10000, None)
            .await
            .unwrap();
        assert!(tactics.contains("[grok]"));

        let _ = tokio::fs::remove_dir_all(tmp.parent().unwrap()).await;
    }

    #[tokio::test]
    async fn pattern_pruning_at_cap() {
        let (store, tmp) = test_store("pattern-pruning").await;

        // Write MAX_PATTERN_ENTRIES + 5 patterns
        for i in 0..MAX_PATTERN_ENTRIES + 5 {
            store
                .memorize(
                    "pattern",
                    &format!("Pattern number {i}"),
                    None,
                    None,
                    None,
                    None,
                )
                .await
                .unwrap();
        }

        let content = tokio::fs::read_to_string(tmp.join("patterns.md"))
            .await
            .unwrap();
        let entries = parse_pattern_entries(&content);
        assert_eq!(
            entries.len(),
            MAX_PATTERN_ENTRIES,
            "should be capped at {MAX_PATTERN_ENTRIES}"
        );

        // Oldest should be pruned — entry 0..4 gone, entry 5 is first
        assert!(
            entries[0].contains("Pattern number 5"),
            "first entry: {}",
            entries[0]
        );

        let _ = tokio::fs::remove_dir_all(tmp.parent().unwrap()).await;
    }

    // --- parse_iso_to_epoch_ms tests ---

    #[test]
    fn parse_iso_known_timestamp() {
        // 2026-02-23T15:14:14Z → manually computed epoch ms
        let ms = parse_iso_to_epoch_ms("2026-02-23T15:14:14Z").unwrap();
        // 2026-02-23 = day 20507 from epoch. 20507*86400 = 1771804800
        // +15*3600 + 14*60 + 14 = 54000 + 840 + 14 = 54854
        // Total secs = 1771804800 + 54854 = 1771859654
        // ms = 1771859654000
        assert_eq!(ms, 1_771_859_654_000);
    }

    #[test]
    fn parse_iso_unix_epoch() {
        let ms = parse_iso_to_epoch_ms("1970-01-01T00:00:00Z").unwrap();
        assert_eq!(ms, 0);
    }

    #[test]
    fn parse_iso_rejects_short_input() {
        assert!(parse_iso_to_epoch_ms("2026-02-23").is_none());
    }

    #[test]
    fn parse_iso_rejects_bad_month() {
        assert!(parse_iso_to_epoch_ms("2026-13-01T00:00:00Z").is_none());
    }

    #[test]
    fn parse_iso_rejects_bad_hour() {
        assert!(parse_iso_to_epoch_ms("2026-02-23T25:00:00Z").is_none());
    }

    #[test]
    fn parse_iso_no_t_separator() {
        assert!(parse_iso_to_epoch_ms("2026-02-23 15:14:14Z").is_none());
    }

    #[test]
    fn parse_iso_trims_whitespace() {
        let ms = parse_iso_to_epoch_ms("  2026-02-23T15:14:14Z  ").unwrap();
        assert_eq!(ms, 1_771_859_654_000);
    }

    // ---- Bug fix tests: pre-epoch date underflow ----

    #[test]
    fn date_to_days_rejects_pre_epoch() {
        // Year 1 would underflow u64 in ymd_to_days → should return None
        assert!(date_to_days("0001-01-01").is_none());
        // Year 1969 is pre-epoch → should return None
        assert!(date_to_days("1969-12-31").is_none());
    }

    #[test]
    fn date_to_days_accepts_epoch() {
        // 1970-01-01 should be 0 days since epoch
        assert_eq!(date_to_days("1970-01-01"), Some(0));
    }

    #[test]
    fn date_to_days_accepts_modern_date() {
        // 2026-02-23 should produce a reasonable day count
        let days = date_to_days("2026-02-23").unwrap();
        assert!(days > 20000, "expected >20000 days since epoch, got {days}");
    }

    #[test]
    fn parse_iso_rejects_pre_epoch() {
        assert!(parse_iso_to_epoch_ms("0001-01-01T00:00:00Z").is_none());
        assert!(parse_iso_to_epoch_ms("1969-12-31T23:59:59Z").is_none());
    }

    #[test]
    fn parse_iso_accepts_epoch() {
        let ms = parse_iso_to_epoch_ms("1970-01-01T00:00:00Z").unwrap();
        assert_eq!(ms, 0);
    }

    // ---- Bug fix test: pre-epoch date gives min confidence, not max ----

    #[test]
    fn pre_epoch_date_gets_minimum_confidence() {
        // If date_to_days returns None, unwrap_or(90) → days_since=90 → confidence=0.1
        // This test verifies the fallback path gives minimum confidence
        let days_since = date_to_days("0001-01-01").unwrap_or(90);
        let confidence = (1.0 - days_since as f64 / 90.0).max(0.1);
        assert!(
            (confidence - 0.1).abs() < f64::EPSILON,
            "pre-epoch should get min confidence 0.1, got {confidence}"
        );
    }
}
