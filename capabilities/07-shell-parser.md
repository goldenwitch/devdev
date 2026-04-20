---
id: shell-parser
title: "Shell Tokenizer & AST"
status: done
type: leaf
phase: 2
crate: devdev-shell
priority: P0
depends-on: []
effort: M
---

# 07 — Shell Tokenizer & AST

Parse bash-like command strings into a structured AST. This is pure parsing — no execution, no VFS access, no tool dispatch. The parser is the agent's interface contract: it must correctly handle the shell syntax that code-editing agents actually emit.

## Scope

**In:**
- Tokenizer: split command strings into typed tokens
- AST: structured representation of commands, pipelines, command lists
- Quoting: single quotes (literal), double quotes (with `$VAR` interpolation markers), backslash escapes
- Multi-line support (backslash continuation, unclosed quotes)
- Unsupported syntax detection with helpful error messages

**Out:**
- Variable expansion (that's `09-shell-executor` at runtime — the parser just marks `$VAR` nodes)
- Glob expansion (runtime, delegates to VFS)
- Execution of any kind
- Builtins (that's `08-shell-builtins`)

## Supported Syntax

### Tokens

| Token | Examples |
|-------|---------|
| `Word` | `grep`, `-rn`, `"hello world"`, `'literal'`, `file.txt` |
| `Pipe` | `\|` |
| `RedirectOut` | `>`, `>>` |
| `RedirectIn` | `<` |
| `RedirectErr` | `2>`, `2>>`, `2>&1` |
| `Semicolon` | `;` |
| `And` | `&&` |
| `Or` | `\|\|` |
| `Assignment` | `FOO=bar` (before a command, it's env; standalone, it's export) |

### AST Nodes

```rust
/// A single command with its arguments and I/O
pub struct Command {
    pub name: Word,                    // command name (may contain $VAR)
    pub args: Vec<Word>,               // arguments
    pub redirects: Vec<Redirect>,      // I/O redirects
    pub env_assignments: Vec<(String, Word)>,  // inline FOO=bar before command
}

/// A word that may contain variable references and literal parts
pub enum WordPart {
    Literal(String),
    Variable(String),         // $VAR or ${VAR}
    LastExitCode,             // $?
    GlobPattern(String),      // unquoted text containing *, ?, [
}
pub struct Word {
    pub parts: Vec<WordPart>,
    pub quoted: bool,          // true if entire word was quoted (suppress glob)
}

/// I/O redirect
pub struct Redirect {
    pub kind: RedirectKind,    // Out, Append, In, ErrOut, ErrAppend, ErrToStdout
    pub target: Word,          // file path (for file redirects)
}

/// A pipeline: cmd1 | cmd2 | cmd3
pub struct Pipeline {
    pub stages: Vec<Command>,
}

/// A command list: pipeline1 && pipeline2 || pipeline3 ; pipeline4
pub struct CommandList {
    pub first: Pipeline,
    pub rest: Vec<(Operator, Pipeline)>,  // (&&, p2), (||, p3), (;, p4)
}

pub enum Operator {
    And,   // &&
    Or,    // ||
    Semi,  // ;
}
```

### Quoting Rules

| Syntax | Behavior |
|--------|----------|
| `'literal $VAR'` | Everything is literal, no expansion |
| `"interp $VAR"` | `$VAR` is marked for expansion, rest is literal |
| `\$VAR` | Escaped — literal `$VAR` |
| `"hello \"world\""` | Backslash escapes inside double quotes |
| Unquoted `$VAR` | Marked for expansion + word splitting |
| Unquoted `*.rs` | Marked as glob pattern |

### Unsupported Syntax

The parser must **detect** these and return a clear error — not silently misparse:

| Syntax | Error message |
|--------|--------------|
| `$(command)` | `devdev: unsupported syntax: command substitution $(). Try: run the commands separately and pipe the output.` |
| `` `command` `` | `devdev: unsupported syntax: backtick substitution. Try: run the commands separately and pipe the output.` |
| `if ... fi` | `devdev: unsupported syntax: if/then/else. Try: use && and \|\| operators.` |
| `for ... done` | `devdev: unsupported syntax: for loop. Try: use find with -exec or pipe to xargs.` |
| `while ... done` | `devdev: unsupported syntax: while loop.` |
| `<<EOF` | `devdev: unsupported syntax: here-document. Try: use echo with pipes.` |
| `arr=(a b)` | `devdev: unsupported syntax: arrays.` |
| `$(( ... ))` | `devdev: unsupported syntax: arithmetic expansion.` |
| `cmd &` | `devdev: unsupported syntax: background jobs.` |
| `function f()` | `devdev: unsupported syntax: function definition.` |

## Interface

```rust
pub fn parse(input: &str) -> Result<CommandList, ParseError>;

pub struct ParseError {
    pub message: String,       // human-readable error
    pub position: usize,       // byte offset in input
    pub suggestion: Option<String>,  // "Try: ..." hint
}
```

## Implementation Notes

- **Proper tokenizer, not regex.** Shell quoting is subtle — `"$VAR"` vs `'$VAR'` vs `\$VAR` vs `"it's"` must all work correctly. A character-by-character state machine is the right approach.
- **Two passes:** Tokenize first (character stream → token stream), then parse tokens into AST. This is simpler to debug than a single-pass parser.
- **Glob detection:** Unquoted words containing `*`, `?`, `[` are tagged as `GlobPattern` in the AST. The parser doesn't expand them — the executor does that against the VFS.
- **Variable detection:** `$` followed by a name or `{name}` creates a `Variable` word part. The parser stores the variable name; the executor resolves it.

## Files

```
crates/devdev-shell/src/tokenizer.rs   — character-by-character tokenizer
crates/devdev-shell/src/ast.rs         — AST type definitions
crates/devdev-shell/src/parser.rs      — token stream → AST
crates/devdev-shell/src/error.rs       — ParseError + suggestions
```

## Acceptance Criteria

- [ ] `parse("cat file.txt")` → Command with name `cat`, one arg `file.txt`
- [ ] `parse("grep -rn 'TODO' src/")` → single-quoted arg preserved literally
- [ ] `parse("echo \"hello $USER\"")` → Word with Literal + Variable parts
- [ ] `parse("cat f.txt | grep foo | wc -l")` → Pipeline with 3 stages
- [ ] `parse("cmd1 && cmd2 || cmd3 ; cmd4")` → CommandList with correct operators
- [ ] `parse("FOO=bar cmd args")` → Command with env_assignment `FOO=bar`
- [ ] `parse("grep foo > out.txt 2>&1")` → correct redirects
- [ ] `parse("echo *.rs")` → arg tagged as GlobPattern
- [ ] `parse("$(git rev-parse HEAD)")` → ParseError with substitution suggestion
- [ ] `parse("for i in *.rs; do echo $i; done")` → ParseError with for-loop suggestion
- [ ] `parse("echo 'it'\\''s'")` → correct single-quote escaping (concatenation)
- [ ] Multi-line: `parse("echo \\\nhello")` → treats continuation correctly
- [ ] Empty input → empty CommandList (not an error)
