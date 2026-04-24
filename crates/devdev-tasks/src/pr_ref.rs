//! Parse PR references from strings.
//!
//! Supports: "owner/repo#123", "https://github.com/owner/repo/pull/123"

use crate::task::TaskError;

/// Parsed PR reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl PrRef {
    /// Parse a PR reference from shorthand ("owner/repo#123") or URL.
    pub fn parse(input: &str) -> Result<Self, TaskError> {
        let input = input.trim();

        // Try URL: https://github.com/owner/repo/pull/123
        if input.starts_with("https://") || input.starts_with("http://") {
            return Self::parse_url(input);
        }

        // Try shorthand: owner/repo#123
        Self::parse_shorthand(input)
    }

    fn parse_shorthand(input: &str) -> Result<Self, TaskError> {
        let Some((repo_part, number_str)) = input.split_once('#') else {
            return Err(TaskError::PollFailed(format!(
                "invalid PR reference: {input} (expected owner/repo#number)"
            )));
        };

        let Some((owner, repo)) = repo_part.split_once('/') else {
            return Err(TaskError::PollFailed(format!(
                "invalid PR reference: {input} (expected owner/repo#number)"
            )));
        };

        let number: u64 = number_str
            .parse()
            .map_err(|_| TaskError::PollFailed(format!("invalid PR number: {number_str}")))?;

        if owner.is_empty() || repo.is_empty() || number == 0 {
            return Err(TaskError::PollFailed(format!(
                "invalid PR reference: {input}"
            )));
        }

        Ok(Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            number,
        })
    }

    fn parse_url(input: &str) -> Result<Self, TaskError> {
        // Remove scheme.
        let path = input
            .strip_prefix("https://github.com/")
            .or_else(|| input.strip_prefix("http://github.com/"))
            .ok_or_else(|| TaskError::PollFailed(format!("unsupported URL host: {input}")))?;

        // Expected: owner/repo/pull/123
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() < 4 || parts[2] != "pull" {
            return Err(TaskError::PollFailed(format!(
                "invalid PR URL: {input} (expected .../owner/repo/pull/number)"
            )));
        }

        let owner = parts[0];
        let repo = parts[1];
        let number: u64 = parts[3].parse().map_err(|_| {
            TaskError::PollFailed(format!("invalid PR number in URL: {}", parts[3]))
        })?;

        if owner.is_empty() || repo.is_empty() || number == 0 {
            return Err(TaskError::PollFailed(format!("invalid PR URL: {input}")));
        }

        Ok(Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            number,
        })
    }
}

impl std::fmt::Display for PrRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}#{}", self.owner, self.repo, self.number)
    }
}
