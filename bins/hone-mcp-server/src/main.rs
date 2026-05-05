use anyhow::Result;
use hone_tools::{GitDiffTool, GitStatusTool, GlobTool, GrepTool, ReadFileTool, ToolContext};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let registry = hone_tools::ToolRegistry::builder()
        .register(Box::new(ReadFileTool))
        .register(Box::new(GlobTool))
        .register(Box::new(GrepTool))
        .register(Box::new(GitStatusTool))
        .register(Box::new(GitDiffTool))
        .build();

    let ctx = ToolContext {
        sandbox: Arc::new(hone_sandbox::NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        working_dir: std::env::current_dir()?,
    };

    let server = Arc::new(hone_mcp_server::McpServer::new(registry, ctx));
    hone_mcp_server::serve_stdio(server).await?;
    Ok(())
}
