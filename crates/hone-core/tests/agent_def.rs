use hone_core::{AgentDef, AgentDefError, AgentRegistry};

// ---------------------------------------------------------------------------
// AgentDef::from_markdown
// ---------------------------------------------------------------------------

#[test]
fn test_parse_agent_frontmatter() {
    let md = "\
---
name: my-agent
description: Does something useful
tools: [read, write, shell]
model: haiku
---

You are a helpful agent.

Follow these steps carefully.
";

    let def = AgentDef::from_markdown(md).expect("should parse valid markdown");
    assert_eq!(def.name, "my-agent");
    assert_eq!(def.description, "Does something useful");
    assert_eq!(def.tools, vec!["read", "write", "shell"]);
    assert_eq!(def.model, "haiku");
    assert!(
        def.system_prompt.contains("You are a helpful agent."),
        "system_prompt should contain the body text"
    );
}

#[test]
fn test_parse_missing_frontmatter() {
    let md = "This is plain markdown with no frontmatter at all.";
    let result = AgentDef::from_markdown(md);
    assert!(result.is_err());
    matches!(result.unwrap_err(), AgentDefError::MissingFrontmatter);
}

#[test]
fn test_parse_empty_system_prompt() {
    let md = "\
---
name: empty-body
description: No body content
---
";

    let def = AgentDef::from_markdown(md).expect("should parse with empty body");
    assert_eq!(def.name, "empty-body");
    assert_eq!(def.system_prompt, "");
}

#[test]
fn test_parse_default_model() {
    let md = "\
---
name: no-model
description: Uses the default model
---

Body here.
";

    let def = AgentDef::from_markdown(md).expect("should parse without explicit model");
    assert_eq!(def.model, "sonnet");
}

// ---------------------------------------------------------------------------
// AgentRegistry
// ---------------------------------------------------------------------------

fn make_def(name: &str, description: &str) -> AgentDef {
    let md = format!(
        "---\nname: {name}\ndescription: {description}\n---\n\nSystem prompt for {name}.\n"
    );
    AgentDef::from_markdown(&md).expect("test helper: valid markdown")
}

#[test]
fn test_agent_registry_register_and_get() {
    let mut registry = AgentRegistry::new();
    let def = make_def("code-reviewer", "Reviews code for quality");
    registry.register(def);

    let found = registry.get("code-reviewer");
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "code-reviewer");

    let missing = registry.get("nonexistent");
    assert!(missing.is_none());
}

#[test]
fn test_agent_registry_find_by_description() {
    let mut registry = AgentRegistry::new();
    registry.register(make_def(
        "code-reviewer",
        "Expert code reviewer for quality and security",
    ));
    registry.register(make_def("debugger", "Systematic debugging specialist"));

    let found = registry.find_by_description("code review");
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "code-reviewer");

    let not_found = registry.find_by_description("zzz-no-match-zzz");
    assert!(not_found.is_none());
}

#[test]
fn test_agent_registry_find_by_description_case_insensitive() {
    let mut registry = AgentRegistry::new();
    registry.register(make_def("debugger", "Systematic Debugging Specialist"));

    let found = registry.find_by_description("debugging specialist");
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "debugger");
}

#[test]
fn test_agent_registry_list() {
    let mut registry = AgentRegistry::new();
    registry.register(make_def("alpha", "First agent"));
    registry.register(make_def("beta", "Second agent"));
    registry.register(make_def("gamma", "Third agent"));

    let list = registry.list();
    assert_eq!(list.len(), 3);
    // list() returns agents sorted by name
    assert_eq!(list[0].name, "alpha");
    assert_eq!(list[1].name, "beta");
    assert_eq!(list[2].name, "gamma");
}

#[test]
fn test_load_builtins_has_agents() {
    let mut registry = AgentRegistry::new();
    registry.load_builtins();

    let list = registry.list();
    assert!(
        list.len() >= 5,
        "expected at least 5 built-in agents, found {}",
        list.len()
    );

    // Spot-check a few known built-ins
    assert!(
        registry.get("code-reviewer").is_some(),
        "code-reviewer should be a built-in"
    );
    assert!(
        registry.get("debugger").is_some(),
        "debugger should be a built-in"
    );
    assert!(
        registry.get("security-auditor").is_some(),
        "security-auditor should be a built-in"
    );
}

// ---------------------------------------------------------------------------
// AgentDef::load_dir
// ---------------------------------------------------------------------------

#[test]
fn test_load_dir() {
    let dir = tempfile::tempdir().expect("create temp dir");

    let agent1 = "\
---
name: dir-agent-one
description: First agent from dir
---

System prompt one.
";
    let agent2 = "\
---
name: dir-agent-two
description: Second agent from dir
---

System prompt two.
";
    std::fs::write(dir.path().join("one.md"), agent1).unwrap();
    std::fs::write(dir.path().join("two.md"), agent2).unwrap();
    std::fs::write(dir.path().join("ignored.txt"), "not an agent").unwrap();

    let defs = AgentDef::load_dir(dir.path()).expect("load_dir should succeed");
    assert_eq!(defs.len(), 2);

    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"dir-agent-one"));
    assert!(names.contains(&"dir-agent-two"));
}
