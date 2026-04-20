//! Character-by-character shell tokenizer.
//!
//! Transforms a raw command string into a stream of [`Token`]s respecting
//! shell quoting rules (single-quote literal, double-quote with `$VAR`
//! interpolation, backslash escapes, and glob detection).

use crate::ast::{Word, WordPart};
use crate::error::ParseError;

/// A token produced by the tokenizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// A word (command name, argument, file path, etc.).
    Word(Word),
    /// `|`
    Pipe,
    /// `>`
    RedirectOut,
    /// `>>`
    RedirectAppend,
    /// `<`
    RedirectIn,
    /// `2>`
    RedirectErrOut,
    /// `2>>`
    RedirectErrAppend,
    /// `2>&1`
    RedirectErrToStdout,
    /// `&&`
    And,
    /// `||`
    Or,
    /// `;`
    Semi,
    /// An inline environment assignment like `FOO=bar` (only before a command).
    Assignment(String, Word),
}

/// Tokenize a shell command string.
pub fn tokenize(input: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;

    while pos < chars.len() {
        // Skip whitespace
        if chars[pos].is_ascii_whitespace() {
            pos += 1;
            continue;
        }

        // Comments: # to end of line
        if chars[pos] == '#' {
            break;
        }

        let start = pos;

        // Two-character operators first
        if pos + 1 < chars.len() {
            let two = (chars[pos], chars[pos + 1]);
            match two {
                ('&', '&') => {
                    tokens.push(Token::And);
                    pos += 2;
                    continue;
                }
                ('|', '|') => {
                    tokens.push(Token::Or);
                    pos += 2;
                    continue;
                }
                ('>', '>') => {
                    tokens.push(Token::RedirectAppend);
                    pos += 2;
                    continue;
                }
                ('2', '>') => {
                    pos += 2;
                    if pos < chars.len() && chars[pos] == '>' {
                        tokens.push(Token::RedirectErrAppend);
                        pos += 1;
                    } else if pos + 1 < chars.len() && chars[pos] == '&' && chars[pos + 1] == '1'
                    {
                        tokens.push(Token::RedirectErrToStdout);
                        pos += 2;
                    } else {
                        tokens.push(Token::RedirectErrOut);
                    }
                    continue;
                }
                ('<', '<') => {
                    return Err(ParseError {
                        message: "devdev: unsupported syntax: here-document.".into(),
                        position: start,
                        suggestion: Some("use echo with pipes.".into()),
                    });
                }
                _ => {}
            }
        }

        // Single-character operators
        match chars[pos] {
            '|' => {
                tokens.push(Token::Pipe);
                pos += 1;
                continue;
            }
            ';' => {
                tokens.push(Token::Semi);
                pos += 1;
                continue;
            }
            '>' => {
                tokens.push(Token::RedirectOut);
                pos += 1;
                continue;
            }
            '<' => {
                tokens.push(Token::RedirectIn);
                pos += 1;
                continue;
            }
            '&' => {
                return Err(ParseError {
                    message: "devdev: unsupported syntax: background jobs.".into(),
                    position: start,
                    suggestion: None,
                });
            }
            _ => {}
        }

        // Word (possibly quoted, with variable interpolation, globs)
        let (word, new_pos) = tokenize_word(&chars, pos)?;
        pos = new_pos;

        // Check if this is an assignment: unquoted WORD containing `=` before any command on this line
        // An assignment is NAME=VALUE where NAME is [A-Za-z_][A-Za-z0-9_]*
        let raw = word.to_unescaped_string();
        if !word.quoted
            && word.parts.len() == 1
            && let WordPart::Literal(ref lit) = word.parts[0]
            && let Some(eq_pos) = lit.find('=')
        {
            let name = &lit[..eq_pos];
            if is_valid_var_name(name) {
                let value_str = &lit[eq_pos + 1..];
                let value = Word {
                    parts: vec![WordPart::Literal(value_str.to_owned())],
                    quoted: false,
                };
                tokens.push(Token::Assignment(name.to_owned(), value));
                continue;
            }
        }

        // Detect unsupported syntax in the raw text
        detect_unsupported(&raw, start)?;

        tokens.push(Token::Word(word));
    }

    Ok(tokens)
}

/// Tokenize a single word starting at `pos`, handling quoting and escaping.
/// Returns the parsed Word and the new position after the word.
fn tokenize_word(chars: &[char], mut pos: usize) -> Result<(Word, usize), ParseError> {
    let mut parts: Vec<WordPart> = Vec::new();
    let mut current_literal = String::new();
    let mut any_quoted = false;
    let mut has_glob_chars = false;

    while pos < chars.len() {
        let ch = chars[pos];

        // Unquoted whitespace or operator chars terminate the word
        if ch.is_ascii_whitespace()
            || ch == '|'
            || ch == ';'
            || ch == '<'
            || ch == '>'
        {
            break;
        }
        // `&` terminates (it's either && or background)
        if ch == '&' {
            break;
        }
        // `2>` redirect: if current_literal is empty and we see `2>`, it's a redirect
        // This is handled at the tokenize level, not here.

        match ch {
            '\'' => {
                // Single-quote: everything until the next `'` is literal.
                any_quoted = true;
                pos += 1;
                while pos < chars.len() && chars[pos] != '\'' {
                    current_literal.push(chars[pos]);
                    pos += 1;
                }
                if pos >= chars.len() {
                    return Err(ParseError {
                        message: "devdev: unterminated single quote.".into(),
                        position: pos,
                        suggestion: None,
                    });
                }
                pos += 1; // skip closing '
            }
            '"' => {
                // Double-quote: literal with $VAR interpolation.
                any_quoted = true;
                pos += 1;
                while pos < chars.len() && chars[pos] != '"' {
                    if chars[pos] == '\\' && pos + 1 < chars.len() {
                        let next = chars[pos + 1];
                        match next {
                            '"' | '\\' | '$' | '`' => {
                                current_literal.push(next);
                                pos += 2;
                                continue;
                            }
                            _ => {
                                // Backslash is literal if next char isn't special
                                current_literal.push('\\');
                                current_literal.push(next);
                                pos += 2;
                                continue;
                            }
                        }
                    }
                    if chars[pos] == '$' {
                        // Flush current literal
                        if !current_literal.is_empty() {
                            parts.push(WordPart::Literal(current_literal.clone()));
                            current_literal.clear();
                        }
                        let (var_part, new_pos) = parse_variable(chars, pos)?;
                        parts.push(var_part);
                        pos = new_pos;
                        continue;
                    }
                    if chars[pos] == '`' {
                        return Err(ParseError {
                            message:
                                "devdev: unsupported syntax: backtick substitution.".into(),
                            position: pos,
                            suggestion: Some(
                                "run the commands separately and pipe the output.".into(),
                            ),
                        });
                    }
                    current_literal.push(chars[pos]);
                    pos += 1;
                }
                if pos >= chars.len() {
                    return Err(ParseError {
                        message: "devdev: unterminated double quote.".into(),
                        position: pos,
                        suggestion: None,
                    });
                }
                pos += 1; // skip closing "
            }
            '\\' => {
                // Backslash escape (outside quotes)
                if pos + 1 < chars.len() {
                    if chars[pos + 1] == '\n' {
                        // Line continuation — skip the backslash-newline
                        pos += 2;
                    } else {
                        current_literal.push(chars[pos + 1]);
                        pos += 2;
                    }
                } else {
                    // Trailing backslash
                    current_literal.push('\\');
                    pos += 1;
                }
            }
            '$' => {
                // Variable reference (unquoted)
                if !current_literal.is_empty() {
                    parts.push(WordPart::Literal(current_literal.clone()));
                    current_literal.clear();
                }
                let (var_part, new_pos) = parse_variable(chars, pos)?;
                parts.push(var_part);
                pos = new_pos;
            }
            '`' => {
                return Err(ParseError {
                    message: "devdev: unsupported syntax: backtick substitution.".into(),
                    position: pos,
                    suggestion: Some(
                        "run the commands separately and pipe the output.".into(),
                    ),
                });
            }
            '*' | '?' | '[' if !any_quoted => {
                // Glob character in unquoted context
                has_glob_chars = true;
                current_literal.push(ch);
                pos += 1;
            }
            _ => {
                current_literal.push(ch);
                pos += 1;
            }
        }
    }

    // Flush remaining literal
    if !current_literal.is_empty() {
        if has_glob_chars && !any_quoted {
            parts.push(WordPart::GlobPattern(current_literal));
        } else {
            parts.push(WordPart::Literal(current_literal));
        }
    }

    let word = Word {
        parts,
        quoted: any_quoted,
    };
    Ok((word, pos))
}

/// Parse a `$VAR`, `${VAR}`, `$?`, or `$(...)` starting at position `pos`.
fn parse_variable(chars: &[char], pos: usize) -> Result<(WordPart, usize), ParseError> {
    debug_assert!(chars[pos] == '$');
    let mut i = pos + 1;

    if i >= chars.len() {
        // Bare `$` at end of input — treat as literal
        return Ok((WordPart::Literal("$".into()), i));
    }

    // $?
    if chars[i] == '?' {
        return Ok((WordPart::LastExitCode, i + 1));
    }

    // $(...)  — unsupported command substitution
    if chars[i] == '(' {
        // Check for $(( )) arithmetic
        if i + 1 < chars.len() && chars[i + 1] == '(' {
            return Err(ParseError {
                message: "devdev: unsupported syntax: arithmetic expansion.".into(),
                position: pos,
                suggestion: None,
            });
        }
        return Err(ParseError {
            message: "devdev: unsupported syntax: command substitution $().".into(),
            position: pos,
            suggestion: Some("run the commands separately and pipe the output.".into()),
        });
    }

    // ${VAR}
    if chars[i] == '{' {
        i += 1;
        let start = i;
        while i < chars.len() && chars[i] != '}' {
            i += 1;
        }
        if i >= chars.len() {
            return Err(ParseError {
                message: "devdev: unterminated variable reference ${...".into(),
                position: pos,
                suggestion: None,
            });
        }
        let name: String = chars[start..i].iter().collect();
        return Ok((WordPart::Variable(name), i + 1));
    }

    // $VAR — collect [A-Za-z_][A-Za-z0-9_]*
    let start = i;
    if i < chars.len() && (chars[i].is_ascii_alphabetic() || chars[i] == '_') {
        i += 1;
        while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
            i += 1;
        }
        let name: String = chars[start..i].iter().collect();
        return Ok((WordPart::Variable(name), i));
    }

    // Bare `$` followed by something unexpected — treat as literal
    Ok((WordPart::Literal("$".into()), i))
}

fn is_valid_var_name(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Detect unsupported syntax patterns that weren't caught during tokenization.
fn detect_unsupported(raw: &str, position: usize) -> Result<(), ParseError> {
    // Keywords that start control flow
    let keyword_checks = [
        ("if", "devdev: unsupported syntax: if/then/else.", Some("use && and || operators.")),
        ("then", "devdev: unsupported syntax: if/then/else.", Some("use && and || operators.")),
        ("else", "devdev: unsupported syntax: if/then/else.", Some("use && and || operators.")),
        ("fi", "devdev: unsupported syntax: if/then/else.", Some("use && and || operators.")),
        ("for", "devdev: unsupported syntax: for loop.", Some("use find with -exec or pipe to xargs.")),
        ("while", "devdev: unsupported syntax: while loop.", None),
        ("do", "devdev: unsupported syntax: for/while loop.", Some("use find with -exec or pipe to xargs.")),
        ("done", "devdev: unsupported syntax: for/while loop.", Some("use find with -exec or pipe to xargs.")),
        ("function", "devdev: unsupported syntax: function definition.", None),
    ];

    for (keyword, message, suggestion) in &keyword_checks {
        if raw == *keyword {
            return Err(ParseError {
                message: (*message).into(),
                position,
                suggestion: suggestion.map(|s| s.into()),
            });
        }
    }

    // Array syntax: VAR=(...)
    if raw.contains("=(") {
        return Err(ParseError {
            message: "devdev: unsupported syntax: arrays.".into(),
            position,
            suggestion: None,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify simple unquoted words tokenize as Word tokens with literal parts.
    #[test]
    fn simple_command() {
        let tokens = tokenize("cat file.txt").unwrap();
        assert_eq!(tokens.len(), 2);
        assert_eq!(
            tokens[0],
            Token::Word(Word::literal("cat"))
        );
        assert_eq!(
            tokens[1],
            Token::Word(Word::literal("file.txt"))
        );
    }

    /// Verify single-quoted text produces a single literal word part with
    /// the quoted flag set, preserving special chars literally.
    #[test]
    fn single_quoted() {
        let tokens = tokenize("echo 'hello $USER'").unwrap();
        assert_eq!(tokens.len(), 2);
        if let Token::Word(ref w) = tokens[1] {
            assert!(w.quoted);
            assert_eq!(w.parts, vec![WordPart::Literal("hello $USER".into())]);
        } else {
            panic!("expected Word token");
        }
    }

    /// Verify double-quoted text splits $VAR into a Variable word part
    /// while keeping surrounding text as Literal parts.
    #[test]
    fn double_quoted_interpolation() {
        let tokens = tokenize(r#"echo "hello $USER""#).unwrap();
        assert_eq!(tokens.len(), 2);
        if let Token::Word(ref w) = tokens[1] {
            assert!(w.quoted);
            assert_eq!(w.parts.len(), 2);
            assert_eq!(w.parts[0], WordPart::Literal("hello ".into()));
            assert_eq!(w.parts[1], WordPart::Variable("USER".into()));
        } else {
            panic!("expected Word token");
        }
    }

    /// Verify pipe operator tokenizes as Pipe.
    #[test]
    fn pipe_token() {
        let tokens = tokenize("a | b").unwrap();
        assert_eq!(tokens, vec![
            Token::Word(Word::literal("a")),
            Token::Pipe,
            Token::Word(Word::literal("b")),
        ]);
    }

    /// Verify && and || tokenize as And and Or operators.
    #[test]
    fn and_or_operators() {
        let tokens = tokenize("a && b || c").unwrap();
        assert!(matches!(tokens[1], Token::And));
        assert!(matches!(tokens[3], Token::Or));
    }

    /// Verify redirect tokens are correctly identified.
    #[test]
    fn redirects() {
        let tokens = tokenize("cmd > out.txt 2>&1").unwrap();
        assert!(matches!(tokens[1], Token::RedirectOut));
        assert!(matches!(tokens[3], Token::RedirectErrToStdout));
    }

    /// Verify FOO=bar tokenizes as Assignment.
    #[test]
    fn assignment() {
        let tokens = tokenize("FOO=bar cmd").unwrap();
        assert!(matches!(tokens[0], Token::Assignment(ref name, _) if name == "FOO"));
        assert_eq!(tokens[1], Token::Word(Word::literal("cmd")));
    }

    /// Verify $( ) triggers the command substitution error.
    #[test]
    fn unsupported_command_substitution() {
        let err = tokenize("echo $(git rev-parse HEAD)").unwrap_err();
        assert!(err.message.contains("command substitution"));
    }

    /// Verify backtick triggers the backtick substitution error.
    #[test]
    fn unsupported_backtick() {
        let err = tokenize("echo `date`").unwrap_err();
        assert!(err.message.contains("backtick substitution"));
    }

    /// Verify unquoted glob chars produce a GlobPattern word part.
    #[test]
    fn glob_detection() {
        let tokens = tokenize("echo *.rs").unwrap();
        if let Token::Word(ref w) = tokens[1] {
            assert!(!w.quoted);
            assert_eq!(w.parts, vec![WordPart::GlobPattern("*.rs".into())]);
        } else {
            panic!("expected Word token");
        }
    }

    /// Verify backslash-newline is treated as line continuation, not a literal.
    #[test]
    fn line_continuation() {
        let tokens = tokenize("echo \\\nhello").unwrap();
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[1], Token::Word(Word::literal("hello")));
    }

    /// Verify `2>>` tokenizes as stderr append redirect.
    #[test]
    fn stderr_append_redirect() {
        let tokens = tokenize("cmd 2>> err.log").unwrap();
        assert!(matches!(tokens[1], Token::RedirectErrAppend));
    }

    /// Verify `>>` tokenizes as stdout append redirect.
    #[test]
    fn stdout_append_redirect() {
        let tokens = tokenize("cmd >> out.log").unwrap();
        assert!(matches!(tokens[1], Token::RedirectAppend));
    }

    /// Verify $? tokenizes as LastExitCode word part.
    #[test]
    fn last_exit_code() {
        let tokens = tokenize("echo $?").unwrap();
        if let Token::Word(ref w) = tokens[1] {
            assert_eq!(w.parts, vec![WordPart::LastExitCode]);
        } else {
            panic!("expected Word token");
        }
    }

    /// Verify ${VAR} braced variable syntax.
    #[test]
    fn braced_variable() {
        let tokens = tokenize("echo ${HOME}").unwrap();
        if let Token::Word(ref w) = tokens[1] {
            assert_eq!(w.parts, vec![WordPart::Variable("HOME".into())]);
        } else {
            panic!("expected Word token");
        }
    }

    /// Verify here-doc `<<` is detected as unsupported.
    #[test]
    fn unsupported_heredoc() {
        let err = tokenize("cat <<EOF").unwrap_err();
        assert!(err.message.contains("here-document"));
    }

    /// Verify background `&` (not `&&`) is detected as unsupported.
    #[test]
    fn unsupported_background() {
        let err = tokenize("sleep 10 &").unwrap_err();
        assert!(err.message.contains("background jobs"));
    }

    /// Verify `$(( ))` arithmetic is detected as unsupported.
    #[test]
    fn unsupported_arithmetic() {
        let err = tokenize("echo $(( 1 + 2 ))").unwrap_err();
        assert!(err.message.contains("arithmetic expansion"));
    }
}
