use hone_core::{RbacError, RbacPolicy, Role, ToolAccess};

fn make_role(name: &str, tools: ToolAccess, models: Vec<String>) -> Role {
    Role {
        name: name.to_owned(),
        allowed_tools: tools,
        allowed_models: models,
        budget_limit: None,
    }
}

fn policy_with_roles(roles: Vec<Role>) -> RbacPolicy {
    let map = roles.into_iter().map(|r| (r.name.clone(), r)).collect();
    RbacPolicy { roles: map }
}

#[test]
fn test_rbac_admin_can_use_all_tools() {
    let policy = RbacPolicy::default_policy();
    assert!(policy.check_tool("admin", "shell").is_ok());
    assert!(policy.check_tool("admin", "write_file").is_ok());
}

#[test]
fn test_rbac_viewer_denied_write_tools() {
    let viewer = make_role(
        "viewer",
        ToolAccess::DenyList(vec![
            "write_file".to_owned(),
            "shell".to_owned(),
            "git_commit".to_owned(),
        ]),
        vec![],
    );
    let policy = policy_with_roles(vec![viewer]);

    let err = policy.check_tool("viewer", "write_file").unwrap_err();
    assert!(matches!(err, RbacError::ToolDenied { .. }));

    assert!(policy.check_tool("viewer", "read_file").is_ok());
}

#[test]
fn test_rbac_developer_allowlist() {
    let developer = make_role(
        "developer",
        ToolAccess::AllowList(vec![
            "read_file".to_owned(),
            "write_file".to_owned(),
            "edit_file".to_owned(),
            "glob".to_owned(),
            "git_status".to_owned(),
            "git_diff".to_owned(),
        ]),
        vec![],
    );
    let policy = policy_with_roles(vec![developer]);

    assert!(policy.check_tool("developer", "read_file").is_ok());

    let err = policy.check_tool("developer", "shell").unwrap_err();
    assert!(matches!(err, RbacError::ToolDenied { .. }));
}

#[test]
fn test_rbac_custom_role_with_allowlist() {
    let intern = make_role(
        "intern",
        ToolAccess::AllowList(vec!["read_file".to_owned()]),
        vec![],
    );
    let policy = policy_with_roles(vec![intern]);

    assert!(policy.check_tool("intern", "read_file").is_ok());
    assert!(policy.check_tool("intern", "write_file").is_err());
    assert!(policy.check_tool("intern", "shell").is_err());
}

#[test]
fn test_rbac_prevents_unauthorized_tool_use() {
    let viewer = make_role(
        "viewer",
        ToolAccess::DenyList(vec!["write_file".to_owned()]),
        vec![],
    );
    let policy = policy_with_roles(vec![viewer]);

    let err = policy.check_tool("viewer", "write_file").unwrap_err();
    match err {
        RbacError::ToolDenied { role, tool } => {
            assert_eq!(role, "viewer");
            assert_eq!(tool, "write_file");
        }
        other => panic!("expected ToolDenied, got {other}"),
    }
}

#[test]
fn test_rbac_model_restriction() {
    let role = make_role(
        "restricted",
        ToolAccess::All,
        vec!["anthropic:claude-sonnet-4-5".to_owned()],
    );
    let policy = policy_with_roles(vec![role]);

    assert!(policy
        .check_model("restricted", "anthropic:claude-sonnet-4-5")
        .is_ok());

    let err = policy
        .check_model("restricted", "anthropic:claude-opus-4-5")
        .unwrap_err();
    assert!(matches!(err, RbacError::ModelDenied { .. }));
}

#[test]
fn test_rbac_empty_models_allows_all() {
    let role = make_role("open", ToolAccess::All, vec![]);
    let policy = policy_with_roles(vec![role]);

    assert!(policy
        .check_model("open", "anthropic:claude-sonnet-4-5")
        .is_ok());
    assert!(policy
        .check_model("open", "openai:gpt-4o")
        .is_ok());
}

#[test]
fn test_rbac_role_not_found() {
    let policy = RbacPolicy::default_policy();

    let err = policy.check_tool("nonexistent", "shell").unwrap_err();
    assert!(matches!(err, RbacError::RoleNotFound { role } if role == "nonexistent"));
}

#[test]
fn test_rbac_default_policy_has_standard_roles() {
    let policy = RbacPolicy::default_policy();
    assert!(policy.roles.contains_key("admin"));
    assert!(policy.roles.contains_key("developer"));
    assert!(policy.roles.contains_key("viewer"));
}
