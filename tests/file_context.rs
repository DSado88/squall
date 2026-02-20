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
    let ctx = result.unwrap().expect("Should have content");
    assert!(ctx.contains("Cargo.toml"));
    assert!(ctx.contains("[package]"));
}

#[tokio::test]
async fn valid_nested_relative_path() {
    let base = std::env::current_dir().unwrap();
    let paths = vec!["src/lib.rs".to_string()];
    let result = squall::context::resolve_file_context(&paths, &base, 512_000).await;
    assert!(result.is_ok());
    let ctx = result.unwrap().expect("Should have content");
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
    let ctx = result.unwrap();
    // Should have some output (manifest/notes) even if file didn't fit
    if let Some(text) = ctx {
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
    let ctx = result.unwrap().expect("Should have content");
    assert!(ctx.contains("src/lib.rs"), "First file should be included");
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
    let ctx = result.unwrap().expect("Should have content from Cargo.toml");
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
    assert!(result.unwrap().is_none());
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
    let ctx = result.unwrap().expect("Should have content");
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
    let ctx = result.unwrap().unwrap();
    assert!(ctx.contains("<file path="), "Should use XML file tags");
    assert!(ctx.contains("</file>"), "Should close XML file tags");
}
