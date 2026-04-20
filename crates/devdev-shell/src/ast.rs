//! AST type definitions for the shell parser.

/// A word that may contain variable references, globs, and literal parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub parts: Vec<WordPart>,
    /// True if the entire word was inside quotes (suppresses glob expansion).
    pub quoted: bool,
}

impl Word {
    /// Create a simple literal word.
    pub fn literal(s: &str) -> Self {
        Self {
            parts: vec![WordPart::Literal(s.to_owned())],
            quoted: false,
        }
    }

    /// Concatenate all parts into a display string (for names/debugging).
    pub fn to_unescaped_string(&self) -> String {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                WordPart::Literal(s) => out.push_str(s),
                WordPart::Variable(v) => {
                    out.push('$');
                    out.push_str(v);
                }
                WordPart::LastExitCode => out.push_str("$?"),
                WordPart::GlobPattern(g) => out.push_str(g),
            }
        }
        out
    }
}

/// A fragment of a word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WordPart {
    /// Literal text.
    Literal(String),
    /// A variable reference: `$VAR` or `${VAR}`.
    Variable(String),
    /// `$?` — last exit code.
    LastExitCode,
    /// An unquoted glob pattern containing `*`, `?`, or `[`.
    GlobPattern(String),
}

/// An I/O redirect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub kind: RedirectKind,
    pub target: Word,
}

/// The kind of redirect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectKind {
    /// `>`
    Out,
    /// `>>`
    Append,
    /// `<`
    In,
    /// `2>`
    ErrOut,
    /// `2>>`
    ErrAppend,
    /// `2>&1`
    ErrToStdout,
}

/// A single command with its arguments and I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub name: Word,
    pub args: Vec<Word>,
    pub redirects: Vec<Redirect>,
    pub env_assignments: Vec<(String, Word)>,
}

/// A pipeline: `cmd1 | cmd2 | cmd3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    pub stages: Vec<Command>,
}

/// A list operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// `&&`
    And,
    /// `||`
    Or,
    /// `;`
    Semi,
}

/// A command list: `pipeline1 && pipeline2 || pipeline3 ; pipeline4`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandList {
    pub first: Pipeline,
    pub rest: Vec<(Operator, Pipeline)>,
}
