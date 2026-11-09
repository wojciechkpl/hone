use anyhow::Result;
use hone_tools::{GitDiffTool, GitStatusTool, GlobTool, GrepTool, ReadFileTool, ToolContext};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let registry = hone_tools::ToolRegistry::builder()
        .register(Box::new(ReadFileTool))
        .register(Box::new(GlobTool))
        .register(Box::new(GrepTool))
        .register(Box::new(GitStatusTool))
        .register(Box::new(GitDiffTool))
        .build();

    let working_dir = std::env::current_dir()?;
    let home = hone_tools::policy_gate::default_policy_home();
    let approve = std::env::var("HONE_APPROVE").ok();
    let policy = match hone_mcp_server::load_policy(&working_dir, home, approve.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("hone-mcp-server: {e}");
            std::process::exit(2);
        }
    };

    let ctx = ToolContext {
        sandbox: Arc::new(hone_sandbox::NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        working_dir,
    };

    let server = Arc::new(hone_mcp_server::McpServer::new(registry, ctx).with_policy(policy));
    hone_mcp_server::serve_stdio(server).await?;
    Ok(())
}
