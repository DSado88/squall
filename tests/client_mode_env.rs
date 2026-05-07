//! Integration test for `SQUALL_CLIENT` env-var resolution.
//!
//! Lives here (not as a unit test in src/config.rs) because it mutates a process-global
//! environment variable. Cargo runs unit tests within a single test binary in parallel
//! threads, so writes from this test would race with the ~20 other unit tests that
//! transitively read `SQUALL_CLIENT` via `Config::from_toml(...) → resolve() → from_env()`.
//!
//! Each integration test file is compiled into its own test binary (its own process),
//! so this test cannot race with anything else. All the assertions live in a single
//! `#[test]` to enforce sequential execution within this file.

use squall::config::ClientMode;

#[test]
fn from_env_resolves_all_values_sequentially() {
    let key = "SQUALL_CLIENT";
    let prior = std::env::var(key).ok();

    unsafe {
        std::env::remove_var(key);
    }
    assert_eq!(
        ClientMode::from_env(),
        ClientMode::Claude,
        "unset SQUALL_CLIENT should default to Claude"
    );

    unsafe {
        std::env::set_var(key, "claude");
    }
    assert_eq!(
        ClientMode::from_env(),
        ClientMode::Claude,
        "explicit 'claude' should resolve to Claude"
    );

    unsafe {
        std::env::set_var(key, "garbage");
    }
    assert_eq!(
        ClientMode::from_env(),
        ClientMode::Claude,
        "unknown values should default to Claude"
    );

    unsafe {
        std::env::set_var(key, "");
    }
    assert_eq!(
        ClientMode::from_env(),
        ClientMode::Claude,
        "empty string should default to Claude"
    );

    unsafe {
        std::env::set_var(key, "codex");
    }
    assert_eq!(ClientMode::from_env(), ClientMode::Codex);

    unsafe {
        std::env::set_var(key, "CODEX");
    }
    assert_eq!(
        ClientMode::from_env(),
        ClientMode::Claude,
        "case-sensitive: only lowercase 'codex' triggers Codex mode"
    );

    // Restore prior value
    unsafe {
        match prior {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}
