//! TDD tests for raw CLI output persistence (Issue #10).
//!
//! Tests cover:
//! - Config parsing (enum variants, defaults, case sensitivity)
//! - persist_cli_output() helper (JSON format, filenames, directory creation)
//! - Integration: query_model() triggers persistence on ALL failure paths
//!   (timeout, overflow, spawn error, process exit, parse error)

use squall::config::{Config, PersistRawOutput};
use squall::dispatch::async_poll::sanitize_model_name;

// ---------------------------------------------------------------------------
// 1. Config parsing: persist_raw_output setting parsed from TOML correctly
// ---------------------------------------------------------------------------

#[test]
fn persist_raw_output_enum_default_is_on_failure() {
    assert_eq!(PersistRawOutput::default(), PersistRawOutput::OnFailure);
}

#[test]
fn persist_raw_output_enum_variants_exist() {
    // Verify all three variants exist and are distinct
    let always = PersistRawOutput::Always;
    let on_failure = PersistRawOutput::OnFailure;
    let never = PersistRawOutput::Never;

    assert_ne!(always, on_failure);
    assert_ne!(always, never);
    assert_ne!(on_failure, never);
}

// ---------------------------------------------------------------------------
// 2. Default value: Config default persistence mode is OnFailure
// ---------------------------------------------------------------------------

#[test]
fn config_default_persist_raw_output_is_on_failure() {
    let config = Config::default();
    assert_eq!(
        config.persist_raw_output,
        PersistRawOutput::OnFailure,
        "Default Config.persist_raw_output should be OnFailure"
    );
}

// ---------------------------------------------------------------------------
// 3. File format: persisted JSON contains expected fields
//    (stdout, stderr, exit_code, timing_ms, model, provider, parse_status)
// ---------------------------------------------------------------------------

#[test]
fn persist_json_contains_required_fields() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let dir = std::path::PathBuf::from("/tmp/squall-test-persist-fields");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let result = squall::dispatch::cli::persist_cli_output(
            &dir,
            "test-model",
            "gemini",
            b"stdout content here",
            b"stderr content here",
            0,         // exit_code
            1234,      // timing_ms
            "success", // parse_status
        )
        .await;

        assert!(
            result.is_ok(),
            "persist_cli_output should succeed: {:?}",
            result.err()
        );
        let path = result.unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&contents).unwrap();

        // All required fields must exist
        assert!(json.get("stdout").is_some(), "JSON missing 'stdout'");
        assert!(json.get("stderr").is_some(), "JSON missing 'stderr'");
        assert!(json.get("exit_code").is_some(), "JSON missing 'exit_code'");
        assert!(json.get("timing_ms").is_some(), "JSON missing 'timing_ms'");
        assert!(json.get("model").is_some(), "JSON missing 'model'");
        assert!(json.get("provider").is_some(), "JSON missing 'provider'");
        assert!(
            json.get("parse_status").is_some(),
            "JSON missing 'parse_status'"
        );

        // Verify field values
        assert_eq!(json["stdout"], "stdout content here");
        assert_eq!(json["stderr"], "stderr content here");
        assert_eq!(json["exit_code"], 0);
        assert_eq!(json["timing_ms"], 1234);
        assert_eq!(json["model"], "test-model");
        assert_eq!(json["provider"], "gemini");
        assert_eq!(json["parse_status"], "success");

        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn persist_json_handles_non_utf8_stdout() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let dir = std::path::PathBuf::from("/tmp/squall-test-persist-binary");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let binary_stdout = b"valid start \xff\xfe invalid bytes";

        let result = squall::dispatch::cli::persist_cli_output(
            &dir,
            "test-model",
            "codex",
            binary_stdout,
            b"",
            0,
            500,
            "success",
        )
        .await;

        assert!(result.is_ok(), "persist should handle non-UTF8 stdout");

        let path = result.unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert!(
            json["stdout"].is_string(),
            "stdout should be lossy-converted to string"
        );

        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn persist_json_records_nonzero_exit_code() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let dir = std::path::PathBuf::from("/tmp/squall-test-persist-exitcode");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let result = squall::dispatch::cli::persist_cli_output(
            &dir,
            "test-model",
            "gemini",
            b"",
            b"error: something went wrong",
            1, // non-zero exit code
            2000,
            "parse_error: invalid JSON",
        )
        .await;

        assert!(result.is_ok());
        let path = result.unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&contents).unwrap();

        assert_eq!(json["exit_code"], 1);
        assert_eq!(json["stderr"], "error: something went wrong");
        assert_eq!(json["parse_status"], "parse_error: invalid JSON");

        let _ = std::fs::remove_dir_all(&dir);
    });
}

// ---------------------------------------------------------------------------
// 4. Directory creation: .squall/raw/ created automatically if missing
// ---------------------------------------------------------------------------

#[test]
fn persist_creates_raw_directory_if_missing() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let dir = std::path::PathBuf::from("/tmp/squall-test-persist-mkdir");
        let raw_dir = dir.join(".squall/raw");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!raw_dir.exists(), "raw dir should not exist before test");

        let result = squall::dispatch::cli::persist_cli_output(
            &dir,
            "test-model",
            "gemini",
            b"output",
            b"",
            0,
            100,
            "success",
        )
        .await;

        assert!(
            result.is_ok(),
            "persist should create directory: {:?}",
            result.err()
        );
        assert!(
            raw_dir.exists(),
            ".squall/raw/ should be created automatically"
        );

        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn persist_succeeds_when_raw_directory_already_exists() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let dir = std::path::PathBuf::from("/tmp/squall-test-persist-existing-dir");
        let raw_dir = dir.join(".squall/raw");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&raw_dir).unwrap();

        let result = squall::dispatch::cli::persist_cli_output(
            &dir,
            "test-model",
            "gemini",
            b"output",
            b"",
            0,
            100,
            "success",
        )
        .await;

        assert!(
            result.is_ok(),
            "persist should work when dir already exists"
        );

        let _ = std::fs::remove_dir_all(&dir);
    });
}

// ---------------------------------------------------------------------------
// 5. Filename format: matches naming convention
//    Pattern: {timestamp}_{pid}_{seq}_{model}.json in .squall/raw/
// ---------------------------------------------------------------------------

#[test]
fn persist_filename_matches_convention() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let dir = std::path::PathBuf::from("/tmp/squall-test-persist-filename");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let result = squall::dispatch::cli::persist_cli_output(
            &dir,
            "gemini-3-pro-preview",
            "gemini",
            b"output",
            b"",
            0,
            100,
            "success",
        )
        .await;

        assert!(result.is_ok());
        let path = result.unwrap();
        let path_str = path.display().to_string();

        // Should end with .json
        assert!(
            path_str.ends_with(".json"),
            "File should end with .json: {path_str}"
        );

        // Should be inside .squall/raw/
        assert!(
            path_str.contains(".squall/raw/"),
            "File should be in .squall/raw/: {path_str}"
        );

        // Extract filename and verify pattern
        let filename = std::path::Path::new(&path_str)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();

        // Model name should be sanitized
        let safe_model = sanitize_model_name("gemini-3-pro-preview");
        assert!(
            filename.ends_with(&format!("{safe_model}.json")),
            "Filename should end with sanitized model name: {filename}"
        );

        // Verify underscore-separated numeric parts: {ts}_{pid}_{seq}_{model}.json
        let parts: Vec<&str> = filename.splitn(4, '_').collect();
        assert!(
            parts.len() >= 3,
            "Filename should have at least 3 underscore-separated parts: {filename}"
        );
        // First part: timestamp (numeric)
        assert!(
            parts[0].chars().all(|c| c.is_ascii_digit()),
            "First part should be numeric timestamp: {}",
            parts[0]
        );
        // Second part: PID (numeric)
        assert!(
            parts[1].chars().all(|c| c.is_ascii_digit()),
            "Second part should be numeric PID: {}",
            parts[1]
        );

        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn persist_filename_sanitizes_model_with_slashes() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let dir = std::path::PathBuf::from("/tmp/squall-test-persist-sanitize");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let result = squall::dispatch::cli::persist_cli_output(
            &dir,
            "moonshotai/Kimi-K2.5",
            "together",
            b"output",
            b"",
            0,
            100,
            "success",
        )
        .await;

        assert!(result.is_ok());
        let path = result.unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap();

        // Filename should NOT contain slashes (except in directory path)
        let name_without_ext = filename.strip_suffix(".json").unwrap();
        assert!(
            !name_without_ext.contains('/'),
            "Filename should not contain slashes: {filename}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn persist_produces_unique_filenames() {
    // Two calls should produce different filenames (seq counter increments)
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let dir = std::path::PathBuf::from("/tmp/squall-test-persist-unique");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let path1 = squall::dispatch::cli::persist_cli_output(
            &dir, "model-a", "gemini", b"out1", b"", 0, 100, "success",
        )
        .await
        .unwrap();

        let path2 = squall::dispatch::cli::persist_cli_output(
            &dir, "model-a", "gemini", b"out2", b"", 0, 200, "success",
        )
        .await
        .unwrap();

        assert_ne!(
            path1, path2,
            "Two persist calls should produce unique filenames"
        );

        let _ = std::fs::remove_dir_all(&dir);
    });
}

// ---------------------------------------------------------------------------
// 6. Config field accessible on public Config struct
// ---------------------------------------------------------------------------

#[test]
fn config_persist_raw_output_field_is_public() {
    // Verify the field is accessible on the Config struct (compile-time check)
    let config = Config::default();
    let _mode: PersistRawOutput = config.persist_raw_output;
    // If this compiles, the field is public and has the right type
}

// ---------------------------------------------------------------------------
// 7. Filename safety: long model names truncated to fit OS limits
// ---------------------------------------------------------------------------

#[test]
fn persist_truncates_long_model_name_in_filename() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let dir = std::path::PathBuf::from("/tmp/squall-test-persist-longname");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 300-char model name would exceed typical 255-byte filename limit
        let long_model = "a".repeat(300);

        let result = squall::dispatch::cli::persist_cli_output(
            &dir,
            &long_model,
            "test",
            b"output",
            b"",
            0,
            100,
            "success",
        )
        .await;

        assert!(
            result.is_ok(),
            "should handle long model names: {:?}",
            result.err()
        );
        let path = result.unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(
            filename.len() <= 255,
            "Filename should be <= 255 bytes, got {} bytes: {filename}",
            filename.len()
        );

        // Verify the JSON still contains the FULL (untruncated) model name
        let contents = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(json["model"], *long_model);

        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn persist_truncates_unicode_model_name_safely() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let dir = std::path::PathBuf::from("/tmp/squall-test-persist-unicode-trunc");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Model name with multi-byte chars that exceeds filename limit.
        // Each '中' is 3 bytes — a naive byte truncation would split one.
        let unicode_model = "中".repeat(200); // 600 bytes

        let result = squall::dispatch::cli::persist_cli_output(
            &dir,
            &unicode_model,
            "test",
            b"output",
            b"",
            0,
            100,
            "success",
        )
        .await;

        assert!(
            result.is_ok(),
            "should handle Unicode model names without panic: {:?}",
            result.err()
        );
        let path = result.unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(
            filename.len() <= 255,
            "Filename should be <= 255 bytes, got {}: {filename}",
            filename.len()
        );

        let _ = std::fs::remove_dir_all(&dir);
    });
}

// ---------------------------------------------------------------------------
// 8. Integration: query_model() triggers persistence on ALL failure paths
//    These tests exercise the full dispatch flow, verifying that the
//    fire-and-forget spawn_persist() creates files for each failure mode.
// ---------------------------------------------------------------------------

/// Helper: count JSON files in .squall/raw/ under a base directory.
fn count_raw_files(base_dir: &std::path::Path) -> usize {
    let raw_dir = base_dir.join(".squall/raw");
    if !raw_dir.exists() {
        return 0;
    }
    std::fs::read_dir(&raw_dir)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .map(|e| e.path().extension().is_some_and(|ext| ext == "json"))
                .unwrap_or(false)
        })
        .count()
}

/// Helper: read the first (or only) JSON file in .squall/raw/.
fn read_first_raw_file(base_dir: &std::path::Path) -> serde_json::Value {
    let raw_dir = base_dir.join(".squall/raw");
    let entry = std::fs::read_dir(&raw_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .expect("no JSON files found in .squall/raw/");
    let contents = std::fs::read_to_string(entry.path()).unwrap();
    serde_json::from_str(&contents).unwrap()
}

/// Helper: build a ProviderRequest pointing at a specific temp directory.
fn make_request(
    dir: &std::path::Path,
    deadline: std::time::Duration,
) -> squall::dispatch::ProviderRequest {
    squall::dispatch::ProviderRequest {
        prompt: "".into(),
        model: "test-model".to_string(),
        deadline: std::time::Instant::now() + deadline,
        working_directory: Some(dir.display().to_string()),
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        cancellation_token: None,
        stall_timeout: None,
    }
}

#[tokio::test]
async fn persist_fires_on_overflow_when_on_failure() {
    use squall::dispatch::cli::{CliDispatch, MAX_OUTPUT_BYTES};
    use squall::parsers::gemini::GeminiParser;

    let dir = std::path::PathBuf::from("/tmp/squall-test-trigger-overflow-onfail");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let dispatch = CliDispatch::new();
    let req = make_request(&dir, std::time::Duration::from_secs(30));

    let result = dispatch
        .query_model(
            &req,
            "test",
            "bash",
            &[
                "-c".to_string(),
                format!("yes | head -c {}", MAX_OUTPUT_BYTES + 1),
            ],
            &GeminiParser,
            PersistRawOutput::OnFailure,
        )
        .await;

    assert!(result.is_err(), "overflow should error");

    // Give fire-and-forget task time to complete
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert!(
        count_raw_files(&dir) > 0,
        "OnFailure should persist on overflow (a failure path)"
    );
    let json = read_first_raw_file(&dir);
    assert_eq!(json["parse_status"], "output_overflow");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn persist_fires_on_timeout_when_always() {
    use squall::dispatch::cli::CliDispatch;
    use squall::parsers::gemini::GeminiParser;

    let dir = std::path::PathBuf::from("/tmp/squall-test-trigger-timeout-always");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let dispatch = CliDispatch::new();
    let req = make_request(&dir, std::time::Duration::from_millis(500));

    let result = dispatch
        .query_model(
            &req,
            "test",
            "sleep",
            &["3600".to_string()],
            &GeminiParser,
            PersistRawOutput::Always,
        )
        .await;

    assert!(result.is_err(), "should timeout");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert!(
        count_raw_files(&dir) > 0,
        "Always should persist even on timeout"
    );
    let json = read_first_raw_file(&dir);
    assert_eq!(json["parse_status"], "timeout");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn persist_fires_on_spawn_error_when_on_failure() {
    use squall::dispatch::cli::CliDispatch;
    use squall::parsers::gemini::GeminiParser;

    let dir = std::path::PathBuf::from("/tmp/squall-test-trigger-spawn-err");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let dispatch = CliDispatch::new();
    let req = make_request(&dir, std::time::Duration::from_secs(10));

    let result = dispatch
        .query_model(
            &req,
            "test",
            "/nonexistent/binary/squall_test_xxx",
            &[],
            &GeminiParser,
            PersistRawOutput::OnFailure,
        )
        .await;

    assert!(result.is_err(), "spawn should fail");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert!(
        count_raw_files(&dir) > 0,
        "OnFailure should persist on spawn error"
    );
    let json = read_first_raw_file(&dir);
    let status = json["parse_status"].as_str().unwrap();
    assert!(
        status.starts_with("spawn_error:"),
        "parse_status should start with 'spawn_error:', got '{status}'"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn persist_skips_on_all_failures_when_never() {
    use squall::dispatch::cli::CliDispatch;
    use squall::parsers::gemini::GeminiParser;

    let dir = std::path::PathBuf::from("/tmp/squall-test-trigger-never");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let dispatch = CliDispatch::new();

    // Trigger timeout with Never mode
    let req = make_request(&dir, std::time::Duration::from_millis(500));
    let _ = dispatch
        .query_model(
            &req,
            "test",
            "sleep",
            &["3600".to_string()],
            &GeminiParser,
            PersistRawOutput::Never,
        )
        .await;

    // Trigger spawn error with Never mode
    let req = make_request(&dir, std::time::Duration::from_secs(10));
    let _ = dispatch
        .query_model(
            &req,
            "test",
            "/nonexistent/binary/squall_test_xxx",
            &[],
            &GeminiParser,
            PersistRawOutput::Never,
        )
        .await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert_eq!(
        count_raw_files(&dir),
        0,
        "Never should not persist anything, regardless of failure type"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn persist_uses_working_directory_not_cwd() {
    use squall::dispatch::cli::CliDispatch;
    use squall::parsers::gemini::GeminiParser;

    let dir = std::path::PathBuf::from("/tmp/squall-test-persist-workdir");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let dispatch = CliDispatch::new();
    let req = make_request(&dir, std::time::Duration::from_secs(10));

    // Spawn error triggers persistence — file should land in working_directory
    let _ = dispatch
        .query_model(
            &req,
            "test",
            "/nonexistent/binary/squall_test_xxx",
            &[],
            &GeminiParser,
            PersistRawOutput::Always,
        )
        .await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let raw_dir = dir.join(".squall/raw");
    assert!(
        raw_dir.exists(),
        "raw files should be written to working_directory/.squall/raw/, not CWD"
    );
    assert!(
        count_raw_files(&dir) > 0,
        "should have persisted a file in the working_directory"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
