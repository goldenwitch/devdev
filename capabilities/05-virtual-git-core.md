---
id: virtual-git-core
title: "Git Object Database (In-Memory)"
status: done
type: leaf
phase: 2
crate: devdev-git
priority: P0
depends-on: [vfs-core]
effort: M
---

# 05 — Git Object Database (In-Memory)

Load a repository's `.git` directory from VFS into libgit2's in-memory object database. This produces a `git2::Repository` that all virtual git commands operate on — no disk I/O, no host git binary.

## Scope

**In:**
- Read `.git` directory contents from VFS
- Set up libgit2 `Odb` with `mempack` backend
- Load pack files and loose objects into memory
- Set up in-memory refdb (HEAD, branches, tags)
- Construct `Repository` from in-memory backends
- Handle common `.git` layouts (standard, bare, shallow)

**Out:**
- Git subcommand implementations (that's `06-virtual-git-commands`)
- Output formatting
- CLI argument parsing for git commands

## Interface

```rust
/// A git repository backed entirely by in-memory storage.
pub struct VirtualRepo {
    repo: git2::Repository,
    // The mempack backend for object storage
    // (held to keep the backend alive)
    _mempack: git2::Mempack,
}

impl VirtualRepo {
    /// Load a git repository from VFS.
    /// Reads .git from the VFS at the given root path.
    pub fn from_vfs(
        vfs: &dyn VirtualFilesystem,
        repo_root: &Path,
    ) -> Result<Self, GitLoadError>;
    
    /// Get a reference to the underlying git2 Repository.
    pub fn repo(&self) -> &git2::Repository;
    
    /// Get the current HEAD ref name (e.g., "refs/heads/main").
    pub fn head_ref(&self) -> Result<String>;
    
    /// Get the current HEAD commit.
    pub fn head_commit(&self) -> Result<git2::Commit>;
}

pub enum GitLoadError {
    NoGitDir,                    // .git not found in VFS
    InvalidGitDir(String),       // .git exists but malformed
    PackLoadError(String),       // failed to load pack file
    RefLoadError(String),        // failed to parse refs
    LibGitError(git2::Error),
}
```

## Implementation Notes

### Loading Strategy

1. **Read `.git/HEAD`** from VFS → determine current branch.
2. **Read `.git/refs/`** and `.git/packed-refs`** from VFS → build in-memory refdb.
3. **Read `.git/objects/pack/*.pack`** + `.idx` files from VFS → feed to mempack backend.
4. **Read `.git/objects/??/*`** (loose objects) from VFS → feed to mempack backend.
5. **Read `.git/index`** from VFS → needed for `git status` later.
6. **Construct `Repository`** from the populated Odb + refdb.

### libgit2 API Sequence

```rust
// 1. Create Odb with mempack backend
let odb = git2::Odb::new()?;
let mempack = odb.add_new_mempack_backend(/* priority */ 2)?;

// 2. Load pack files from VFS into Odb
for pack_path in vfs.glob(".git/objects/pack/*.pack")? {
    let pack_bytes = vfs.read(&pack_path)?;
    let idx_bytes = vfs.read(&pack_path.with_extension("idx"))?;
    // Feed pack + index to Odb
    // (may need to use Odb::add_disk_alternate or manual pack parsing)
}

// 3. Load loose objects
for obj_path in vfs.glob(".git/objects/??/*")? {
    let obj_bytes = vfs.read(&obj_path)?;
    // Parse object header, write to mempack
}

// 4. Build Repository from Odb
let repo = git2::Repository::from_odb(odb)?;

// 5. Set up refs from VFS
let head_content = String::from_utf8(vfs.read(".git/HEAD")?)?;
// Parse ref: "ref: refs/heads/main\n" or raw SHA
```

### Challenges

- **Pack file parsing:** libgit2's `Odb::add_disk_alternate` expects a filesystem path, not memory. The approach may require either:
  - Writing a temporary adapter that makes VFS look like disk to libgit2 (via `git_odb_backend` custom implementation), or
  - Pre-parsing pack files ourselves and writing individual objects to mempack via `odb.write()`.
  - **Investigate:** Does `git2::Odb::new_reader()` or `mempack.push()` accept raw object bytes?
  
- **Refdb:** libgit2's refdb is also typically disk-backed. May need a custom `git_refdb_backend` that reads from VFS-stored refs, or pre-populate refs via the repository API after construction.

- **Index:** The git index (`.git/index`) is a binary format. libgit2 can parse it, but loading from memory rather than disk may require the same adapter approach.

### Fallback Strategy

If the pluggable backend approach proves too complex, an alternative is:
1. Write `.git` contents from VFS to a temporary directory on the host.
2. Open with standard `Repository::open()`.
3. Clean up temp dir after evaluation.

This sacrifices pure-memory isolation but is simpler. Use as escape hatch only.

## Files

```
crates/devdev-git/Cargo.toml
crates/devdev-git/src/lib.rs       — VirtualRepo, GitLoadError, re-exports
crates/devdev-git/src/loader.rs    — .git loading logic, pack/loose/ref parsing
crates/devdev-git/src/mempack.rs   — Odb + mempack backend setup
```

## Acceptance Criteria

- [ ] Load `.git` from VFS of a test repo → `VirtualRepo` constructed successfully
- [ ] `head_ref()` returns current branch name
- [ ] `head_commit()` returns a valid commit object
- [ ] `repo.revwalk()` from HEAD produces commit history
- [ ] `repo.find_blob()` retrieves file contents for a known path at HEAD
- [ ] Repo with packed refs → branches and tags resolve correctly
- [ ] Repo with no `.git` directory → `NoGitDir` error
- [ ] Large pack file (~50 MB test fixture) loads without error
