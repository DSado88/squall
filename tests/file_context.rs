//! TDD tests for file context support.
//! Tests path sandboxing, budget enforcement, XML escaping,
//! async file reads, and request struct field presence.

// ---------------------------------------------------------------------------
// Path sandboxing: reject absolute paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reject_absolute_file_path() {
    let base = std::env::current_dir().unwrap();
    let paths = vec!["/etc/passwd".to_string()];
    let result = squall::context::resolve_file_context(&paths, &base, 512_000).await;
    assert!(result.is_err(), "Absolute paths must be rejected");
    let err = format!("{}", result.unwrap_err());
    assert!(err.contains("absolute") || err.contains("traversal"), "Error: {err}");
}

#[tokio::test]
async fn reject_dot_dot_traversal() {
    let base = std::env::current_dir().unwrap();
    let paths = vec!["../../etc/passwd".to_string()];
    let result = squall::context::resolve_file_context(&paths, &base, 512_000).await;
    assert!(result.is_err(), "Path traversal with .. must be rejected");
}

#[tokio::test]
async fn reject_dot_dot_in_middle() {
    let base = std::env::current_dir().unwrap();
    let paths = vec!["src/../../../etc/passwd".to_string()];
    let result = squall::context::resolve_file_context(&paths, &base, 512_000).await;
    assert!(result.is_err(), "Path traversal with embedded .. must be rejected");
}

// ---------------------------------------------------------------------------
// Path sandboxing: valid relative paths work
// ---------------------------------------------------------------------------

#[tokio::test]
async fn valid_relative_path_succeeds() {
    let base = std::env::current_dir().unwrap();
    let paths = vec!["Cargo.toml".to_string()];
    let result = squall::context::resolve_file_context(&paths, &base, 512_000).await;
    assert!(result.is_ok(), "Valid relative path should succeed");
    let ctx = result.unwrap().context.expect("Should have content");
    assert!(ctx.contains("Cargo.toml"));
    assert!(ctx.contains("[package]"));
}

#[tokio::test]
async fn valid_nested_relative_path() {
    let base = std::env::current_dir().unwrap();
    let paths = vec!["src/lib.rs".to_string()];
    let result = squall::context::resolve_file_context(&paths, &base, 512_000).await;
    assert!(result.is_ok());
    let ctx = result.unwrap().context.expect("Should have content");
    assert!(ctx.contains("src/lib.rs"));
}

// ---------------------------------------------------------------------------
// XML escaping
// ---------------------------------------------------------------------------

#[test]
fn xml_content_escaping() {
    // Content with angle brackets and ampersands should be escaped
    let escaped = squall::context::escape_xml_content("<script>alert('xss')</script> & more");
    assert!(escaped.contains("&lt;script&gt;"));
    assert!(escaped.contains("&amp; more"));
    assert!(!escaped.contains("<script>"));
}

#[test]
fn xml_attr_escaping() {
    let escaped = squall::context::escape_xml_attr("path/with\"quotes");
    assert!(escaped.contains("&quot;"));
    assert!(!escaped.contains('"'));
}

// ---------------------------------------------------------------------------
// Budget enforcement
// ---------------------------------------------------------------------------

#[tokio::test]
async fn budget_skips_oversized_file() {
    let base = std::env::current_dir().unwrap();
    let paths = vec!["Cargo.toml".to_string()];
    // Tiny budget that won't fit Cargo.toml
    let result = squall::context::resolve_file_context(&paths, &base, 10).await;
    assert!(result.is_ok());
    let file_result = result.unwrap();
    // Should have some output (manifest/notes) even if file didn't fit
    if let Some(text) = file_result.context {
        assert!(
            text.contains("skipped") || text.contains("Budget") || text.contains("exceeds"),
            "Should note the skipped file: {text}"
        );
    }
}

#[tokio::test]
async fn budget_includes_first_file_skips_second() {
    let base = std::env::current_dir().unwrap();
    let paths = vec![
        "src/lib.rs".to_string(),   // small
        "Cargo.lock".to_string(),   // large
    ];
    // Budget enough for lib.rs but not Cargo.lock
    let result = squall::context::resolve_file_context(&paths, &base, 300).await;
    assert!(result.is_ok());
    let ctx = result.unwrap().context.expect("Should have content");
    assert!(ctx.contains("src/lib.rs"), "First file should be included");
}

#[tokio::test]
async fn budget_skipped_populates_metadata() {
    let base = std::env::current_dir().unwrap();
    let paths = vec![
        "src/lib.rs".to_string(),   // small — fits
        "Cargo.lock".to_string(),   // large — skipped
    ];
    // Budget enough for lib.rs but not Cargo.lock
    let result = squall::context::resolve_file_context(&paths, &base, 300).await;
    let file_result = result.unwrap();
    assert!(file_result.context.is_some(), "lib.rs should be included");
    assert!(!file_result.skipped.is_empty(), "Cargo.lock should be in skipped list");
    assert_eq!(file_result.skipped[0].0, "Cargo.lock");
    assert!(file_result.skipped[0].1 > 0, "Skipped file size should be > 0");
}

#[tokio::test]
async fn budget_constant_is_reasonable() {
    let budget = squall::context::MAX_FILE_CONTEXT_BYTES;
    assert!(budget >= 100_000, "Budget too small: {budget}");
    assert!(budget <= 1_000_000, "Budget too large: {budget}");
}

// ---------------------------------------------------------------------------
// Non-existent files: non-fatal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nonexistent_file_is_nonfatal() {
    let base = std::env::current_dir().unwrap();
    let paths = vec![
        "Cargo.toml".to_string(),
        "this_file_does_not_exist_xyz.rs".to_string(),
    ];
    let result = squall::context::resolve_file_context(&paths, &base, 512_000).await;
    assert!(result.is_ok(), "Non-existent file should not fail the whole request");
    let ctx = result.unwrap().context.expect("Should have content from Cargo.toml");
    assert!(ctx.contains("Cargo.toml"));
    assert!(ctx.contains("this_file_does_not_exist_xyz.rs")); // noted in errors
}

#[tokio::test]
async fn all_files_nonexistent_returns_error() {
    let base = std::env::current_dir().unwrap();
    let paths = vec![
        "nonexistent_a.rs".to_string(),
        "nonexistent_b.rs".to_string(),
    ];
    let result = squall::context::resolve_file_context(&paths, &base, 512_000).await;
    // All files unreadable should be an error, not silent None
    assert!(result.is_err(), "All files unreadable should return Err");
}

// ---------------------------------------------------------------------------
// Empty paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_paths_returns_none() {
    let base = std::env::current_dir().unwrap();
    let result = squall::context::resolve_file_context(&[], &base, 512_000).await;
    assert!(result.is_ok());
    assert!(result.unwrap().context.is_none());
}

// ---------------------------------------------------------------------------
// CLI manifest (paths only, no content)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cli_manifest_contains_paths_not_content() {
    let base = std::env::current_dir().unwrap();
    let paths = vec!["Cargo.toml".to_string()];
    let result = squall::context::resolve_file_manifest(&paths, &base).await;
    assert!(result.is_ok());
    let manifest = result.unwrap().expect("Should have manifest");
    assert!(manifest.contains("Cargo.toml"), "Manifest should list file path");
    // Should NOT contain file content
    assert!(!manifest.contains("[package]"), "Manifest should NOT contain file content");
}

#[tokio::test]
async fn cli_manifest_rejects_traversal() {
    let base = std::env::current_dir().unwrap();
    let paths = vec!["../../etc/passwd".to_string()];
    let result = squall::context::resolve_file_manifest(&paths, &base).await;
    assert!(result.is_err(), "Manifest should also enforce path sandboxing");
}

// ---------------------------------------------------------------------------
// validate_working_directory
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validate_valid_directory() {
    let result = squall::context::validate_working_directory("src").await;
    assert!(result.is_ok());
    let canonical = result.unwrap();
    assert!(canonical.is_absolute());
}

#[tokio::test]
async fn validate_nonexistent_directory() {
    let result = squall::context::validate_working_directory("/nonexistent/path/xyz").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn validate_file_as_directory() {
    let result = squall::context::validate_working_directory("Cargo.toml").await;
    assert!(result.is_err());
    let err = format!("{}", result.unwrap_err());
    assert!(err.contains("not a directory"));
}

// ---------------------------------------------------------------------------
// Symlink traversal: reject symlinks that escape base_dir
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reject_symlink_escaping_base_dir() {
    use std::os::unix::fs::symlink;

    let dir = std::env::temp_dir().join("squall-test-symlink-escape");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Symlink: dir/evil -> /etc/passwd (escapes sandbox)
    symlink("/etc/passwd", dir.join("evil")).unwrap();

    let paths = vec!["evil".to_string()];
    let result = squall::context::resolve_file_context(&paths, &dir, 512_000).await;

    assert!(result.is_err(), "Symlink escaping base_dir must be rejected");
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.contains("escapes") || err.contains("outside") || err.contains("traversal"),
        "Error should mention escape: {err}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn reject_symlink_directory_traversal() {
    use std::os::unix::fs::symlink;

    let dir = std::env::temp_dir().join("squall-test-symdir-escape");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("subdir")).unwrap();

    // Symlink directory: dir/subdir/escape -> /etc
    symlink("/etc", dir.join("subdir").join("escape")).unwrap();

    let paths = vec!["subdir/escape/passwd".to_string()];
    let result = squall::context::resolve_file_context(&paths, &dir, 512_000).await;

    assert!(result.is_err(), "Symlink directory escaping base_dir must be rejected");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn allow_symlink_within_sandbox() {
    use std::os::unix::fs::symlink;

    let dir = std::env::temp_dir().join("squall-test-symlink-ok");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Real file + symlink that stays inside sandbox
    std::fs::write(dir.join("real.txt"), "hello from real file").unwrap();
    symlink(dir.join("real.txt"), dir.join("link.txt")).unwrap();

    let paths = vec!["link.txt".to_string()];
    let result = squall::context::resolve_file_context(&paths, &dir, 512_000).await;

    assert!(result.is_ok(), "Symlink within sandbox should be allowed");
    let ctx = result.unwrap().context.expect("Should have content");
    assert!(ctx.contains("hello from real file"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn cli_manifest_rejects_symlink_escape() {
    use std::os::unix::fs::symlink;

    let dir = std::env::temp_dir().join("squall-test-symlink-manifest");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    symlink("/etc/passwd", dir.join("evil")).unwrap();

    let paths = vec!["evil".to_string()];
    let result = squall::context::resolve_file_manifest(&paths, &dir).await;

    assert!(result.is_err(), "Manifest should reject symlink escapes too");

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Request struct field presence
// ---------------------------------------------------------------------------

#[test]
fn chat_request_has_file_context_fields() {
    use squall::tools::chat::ChatRequest;
    let _req = ChatRequest {
        prompt: "hello".to_string(),
        model: None,
        file_paths: Some(vec!["src/lib.rs".to_string()]),
        working_directory: Some("/tmp".to_string()),
        system_prompt: None,
        temperature: None,
    };
}

#[test]
fn clink_request_has_file_context_fields() {
    use squall::tools::clink::ClinkRequest;
    let _req = ClinkRequest {
        prompt: "hello".to_string(),
        cli_name: "gemini".to_string(),
        file_paths: Some(vec!["src/lib.rs".to_string()]),
        working_directory: Some("/tmp".to_string()),
        system_prompt: None,
        temperature: None,
    };
}

// ---------------------------------------------------------------------------
// ProviderRequest has working_directory
// ---------------------------------------------------------------------------

#[test]
fn provider_request_has_working_directory() {
    use squall::dispatch::ProviderRequest;
    use std::time::{Duration, Instant};
    let _req = ProviderRequest {
        prompt: "test".to_string(),
        model: "test".to_string(),
        deadline: Instant::now() + Duration::from_secs(10),
        working_directory: Some("/tmp".to_string()),
        system_prompt: None,
        temperature: None,
    };
}

// ---------------------------------------------------------------------------
// File format correctness
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_context_uses_xml_tags() {
    let base = std::env::current_dir().unwrap();
    let paths = vec!["Cargo.toml".to_string()];
    let result = squall::context::resolve_file_context(&paths, &base, 512_000).await;
    let ctx = result.unwrap().context.unwrap();
    assert!(ctx.contains("<file path="), "Should use XML file tags");
    assert!(ctx.contains("</file>"), "Should close XML file tags");
}

// ---------------------------------------------------------------------------
// Diff context wrapping
// ---------------------------------------------------------------------------

#[test]
fn wrap_diff_basic() {
    let diff = "--- a/foo.rs\n+++ b/foo.rs\n@@ -1 +1 @@\n-old\n+new\n";
    let result = squall::context::wrap_diff_context(diff, 10_000);
    let wrapped = result.expect("Should wrap non-empty diff");
    assert!(wrapped.starts_with("<diff>"));
    assert!(wrapped.ends_with("</diff>"));
    assert!(wrapped.contains("+new"));
    assert!(!wrapped.contains("truncated"));
}

#[test]
fn wrap_diff_empty_returns_none() {
    assert!(squall::context::wrap_diff_context("", 10_000).is_none());
    assert!(squall::context::wrap_diff_context("   \n  ", 10_000).is_none());
}

#[test]
fn wrap_diff_zero_budget_returns_none() {
    assert!(squall::context::wrap_diff_context("some diff", 0).is_none());
}

#[test]
fn wrap_diff_truncates_at_line_boundary() {
    let diff = "line1\nline2\nline3\nline4\n";
    // Budget of 12 bytes: "line1\nline2\n" is exactly 12
    let result = squall::context::wrap_diff_context(diff, 12).unwrap();
    assert!(result.contains("line1"));
    assert!(result.contains("line2"));
    assert!(!result.contains("line3"), "line3 should be truncated");
    assert!(result.contains("truncated"));
}

#[test]
fn wrap_diff_escapes_xml_content() {
    // A diff editing an XML file could contain </diff> which would break framing
    let diff = "--- a/test.xml\n+++ b/test.xml\n-<old>value</old>\n+</diff>injection\n";
    let result = squall::context::wrap_diff_context(diff, 10_000).unwrap();
    // The literal </diff> in the diff content must be escaped
    assert!(!result.contains("+</diff>injection"), "XML in diff must be escaped");
    assert!(result.contains("&lt;/diff&gt;"), "Should use XML entities");
}

// ---------------------------------------------------------------------------
// Bug #1: wrap_diff_context panics on non-UTF-8 boundary
// ---------------------------------------------------------------------------

#[test]
fn wrap_diff_multibyte_budget_does_not_panic() {
    // Each emoji is 4 bytes. Budget 5 lands inside the 2nd emoji.
    // BUG: &diff[..5] panics because byte 5 is not a char boundary.
    // GREEN: should truncate safely to the last char boundary.
    let diff = "\u{1F600}\u{1F601}\u{1F602}\n"; // 3 emojis + newline = 13 bytes
    let result = squall::context::wrap_diff_context(diff, 5);
    // Should not panic — should return Some with at least the first emoji
    assert!(result.is_some(), "Should handle multi-byte chars without panicking");
}

// ---------------------------------------------------------------------------
// Bug #4: wrap_diff_context post-escape output exceeds budget
// ---------------------------------------------------------------------------

#[test]
fn wrap_diff_escaped_output_respects_budget() {
    // Each '<' (1 byte) becomes '&lt;' (4 bytes) after escaping — 4x expansion.
    // Budget 100 on raw text → 100 '<' chars → 400 bytes escaped content.
    // BUG: budget enforced on pre-escape text, so escaped output blows past it.
    // GREEN: escaped content between tags should not exceed budget.
    let diff = "<".repeat(200) + "\n";
    let result = squall::context::wrap_diff_context(&diff, 100).unwrap();
    // Strip the <diff>\n...\n</diff> wrapper to measure content size
    let content = result
        .strip_prefix("<diff>\n").unwrap_or(&result)
        .strip_suffix("\n</diff>").unwrap_or(&result);
    // Content is escaped text + possible truncation comment.
    // Strip truncation comment to measure just the escaped diff content.
    let escaped_content = if let Some(pos) = content.find("\n<!-- diff truncated") {
        &content[..pos]
    } else {
        content
    };
    assert!(
        escaped_content.len() <= 100,
        "Escaped diff content should respect budget of 100 bytes, got {} bytes: {:?}",
        escaped_content.len(),
        &escaped_content[..escaped_content.len().min(80)]
    );
}
