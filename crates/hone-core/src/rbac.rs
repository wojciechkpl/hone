use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RbacError {
    #[error("access denied: role '{role}' cannot use tool '{tool}'")]
    ToolDenied { role: String, tool: String },
    #[error("access denied: role '{role}' cannot use model '{model}'")]
    ModelDenied { role: String, model: String },
    #[error("role '{role}' not found in policy")]
    RoleNotFound { role: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RbacPolicy {
    pub roles: HashMap<String, Role>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    pub allowed_tools: ToolAccess,
    pub allowed_models: Vec<String>,
    pub budget_limit: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolAccess {
    All,
    AllowList(Vec<String>),
    DenyList(Vec<String>),
}

impl RbacPolicy {
    pub fn check_tool(&self, role: &str, tool: &str) -> Result<(), RbacError> {
        let r = self.roles.get(role).ok_or_else(|| RbacError::RoleNotFound {
            role: role.to_owned(),
        })?;

        match &r.allowed_tools {
            ToolAccess::All => Ok(()),
            ToolAccess::AllowList(list) => {
                if list.iter().any(|t| t == tool) {
                    Ok(())
                } else {
                    Err(RbacError::ToolDenied {
                        role: role.to_owned(),
                        tool: tool.to_owned(),
                    })
                }
            }
            ToolAccess::DenyList(list) => {
                if list.iter().any(|t| t == tool) {
                    Err(RbacError::ToolDenied {
                        role: role.to_owned(),
                        tool: tool.to_owned(),
                    })
                } else {
                    Ok(())
                }
            }
        }
    }

    pub fn check_model(&self, role: &str, model_id: &str) -> Result<(), RbacError> {
        let r = self.roles.get(role).ok_or_else(|| RbacError::RoleNotFound {
            role: role.to_owned(),
        })?;

        if r.allowed_models.is_empty() {
            return Ok(());
        }

        if r.allowed_models.iter().any(|m| m == model_id) {
            Ok(())
        } else {
            Err(RbacError::ModelDenied {
                role: role.to_owned(),
                model: model_id.to_owned(),
            })
        }
    }

    pub fn get_budget(&self, role: &str) -> Option<f64> {
        self.roles.get(role)?.budget_limit
    }

    pub fn default_policy() -> Self {
        let mut roles = HashMap::new();

        roles.insert(
            "admin".to_owned(),
            Role {
                name: "admin".to_owned(),
                allowed_tools: ToolAccess::All,
                allowed_models: vec![],
                budget_limit: None,
            },
        );

        roles.insert(
            "developer".to_owned(),
            Role {
                name: "developer".to_owned(),
                allowed_tools: ToolAccess::AllowList(vec![
                    "read_file".to_owned(),
                    "write_file".to_owned(),
                    "edit_file".to_owned(),
                    "glob".to_owned(),
                    "git_status".to_owned(),
                    "git_diff".to_owned(),
                ]),
                allowed_models: vec![],
                budget_limit: None,
            },
        );

        roles.insert(
            "viewer".to_owned(),
            Role {
                name: "viewer".to_owned(),
                allowed_tools: ToolAccess::DenyList(vec![
                    "write_file".to_owned(),
                    "shell".to_owned(),
                    "git_commit".to_owned(),
                ]),
                allowed_models: vec![],
                budget_limit: None,
            },
        );

        Self { roles }
    }
}
