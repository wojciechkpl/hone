use async_trait::async_trait;
use git2::{Repository, StatusOptions};
use hone_proto::ToolLayer;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

/// Returns the working-tree status of the git repository.
pub struct GitStatusTool;

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "Return the git status of the repository: modified, staged, and untracked files."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(&self, _input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let working_dir = ctx.working_dir.clone();

        let result = tokio::task::spawn_blocking(move || {
            let repo = Repository::discover(&working_dir)?;

            let mut opts = StatusOptions::new();
            opts.include_untracked(true)
                .recurse_untracked_dirs(true)
                .include_ignored(false);

            let statuses = repo.statuses(Some(&mut opts))?;

            let mut modified: Vec<String> = Vec::new();
            let mut staged: Vec<String> = Vec::new();
            let mut untracked: Vec<String> = Vec::new();

            for entry in statuses.iter() {
                let path = entry.path().unwrap_or("").to_string();
                let flags = entry.status();

                if flags.is_index_new()
                    || flags.is_index_modified()
                    || flags.is_index_deleted()
                    || flags.is_index_renamed()
                    || flags.is_index_typechange()
                {
                    staged.push(path.clone());
                }

                if flags.is_wt_modified()
                    || flags.is_wt_deleted()
                    || flags.is_wt_renamed()
                    || flags.is_wt_typechange()
                {
                    modified.push(path.clone());
                }

                if flags.is_wt_new() {
                    untracked.push(path);
                }
            }

            Ok::<_, git2::Error>(json!({
                "modified": modified,
                "staged": staged,
                "untracked": untracked
            }))
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "git_status".to_string(),
            message: e.to_string(),
        })?
        .map_err(ToolError::from)?;

        Ok(ToolOutput::new(result))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }

    fn is_read_only(&self) -> bool {
        true
    }
}
