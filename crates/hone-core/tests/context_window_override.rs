//! Contract tests for the `HONE_CONTEXT_WINDOW` override.
//!
//! The env seam (`for_model_with_env`) is exercised with closures so these
//! tests never touch the real process environment. End-to-end tests that need
//! the real variable live in `context_window_override_env.rs`.

use std::cell::RefCell;

use hone_core::{
    CONTEXT_WINDOW_ENV, ContextConfig, MAX_CONTEXT_WINDOW, MIN_CONTEXT_WINDOW, parse_context_window,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

#[test]
fn test_constants_when_read_should_match_contract() {
    assert_eq!(CONTEXT_WINDOW_ENV, "HONE_CONTEXT_WINDOW");
    assert_eq!(MIN_CONTEXT_WINDOW, 4_096);
    assert_eq!(MAX_CONTEXT_WINDOW, 10_000_000);
}

// ---------------------------------------------------------------------------
// parse_context_window
// ---------------------------------------------------------------------------

#[test]
fn test_parse_when_valid_integer_should_return_some() {
    assert_eq!(parse_context_window("32768"), Some(32_768));
    assert_eq!(parse_context_window("200000"), Some(200_000));
}

#[test]
fn test_parse_when_surrounding_whitespace_should_trim() {
    assert_eq!(parse_context_window("  32768 "), Some(32_768));
    assert_eq!(parse_context_window("\t65536\n"), Some(65_536));
}

#[test]
fn test_parse_when_at_inclusive_bounds_should_accept() {
    assert_eq!(parse_context_window("4096"), Some(4_096));
    assert_eq!(parse_context_window("10000000"), Some(10_000_000));
}

#[test]
fn test_parse_when_just_outside_bounds_should_reject() {
    assert_eq!(parse_context_window("4095"), None);
    assert_eq!(parse_context_window("10000001"), None);
    assert_eq!(parse_context_window("0"), None);
}

#[test]
fn test_parse_when_malformed_should_return_none() {
    for raw in [
        "",
        "   ",
        "-32768",
        "+32768",
        "32k",
        "32K",
        "1e5",
        "0x8000",
        "32768.0",
        "32768.5",
        "32_768",
        "32 768",
        "abc",
        "99999999999999999999", // overflows u32 and u64
        "4294967296",           // u32::MAX + 1
    ] {
        assert_eq!(parse_context_window(raw), None, "input {raw:?}");
    }
}

// ---------------------------------------------------------------------------
// with_context_window
// ---------------------------------------------------------------------------

#[test]
fn test_with_context_window_when_some_should_replace_only_advertised_length() {
    let base = ContextConfig::for_model("qwen3-coder");
    let out = base.clone().with_context_window(Some(32_768));

    assert_eq!(out.advertised_context_length, 32_768);
    assert_eq!(out.effective_budget_ratio, base.effective_budget_ratio);
    assert_eq!(out.recent_window, base.recent_window);
    assert_eq!(out.tool_compress_age, base.tool_compress_age);
    assert_eq!(out.tool_oneliner_age_offset, base.tool_oneliner_age_offset);
    assert_eq!(out.reorder_middle_zone, base.reorder_middle_zone);
}

#[test]
fn test_with_context_window_when_none_should_leave_config_unchanged() {
    let base = ContextConfig::for_model("claude-sonnet");
    let out = base.clone().with_context_window(None);

    assert_eq!(
        out.advertised_context_length,
        base.advertised_context_length
    );
    assert_eq!(out.effective_budget_ratio, base.effective_budget_ratio);
    assert_eq!(out.recent_window, base.recent_window);
    assert_eq!(out.tool_compress_age, base.tool_compress_age);
    assert_eq!(out.tool_oneliner_age_offset, base.tool_oneliner_age_offset);
    assert_eq!(out.reorder_middle_zone, base.reorder_middle_zone);
}

// ---------------------------------------------------------------------------
// for_model_with_env
// ---------------------------------------------------------------------------

fn env_with(value: &'static str) -> impl Fn(&str) -> Option<String> {
    move |k| (k == "HONE_CONTEXT_WINDOW").then(|| value.to_owned())
}

#[test]
fn test_for_model_with_env_when_qwen_and_valid_override_should_budget_ratio_times_window() {
    let cfg = ContextConfig::for_model_with_env("qwen3-coder", env_with("32768"));
    assert_eq!(cfg.advertised_context_length, 32_768);
    assert_eq!(cfg.effective_budget(), 14_745);
}

#[test]
fn test_for_model_with_env_when_claude_and_valid_override_should_budget_ratio_times_window() {
    let cfg = ContextConfig::for_model_with_env("claude-sonnet", env_with("65536"));
    assert_eq!(cfg.advertised_context_length, 65_536);
    assert_eq!(cfg.effective_budget(), 39_321);
}

#[test]
fn test_for_model_with_env_when_unknown_model_and_override_should_apply_default_ratio() {
    let cfg = ContextConfig::for_model_with_env("mystery-model", env_with("32768"));
    assert_eq!(cfg.advertised_context_length, 32_768);
    assert_eq!(cfg.effective_budget(), 16_384);
}

#[test]
fn test_for_model_with_env_when_override_should_keep_model_tuning() {
    let base = ContextConfig::for_model("qwen3-coder");
    let cfg = ContextConfig::for_model_with_env("qwen3-coder", env_with("32768"));
    assert_eq!(cfg.effective_budget_ratio, base.effective_budget_ratio);
    assert_eq!(cfg.recent_window, base.recent_window);
    assert_eq!(cfg.reorder_middle_zone, base.reorder_middle_zone);
}

#[test]
fn test_for_model_with_env_when_var_missing_should_equal_for_model() {
    for model in ["qwen3-coder", "claude-sonnet", "mystery-model"] {
        let base = ContextConfig::for_model(model);
        let cfg = ContextConfig::for_model_with_env(model, |_| None);
        assert_eq!(
            cfg.advertised_context_length,
            base.advertised_context_length
        );
        assert_eq!(cfg.effective_budget(), base.effective_budget());
        assert_eq!(cfg.recent_window, base.recent_window);
        assert_eq!(cfg.reorder_middle_zone, base.reorder_middle_zone);
    }
}

#[test]
fn test_for_model_with_env_when_var_unparsable_should_equal_for_model() {
    for bad in ["", "32k", "-1", "100", "99999999999", "abc"] {
        let base = ContextConfig::for_model("qwen3-coder");
        let cfg = ContextConfig::for_model_with_env("qwen3-coder", move |_| Some(bad.to_owned()));
        assert_eq!(
            cfg.advertised_context_length, base.advertised_context_length,
            "value {bad:?}"
        );
        assert_eq!(
            cfg.effective_budget(),
            base.effective_budget(),
            "value {bad:?}"
        );
    }
}

#[test]
fn test_for_model_with_env_when_called_should_look_up_exact_key() {
    let seen: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let _ = ContextConfig::for_model_with_env("qwen3-coder", |k| {
        seen.borrow_mut().push(k.to_owned());
        None
    });
    let seen = seen.into_inner();
    assert!(!seen.is_empty(), "env closure was never consulted");
    assert!(
        seen.iter().all(|k| k == "HONE_CONTEXT_WINDOW"),
        "unexpected keys: {seen:?}"
    );
}
