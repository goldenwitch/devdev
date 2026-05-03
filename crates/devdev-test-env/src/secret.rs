//! Redacting wrapper for short-lived auth tokens (PATs, GitHub App
//! installation tokens, AAD bearer tokens).
//!
//! Holds the raw value as a `String`, but `Debug`/`Display` always
//! redact. The only way to read the value is [`Token::expose`], which
//! is intentionally verbose to make leak-introducing edits show up in
//! review.
//!
//! This is a deliberately *minimal* type — we don't need full
//! `secrecy::Secret` semantics (no zero-on-drop guarantee; the OS can
//! always page the value to disk regardless). The goal is "no
//! accidental `{:?}` leak", not memory hardening.

use std::fmt;

#[derive(Clone)]
pub struct Token(String);

impl Token {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// Borrow the raw token. The verbose name is intentional —
    /// every call site should be auditable.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Token([redacted; {} bytes])", self.0.len())
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_value() {
        let t = Token::new("ghs_supersecret");
        let s = format!("{t:?}");
        assert!(s.contains("redacted"));
        assert!(!s.contains("ghs_"));
        assert!(!s.contains("supersecret"));
    }

    #[test]
    fn display_redacts_value() {
        let t = Token::new("ghs_supersecret");
        let s = format!("{t}");
        assert_eq!(s, "[redacted]");
    }

    #[test]
    fn expose_returns_raw() {
        let t = Token::new("ghs_supersecret");
        assert_eq!(t.expose(), "ghs_supersecret");
    }
}
