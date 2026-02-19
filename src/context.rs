use std::path::{Component, Path, PathBuf};

use crate::error::SquallError;

/// Maximum bytes of file content to inject into HTTP model prompts.
pub const MAX_FILE_CONTEXT_BYTES: usize = 512 * 1024;

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
    let canonical = tokio::fs::canonicalize(full_path).await.map_err(|e| {
        SquallError::FileContext(format!("{rel_path}: {e}"))
    })?;

    if !canonical.starts_with(base_dir) {
        return Err(SquallError::FileContext(format!(
            "path escapes base directory: {rel_path}"
        )));
    }

    Ok(canonical)
}

/// Read files and format as XML context. All paths must be relative to `base_dir`.
/// Path traversal attempts reject the entire request.
/// Non-existent or unreadable files are noted but non-fatal (unless ALL fail).
pub async fn resolve_file_context(
    paths: &[String],
    base_dir: &Path,
    budget: usize,
) -> Result<Option<String>, SquallError> {
    if paths.is_empty() {
        return Ok(None);
    }

    // Validate all paths first — traversal = reject entire request
    for p in paths {
        validate_path(p)?;
    }

    // Canonicalize base_dir for symlink checks (e.g., /tmp → /private/tmp on macOS).
    // In production this is a no-op (validate_working_directory already canonicalizes).
    let base_dir = &tokio::fs::canonicalize(base_dir).await.map_err(|e| {
        SquallError::FileContext(format!("cannot resolve base directory: {e}"))
    })?;

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
            Err(e) => {
                // Symlink escape = hard reject (security). File not found = soft skip.
                if e.to_string().contains("escapes") {
                    return Err(e);
                }
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

        let escaped = escape_xml_content(&content);
        let entry = format!(
            "<file path=\"{}\">\n{}\n</file>\n",
            escape_xml_attr(rel_path),
            escaped
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

    // Append manifest comment noting skipped/errored files
    if !skipped.is_empty() || !errors.is_empty() {
        output.push_str("<!-- ");
        if !skipped.is_empty() {
            let names: Vec<_> = skipped
                .iter()
                .map(|(n, sz)| format!("{n} ({sz}B)"))
                .collect();
            output.push_str(&format!("Budget skipped: {}. ", names.join(", ")));
        }
        if !errors.is_empty() {
            output.push_str(&format!("Errors: {}. ", errors.join("; ")));
        }
        output.push_str("-->\n");
    }

    if output.is_empty() {
        Ok(None)
    } else {
        Ok(Some(output))
    }
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

    for p in paths {
        validate_path(p)?;
    }

    let base_dir = &tokio::fs::canonicalize(base_dir).await.map_err(|e| {
        SquallError::FileContext(format!("cannot resolve base directory: {e}"))
    })?;

    let mut lines = Vec::new();
    for rel_path in paths {
        let full_path = base_dir.join(rel_path);

        // Canonicalize to catch symlink escapes
        match validate_no_symlink_escape(&full_path, base_dir, rel_path).await {
            Ok(_) => {
                lines.push(format!("- {rel_path} (exists)"));
            }
            Err(e) => {
                // Symlink escape = hard reject
                if e.to_string().contains("escapes") {
                    return Err(e);
                }
                lines.push(format!("- {rel_path} (not found)"));
            }
        }
    }

    let manifest = format!("Files referenced:\n{}", lines.join("\n"));
    Ok(Some(manifest))
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
