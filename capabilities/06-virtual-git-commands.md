---
id: virtual-git-commands
title: "Git Subcommand Implementations"
status: done
type: leaf
phase: 2
crate: devdev-git
priority: P0
depends-on: [virtual-git-core]
effort: L
---

# 06 — Git Subcommand Implementations

Implement the git subcommands the agent actually uses: diff, log, status, show, blame, branch, etc. Each command parses the agent's arguments, calls libgit2 APIs on the in-memory repository, and formats output to match real `git` CLI defaults.

## Scope

**In:**
- P0 commands: `diff`, `log`, `status`, `show`, `blame`
- P1 commands: `diff --stat`, `log --graph`, `rev-parse`, `branch`, `tag`, `ls-files`
- Argument parsing for each subcommand (common flags only)
- Output formatting matching real git defaults
- Unsupported subcommand error handling

**Out:**
- Mutating commands (`push`, `pull`, `merge`, `rebase`, `commit`, `add`)
- Network operations
- The in-memory Odb setup (that's `05-virtual-git-core`)

## Interface

```rust
/// Same shape as ToolEngine — stdout, stderr, exit code.
pub trait VirtualGit: Send + Sync {
    fn execute(
        &self,
        args: &[String],           // e.g., ["log", "--oneline", "-10"]
        cwd: &str,                 // working directory in VFS
        vfs: &dyn VirtualFilesystem,
    ) -> GitResult;
}

pub struct GitResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}
```

## Command Specifications

### `git diff` (P0)

| Flag | Behavior |
|------|----------|
| *(no args)* | Diff working tree vs index (VFS files vs last commit) |
| `HEAD~N..HEAD` | Diff between commits |
| `-- path` | Restrict to specific path |
| `--stat` | Summary only (P1) |
| `--name-only` | List changed file names |
| `--cached` | Diff index vs HEAD |

Implementation: `repo.diff_tree_to_tree()` for commit ranges, `repo.diff_index_to_workdir()` for working tree. Format as unified diff.

### `git log` (P0)

| Flag | Behavior |
|------|----------|
| `--oneline` | Short format: `<sha7> <subject>` |
| `--format=<fmt>` | Custom format (at least `%H`, `%h`, `%s`, `%an`, `%ae`, `%ad`) |
| `-n <N>` / `-<N>` | Limit to N commits |
| `--author=<pat>` | Filter by author |
| `--since=<date>` | Filter by date |
| `--follow` | Follow file renames |
| `--graph` | ASCII commit graph (P1) |
| `-- <path>` | File-scoped history |

Implementation: `repo.revwalk()` with filters. Format each commit.

### `git status` (P0)

Compare VFS working tree against git index. This is ~200 lines of custom code:

1. Read the git index from the loaded repo.
2. Walk the VFS working tree.
3. For each file: compare VFS content hash against index entry hash.
4. Classify as: modified, deleted, new (untracked), unchanged.
5. Format output matching `git status` default (long format) and `--short` flag.

### `git show <ref>` (P0)

Implementation: `repo.revparse_single(ref)` → `find_commit()` → print commit metadata + diff.

### `git blame <file>` (P0)

Implementation: `repo.blame_file(path, None)` → format each line with commit SHA, author, date, line number.

### `git rev-parse <ref>` (P1)

Implementation: `repo.revparse_single(ref)` → print full SHA.

### `git branch` (P1)

Implementation: `repo.branches(Some(BranchType::Local))` → list names. Current branch marked with `*`.

### `git tag` (P1)

Implementation: `repo.tag_names(None)` → list names.

### `git ls-files` (P1)

Implementation: Walk the git index → list tracked file paths.

### Unsupported Commands

```rust
const BLOCKED: &[&str] = &[
    "push", "pull", "fetch", "merge", "rebase", 
    "cherry-pick", "commit", "add", "reset", "stash",
    "checkout", "switch", "clone", "init",
];

// → stderr: "devdev: git push is not available in the virtual workspace."
// → exit_code: 1
```

## Output Format Fidelity

**This is critical.** The agent parses git output. If our format diverges, the agent gets confused.

Strategy:
1. For each command, run real `git` on a test repo and capture the exact output.
2. Write tests that compare our output to real git's output.
3. Match format precisely: field widths, colors (disabled by default — match `--no-color`), date formats, SHA prefix lengths.

Colors: Default to no color (`--no-color` equivalent). The agent doesn't use terminal colors.

## Files

```
crates/devdev-git/src/commands/mod.rs      — VirtualGit trait, VirtualGitRepo, BLOCKED guard, subcommand dispatch
crates/devdev-git/src/commands/diff.rs     — git diff
crates/devdev-git/src/commands/log.rs      — git log (inlines format_time helper)
crates/devdev-git/src/commands/status.rs   — git status (custom tree-walk)
crates/devdev-git/src/commands/show.rs     — git show
crates/devdev-git/src/commands/blame.rs    — git blame
crates/devdev-git/src/commands/branch.rs   — git branch
crates/devdev-git/src/commands/tag.rs      — git tag
crates/devdev-git/src/commands/rev_parse.rs — git rev-parse
crates/devdev-git/src/commands/ls_files.rs — git ls-files
```

Formatting and arg-parsing helpers are inlined per-subcommand module rather than extracted into shared `format.rs` / `args.rs` — each command's surface is small enough that centralisation would add indirection without deduplication.

## Acceptance Criteria

- [ ] `git log --oneline -5` output matches real git on test repo (byte-for-byte comparison)
- [ ] `git diff HEAD~1..HEAD` produces valid unified diff
- [ ] `git status` shows VFS-modified files as "modified"
- [ ] `git blame src/main.rs` attributes each line to a commit
- [ ] `git show HEAD` displays commit metadata + diff
- [ ] `git rev-parse HEAD` returns full 40-char SHA
- [ ] `git branch` lists branches with `*` on current
- [ ] `git push` → `"devdev: git push is not available in the virtual workspace."`, exit code 1
- [ ] `git log --author="name"` filters correctly
- [ ] `git diff --stat` produces file change summary
- [ ] Unknown flag → stderr error (not crash)
