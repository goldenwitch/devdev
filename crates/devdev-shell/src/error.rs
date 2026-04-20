//! Parse error types with suggestions.

/// An error produced by the shell parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// Human-readable error message.
    pub message: String,
    /// Byte offset in the input where the error was detected.
    pub position: usize,
    /// A "Try: ..." hint for the user/agent.
    pub suggestion: Option<String>,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(ref s) = self.suggestion {
            write!(f, " Try: {s}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ParseError {}
