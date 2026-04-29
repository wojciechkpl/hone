use async_trait::async_trait;
use git2::{Repository, Signature};
use hone_proto::ToolLayer;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

/// Creates a git commit from the current index with the provided message.
pub struct GitCommitTool;

#[async_trait]
impl Tool for GitCommitTool {
    fn name(&self) -> &str {
        "git_commit"
    }

    fn description(&self) -> &str {
        "Create a git commit from staged changes with the provided commit message. \
         Returns the full SHA of the new commit."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The commit message."
                }
            },
            "required": ["message"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let message = input
            .get_str("message")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'message'".to_string(),
            })?
            .to_string();

        let working_dir = ctx.working_dir.clone();

        let sha = tokio::task::spawn_blocking(move || {
            let repo = Repository::discover(&working_dir)?;

            // Build a signature from the repo config, falling back to sensible defaults.
            let sig = repo
                .signature()
                .or_else(|_| Signature::now("Hone Agent", "agent@hone"))?;

            let mut index = repo.index()?;
            index.write()?;
            let tree_oid = index.write_tree()?;
            let tree = repo.find_tree(tree_oid)?;

            // Parent is HEAD if it exists; for the very first commit there is none.
            let parents: Vec<git2::Commit<'_>> = match repo.head() {
                Ok(head_ref) => {
                    let oid = head_ref
                        .target()
                        .ok_or_else(|| git2::Error::from_str("HEAD reference has no target OID"))?;
                    vec![repo.find_commit(oid)?]
                }
                Err(_) => vec![],
            };

            let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();

            let oid = repo.commit(Some("HEAD"), &sig, &sig, &message, &tree, &parent_refs)?;

            Ok::<_, git2::Error>(oid.to_string())
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "git_commit".to_string(),
            message: e.to_string(),
        })?
        .map_err(ToolError::from)?;

        Ok(ToolOutput::new(json!({ "sha": sha })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
