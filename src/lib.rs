//! MCP server that lets LLMs explore and search Git repositories.
//!
//! Provides tools for listing repos, browsing commit logs, viewing diffs,
//! searching commit messages, and inspecting branches — all read-only.

pub mod error;
pub mod server;
