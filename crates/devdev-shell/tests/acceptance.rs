//! Acceptance tests for Cap 07 — Shell Tokenizer & AST.
//!
//! Each test maps to one acceptance criterion from capabilities/07-shell-parser.md.

use devdev_shell::{parse, Operator, RedirectKind, Word, WordPart};

/// AC: `parse("cat file.txt")` → Command with name `cat`, one arg `file.txt`.
#[test]
fn simple_command() {
    let list = parse("cat file.txt").unwrap();
    assert_eq!(list.first.stages.len(), 1);
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.name, Word::literal("cat"));
    assert_eq!(cmd.args.len(), 1);
    assert_eq!(cmd.args[0], Word::literal("file.txt"));
    assert!(list.rest.is_empty());
}

/// AC: `parse("grep -rn 'TODO' src/")` → single-quoted arg preserved literally.
#[test]
fn single_quoted_literal() {
    let list = parse("grep -rn 'TODO' src/").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.name, Word::literal("grep"));
    assert_eq!(cmd.args.len(), 3);
    // 'TODO' should be quoted + literal
    let todo_arg = &cmd.args[1];
    assert!(todo_arg.quoted);
    assert_eq!(todo_arg.parts, vec![WordPart::Literal("TODO".into())]);
    // No variable expansion inside single quotes
    assert!(!todo_arg.parts.iter().any(|p| matches!(p, WordPart::Variable(_))));
}

/// AC: `parse("echo \"hello $USER\"")` → Word with Literal + Variable parts.
#[test]
fn double_quoted_interpolation() {
    let list = parse(r#"echo "hello $USER""#).unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.args.len(), 1);
    let word = &cmd.args[0];
    assert!(word.quoted);
    assert_eq!(word.parts.len(), 2);
    assert_eq!(word.parts[0], WordPart::Literal("hello ".into()));
    assert_eq!(word.parts[1], WordPart::Variable("USER".into()));
}

/// AC: `parse("cat f.txt | grep foo | wc -l")` → Pipeline with 3 stages.
#[test]
fn pipeline_three_stages() {
    let list = parse("cat f.txt | grep foo | wc -l").unwrap();
    assert_eq!(list.first.stages.len(), 3);
    assert_eq!(list.first.stages[0].name, Word::literal("cat"));
    assert_eq!(list.first.stages[1].name, Word::literal("grep"));
    assert_eq!(list.first.stages[2].name, Word::literal("wc"));
    assert!(list.rest.is_empty());
}

/// AC: `parse("cmd1 && cmd2 || cmd3 ; cmd4")` → CommandList with correct operators.
#[test]
fn command_list_operators() {
    let list = parse("cmd1 && cmd2 || cmd3 ; cmd4").unwrap();
    assert_eq!(list.first.stages[0].name, Word::literal("cmd1"));
    assert_eq!(list.rest.len(), 3);
    assert_eq!(list.rest[0].0, Operator::And);
    assert_eq!(list.rest[0].1.stages[0].name, Word::literal("cmd2"));
    assert_eq!(list.rest[1].0, Operator::Or);
    assert_eq!(list.rest[1].1.stages[0].name, Word::literal("cmd3"));
    assert_eq!(list.rest[2].0, Operator::Semi);
    assert_eq!(list.rest[2].1.stages[0].name, Word::literal("cmd4"));
}

/// AC: `parse("FOO=bar cmd args")` → Command with env_assignment `FOO=bar`.
#[test]
fn env_assignment() {
    let list = parse("FOO=bar cmd args").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.env_assignments.len(), 1);
    assert_eq!(cmd.env_assignments[0].0, "FOO");
    assert_eq!(
        cmd.env_assignments[0].1.parts,
        vec![WordPart::Literal("bar".into())]
    );
    assert_eq!(cmd.name, Word::literal("cmd"));
    assert_eq!(cmd.args.len(), 1);
    assert_eq!(cmd.args[0], Word::literal("args"));
}

/// AC: `parse("grep foo > out.txt 2>&1")` → correct redirects.
#[test]
fn redirects() {
    let list = parse("grep foo > out.txt 2>&1").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.name, Word::literal("grep"));
    assert_eq!(cmd.args.len(), 1);
    assert_eq!(cmd.args[0], Word::literal("foo"));
    assert_eq!(cmd.redirects.len(), 2);
    assert_eq!(cmd.redirects[0].kind, RedirectKind::Out);
    assert_eq!(cmd.redirects[0].target, Word::literal("out.txt"));
    assert_eq!(cmd.redirects[1].kind, RedirectKind::ErrToStdout);
}

/// AC: `parse("echo *.rs")` → arg tagged as GlobPattern.
#[test]
fn glob_pattern() {
    let list = parse("echo *.rs").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.args.len(), 1);
    let arg = &cmd.args[0];
    assert!(!arg.quoted);
    assert_eq!(arg.parts, vec![WordPart::GlobPattern("*.rs".into())]);
}

/// AC: `parse("$(git rev-parse HEAD)")` → ParseError with substitution suggestion.
#[test]
fn unsupported_command_substitution() {
    let err = parse("$(git rev-parse HEAD)").unwrap_err();
    assert!(err.message.contains("command substitution"));
    assert!(err.suggestion.is_some());
}

/// AC: `parse("for i in *.rs; do echo $i; done")` → ParseError with for-loop suggestion.
#[test]
fn unsupported_for_loop() {
    let err = parse("for i in *.rs; do echo $i; done").unwrap_err();
    assert!(err.message.contains("for loop"));
    assert!(err.suggestion.is_some());
}

/// AC: `parse("echo 'it'\\''s'")` → correct single-quote escaping (concatenation).
///
/// In bash, `'it'\''s'` is three pieces concatenated:
///   'it'  →  literal "it"
///   \'    →  literal "'"
///   's'   →  literal "s"
/// Result: "it's"
#[test]
fn single_quote_escaping_concatenation() {
    let list = parse("echo 'it'\\''s'").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.args.len(), 1);
    let full = cmd.args[0].to_unescaped_string();
    assert_eq!(full, "it's");
}

/// AC: Multi-line: `parse("echo \\\nhello")` → treats continuation correctly.
#[test]
fn multiline_continuation() {
    let list = parse("echo \\\nhello").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.name, Word::literal("echo"));
    assert_eq!(cmd.args.len(), 1);
    assert_eq!(cmd.args[0], Word::literal("hello"));
}

/// AC: Empty input → empty CommandList (not an error).
#[test]
fn empty_input() {
    let list = parse("").unwrap();
    assert!(list.first.stages.is_empty());
    assert!(list.rest.is_empty());
}

/// Whitespace-only input also produces an empty CommandList.
#[test]
fn whitespace_only() {
    let list = parse("   \t\n  ").unwrap();
    assert!(list.first.stages.is_empty());
}

// ── Additional robustness tests ─────────────────────────────────

/// Verify `if` keyword is detected and rejected with a suggestion.
#[test]
fn unsupported_if() {
    let err = parse("if true; then echo yes; fi").unwrap_err();
    assert!(err.message.contains("if/then/else"));
}

/// Verify `while` keyword is detected and rejected.
#[test]
fn unsupported_while() {
    let err = parse("while true; do echo loop; done").unwrap_err();
    assert!(err.message.contains("while loop"));
}

/// Verify here-document `<<` is detected and rejected.
#[test]
fn unsupported_heredoc() {
    let err = parse("cat <<EOF").unwrap_err();
    assert!(err.message.contains("here-document"));
}

/// Verify background `&` is detected and rejected.
#[test]
fn unsupported_background() {
    let err = parse("sleep 10 &").unwrap_err();
    assert!(err.message.contains("background jobs"));
}

/// Verify `$(( ))` arithmetic is detected and rejected.
#[test]
fn unsupported_arithmetic() {
    let err = parse("echo $(( 1 + 2 ))").unwrap_err();
    assert!(err.message.contains("arithmetic"));
}

/// Verify backtick substitution is detected and rejected.
#[test]
fn unsupported_backtick() {
    let err = parse("echo `date`").unwrap_err();
    assert!(err.message.contains("backtick substitution"));
}

/// Verify function definition keyword is detected and rejected.
#[test]
fn unsupported_function() {
    let err = parse("function foo { echo bar; }").unwrap_err();
    assert!(err.message.contains("function definition"));
}

/// Verify $? is parsed as LastExitCode.
#[test]
fn last_exit_code() {
    let list = parse("echo $?").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.args[0].parts, vec![WordPart::LastExitCode]);
}

/// Verify ${VAR} braced variable syntax.
#[test]
fn braced_variable() {
    let list = parse("echo ${HOME}").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.args[0].parts, vec![WordPart::Variable("HOME".into())]);
}

/// Verify redirect append `>>`.
#[test]
fn redirect_append() {
    let list = parse("echo hello >> out.log").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.redirects.len(), 1);
    assert_eq!(cmd.redirects[0].kind, RedirectKind::Append);
}

/// Verify input redirect `<`.
#[test]
fn redirect_input() {
    let list = parse("sort < data.txt").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.redirects.len(), 1);
    assert_eq!(cmd.redirects[0].kind, RedirectKind::In);
}

/// Verify stderr redirect `2>`.
#[test]
fn redirect_stderr() {
    let list = parse("cmd 2> err.log").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.redirects.len(), 1);
    assert_eq!(cmd.redirects[0].kind, RedirectKind::ErrOut);
}

/// Verify stderr append `2>>`.
#[test]
fn redirect_stderr_append() {
    let list = parse("cmd 2>> err.log").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.redirects.len(), 1);
    assert_eq!(cmd.redirects[0].kind, RedirectKind::ErrAppend);
}

/// Verify escaped dollar sign is treated as literal text, not a variable.
#[test]
fn escaped_dollar() {
    let list = parse(r"echo \$HOME").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.args[0], Word::literal("$HOME"));
}

/// Verify trailing semicolon doesn't cause an error.
#[test]
fn trailing_semicolon() {
    let list = parse("echo hello ;").unwrap();
    assert_eq!(list.first.stages[0].name, Word::literal("echo"));
    // Trailing ; produces no second pipeline
    assert!(list.rest.is_empty());
}

/// Verify multiple env assignments before a command.
#[test]
fn multiple_assignments() {
    let list = parse("A=1 B=2 cmd").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.env_assignments.len(), 2);
    assert_eq!(cmd.env_assignments[0].0, "A");
    assert_eq!(cmd.env_assignments[1].0, "B");
    assert_eq!(cmd.name, Word::literal("cmd"));
}

/// Verify comment lines are ignored (# to end of line).
#[test]
fn comment_ignored() {
    let list = parse("echo hello # this is a comment").unwrap();
    let cmd = &list.first.stages[0];
    assert_eq!(cmd.args.len(), 1);
    assert_eq!(cmd.args[0], Word::literal("hello"));
}
