use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::error::SquallError;

/// Format for file context injection into model prompts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ContextFormat {
    /// Standard XML format: full file content wrapped in `<file>` tags.
    #[default]
    Xml,
    /// Hashline format: each line tagged with `line_number:hash|content`.
    /// The 2-char hex hash lets models reference specific lines compactly
    /// (e.g., "line 42:a3 has a bug") while saving tokens on long files.
    Hashline,
}

/// Git repository context: branch and commit SHA.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GitContext {
    /// Short commit SHA (7 chars).
    pub commit_sha: Option<String>,
    /// Current branch name (None if detached HEAD).
    pub branch: Option<String>,
}

/// Cache for git context to avoid repeated subprocess calls.
/// TTL of 5 seconds — commit/branch don't change during a single MCP tool execution.
/// Keyed by canonical working directory path to avoid cross-repo cache pollution.
pub struct GitContextCache {
    inner: Mutex<HashMap<PathBuf, (Instant, GitContext)>>,
}

impl Default for GitContextCache {
    fn default() -> Self {
        Self::new()
    }
}

impl GitContextCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Get cached git context, or detect fresh if expired/empty.
    pub async fn get_or_detect(&self, working_directory: &Path) -> Option<GitContext> {
        // Canonicalize to normalize symlinks and relative paths for cache key.
        let canonical = tokio::fs::canonicalize(working_directory).await.ok()?;

        let guard = self.inner.lock().await;
        if let Some((cached_at, ctx)) = guard.get(&canonical)
            && cached_at.elapsed() < Duration::from_secs(5)
        {
            return Some(ctx.clone());
        }
        // Drop guard before subprocess to avoid holding lock during I/O.
        drop(guard);

        let ctx = detect_git_context(working_directory).await?;

        let mut guard = self.inner.lock().await;
        guard.insert(canonical, (Instant::now(), ctx.clone()));
        Some(ctx)
    }
}

/// Detect git context (branch + short SHA) from a working directory.
/// Returns None if not a git repo or git is not available.
async fn detect_git_context(working_directory: &Path) -> Option<GitContext> {
    let sha = tokio::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(working_directory)
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    let branch = tokio::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(working_directory)
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    // If neither succeeded, we're not in a git repo
    if sha.is_none() && branch.is_none() {
        return None;
    }

    Some(GitContext {
        commit_sha: sha,
        branch,
    })
}

/// Derive a default scope string from git context.
/// - If branch is available: "branch:{name}"
/// - If only commit: "commit:{sha}"
/// - If no git context: "codebase"
pub fn default_scope_from_git(ctx: Option<&GitContext>) -> String {
    match ctx {
        Some(gc) if gc.branch.as_ref().is_some_and(|s| !s.is_empty()) => {
            format!("branch:{}", gc.branch.as_ref().unwrap())
        }
        Some(gc) if gc.commit_sha.as_ref().is_some_and(|s| !s.is_empty()) => {
            format!("commit:{}", gc.commit_sha.as_ref().unwrap())
        }
        _ => "codebase".to_string(),
    }
}

/// Get the normalized git remote URL for the given working directory.
/// Runs `git remote get-url origin` and normalizes SSH/HTTPS URLs to a canonical form.
/// Returns None if not a git repo, no `origin` remote, or git is not available.
#[cfg(feature = "global-memory")]
async fn detect_git_remote_url(working_directory: &Path) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(working_directory)
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())?;

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }

    Some(normalize_git_url(&raw))
}

/// Normalize a git remote URL to a canonical form for project identification.
///
/// Handles SSH (`git@github.com:user/repo.git`) and HTTPS (`https://github.com/user/repo.git`)
/// formats, producing identical output for the same repo regardless of access method.
///
/// Rules:
/// 1. Strip `.git` suffix
/// 2. Lowercase the host
/// 3. Convert SSH `git@host:path` to `host/path`
/// 4. Strip `https://` or `http://` prefix
/// 5. Strip trailing slashes
#[cfg(feature = "global-memory")]
pub fn normalize_git_url(url: &str) -> String {
    let mut s = url.trim().to_string();

    // Strip trailing slashes FIRST (before .git check, so "repo.git/" → "repo.git" → "repo")
    while s.ends_with('/') {
        s.pop();
    }

    // Strip .git suffix
    if s.ends_with(".git") {
        s.truncate(s.len() - 4);
    }

    // Strip ssh:// scheme (before git@ check, so "ssh://git@host/path" → "git@host/path")
    if let Some(rest) = s.strip_prefix("ssh://") {
        s = rest.to_string();
    }

    // SSH format: git@host:user/repo → host/user/repo
    // Also handles git@host/user/repo (from ssh:// stripping above)
    if let Some(rest) = s.strip_prefix("git@") {
        // Find separator: colon for shorthand (git@host:path), slash for ssh:// (git@host/path)
        if let Some(sep_pos) = rest.find(':').or_else(|| rest.find('/')) {
            let host = rest[..sep_pos].to_lowercase();
            let path = &rest[sep_pos + 1..];
            s = format!("{host}/{path}");
        }
    }

    // HTTPS/HTTP: strip scheme
    if let Some(rest) = s.strip_prefix("https://") {
        s = rest.to_string();
    } else if let Some(rest) = s.strip_prefix("http://") {
        s = rest.to_string();
    }

    // Lowercase the host portion (everything before the first /)
    if let Some(slash_pos) = s.find('/') {
        let host = s[..slash_pos].to_lowercase();
        s = format!("{host}{}", &s[slash_pos..]);
    } else {
        s = s.to_lowercase();
    }

    // Strip any remaining trailing slashes (after scheme stripping)
    while s.ends_with('/') {
        s.pop();
    }

    s
}

/// Compute a stable project identifier from a working directory.
///
/// Strategy:
/// 1. Try `git remote get-url origin` → normalize → sha256 hash with "git:" prefix
/// 2. Fallback: canonicalize the path → sha256 hash with "path:" prefix
///
/// Returns a string like `git:a1b2c3d4e5f6a1b2` or `path:f6e5d4c3b2a1f6e5`.
/// The 16-char hex suffix is the first 8 bytes of SHA-256, ensuring stability across
/// Rust versions (unlike DefaultHasher/SipHash which is not guaranteed stable).
#[cfg(feature = "global-memory")]
pub async fn compute_project_id(working_directory: &Path) -> String {
    use sha2::{Digest, Sha256};

    // Try git remote first
    if let Some(normalized_url) = detect_git_remote_url(working_directory).await {
        let digest = Sha256::digest(normalized_url.as_bytes());
        let hash_hex = hex::encode(&digest[..8]);
        return format!("git:{hash_hex}");
    }

    // Fallback: canonical path
    let canonical = tokio::fs::canonicalize(working_directory)
        .await
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| working_directory.to_string_lossy().to_string());

    let digest = Sha256::digest(canonical.as_bytes());
    let hash_hex = hex::encode(&digest[..8]);
    format!("path:{hash_hex}")
}

/// Maximum bytes of file content to inject into model prompts.
/// 2 MB ≈ 500K–700K tokens — well within frontier model context windows.
/// Models with smaller windows will reject at the provider level, which is
/// handled gracefully by the dispatch error path.
pub const MAX_FILE_CONTEXT_BYTES: usize = 2 * 1024 * 1024;

/// Minimum bytes reserved for diff context in review requests.
/// When both file_paths and diff are provided, file context is capped at
/// `MAX_FILE_CONTEXT_BYTES - MIN_DIFF_BUDGET` so the diff always gets space.
pub const MIN_DIFF_BUDGET: usize = 128 * 1024;

/// Maximum number of file paths allowed per request (prevents DoS).
pub const MAX_FILE_PATHS: usize = 100;

/// Escape XML content characters: `<`, `>`, `&`.
pub fn escape_xml_content(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escape XML attribute values: `"`, `<`, `>`, `&`.
pub fn escape_xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Escape text for inclusion in XML comments.
/// XML comments cannot contain `--`, so we replace it to prevent injection.
fn escape_xml_comment(s: &str) -> String {
    s.replace("--", "&#45;&#45;")
}

/// Format file content in hashline format: `line_number:hash|content\n` per line.
/// Hash is first 2 hex chars of a fast hash of the line content. This gives models
/// a compact way to reference specific lines (e.g., "line 42:a3") while still being
/// readable. XML escaping is applied to the content portion.
pub fn format_hashline(content: &str) -> String {
    let mut output = String::with_capacity(content.len() + content.lines().count() * 6);
    for (i, line) in content.lines().enumerate() {
        let hash = line_hash(line);
        let escaped = escape_xml_content(line);
        output.push_str(&format!("{}:{:02x}|{}\n", i + 1, hash, escaped));
    }
    output
}

/// Compute a 1-byte (0-255) hash of a line for hashline format.
/// Uses DefaultHasher (SipHash) for consistency with the memory system.
fn line_hash(line: &str) -> u8 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    line.hash(&mut hasher);
    hasher.finish() as u8
}

/// Validate that a path is safe: relative, no `..` components.
fn validate_path(path: &str) -> Result<(), SquallError> {
    let p = Path::new(path);
    if p.is_absolute() {
        return Err(SquallError::FileContext(format!(
            "absolute path not allowed: {path}"
        )));
    }
    for component in p.components() {
        if matches!(component, Component::ParentDir) {
            return Err(SquallError::FileContext(format!(
                "path traversal not allowed: {path}"
            )));
        }
    }
    Ok(())
}

/// Canonicalize a resolved path and verify it stays within base_dir.
/// Prevents symlink traversal attacks where a symlink escapes the sandbox.
async fn validate_no_symlink_escape(
    full_path: &Path,
    base_dir: &Path,
    rel_path: &str,
) -> Result<PathBuf, SquallError> {
    let canonical = tokio::fs::canonicalize(full_path)
        .await
        .map_err(|e| SquallError::FileContext(format!("{rel_path}: {e}")))?;

    if !canonical.starts_with(base_dir) {
        return Err(SquallError::SymlinkEscape(rel_path.to_string()));
    }

    Ok(canonical)
}

/// Result of resolving file context, with structured skip/error metadata.
#[derive(Debug)]
pub struct FileContextResult {
    /// The XML-formatted file context string (None if no files included).
    pub context: Option<String>,
    /// Files skipped due to budget (filename, size in bytes).
    pub skipped: Vec<(String, usize)>,
    /// Files that had read errors (non-fatal).
    pub errors: Vec<String>,
}

/// Read files and format as context for model prompts. All paths must be relative to `base_dir`.
/// Path traversal attempts reject the entire request.
/// Non-existent or unreadable files are noted but non-fatal (unless ALL fail).
///
/// `format` controls how file content is rendered:
/// - `Xml` (default): full content with XML escaping inside `<file>` tags
/// - `Hashline`: each line as `line_num:hash|content` inside `<file>` tags
pub async fn resolve_file_context(
    paths: &[String],
    base_dir: &Path,
    budget: usize,
    format: ContextFormat,
) -> Result<FileContextResult, SquallError> {
    if paths.is_empty() {
        return Ok(FileContextResult {
            context: None,
            skipped: vec![],
            errors: vec![],
        });
    }

    if paths.len() > MAX_FILE_PATHS {
        return Err(SquallError::FileContext(format!(
            "too many file paths: {} (max {})",
            paths.len(),
            MAX_FILE_PATHS
        )));
    }

    // Validate all paths first — traversal = reject entire request
    for p in paths {
        validate_path(p)?;
    }

    // Canonicalize base_dir for symlink checks (e.g., /tmp → /private/tmp on macOS).
    // In production this is a no-op (validate_working_directory already canonicalizes).
    let base_dir = &tokio::fs::canonicalize(base_dir)
        .await
        .map_err(|e| SquallError::FileContext(format!("cannot resolve base directory: {e}")))?;

    let mut output = String::new();
    let mut used = 0usize;
    let mut included = 0usize;
    let mut skipped: Vec<(String, usize)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for rel_path in paths {
        let full_path = base_dir.join(rel_path);

        // Canonicalize and verify the path stays within base_dir.
        // Prevents symlink traversal attacks. Non-existent files fail
        // canonicalize and go into errors (non-fatal).
        let canonical = match validate_no_symlink_escape(&full_path, base_dir, rel_path).await {
            Ok(c) => c,
            Err(e @ SquallError::SymlinkEscape(_)) => return Err(e),
            Err(e) => {
                errors.push(format!("{rel_path}: {e}"));
                continue;
            }
        };

        // Check file size via metadata BEFORE reading — prevents OOM on large files.
        // If raw file size alone exceeds remaining budget, the escaped+wrapped version
        // will certainly exceed it too, so we can skip without reading.
        let file_size = match tokio::fs::metadata(&canonical).await {
            Ok(m) => m.len() as usize,
            Err(e) => {
                errors.push(format!("{rel_path}: {e}"));
                continue;
            }
        };

        if file_size > budget.saturating_sub(used) {
            skipped.push((rel_path.clone(), file_size));
            continue;
        }

        let content = match tokio::fs::read_to_string(&canonical).await {
            Ok(c) => c,
            Err(e) => {
                errors.push(format!("{rel_path}: {e}"));
                continue;
            }
        };

        let formatted = match format {
            ContextFormat::Xml => escape_xml_content(&content),
            ContextFormat::Hashline => format_hashline(&content),
        };
        let entry = format!(
            "<file path=\"{}\">\n{}</file>\n",
            escape_xml_attr(rel_path),
            // Hashline already ends with \n per line; Xml needs trailing \n
            if format == ContextFormat::Xml {
                format!("{formatted}\n")
            } else {
                formatted
            }
        );

        // Post-read check: escaped content may be larger than raw (XML entities)
        if used + entry.len() > budget {
            skipped.push((rel_path.clone(), content.len()));
            continue;
        }

        output.push_str(&entry);
        used += entry.len();
        included += 1;
    }

    // All files had read errors (none skipped for budget) → hard error
    if included == 0 && skipped.is_empty() && !errors.is_empty() {
        return Err(SquallError::FileContext(format!(
            "all files unreadable: {}",
            errors.join("; ")
        )));
    }

    // Append manifest comment noting skipped/errored files.
    // Escape "--" sequences to prevent XML comment injection from filenames.
    if !skipped.is_empty() || !errors.is_empty() {
        let mut comment = String::new();
        if !skipped.is_empty() {
            let names: Vec<_> = skipped
                .iter()
                .map(|(n, sz)| format!("{n} ({sz}B)"))
                .collect();
            comment.push_str(&format!("Budget skipped: {}. ", names.join(", ")));
        }
        if !errors.is_empty() {
            comment.push_str(&format!("Errors: {}. ", errors.join("; ")));
        }
        output.push_str(&format!("<!-- {} -->\n", escape_xml_comment(&comment)));
    }

    Ok(FileContextResult {
        context: if output.is_empty() {
            None
        } else {
            Some(output)
        },
        skipped,
        errors,
    })
}

/// Lightweight manifest for CLI backends (paths only, no content).
/// CLI agents can read files themselves via `working_directory` as cwd.
pub async fn resolve_file_manifest(
    paths: &[String],
    base_dir: &Path,
) -> Result<Option<String>, SquallError> {
    if paths.is_empty() {
        return Ok(None);
    }

    if paths.len() > MAX_FILE_PATHS {
        return Err(SquallError::FileContext(format!(
            "too many file paths: {} (max {})",
            paths.len(),
            MAX_FILE_PATHS
        )));
    }

    for p in paths {
        validate_path(p)?;
    }

    let base_dir = &tokio::fs::canonicalize(base_dir)
        .await
        .map_err(|e| SquallError::FileContext(format!("cannot resolve base directory: {e}")))?;

    let mut lines = Vec::new();
    for rel_path in paths {
        let full_path = base_dir.join(rel_path);

        // Canonicalize to catch symlink escapes
        match validate_no_symlink_escape(&full_path, base_dir, rel_path).await {
            Ok(_) => {
                lines.push(format!("- {rel_path} (exists)"));
            }
            Err(e @ SquallError::SymlinkEscape(_)) => return Err(e),
            Err(_) => {
                lines.push(format!("- {rel_path} (not found)"));
            }
        }
    }

    let manifest = format!("Files referenced:\n{}", lines.join("\n"));
    Ok(Some(manifest))
}

/// Wrap diff text in XML tags for model prompt injection.
/// XML-escapes content to prevent prompt framing breaks (e.g. diff editing XML files
/// could contain `</diff>`). Budget is enforced on the **escaped** output to prevent
/// XML entity expansion (e.g. `<` → `&lt;`) from blowing past the limit.
/// Returns None if diff is empty or budget is zero.
pub fn wrap_diff_context(diff: &str, budget: usize) -> Option<String> {
    if diff.trim().is_empty() || budget == 0 {
        return None;
    }

    // Pre-truncate raw text to prevent OOM from huge inputs.
    // Without this, escape_xml_content allocates proportional to full input
    // (e.g., 500MB diff → 500MB+ allocation before truncation).
    let was_pre_truncated = diff.len() > budget;
    let diff = if was_pre_truncated {
        let safe_end = floor_char_boundary(diff, budget);
        &diff[..safe_end]
    } else {
        diff
    };

    // Escape then enforce budget on escaped output.
    let escaped = escape_xml_content(diff);

    let truncated = if escaped.len() > budget {
        // Find a safe UTF-8 char boundary, then find the last newline before it
        let safe_end = floor_char_boundary(&escaped, budget);
        // Backtrack past any partial XML entity (e.g. "&l" from "&lt;")
        let safe_end = floor_entity_boundary(&escaped, safe_end);
        match escaped[..safe_end].rfind('\n') {
            Some(pos) => &escaped[..pos + 1],
            None => &escaped[..safe_end], // single long line — hard cut
        }
    } else {
        &escaped
    };

    let was_truncated = was_pre_truncated || truncated.len() < escaped.len();
    let suffix = if was_truncated {
        "\n<!-- diff truncated due to budget -->"
    } else {
        ""
    };

    Some(format!("<diff>\n{truncated}{suffix}\n</diff>"))
}

/// Find the largest byte index ≤ `index` that doesn't split an XML entity.
/// If `index` lands inside `&amp;`, `&lt;`, or `&gt;`, backtrack to just before the `&`.
fn floor_entity_boundary(s: &str, index: usize) -> usize {
    if index == 0 || index >= s.len() {
        return index;
    }
    // Search backwards from index for '&'. If found, check whether a complete
    // entity (ending with ';') exists between that '&' and index.
    // Max entity length is 5 ("&amp;"), so look back at most 4 bytes.
    // Use floor_char_boundary to avoid slicing inside a multibyte character.
    let start = floor_char_boundary(s, index.saturating_sub(4));
    if let Some(amp_offset) = s[start..index].rfind('&') {
        let amp_pos = start + amp_offset;
        // Check if there's a ';' completing the entity before our cut point
        let after_amp = &s[amp_pos..s.len().min(amp_pos + 5)];
        if let Some(semi) = after_amp.find(';')
            && amp_pos + semi >= index
        {
            // The ';' is at or beyond our cut point → entity is split → backtrack
            return amp_pos;
        }
    }
    index
}

/// Find the largest byte index ≤ `index` that is a valid UTF-8 char boundary.
/// Equivalent to `str::floor_char_boundary` (nightly-only as of Rust 1.xx).
fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Validate temperature parameter: must be finite and in [0.0, 2.0].
pub fn validate_temperature(temp: Option<f64>) -> Result<(), String> {
    if let Some(t) = temp
        && (t.is_nan() || t.is_infinite() || !(0.0..=2.0).contains(&t))
    {
        return Err(format!("temperature must be between 0.0 and 2.0, got {t}"));
    }
    Ok(())
}

/// Validate prompt is non-empty.
pub fn validate_prompt(prompt: &str) -> Result<(), String> {
    if prompt.trim().is_empty() {
        return Err("prompt must not be empty".to_string());
    }
    Ok(())
}

/// Validate working directory exists, is a directory, and canonicalize it.
pub async fn validate_working_directory(path: &str) -> Result<PathBuf, SquallError> {
    let canonical = tokio::fs::canonicalize(path).await.map_err(|e| {
        SquallError::FileContext(format!("working directory not found: {path}: {e}"))
    })?;

    let meta = tokio::fs::metadata(&canonical).await.map_err(|e| {
        SquallError::FileContext(format!("cannot stat working directory: {path}: {e}"))
    })?;

    if !meta.is_dir() {
        return Err(SquallError::FileContext(format!(
            "{path} is not a directory"
        )));
    }

    Ok(canonical)
}

#[cfg(test)]
#[cfg(feature = "global-memory")]
mod project_id_tests {
    use super::*;

    #[test]
    fn normalize_ssh_and_https_match() {
        let ssh = normalize_git_url("git@github.com:user/repo.git");
        let https = normalize_git_url("https://github.com/user/repo.git");
        assert_eq!(ssh, https);
        assert_eq!(ssh, "github.com/user/repo");
    }

    #[test]
    fn normalize_strips_git_suffix() {
        let with = normalize_git_url("https://github.com/user/repo.git");
        let without = normalize_git_url("https://github.com/user/repo");
        assert_eq!(with, without);
    }

    #[test]
    fn normalize_lowercases_host() {
        let upper = normalize_git_url("https://GitHub.COM/user/repo");
        assert_eq!(upper, "github.com/user/repo");
    }

    #[test]
    fn normalize_preserves_path_case() {
        let url = normalize_git_url("https://github.com/User/Repo-Name.git");
        assert_eq!(url, "github.com/User/Repo-Name");
    }

    #[test]
    fn normalize_strips_trailing_slashes() {
        let url = normalize_git_url("https://github.com/user/repo///");
        assert_eq!(url, "github.com/user/repo");
    }

    #[test]
    fn normalize_handles_http() {
        let url = normalize_git_url("http://gitlab.com/user/repo.git");
        assert_eq!(url, "gitlab.com/user/repo");
    }

    #[test]
    fn normalize_ssh_custom_host() {
        let url = normalize_git_url("git@gitlab.company.com:team/project.git");
        assert_eq!(url, "gitlab.company.com/team/project");
    }

    #[test]
    fn normalize_different_repos_differ() {
        let a = normalize_git_url("https://github.com/user/repo-a.git");
        let b = normalize_git_url("https://github.com/user/repo-b.git");
        assert_ne!(a, b);
    }

    #[test]
    fn normalize_git_suffix_with_trailing_slash() {
        // Bug: .git stripped before trailing slash → "repo.git/" becomes "repo.git" not "repo"
        let url = normalize_git_url("https://github.com/user/repo.git/");
        assert_eq!(
            url, "github.com/user/repo",
            "trailing slash after .git should normalize correctly"
        );
    }

    #[test]
    fn normalize_ssh_scheme_matches_ssh_shorthand() {
        // ssh://git@host/path should normalize the same as git@host:path
        let ssh_scheme = normalize_git_url("ssh://git@github.com/user/repo.git");
        let ssh_shorthand = normalize_git_url("git@github.com:user/repo.git");
        assert_eq!(
            ssh_scheme, ssh_shorthand,
            "ssh:// scheme and git@ shorthand should normalize identically"
        );
        assert_eq!(ssh_scheme, "github.com/user/repo");
    }

    #[tokio::test]
    async fn compute_project_id_non_git_uses_path_prefix() {
        let tmp = std::env::temp_dir().join("squall-test-project-id-no-git");
        let _ = tokio::fs::create_dir_all(&tmp).await;
        let id = compute_project_id(&tmp).await;
        assert!(
            id.starts_with("path:"),
            "Non-git dir should use path: prefix, got: {id}"
        );
        assert_eq!(id.len(), 5 + 16, "path: prefix + 16 hex chars, got: {id}");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }

    #[tokio::test]
    async fn compute_project_id_git_repo_uses_git_prefix() {
        let id = compute_project_id(std::path::Path::new(".")).await;
        assert!(
            id.starts_with("git:"),
            "Git repo should use git: prefix, got: {id}"
        );
        assert_eq!(id.len(), 4 + 16, "git: prefix + 16 hex chars, got: {id}");
    }

    #[tokio::test]
    async fn compute_project_id_deterministic() {
        let id1 = compute_project_id(std::path::Path::new(".")).await;
        let id2 = compute_project_id(std::path::Path::new(".")).await;
        assert_eq!(id1, id2, "Same directory should produce same project ID");
    }
}
