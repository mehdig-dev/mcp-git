use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler};
use serde::Deserialize;

use crate::error::McpGitError;
use std::process::Command;

#[derive(Clone)]
pub struct RepoEntry {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone)]
pub struct McpGitServer {
    repos: Arc<Vec<RepoEntry>>,
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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FileAtRefParams {
    #[schemars(description = "Repository name (optional if only one repo is connected)")]
    #[serde(default)]
    pub repo: Option<String>,

    #[schemars(description = "Path to the file within the repository")]
    pub path: String,

    #[schemars(description = "Git ref (commit SHA, branch, or tag). Default: HEAD")]
    #[serde(default, rename = "ref")]
    pub rev: Option<String>,
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
            .map_err(|e| McpGitError::Git(format!("Cannot open repository '{}': {}", entry.name, e)))
    }

    fn err(&self, e: McpGitError) -> ErrorData {
        e.to_mcp_error()
    }
}

// -- Public methods for testability --

impl McpGitServer {
    pub fn do_list_repos(&self) -> Result<CallToolResult, ErrorData> {
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

    pub fn do_log(&self, params: LogParams) -> Result<CallToolResult, ErrorData> {
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

        for info in walk {
            if commits.len() >= max as usize {
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

    pub fn do_diff(&self, params: DiffParams) -> Result<CallToolResult, ErrorData> {
        let entry = self.resolve(params.repo.as_deref()).map_err(|e| self.err(e))?;
        let repo = self.open_repo(entry).map_err(|e| self.err(e))?;

        let from = repo
            .rev_parse_single(gix::bstr::BStr::new(params.from_ref.as_bytes()))
            .map_err(|e| self.err(McpGitError::InvalidRef(format!("{}: {}", params.from_ref, e))))?;
        let to_ref = params.to_ref.as_deref().unwrap_or("HEAD");
        let to = repo
            .rev_parse_single(gix::bstr::BStr::new(to_ref.as_bytes()))
            .map_err(|e| self.err(McpGitError::InvalidRef(format!("{}: {}", to_ref, e))))?;

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

        let from_tree = from_commit
            .tree()
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;
        let to_tree = to_commit
            .tree()
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;

        // Compute tree diff to find changed files
        use gix::object::tree::diff::{Action as DiffAction, Change as DiffChange};
        let mut changes = Vec::new();
        let max_files = self.max_diff_lines as usize;

        from_tree
            .changes()
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?
            .for_each_to_obtain_tree(&to_tree, |change: DiffChange<'_, '_, '_>| {
                let path = change.location().to_string();

                // Apply path filter if specified
                if let Some(ref filter_path) = params.path {
                    if !path.starts_with(filter_path.as_str()) {
                        return Ok::<_, std::convert::Infallible>(DiffAction::Continue);
                    }
                }

                let change_type = match &change {
                    DiffChange::Addition { .. } => "added",
                    DiffChange::Deletion { .. } => "deleted",
                    DiffChange::Modification { .. } => "modified",
                    DiffChange::Rewrite { copy: true, .. } => "copied",
                    DiffChange::Rewrite { .. } => "renamed",
                };

                if changes.len() < max_files {
                    changes.push(serde_json::json!({
                        "path": path,
                        "change": change_type,
                    }));
                }

                Ok(DiffAction::Continue)
            })
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;

        let text = serde_json::to_string_pretty(&serde_json::json!({
            "from": params.from_ref,
            "to": to_ref,
            "from_sha": from_commit.id().to_string(),
            "to_sha": to_commit.id().to_string(),
            "files": changes,
            "file_count": changes.len(),
        }))
        .unwrap_or_else(|_| "{}".to_string());
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    pub fn do_show_commit(&self, params: CommitParams) -> Result<CallToolResult, ErrorData> {
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

    pub fn do_list_branches(&self, params: RepoParam) -> Result<CallToolResult, ErrorData> {
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

    pub fn do_search_commits(&self, params: SearchParams) -> Result<CallToolResult, ErrorData> {
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

    pub fn do_status(&self, params: RepoParam) -> Result<CallToolResult, ErrorData> {
        let entry = self.resolve(params.repo.as_deref()).map_err(|e| self.err(e))?;

        let output = Command::new("git")
            .args(["status", "--porcelain=v1"])
            .current_dir(&entry.path)
            .output()
            .map_err(|e| {
                self.err(McpGitError::Git(format!(
                    "Failed to run git status: {}",
                    e
                )))
            })?;

        if !output.status.success() {
            return Err(self.err(McpGitError::Git(format!(
                "git status failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ))));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut staged = Vec::new();
        let mut unstaged = Vec::new();
        let mut untracked = Vec::new();

        for line in stdout.lines() {
            if line.len() < 3 {
                continue;
            }
            let bytes = line.as_bytes();
            let index_status = bytes[0] as char;
            let worktree_status = bytes[1] as char;
            let path = &line[3..];

            if index_status == '?' {
                untracked.push(path.to_string());
            } else {
                if index_status != ' ' {
                    staged.push(serde_json::json!({
                        "path": path,
                        "status": match index_status {
                            'A' => "added",
                            'M' => "modified",
                            'D' => "deleted",
                            'R' => "renamed",
                            'C' => "copied",
                            _ => "unknown",
                        },
                    }));
                }
                if worktree_status != ' ' {
                    unstaged.push(serde_json::json!({
                        "path": path,
                        "status": match worktree_status {
                            'M' => "modified",
                            'D' => "deleted",
                            _ => "unknown",
                        },
                    }));
                }
            }
        }

        let text = serde_json::to_string_pretty(&serde_json::json!({
            "staged": staged,
            "unstaged": unstaged,
            "untracked": untracked,
            "is_clean": staged.is_empty() && unstaged.is_empty() && untracked.is_empty(),
        }))
        .unwrap_or_else(|_| "{}".to_string());
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    pub fn do_get_file_contents(
        &self,
        params: FileAtRefParams,
    ) -> Result<CallToolResult, ErrorData> {
        let entry = self.resolve(params.repo.as_deref()).map_err(|e| self.err(e))?;
        let repo = self.open_repo(entry).map_err(|e| self.err(e))?;

        let rev = params.rev.as_deref().unwrap_or("HEAD");
        let spec = format!("{}:{}", rev, params.path);

        let id = repo
            .rev_parse_single(gix::bstr::BStr::new(spec.as_bytes()))
            .map_err(|e| self.err(McpGitError::InvalidRef(format!("{}: {}", spec, e))))?;

        let object = repo
            .find_object(id)
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;

        let data = &object.data;
        let is_binary = data.iter().take(8192).any(|&b| b == 0);

        if is_binary {
            let text = serde_json::to_string_pretty(&serde_json::json!({
                "path": params.path,
                "ref": rev,
                "binary": true,
                "size": data.len(),
            }))
            .unwrap_or_else(|_| "{}".to_string());
            Ok(CallToolResult::success(vec![Content::text(text)]))
        } else {
            let content = String::from_utf8_lossy(data);
            let text = serde_json::to_string_pretty(&serde_json::json!({
                "path": params.path,
                "ref": rev,
                "content": content,
                "size": data.len(),
            }))
            .unwrap_or_else(|_| "{}".to_string());
            Ok(CallToolResult::success(vec![Content::text(text)]))
        }
    }

    pub fn do_list_tags(&self, params: RepoParam) -> Result<CallToolResult, ErrorData> {
        let entry = self.resolve(params.repo.as_deref()).map_err(|e| self.err(e))?;
        let repo = self.open_repo(entry).map_err(|e| self.err(e))?;

        let platform = repo
            .references()
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;

        let tag_refs = platform
            .tags()
            .map_err(|e| self.err(McpGitError::Git(e.to_string())))?;

        let mut tags = Vec::new();
        for mut reference in tag_refs.flatten() {
            let name = reference.name().shorten().to_string();
            let sha = reference
                .peel_to_id_in_place()
                .map(|id| id.to_string())
                .unwrap_or_else(|_| "unknown".to_string());

            tags.push(serde_json::json!({
                "name": name,
                "sha": sha,
            }));
        }

        let text = serde_json::to_string_pretty(&serde_json::json!({
            "tags": tags,
            "count": tags.len(),
        }))
        .unwrap_or_else(|_| "{}".to_string());
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    pub fn do_get_remote_info(&self, params: RepoParam) -> Result<CallToolResult, ErrorData> {
        let entry = self.resolve(params.repo.as_deref()).map_err(|e| self.err(e))?;
        let repo = self.open_repo(entry).map_err(|e| self.err(e))?;

        let names = repo.remote_names();
        let mut remotes = Vec::new();

        for name in &names {
            match repo.find_remote(name.as_ref()) {
                Ok(remote) => {
                    let fetch_url = remote
                        .url(gix::remote::Direction::Fetch)
                        .map(|u| u.to_bstring().to_string())
                        .unwrap_or_default();
                    let push_url = remote
                        .url(gix::remote::Direction::Push)
                        .map(|u| u.to_bstring().to_string())
                        .unwrap_or_default();

                    remotes.push(serde_json::json!({
                        "name": name.to_string(),
                        "fetch_url": fetch_url,
                        "push_url": push_url,
                    }));
                }
                Err(_) => continue,
            }
        }

        let text = serde_json::to_string_pretty(&serde_json::json!({
            "remotes": remotes,
            "count": remotes.len(),
        }))
        .unwrap_or_else(|_| "{}".to_string());
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    pub fn do_blame(&self, params: FileAtRefParams) -> Result<CallToolResult, ErrorData> {
        let entry = self.resolve(params.repo.as_deref()).map_err(|e| self.err(e))?;
        let rev = params.rev.as_deref().unwrap_or("HEAD");

        let output = Command::new("git")
            .args(["blame", "--line-porcelain", rev, "--", &params.path])
            .current_dir(&entry.path)
            .output()
            .map_err(|e| {
                self.err(McpGitError::Git(format!(
                    "Failed to run git blame: {}",
                    e
                )))
            })?;

        if !output.status.success() {
            return Err(self.err(McpGitError::Git(format!(
                "git blame failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ))));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        struct BlameLine {
            sha: String,
            author: String,
            line_no: u32,
        }

        let mut lines: Vec<BlameLine> = Vec::new();
        let mut sha = String::new();
        let mut author = String::new();
        let mut line_no = 0u32;

        for raw in stdout.lines() {
            if raw.starts_with('\t') {
                lines.push(BlameLine {
                    sha: sha.clone(),
                    author: author.clone(),
                    line_no,
                });
                continue;
            }

            if raw.len() > 40 && raw.as_bytes()[40] == b' ' {
                let maybe_sha = &raw[..40];
                if maybe_sha.chars().all(|c| c.is_ascii_hexdigit()) {
                    sha = maybe_sha.to_string();
                    let rest: Vec<&str> = raw[41..].splitn(3, ' ').collect();
                    if rest.len() >= 2 {
                        line_no = rest[1].parse().unwrap_or(0);
                    }
                    continue;
                }
            }

            if let Some(a) = raw.strip_prefix("author ") {
                author = a.to_string();
            }
        }

        // Group consecutive lines by same commit
        let mut groups = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let start = lines[i].line_no;
            let group_sha = lines[i].sha.clone();
            let group_author = lines[i].author.clone();
            let mut end = start;
            i += 1;

            while i < lines.len() && lines[i].sha == group_sha {
                end = lines[i].line_no;
                i += 1;
            }

            let line_range = if start == end {
                format!("{}", start)
            } else {
                format!("{}-{}", start, end)
            };

            groups.push(serde_json::json!({
                "commit": &group_sha[..std::cmp::min(8, group_sha.len())],
                "author": group_author,
                "lines": line_range,
            }));
        }

        let text = serde_json::to_string_pretty(&serde_json::json!({
            "path": params.path,
            "ref": rev,
            "blame": groups,
            "total_lines": lines.len(),
        }))
        .unwrap_or_else(|_| "{}".to_string());
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
}

// -- MCP tool handlers (thin wrappers) --

#[tool_router]
impl McpGitServer {
    #[tool(
        name = "list_repos",
        description = "List all connected Git repositories with their paths and current branch"
    )]
    async fn list_repos(&self) -> Result<CallToolResult, ErrorData> {
        self.do_list_repos()
    }

    #[tool(
        name = "log",
        description = "Show commit history for a repository. Returns commit SHA, author, date, and message."
    )]
    async fn log(
        &self,
        Parameters(params): Parameters<LogParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_log(params)
    }

    #[tool(
        name = "diff",
        description = "Show the diff between two refs (commits, branches, or tags)"
    )]
    async fn diff(
        &self,
        Parameters(params): Parameters<DiffParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_diff(params)
    }

    #[tool(
        name = "show_commit",
        description = "Show details of a specific commit including message, author, date, and files changed"
    )]
    async fn show_commit(
        &self,
        Parameters(params): Parameters<CommitParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_show_commit(params)
    }

    #[tool(
        name = "list_branches",
        description = "List all branches in the repository with current branch marked"
    )]
    async fn list_branches(
        &self,
        Parameters(params): Parameters<RepoParam>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_list_branches(params)
    }

    #[tool(
        name = "search_commits",
        description = "Search commit messages for a given query string"
    )]
    async fn search_commits(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_search_commits(params)
    }

    #[tool(
        name = "status",
        description = "Show working tree status including staged, unstaged, and untracked files"
    )]
    async fn status(
        &self,
        Parameters(params): Parameters<RepoParam>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_status(params)
    }

    #[tool(
        name = "get_file_contents",
        description = "Get the content of a file at a specific Git revision"
    )]
    async fn get_file_contents(
        &self,
        Parameters(params): Parameters<FileAtRefParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_get_file_contents(params)
    }

    #[tool(
        name = "list_tags",
        description = "List all tags in the repository with their commit SHAs"
    )]
    async fn list_tags(
        &self,
        Parameters(params): Parameters<RepoParam>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_list_tags(params)
    }

    #[tool(
        name = "get_remote_info",
        description = "List configured Git remotes with their fetch and push URLs"
    )]
    async fn get_remote_info(
        &self,
        Parameters(params): Parameters<RepoParam>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_get_remote_info(params)
    }

    #[tool(
        name = "blame",
        description = "Show line-by-line authorship for a file, grouped by commit"
    )]
    async fn blame(
        &self,
        Parameters(params): Parameters<FileAtRefParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_blame(params)
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
                "Git repository server. Tools: list_repos (connected repos), log (commit history), \
                 diff (compare refs), show_commit (commit details), list_branches (branches), \
                 search_commits (search messages), status (working tree status), \
                 get_file_contents (file at revision), list_tags (tags), \
                 get_remote_info (remotes), blame (line authorship)."
                    .to_string(),
            ),
        }
    }
}
