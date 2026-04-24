# Spec: Virtual Git

> **⚠️ HISTORICAL — describes the pre-Phase-3 architecture.** This spec describes an in-memory libgit2 (mempack-backed) git layer that was deleted during the Phase 3 consolidation (2026-04-22). The current implementation invokes the host's real `git` binary inside the FUSE/WinFSP mount via `Workspace::exec`. Retained for design-history context; **do not use as a spec for current or future work.**

**Status:** Historical — superseded by host-`git`-in-mount.
**Original status:** Draft — Updated with research findings (April 2026)
**Depends on:** Virtual Filesystem (spec-virtual-filesystem.md)

---

## Purpose

Provide git operations (diff, log, status, blame, etc.) as first-class virtual commands inside the sandbox. The agent frequently uses git to understand code context. Rather than compiling the entire git binary to WASM (impractical), DevDev implements git operations natively using a git library operating directly on the in-memory VFS.

---

## Requirements

### Supported Commands

The following git subcommands must be available to the agent. They should behave identically to their real `git` counterparts in output format and flag support (at least for commonly-used flags).

**Priority 0 (essential for code review workflows):**
- `git diff` — show changes between commits, working tree, etc.
- `git log` — commit history with standard format options (`--oneline`, `--format`, `-n`, `--author`, `--since`, `--follow`)
- `git status` — working tree status
- `git show <ref>` — show commit contents
- `git blame <file>` — line-by-line attribution

**Priority 1 (useful for deeper analysis):**
- `git diff --stat` — summary of changes
- `git log --graph` — ASCII commit graph
- `git rev-parse` — resolve refs to SHAs
- `git branch` — list branches
- `git tag` — list tags
- `git ls-files` — list tracked files

**Priority 2 (nice to have):**
- `git shortlog` — summarize commits by author
- `git log -p` — log with patches
- `git stash list` — if the agent looks for stashes

### Explicitly Unsupported (Mutating Remote State)

The following are **never** available in the sandbox:
- `git push`, `git fetch`, `git pull` — no network operations
- `git merge`, `git rebase`, `git cherry-pick` — no ref mutation (the VFS snapshot is read-only-ish for git purposes)
- `git commit`, `git add` — the agent can modify files in VFS, but cannot create new git commits

If the agent attempts these, return: `devdev: git <subcommand> is not available in the virtual workspace.`

---

## Implementation Approach

Git operations are implemented using a **git library with pluggable storage backends** rather than the git CLI. Research confirms this is fully feasible.

### Proven Approach: In-Memory Object Database

libgit2 (and its Rust bindings, `git2-rs`) provides a **pluggable backend system** with a built-in in-memory backend (`mempack`):

- **`git_odb_backend`** — a trait/interface for custom object database backends. Supports: `read`, `write`, `exists`, `read_header`, `foreach`.
- **`git_mempack_new()`** — a built-in in-memory ODB backend. Objects are stored in memory, not on disk.
- **`git_refdb_backend`** — similarly pluggable backend for refs (branches, tags, HEAD).
- **`Repository::from_odb()`** — create a repository instance backed entirely by a custom ODB, with no on-disk `.git` directory required.

The Rust bindings expose all of this:
```
// Conceptual (not language-specific):
odb = new ObjectDatabase()
mempack = odb.add_mempack_backend(priority: 2)
// Load objects from .git pack files into mempack...
repo = Repository.from_odb(odb)
// Now: repo.revwalk(), repo.diff(), repo.blame(), etc.
```

This is **production-grade** — libgit2 is used by GitHub, GitLab, and Azure DevOps.

### Supported Operations via Library

| Operation | Library API | Feasibility |
|-----------|-------------|-------------|
| `git log` | `revwalk()` | ✅ Works directly |
| `git diff` | `diff_tree_to_tree()` | ✅ Works directly |
| `git show` | `find_commit()` + `find_blob()` | ✅ Works directly |
| `git blame` | `blame_file()` | ✅ Works directly |
| `git branch` | `branches()` | ✅ Works directly |
| `git status` | Manual tree-walk vs index | ⚠️ ~200 lines of custom code |
| `git rev-parse` | `revparse_single()` | ✅ Works directly |

### Alternative: Pure Rust Git (Gitoxide)

`gitoxide` (Byron/gitoxide) is a pure Rust git implementation that avoids the C dependency. It supports pack file reading, revision walking, and diffing. However:
- Less mature (pre-1.0, "initial development" status on most components).
- No documented custom storage backend API.
- Blame implementation is minimal.

Recommendation: Start with libgit2 bindings for stability. Gitoxide is the long-term pure-Rust escape hatch if the C dependency becomes a portability issue.

Key aspects:
- The `.git` directory (or relevant portions) is loaded into the in-memory ODB during repo snapshot creation.
- Pack files can be decompressed and traversed from in-memory buffers — libgit2 handles this transparently.
- Output formatting mimics the real `git` CLI output so the agent can parse it as expected.

### .git Loading Strategy

Loading an entire `.git` directory can be expensive for large repos (potentially gigabytes of pack files). Options:

1. **Full load:** Load the entire `.git` directory into memory. Simple, works for small repos, may blow the 2 GB cap on large repos.
2. **Lazy/partial load:** Load refs, HEAD, and index eagerly. Load pack files and loose objects on demand as git operations reference them. This keeps initial memory low but requires a read-back channel to the host filesystem during the evaluation (breaking pure-memory isolation).
3. **Pre-extracted metadata:** At snapshot time, pre-compute commonly needed data (recent commits, current branch, diff against HEAD) and store it as structured data in the VFS. Virtual git commands consult this cache first. Only fall back to raw object access when the cache misses.

Recommendation: Start with option 1 (full load) and the 2 GB cap as the safety valve. Add lazy loading only if real-world repos routinely exceed the cap.

---

## Interface

Virtual git is registered as a command in the shell parser's tool lookup, just like WASM coreutils. When the shell parser encounters `git <subcommand>`, it routes to the virtual git implementation instead of the WASM tool engine.

```
interface VirtualGit {
  execute(
    args: [string],           // e.g., ["log", "--oneline", "-10"]
    cwd: string,              // working directory in VFS
    fs: VirtualFilesystem     // the VFS containing .git
  ) -> { stdout: bytes, stderr: bytes, exit_code: int }
}
```

Same interface shape as the WASM tool engine — stdout, stderr, exit code. The shell parser doesn't need to know or care that git is implemented differently from grep.

---

## Design Notes

- Git output format fidelity matters. The agent will parse `git log --oneline` output, `git diff` unified diffs, `git blame` output, etc. If our format diverges from real git, the agent gets confused. Invest in matching the default output formats precisely.
- The agent may use `git diff HEAD~3..HEAD -- src/` style range syntax. The ref resolution logic needs to handle standard revspecs.
- `git status` needs to compare the git index (from `.git`) against the VFS working tree. If the agent has modified files in the VFS, `git status` should reflect those changes as "modified" — this is expected behavior.

---

## Resolved Questions

1. **Git library compatibility with in-memory FS:** ✅ Resolved. libgit2 has pluggable ODB/refdb backends and a built-in in-memory backend (`mempack`). The Rust bindings (`git2-rs`) expose full access. Custom storage is a first-class feature, not a hack.
2. **Pack file handling:** ✅ Resolved. libgit2 transparently decompresses and traverses pack files. When loaded into the mempack backend, pack contents are accessible from memory without disk I/O.

## Open Questions

1. **`git status` implementation:** Status requires comparing the git index against the VFS working tree. This isn't a single library call — it requires ~200 lines of custom code to walk the tree and diff against the index. How faithfully must the output match real `git status`?
2. **Shallow clones:** If the repo was shallow-cloned, `.git` history is truncated. Should DevDev detect this and warn the agent, or silently return what's available?
3. **C dependency tradeoff:** libgit2 is C. It compiles everywhere and is battle-tested, but it's not pure Rust. Is this acceptable for the portability goals, or should we invest in gitoxide (pure Rust, less mature)?
