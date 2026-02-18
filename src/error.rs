use rmcp::model::ErrorData;

#[derive(Debug, thiserror::Error)]
pub enum McpGitError {
    #[error("Git error: {0}")]
    Git(String),

    #[error("Repository not found: {0}")]
    RepoNotFound(String),

    #[error("Ambiguous repository: multiple repos connected, specify the 'repo' parameter")]
    AmbiguousRepo,

    #[error("Invalid ref: {0}")]
    InvalidRef(String),

    #[error("{0}")]
    Other(String),
}

impl McpGitError {
    pub fn to_mcp_error(&self) -> ErrorData {
        match self {
            McpGitError::RepoNotFound(_) | McpGitError::AmbiguousRepo => {
                ErrorData::invalid_params(self.to_string(), None)
            }
            McpGitError::InvalidRef(_) => ErrorData::invalid_params(self.to_string(), None),
            McpGitError::Git(_) | McpGitError::Other(_) => {
                ErrorData::internal_error(self.to_string(), None)
            }
        }
    }
}
