//! Phase 4 defect tests (TDD RED phase).
//! Each test proves a specific defect found by the deep codebase review.
//! Tests should FAIL before fixes and PASS after.
//!
//! Defect numbering: find4_{N}_{short_name}

use squall::tools::review::ReviewRequest;

// ---------------------------------------------------------------------------
// Defect 3: Anthropic parser drops thinking_delta and ignores error events
// (http.rs:537-558)
//
// parse_anthropic_event only handles text_delta. thinking_delta blocks are
// silently dropped, and "error" events are skipped instead of terminating.
// ---------------------------------------------------------------------------

#[test]
fn find4_3a_anthropic_parser_handles_thinking_delta() {
    use squall::dispatch::http::parse_anthropic_event_pub;

    let event = r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"Let me analyze..."}}"#;
    let result = parse_anthropic_event_pub(event);
    assert!(
        result.is_text(),
        "thinking_delta should produce Text, got: {result:?}"
    );
    assert_eq!(
        result.text().unwrap(),
        "Let me analyze...",
        "thinking_delta text should be extracted"
    );
}

#[test]
fn find4_3b_anthropic_parser_handles_error_event() {
    use squall::dispatch::http::parse_anthropic_event_pub;

    let event = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
    let result = parse_anthropic_event_pub(event);
    assert!(
        result.is_error(),
        "error event should produce Error, got: {result:?}"
    );
}

#[test]
fn find4_3c_anthropic_parser_text_delta_still_works() {
    use squall::dispatch::http::parse_anthropic_event_pub;

    let event = r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello world"}}"#;
    let result = parse_anthropic_event_pub(event);
    assert!(result.is_text());
    assert_eq!(result.text().unwrap(), "Hello world");
}

// ---------------------------------------------------------------------------
// Defect 5: content_hash uses DefaultHasher — NOT stable across Rust versions.
// (memory.rs:1035-1048)
//
// We can't test cross-version instability in a single build, but we CAN verify
// the hash is deterministic within a single run (baseline) and test that the
// function exists with a stable API we can swap out.
// ---------------------------------------------------------------------------

#[test]
fn find4_5_content_hash_is_deterministic_within_session() {
    use squall::memory::content_hash_pub;

    let h1 = content_hash_pub("Race condition in auth", Some("codebase"));
    let h2 = content_hash_pub("Race condition in auth", Some("codebase"));
    assert_eq!(h1, h2, "same input must produce same hash within session");

    let h3 = content_hash_pub("Race condition in auth", Some("branch:main"));
    assert_ne!(h1, h3, "different scope must produce different hash");
}

// ---------------------------------------------------------------------------
// Defect 6: read_body_capped allocates entire chunk before checking cap
// (async_poll.rs:550-554)
//
// extend_from_slice allocates the entire chunk, then checks if cap exceeded.
// A 500MB chunk causes 500MB allocation. Should slice before extending.
// ---------------------------------------------------------------------------

// Defect 6 (read_body_capped OOM) is tested via inline unit test in async_poll.rs.
// Can't easily construct reqwest::Response in integration tests without the http crate.

// ---------------------------------------------------------------------------
// Defect 7: Recommendation scoring ignores sample count
// (memory.rs:895)
//
// A model with 1/1 success (100%) outranks a model with 95/100 (95%).
// score = confidence * success_rate, but count is unused.
// ---------------------------------------------------------------------------

#[test]
fn find4_7_recommendation_scoring_uses_sample_count() {
    use squall::memory::generate_recommendations_pub;

    // Model A: 1 success out of 1 attempt (100% success, tiny sample)
    // Model B: 95 successes out of 100 attempts (95% success, large sample)
    // Both have same recency (today).
    //
    // BUG: Model A ranks higher because 1.0 * confidence > 0.95 * confidence
    // FIX: Bayesian smoothing or sample-count weighting should prefer Model B.

    let today = squall::memory::iso_date_pub();

    let mut events = String::new();
    // Model A: 1 success
    events.push_str(&format!(
        "| {today}T10:00:00Z | model-a | 10.0s | success | no | — | 1000 |\n"
    ));
    // Model B: 95 successes, 5 failures (100 events)
    for i in 0..95 {
        events.push_str(&format!(
            "| {today}T10:{:02}:00Z | model-b | 20.0s | success | no | — | 2000 |\n",
            i % 60
        ));
    }
    for i in 0..5 {
        events.push_str(&format!(
            "| {today}T11:{:02}:00Z | model-b | 20.0s | error | no | timeout | 2000 |\n",
            i
        ));
    }

    let models_content = format!(
        "# Model Performance\n\n\
         ## Summary\n(generated)\n\n\
         ## Recent Events\n\
         | Timestamp | Model | Latency | Status | Partial | Reason | Tokens |\n\
         |-----------|-------|---------|--------|---------|--------|--------|\n\
         {events}"
    );

    let recommendations = generate_recommendations_pub(&models_content);

    // After fix: model-b should appear before model-a in the ranking TABLE
    // because it has overwhelmingly more evidence.
    // Note: "Quick triage" picks the fastest model with >80% success (model-a),
    // which is correct behavior — triage is about speed, not sample size.
    // The RANKING table is what should reflect Bayesian smoothing.
    let table_start = recommendations.find("| Model |").expect("ranking table should exist");
    let table_section = &recommendations[table_start..];
    let pos_a = table_section.find("model-a");
    let pos_b = table_section.find("model-b");

    assert!(
        pos_a.is_some() && pos_b.is_some(),
        "both models should appear in ranking table: {table_section}"
    );
    assert!(
        pos_b.unwrap() < pos_a.unwrap(),
        "model-b (95/100) should rank higher than model-a (1/1) in ranking table.\n\
         Table:\n{table_section}"
    );
}

// ---------------------------------------------------------------------------
// Defect 8: Gemini error parsing masks internal error messages
// (async_poll.rs:228-232)
//
// v["error"].as_str() returns None when error is a JSON object, falling back
// to bare "failed" status string. Should check v["error"]["message"].
// ---------------------------------------------------------------------------

#[test]
fn find4_8_gemini_error_parsing_extracts_nested_message() {
    use squall::dispatch::async_poll::GeminiInteractionsApi;
    use squall::dispatch::async_poll::AsyncPollApi;

    let api = GeminiInteractionsApi;

    // Gemini returns error as object: {"error": {"code": 400, "message": "Context too long"}}
    let body = serde_json::json!({
        "status": "failed",
        "error": {
            "code": 400,
            "message": "Context length exceeds maximum"
        }
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();

    let result = api.parse_poll_response(&body_bytes).unwrap();
    match result {
        squall::dispatch::async_poll::PollStatus::Failed(msg) => {
            assert!(
                msg.contains("Context length"),
                "error message should contain nested message, got: {msg}"
            );
            assert!(
                msg != "failed",
                "should not fall back to bare 'failed' status"
            );
        }
        other => panic!("expected Failed, got: {other:?}"),
    }
}

#[test]
fn find4_8b_gemini_error_string_still_works() {
    use squall::dispatch::async_poll::GeminiInteractionsApi;
    use squall::dispatch::async_poll::AsyncPollApi;

    let api = GeminiInteractionsApi;

    // Some APIs return error as a plain string
    let body = serde_json::json!({
        "status": "failed",
        "error": "simple error message"
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();

    let result = api.parse_poll_response(&body_bytes).unwrap();
    match result {
        squall::dispatch::async_poll::PollStatus::Failed(msg) => {
            assert_eq!(msg, "simple error message");
        }
        other => panic!("expected Failed, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Defect 10: deep timeout override conflicts with schema docs
// (tools/review.rs:73)
//
// Doc says "Individual fields override deep defaults" but code uses .max()
// which forces minimum 600s. If user passes timeout_secs: 180 with deep: true,
// they get 600s, not 180s.
// ---------------------------------------------------------------------------

#[test]
fn find4_10_deep_mode_respects_explicit_timeout() {
    let req = ReviewRequest {
        prompt: "test".into(),
        models: None,
        timeout_secs: Some(180),
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: None,
        deep: Some(true),
        context_format: None,
        investigation_context: None,
    };

    // Doc says: "Individual fields (timeout_secs, reasoning_effort, max_tokens)
    // override deep defaults."
    // BUG: .max(600) forces 600 even when user explicitly says 180.
    // FIX: explicit timeout_secs should override deep default.
    assert_eq!(
        req.effective_timeout_secs(),
        180,
        "explicit timeout_secs should override deep default, not be clamped to 600"
    );
}

#[test]
fn find4_10b_deep_mode_defaults_to_600_when_not_set() {
    let req = ReviewRequest {
        prompt: "test".into(),
        models: None,
        timeout_secs: None, // not set
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        file_paths: None,
        working_directory: None,
        diff: None,
        per_model_system_prompts: None,
        per_model_timeout_secs: None,
        deep: Some(true),
        context_format: None,
        investigation_context: None,
    };

    // When timeout_secs is NOT set, deep mode should default to 600.
    assert_eq!(
        req.effective_timeout_secs(),
        600,
        "deep mode without explicit timeout should default to 600"
    );
}

// ---------------------------------------------------------------------------
// Defect 11: extract_evidence_count parses [xNaN] as 1
// (memory.rs:1063-1071)
//
// If heading contains [xNaN] or [xfoo], parse::<usize>() fails → returns 1.
// This can cause incorrect archival decisions in flush_branch.
// ---------------------------------------------------------------------------

#[test]
fn find4_11_extract_evidence_count_rejects_nan() {
    use squall::memory::extract_evidence_count_pub;

    // Valid evidence
    assert_eq!(extract_evidence_count_pub("## [2026-02-23] Bug [x5]"), 5);
    assert_eq!(extract_evidence_count_pub("## [2026-02-23] Bug [x1]"), 1);

    // Malformed: [xNaN] should NOT silently become 1 — it should return 0
    // or keep 1 but at minimum the function shouldn't confuse NaN with "first occurrence"
    // For now: the fix should make this return 0 to indicate unparseable.
    let nan_result = extract_evidence_count_pub("## [2026-02-23] Bug [xNaN]");
    assert_ne!(
        nan_result, 1,
        "[xNaN] should not be treated as evidence count 1 (first occurrence)"
    );

    // No evidence marker at all → genuinely 1
    assert_eq!(extract_evidence_count_pub("## [2026-02-23] Bug"), 1);
}
