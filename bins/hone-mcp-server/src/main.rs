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
    // Read once at startup: the built-in rules are opt-in.
    let env_value = std::env::var(hone_tools::policy_gate::POLICY_BUILTINS_ENV).ok();
    if let Some(w) = hone_tools::policy_gate::builtins_env_warning(env_value.as_deref()) {
        eprintln!("hone-mcp-server: warning: {w}");
    }
    let builtins = hone_tools::policy_gate::builtins_enabled(env_value.as_deref());
    let policy = match hone_mcp_server::load_policy(&working_dir, home, approve.as_deref(), builtins)
    {
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
        read_filter: None,
    };

    let server = Arc::new(hone_mcp_server::McpServer::new(registry, ctx).with_policy(policy));
    hone_mcp_server::serve_stdio(server).await?;
    Ok(())
}
