//! Tests for the clink recursion guard. The guard prevents clinking to the same
//! CLI that's currently hosting Squall (which would recurse infinitely).
//!
//! These are pure function tests — no subprocess spawning, no env-var mutation.

use squall::config::ClientMode;
use squall::dispatch::registry::recursion_guard;

fn no_args() -> Vec<String> {
    Vec::new()
}

#[test]
fn codex_mode_blocks_codex_target() {
    let result = recursion_guard(ClientMode::Codex, "codex", &no_args());
    assert!(result.is_some(), "codex hosting Codex should be blocked");
    let msg = result.unwrap();
    assert!(msg.contains("recurse"), "message should explain why: {msg}");
    assert!(
        msg.contains("SQUALL_CLIENT"),
        "message should mention env var: {msg}"
    );
}

#[test]
fn claude_mode_blocks_claude_target() {
    let result = recursion_guard(ClientMode::Claude, "claude", &no_args());
    assert!(result.is_some(), "Claude hosting Claude should be blocked");
    let msg = result.unwrap();
    assert!(msg.contains("recurse"), "message should explain why: {msg}");
}

#[test]
fn cross_client_clinks_allowed() {
    // The whole point of the symmetry: Codex users clink to claude, Claude users clink to codex.
    assert!(recursion_guard(ClientMode::Codex, "claude", &no_args()).is_none());
    assert!(recursion_guard(ClientMode::Claude, "codex", &no_args()).is_none());
}

#[test]
fn gemini_target_always_allowed() {
    // Gemini is neutral — not the host of either client mode.
    assert!(recursion_guard(ClientMode::Claude, "gemini", &no_args()).is_none());
    assert!(recursion_guard(ClientMode::Codex, "gemini", &no_args()).is_none());
}

#[test]
fn full_path_basename_matched() {
    // User might have claude/codex installed at custom paths.
    assert!(recursion_guard(ClientMode::Codex, "/usr/local/bin/codex", &no_args()).is_some());
    assert!(recursion_guard(ClientMode::Claude, "/opt/anthropic/claude", &no_args()).is_some());
}

#[test]
fn case_insensitive_basename() {
    // Defensive: handle uppercase variants without false negatives.
    assert!(recursion_guard(ClientMode::Codex, "CODEX", &no_args()).is_some());
    assert!(recursion_guard(ClientMode::Claude, "Claude", &no_args()).is_some());
}

#[test]
fn unrelated_executable_not_blocked() {
    assert!(recursion_guard(ClientMode::Codex, "python", &no_args()).is_none());
    assert!(recursion_guard(ClientMode::Claude, "node", &no_args()).is_none());
}

// -----------------------------------------------------------------------------
// Wrapper-script bypass coverage — gemini & kimi flagged this as defense-in-depth gap
// -----------------------------------------------------------------------------

#[test]
fn sh_wrapper_with_codex_blocked_in_codex_mode() {
    // executable="sh" and args=["-c", "codex ..."] hides codex behind shell — must catch.
    let args = vec!["-c".to_string(), "codex --model gpt-5.5".to_string()];
    let result = recursion_guard(ClientMode::Codex, "sh", &args);
    assert!(result.is_some(), "sh -c 'codex ...' should be blocked");
}

#[test]
fn sh_wrapper_with_claude_blocked_in_claude_mode() {
    let args = vec!["-c".to_string(), "claude --print --model opus".to_string()];
    let result = recursion_guard(ClientMode::Claude, "sh", &args);
    assert!(result.is_some(), "sh -c 'claude ...' should be blocked");
}

#[test]
fn npx_wrapper_blocked() {
    // Common Node-ecosystem wrapper. npx may resolve to claude-code package.
    let args = vec!["claude".to_string()];
    let result = recursion_guard(ClientMode::Claude, "npx", &args);
    assert!(result.is_some(), "npx claude should be blocked");

    let args = vec!["codex".to_string()];
    let result = recursion_guard(ClientMode::Codex, "npx", &args);
    assert!(result.is_some(), "npx codex should be blocked");
}

#[test]
fn env_wrapper_blocked() {
    let args = vec!["codex".to_string()];
    let result = recursion_guard(ClientMode::Codex, "/usr/bin/env", &args);
    assert!(result.is_some(), "env codex should be blocked");
}

#[test]
fn wrapper_with_full_path_in_args_blocked() {
    let args = vec!["-c".to_string(), "/usr/local/bin/codex --json".to_string()];
    let result = recursion_guard(ClientMode::Codex, "sh", &args);
    assert!(result.is_some(), "sh -c '/path/to/codex' should be blocked");
}

#[test]
fn wrapper_with_unrelated_args_allowed() {
    // python -c "print('hi')" is fine in any mode.
    let args = vec!["-c".to_string(), "print('hi')".to_string()];
    assert!(recursion_guard(ClientMode::Codex, "python", &args).is_none());
    assert!(recursion_guard(ClientMode::Claude, "python", &args).is_none());

    // Cross-mode wrappers stay allowed: codex user invoking claude via shell.
    let args = vec!["-c".to_string(), "claude --print".to_string()];
    assert!(recursion_guard(ClientMode::Codex, "sh", &args).is_none());
}

#[test]
fn refusal_message_surfaces_full_command() {
    // Diagnostic value: the user should see what command was rejected, not just "codex".
    let args = vec!["-c".to_string(), "codex --json".to_string()];
    let msg = recursion_guard(ClientMode::Codex, "sh", &args).unwrap();
    assert!(
        msg.contains("sh") && msg.contains("codex"),
        "refusal should mention both the executable and offending arg. Got: {msg}"
    );
}
