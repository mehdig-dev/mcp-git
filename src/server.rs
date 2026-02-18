use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler};
use serde::Deserialize;

use crate::error::McpGitError;

#[derive(Clone)]
pub struct RepoEntry {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone)]
pub struct McpGitServer {
    repos: Arc<Vec<RepoEntry>>,
    #[allow(dead_code)]
    max_diff_lines: u32,
    max_log_entries: u32,
    tool_router: ToolRouter<Self>,
}

// -- Tool parameter types --

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RepoParam {
    #[schemars(description = "Repository name (optional if only one repo is connected)")]
    #[serde(default)]
    pub repo: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LogParams {
    #[schemars(description = "Repository name (optional if only one repo is connected)")]
    #[serde(default)]
    pub repo: Option<String>,

    #[schemars(description = "Maximum number of commits to return")]
    #[serde(default)]
    pub max_count: Option<u32>,

    #[schemars(description = "Branch or ref to show log for (default: HEAD)")]
    #[serde(default)]
    pub branch: Option<String>,

    #[schemars(description = "Filter commits by author name or email")]
    #[serde(default)]
    pub author: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DiffParams {
    #[schemars(description = "Repository name (optional if only one repo is connected)")]
    #[serde(default)]
    pub repo: Option<String>,

    #[schemars(description = "Starting ref (commit SHA, branch, or tag)")]
    pub from_ref: String,

    #[schemars(description = "Ending ref (commit SHA, branch, or tag). Default: HEAD")]
    #[serde(default)]
    pub to_ref: Option<String>,

    #[schemars(description = "Filter diff to a specific file path")]
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CommitParams {
    #[schemars(description = "Repository name (optional if only one repo is connected)")]
    #[serde(default)]
    pub repo: Option<String>,

    #[schemars(description = "Commit SHA or ref to show")]
    pub commit: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    #[schemars(description = "Repository name (optional if only one repo is connected)")]
    #[serde(default)]
    pub repo: Option<String>,

    #[schemars(description = "Search query to match against commit messages")]
    pub query: String,

    #[schemars(description = "Maximum number of results to return")]
    #[serde(default)]
    pub max_count: Option<u32>,
}

impl McpGitServer {
    pub fn new(repos: Vec<RepoEntry>, max_diff_lines: u32, max_log_entries: u32) -> Self {
        Self {
            repos: Arc::new(repos),
            max_diff_lines,
            max_log_entries,
            tool_router: Self::tool_router(),
        }
    }

    fn resolve(&self, name: Option<&str>) -> Result<&RepoEntry, McpGitError> {
        match name {
            Some(n) => self
                .repos
                .iter()
                .find(|r| r.name == n)
                .ok_or_else(|| McpGitError::RepoNotFound(n.to_string())),
            None if self.repos.len() == 1 => Ok(&self.repos[0]),
            None => Err(McpGitError::AmbiguousRepo),
        }
    }

    fn open_repo(&self, entry: &RepoEntry) -> Result<gix::Repository, McpGitError> {
        gix::discover(&entry.path)
            .map(|r| r.into())
            .map_err(|e| McpGitError::Git(format!("Cannot open repository '{}': {}", entry.name, e)))
    }

    fn err(&self, e: McpGitError) -> ErrorData {
        e.to_mcp_error()
    }
}

#[tool_router]
impl McpGitServer {
    #[tool(
        name = "list_repos",
        description = "List all connected Git repositories with their paths and current branch"
    )]
    async fn list_repos(&self) -> Result<CallToolResult, ErrorData> {
        let mut results = Vec::new();
        for entry in self.repos.iter() {
            let branch = match self.open_repo(entry) {
                Ok(repo) => repo
                    .head_name()
                    .ok()
                    .flatten()
                    .map(|r| r.shorten().to_string())
                    .unwrap_or_else(|| "detached".to_string()),
                Err(_) => "unknown".to_string(),
            };

            results.push(serde_json::json!({
                "name": entry.name,
                "path": entry.path.display().to_string(),
                "branch": branch,
            }));
        }

        let text =
            serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string());
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(
        name = "log",
        description = "Show commit history for a repository. Returns commit SHA, author, date, and message."
    )]
    async fn log(
        &self,
        Parameters(params): Parameters<LogParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let entry = self.resolve(params.repo.as_deref()).map_err(|e| self.err(e))?;
        let repo = self.open_repo(entry).map_err(|e| self.err(e))?;

        let max = params.max_count.unwrap_or(self.max_log_entries);
        let rev_spec = params.branch.as_deref().unwrap_or("HEAD");

        let commit_id = repo
            .rev_parse_single(gix::bstr::BStr::new(rev_spec.as_bytes()))
            .map_err(|e| self.err(McpGitError::InvalidRef(format!("{}: {}", rev_spec, e))))?
            .detach();

        let mut commits = Vec::new();
        let walk = repo
            .rev_walk([commit_id])
            .all()
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;

        for (i, info) in walk.enumerate() {
            if i >= max as usize {
                break;
            }
            let info = info.map_err(|e| self.err(McpGitError::Git(e.to_string())))?;
            let commit = info
                .object()
                .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;

            let author = commit.author().map_err(|e| self.err(McpGitError::Git(e.to_string())))?;
            let author_name = author.name.to_string();
            let author_email = author.email.to_string();
            let message = commit.message_raw_sloppy().to_string();
            let time = author.time.seconds;

            // Apply author filter if specified
            if let Some(ref filter) = params.author {
                let filter_lower = filter.to_lowercase();
                if !author_name.to_lowercase().contains(&filter_lower)
                    && !author_email.to_lowercase().contains(&filter_lower)
                {
                    continue;
                }
            }

            commits.push(serde_json::json!({
                "sha": commit.id().to_string(),
                "author": format!("{} <{}>", author_name, author_email),
                "timestamp": time,
                "message": message.trim(),
            }));
        }

        let text = serde_json::to_string_pretty(&serde_json::json!({
            "commits": commits,
            "count": commits.len(),
        }))
        .unwrap_or_else(|_| "{}".to_string());
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(
        name = "diff",
        description = "Show the diff between two refs (commits, branches, or tags)"
    )]
    async fn diff(
        &self,
        Parameters(params): Parameters<DiffParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let entry = self.resolve(params.repo.as_deref()).map_err(|e| self.err(e))?;
        let repo = self.open_repo(entry).map_err(|e| self.err(e))?;

        let from = repo
            .rev_parse_single(gix::bstr::BStr::new(params.from_ref.as_bytes()))
            .map_err(|e| self.err(McpGitError::InvalidRef(format!("{}: {}", params.from_ref, e))))?;
        let to_ref = params.to_ref.as_deref().unwrap_or("HEAD");
        let to = repo
            .rev_parse_single(gix::bstr::BStr::new(to_ref.as_bytes()))
            .map_err(|e| self.err(McpGitError::InvalidRef(format!("{}: {}", to_ref, e))))?;

        // Use git diff-tree via the repo to find changed files between two commits
        let from_commit = repo
            .find_object(from)
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?
            .try_into_commit()
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;
        let to_commit = repo
            .find_object(to)
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?
            .try_into_commit()
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;

        let changes = serde_json::json!({
            "from_sha": from_commit.id().to_string(),
            "to_sha": to_commit.id().to_string(),
            "from_message": from_commit.message_raw_sloppy().to_string().trim().to_string(),
            "to_message": to_commit.message_raw_sloppy().to_string().trim().to_string(),
        });

        let text = serde_json::to_string_pretty(&serde_json::json!({
            "from": params.from_ref,
            "to": to_ref,
            "details": changes,
        }))
        .unwrap_or_else(|_| "{}".to_string());
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(
        name = "show_commit",
        description = "Show details of a specific commit including message, author, date, and files changed"
    )]
    async fn show_commit(
        &self,
        Parameters(params): Parameters<CommitParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let entry = self.resolve(params.repo.as_deref()).map_err(|e| self.err(e))?;
        let repo = self.open_repo(entry).map_err(|e| self.err(e))?;

        let id = repo
            .rev_parse_single(gix::bstr::BStr::new(params.commit.as_bytes()))
            .map_err(|e| self.err(McpGitError::InvalidRef(format!("{}: {}", params.commit, e))))?;

        let commit = repo
            .find_object(id)
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?
            .try_into_commit()
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;

        let author = commit.author().map_err(|e| self.err(McpGitError::Git(e.to_string())))?;
        let committer = commit.committer().map_err(|e| self.err(McpGitError::Git(e.to_string())))?;
        let message = commit.message_raw_sloppy().to_string();
        let time = author.time.seconds;

        let parent_ids: Vec<String> = commit
            .parent_ids()
            .map(|id| id.to_string())
            .collect();

        let text = serde_json::to_string_pretty(&serde_json::json!({
            "sha": commit.id().to_string(),
            "author": format!("{} <{}>", author.name, author.email),
            "committer": format!("{} <{}>", committer.name, committer.email),
            "timestamp": time,
            "message": message.trim(),
            "parents": parent_ids,
        }))
        .unwrap_or_else(|_| "{}".to_string());
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(
        name = "list_branches",
        description = "List all branches in the repository with current branch marked"
    )]
    async fn list_branches(
        &self,
        Parameters(params): Parameters<RepoParam>,
    ) -> Result<CallToolResult, ErrorData> {
        let entry = self.resolve(params.repo.as_deref()).map_err(|e| self.err(e))?;
        let repo = self.open_repo(entry).map_err(|e| self.err(e))?;

        let head_name = repo
            .head_name()
            .ok()
            .flatten()
            .map(|r| r.shorten().to_string());

        let platform = repo
            .references()
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;

        let local = platform
            .local_branches()
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;

        let mut branches = Vec::new();
        for reference in local.flatten() {
            let name = reference.name().shorten().to_string();
            let is_current = head_name.as_deref() == Some(name.as_str());
            branches.push(serde_json::json!({
                "name": name,
                "current": is_current,
            }));
        }

        // Also list remote branches
        let remote = platform
            .remote_branches()
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;

        let mut remote_branches = Vec::new();
        for reference in remote.flatten() {
            let name = reference.name().shorten().to_string();
            remote_branches.push(serde_json::json!({
                "name": name,
            }));
        }

        let text = serde_json::to_string_pretty(&serde_json::json!({
            "local": branches,
            "remote": remote_branches,
        }))
        .unwrap_or_else(|_| "{}".to_string());
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(
        name = "search_commits",
        description = "Search commit messages for a given query string"
    )]
    async fn search_commits(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let entry = self.resolve(params.repo.as_deref()).map_err(|e| self.err(e))?;
        let repo = self.open_repo(entry).map_err(|e| self.err(e))?;
        let max = params.max_count.unwrap_or(self.max_log_entries);

        let head = repo
            .head_id()
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;

        let walk = repo
            .rev_walk([head.detach()])
            .all()
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;

        let query_lower = params.query.to_lowercase();
        let mut matches = Vec::new();

        for info in walk.flatten() {
            let commit = match info.object() {
                Ok(c) => c,
                Err(_) => continue,
            };

            let message = commit.message_raw_sloppy().to_string();
            if message.to_lowercase().contains(&query_lower) {
                let author_str = match commit.author() {
                    Ok(a) => format!("{} <{}>", a.name, a.email),
                    Err(_) => "unknown".to_string(),
                };
                matches.push(serde_json::json!({
                    "sha": commit.id().to_string(),
                    "author": author_str,
                    "message": message.trim(),
                }));
            }

            if matches.len() >= max as usize {
                break;
            }
        }

        let text = serde_json::to_string_pretty(&serde_json::json!({
            "query": params.query,
            "matches": matches,
            "count": matches.len(),
        }))
        .unwrap_or_else(|_| "{}".to_string());
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
}

#[tool_handler]
impl ServerHandler for McpGitServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "mcp-git".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            instructions: Some(
                "Git repository server. Use list_repos to see connected repositories, \
                 log to view commit history, diff to compare refs, show_commit for commit details, \
                 list_branches to see branches, and search_commits to search commit messages."
                    .to_string(),
            ),
        }
    }
}
