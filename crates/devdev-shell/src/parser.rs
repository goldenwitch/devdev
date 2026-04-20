//! Token stream → AST parser.
//!
//! Consumes the flat token stream from [`crate::tokenizer`] and produces
//! a [`CommandList`] AST.

use crate::ast::{Command, CommandList, Operator, Pipeline, Redirect, RedirectKind, Word};
use crate::error::ParseError;
use crate::tokenizer::Token;

/// Parse a shell command string into an AST.
pub fn parse(input: &str) -> Result<CommandList, ParseError> {
    // Handle empty / whitespace-only input
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(CommandList {
            first: Pipeline {
                stages: Vec::new(),
            },
            rest: Vec::new(),
        });
    }

    let tokens = crate::tokenizer::tokenize(input)?;
    if tokens.is_empty() {
        return Ok(CommandList {
            first: Pipeline {
                stages: Vec::new(),
            },
            rest: Vec::new(),
        });
    }

    let mut parser = Parser {
        tokens,
        pos: 0,
    };
    parser.parse_command_list()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<Token> {
        if self.pos < self.tokens.len() {
            let tok = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(tok)
        } else {
            None
        }
    }

    fn parse_command_list(&mut self) -> Result<CommandList, ParseError> {
        let first = self.parse_pipeline()?;
        let mut rest = Vec::new();

        while let Some(op) = self.try_operator() {
            // Allow trailing operators (e.g. `cmd ;`)
            if self.pos >= self.tokens.len() {
                break;
            }
            let pipeline = self.parse_pipeline()?;
            rest.push((op, pipeline));
        }

        Ok(CommandList { first, rest })
    }

    fn try_operator(&mut self) -> Option<Operator> {
        match self.peek() {
            Some(Token::And) => {
                self.advance();
                Some(Operator::And)
            }
            Some(Token::Or) => {
                self.advance();
                Some(Operator::Or)
            }
            Some(Token::Semi) => {
                self.advance();
                Some(Operator::Semi)
            }
            _ => None,
        }
    }

    fn parse_pipeline(&mut self) -> Result<Pipeline, ParseError> {
        let mut stages = Vec::new();
        let cmd = self.parse_command()?;
        stages.push(cmd);

        while matches!(self.peek(), Some(Token::Pipe)) {
            self.advance(); // consume `|`
            let cmd = self.parse_command()?;
            stages.push(cmd);
        }

        Ok(Pipeline { stages })
    }

    fn parse_command(&mut self) -> Result<Command, ParseError> {
        let mut env_assignments = Vec::new();
        let mut redirects = Vec::new();

        // Collect leading assignments
        while let Some(Token::Assignment(_, _)) = self.peek() {
            if let Some(Token::Assignment(name, value)) = self.advance() {
                env_assignments.push((name, value));
            }
        }

        // Collect the command name
        let name = match self.peek() {
            Some(Token::Word(_)) => {
                if let Some(Token::Word(w)) = self.advance() {
                    w
                } else {
                    unreachable!()
                }
            }
            Some(tok) => {
                return Err(ParseError {
                    message: format!("devdev: expected command name, got {tok:?}"),
                    position: self.pos,
                    suggestion: None,
                });
            }
            None => {
                // Assignments without a command — synthesize an empty command
                return Ok(Command {
                    name: Word::literal(""),
                    args: Vec::new(),
                    redirects,
                    env_assignments,
                });
            }
        };

        // Detect unsupported control-flow keywords used as command names
        detect_keyword_command(&name, self.pos)?;

        // Collect args and redirects
        let mut args = Vec::new();
        loop {
            match self.peek() {
                Some(Token::Word(_)) => {
                    if let Some(Token::Word(w)) = self.advance() {
                        // Check for keywords in argument position too  
                        // (e.g. `for i in *.rs ; do echo $i ; done`)
                        detect_keyword_arg(&w, self.pos)?;
                        args.push(w);
                    }
                }
                Some(Token::RedirectOut) => {
                    self.advance();
                    let target = self.expect_word("redirect target")?;
                    redirects.push(Redirect {
                        kind: RedirectKind::Out,
                        target,
                    });
                }
                Some(Token::RedirectAppend) => {
                    self.advance();
                    let target = self.expect_word("redirect target")?;
                    redirects.push(Redirect {
                        kind: RedirectKind::Append,
                        target,
                    });
                }
                Some(Token::RedirectIn) => {
                    self.advance();
                    let target = self.expect_word("redirect target")?;
                    redirects.push(Redirect {
                        kind: RedirectKind::In,
                        target,
                    });
                }
                Some(Token::RedirectErrOut) => {
                    self.advance();
                    let target = self.expect_word("redirect target")?;
                    redirects.push(Redirect {
                        kind: RedirectKind::ErrOut,
                        target,
                    });
                }
                Some(Token::RedirectErrAppend) => {
                    self.advance();
                    let target = self.expect_word("redirect target")?;
                    redirects.push(Redirect {
                        kind: RedirectKind::ErrAppend,
                        target,
                    });
                }
                Some(Token::RedirectErrToStdout) => {
                    self.advance();
                    redirects.push(Redirect {
                        kind: RedirectKind::ErrToStdout,
                        target: Word::literal(""),
                    });
                }
                _ => break,
            }
        }

        Ok(Command {
            name,
            args,
            redirects,
            env_assignments,
        })
    }

    fn expect_word(&mut self, context: &str) -> Result<Word, ParseError> {
        match self.advance() {
            Some(Token::Word(w)) => Ok(w),
            _ => Err(ParseError {
                message: format!("devdev: expected {context}"),
                position: self.pos,
                suggestion: None,
            }),
        }
    }
}

/// Detect unsupported keywords used as the command name.
fn detect_keyword_command(name: &Word, position: usize) -> Result<(), ParseError> {
    if name.parts.len() != 1 || name.quoted {
        return Ok(());
    }
    let raw = name.to_unescaped_string();
    let checks: &[(&str, &str, Option<&str>)] = &[
        ("if", "devdev: unsupported syntax: if/then/else.", Some("use && and || operators.")),
        ("for", "devdev: unsupported syntax: for loop.", Some("use find with -exec or pipe to xargs.")),
        ("while", "devdev: unsupported syntax: while loop.", None),
        ("case", "devdev: unsupported syntax: case statement.", None),
        ("select", "devdev: unsupported syntax: select statement.", None),
        ("function", "devdev: unsupported syntax: function definition.", None),
    ];
    for (kw, msg, sug) in checks {
        if raw == *kw {
            return Err(ParseError {
                message: (*msg).into(),
                position,
                suggestion: sug.map(|s| s.into()),
            });
        }
    }

    // function-style: `name()` or `name ()`
    if raw.ends_with("()") {
        return Err(ParseError {
            message: "devdev: unsupported syntax: function definition.".into(),
            position,
            suggestion: None,
        });
    }

    Ok(())
}

/// Detect unsupported keywords in argument position (e.g. `do`, `done`, `then`).
fn detect_keyword_arg(word: &Word, position: usize) -> Result<(), ParseError> {
    if word.parts.len() != 1 || word.quoted {
        return Ok(());
    }
    let raw = word.to_unescaped_string();
    let checks: &[(&str, &str, Option<&str>)] = &[
        ("do", "devdev: unsupported syntax: for/while loop.", Some("use find with -exec or pipe to xargs.")),
        ("done", "devdev: unsupported syntax: for/while loop.", Some("use find with -exec or pipe to xargs.")),
        ("then", "devdev: unsupported syntax: if/then/else.", Some("use && and || operators.")),
        ("else", "devdev: unsupported syntax: if/then/else.", Some("use && and || operators.")),
        ("elif", "devdev: unsupported syntax: if/then/else.", Some("use && and || operators.")),
        ("fi", "devdev: unsupported syntax: if/then/else.", Some("use && and || operators.")),
    ];
    for (kw, msg, sug) in checks {
        if raw == *kw {
            return Err(ParseError {
                message: (*msg).into(),
                position,
                suggestion: sug.map(|s| s.into()),
            });
        }
    }
    Ok(())
}
