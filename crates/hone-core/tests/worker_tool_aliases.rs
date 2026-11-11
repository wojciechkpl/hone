#![allow(clippy::expect_used, clippy::unwrap_used)]
//! W3 of `docs/plans/worker-tools-spec.md`: `filtered_registry` resolves the
//! short tool names used in agent definition files (`read`, `write`, `edit`)
//! to the real tool names.

mod worker_tools_support;

use hone_core::{AgentRegistry, filtered_registry};

use worker_tools_support::*;

fn names_of(allowed: &[&str]) -> Vec<String> {
    let log = new_log();
    let base = session_tools(&log);
    let allowed: Vec<String> = allowed.iter().map(|s| (*s).to_owned()).collect();
    let mut names: Vec<String> = filtered_registry(&base, &allowed)
        .names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    names.sort();
    names
}

#[test]
fn test_w3_filtered_registry_when_short_names_listed_should_resolve_to_real_tools() {
    assert_eq!(
        names_of(&["read", "write", "edit"]),
        sorted(&["read_file", "write_file", "edit_file"])
    );
}

#[test]
fn test_w3_filtered_registry_when_each_short_name_alone_should_resolve_to_its_tool() {
    assert_eq!(names_of(&["read"]), sorted(&["read_file"]));
    assert_eq!(names_of(&["write"]), sorted(&["write_file"]));
    assert_eq!(names_of(&["edit"]), sorted(&["edit_file"]));
}

#[test]
fn test_w3_filtered_registry_when_real_names_listed_should_still_work() {
    assert_eq!(
        names_of(&["read_file", "write_file", "edit_file", "shell"]),
        sorted(&["read_file", "write_file", "edit_file", "shell"])
    );
}

#[test]
fn test_w3_filtered_registry_when_short_and_real_names_mixed_should_resolve_both() {
    assert_eq!(
        names_of(&["read", "grep", "edit_file", "glob", "shell"]),
        sorted(&["read_file", "grep", "edit_file", "glob", "shell"])
    );
}

#[test]
fn test_w3_filtered_registry_when_same_tool_listed_twice_should_register_it_once() {
    assert_eq!(names_of(&["read", "read_file"]), sorted(&["read_file"]));
    assert_eq!(
        names_of(&["edit_file", "edit", "edit"]),
        sorted(&["edit_file"])
    );
}

#[test]
fn test_w3_filtered_registry_when_a_name_resolves_to_nothing_should_skip_it() {
    assert_eq!(
        names_of(&["read", "no_such_tool", "frobnicate"]),
        sorted(&["read_file"])
    );
}

#[test]
fn test_w3_filtered_registry_when_alias_target_missing_from_base_should_skip_it() {
    // A base registry without `write_file`: `write` resolves to nothing.
    let log = new_log();
    let mut b = hone_tools::ToolRegistry::builder();
    b = b.register(Box::new(FakeTool {
        name: "read_file",
        read_only: true,
        command_subject: None,
        log: log.clone(),
    }));
    let base = b.build();
    let out = filtered_registry(&base, &["read".to_owned(), "write".to_owned()]);
    assert_eq!(out.names(), vec!["read_file"]);
}

#[test]
fn test_w3_filtered_registry_when_allowlist_empty_should_return_everything() {
    assert_eq!(names_of(&[]), sorted(&SESSION_TOOL_NAMES));
}

#[test]
fn test_w3_every_builtin_agent_should_get_all_the_tools_its_file_lists() {
    let log = new_log();
    let base = session_tools(&log);
    let mut registry = AgentRegistry::new();
    registry.load_builtins();
    for def in registry.list() {
        // Built-in files list a subset of: read, edit, write, shell, grep, glob.
        let expected: Vec<String> = {
            let mut v: Vec<String> = def
                .tools
                .iter()
                .map(|t| match t.as_str() {
                    "read" => "read_file".to_owned(),
                    "write" => "write_file".to_owned(),
                    "edit" => "edit_file".to_owned(),
                    other => other.to_owned(),
                })
                .collect();
            v.sort();
            v
        };
        let mut got: Vec<String> = filtered_registry(&base, &def.tools)
            .names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        got.sort();
        assert_eq!(got, expected, "agent {} (tools {:?})", def.name, def.tools);
    }
}
