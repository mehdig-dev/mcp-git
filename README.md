# mcp-git

MCP server that lets LLMs explore Git repositories — commit history, diffs, branches, and search. Single binary, read-only.

## Install

```bash
cargo install mcp-git
```

## Usage

```bash
# Current directory
mcp-git

# Specific repo
mcp-git --repo /path/to/repo

# Multiple repos
mcp-git --repo /path/to/repo1 --repo /path/to/repo2

# Custom limits
mcp-git --repo . --max-log-entries 100 --max-diff-lines 1000
```

## Configuration

### Claude Code

```bash
claude mcp add git -- mcp-git --repo /path/to/your/repo
```

### Claude Desktop

```json
{
  "mcpServers": {
    "git": {
      "command": "mcp-git",
      "args": ["--repo", "/path/to/your/repo"]
    }
  }
}
```

### Cursor / VS Code

```json
{
  "mcpServers": {
    "git": {
      "command": "mcp-git",
      "args": ["--repo", "/path/to/your/repo"]
    }
  }
}
```

## Tools

| Tool | Description |
|------|-------------|
| `list_repos` | Show all connected repositories with current branch |
| `log` | Show commit history with author, date, and message |
| `diff` | Show changes between two refs (commits, branches, tags) |
| `show_commit` | Show details of a specific commit |
| `list_branches` | List local and remote branches |
| `search_commits` | Search commit messages for a query string |

All tools accept an optional `repo` parameter when multiple repositories are connected.

## CLI Options

| Flag | Default | Description |
|------|---------|-------------|
| `--repo` | `.` | Path to a Git repository (repeatable) |
| `--max-diff-lines` | `500` | Maximum diff output lines |
| `--max-log-entries` | `50` | Maximum log entries returned |

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.
