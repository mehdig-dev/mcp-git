use anyhow::{bail, Result};
use clap::Parser;
use mcp_git::server;
use rmcp::{transport::stdio, ServiceExt};
use tracing_subscriber::EnvFilter;

/// MCP server for Git repositories — lets LLMs explore commit history, diffs, and branches
#[derive(Parser)]
#[command(name = "mcp-git", version, about)]
struct Cli {
    /// Path to a Git repository (repeatable). Defaults to current directory.
    #[arg(long = "repo")]
    repos: Vec<String>,

    /// Maximum number of diff lines returned (default: 500)
    #[arg(long, default_value = "500")]
    max_diff_lines: u32,

    /// Maximum number of log entries returned (default: 50)
    #[arg(long, default_value = "50")]
    max_log_entries: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let repo_paths = if cli.repos.is_empty() {
        vec![".".to_string()]
    } else {
        cli.repos
    };

    // Validate all repo paths exist and are git repos
    let mut entries = Vec::new();
    for path in &repo_paths {
        let canonical = std::fs::canonicalize(path)
            .map_err(|e| anyhow::anyhow!("Cannot resolve path '{}': {}", path, e))?;

        let repo = gix::discover(&canonical)
            .map_err(|e| anyhow::anyhow!("Not a git repository '{}': {}", path, e))?;

        let name = canonical
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());

        let work_dir = repo
            .work_dir()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| canonical.clone());

        entries.push(server::RepoEntry {
            name,
            path: work_dir,
        });

        tracing::info!(path = %canonical.display(), "Discovered git repository");
    }

    if entries.is_empty() {
        bail!("No valid git repositories found.");
    }

    tracing::info!(
        repos = entries.len(),
        max_diff_lines = cli.max_diff_lines,
        max_log_entries = cli.max_log_entries,
        "Starting mcp-git server"
    );

    let service = server::McpGitServer::new(entries, cli.max_diff_lines, cli.max_log_entries);
    let running = service.serve(stdio()).await?;
    running.waiting().await?;

    Ok(())
}
