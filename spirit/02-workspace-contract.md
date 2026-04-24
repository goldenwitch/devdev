# 02 — The Workspace Contract

This document specifies the workspace layer: the piece that can be
used standalone, without the rest of DevDev. A caller that only wants
*a place to drop files and run processes* should find everything they
need here.

Implementation lives in the `devdev-workspace` crate. This document
intentionally does not talk about module or type names; it talks
about the contract the crate implements.

## Three collaborators

The workspace layer is three things working together:

1. **An in-memory filesystem** with POSIX-ish semantics (inodes,
   modes, symlinks, a root at `/`).
2. **A mount driver** that presents that filesystem as a real
   directory on the host OS (FUSE on Linux, WinFSP on Windows).
3. **A process launcher** that spawns a host binary inside the
   mount with a curated environment.

The three are separable. You can drive the filesystem without
mounting. You can mount without launching anything. But the typical
loop uses all three.

## The in-memory filesystem

### What it models

A single-rooted directory tree. Directories contain entries. Entries
have inodes; inodes carry bytes, mode bits, timestamps, and a link
count. Symlinks resolve to POSIX-shaped targets. Hard links are not
provided today (out of scope; see roadmap).

### What it supports

- **Path-addressed I/O.** Callers can read, write, create, delete,
  and stat paths directly, without having to resolve handles
  themselves. `/home/agent/workspace/README.md` is a well-formed
  address.
- **Recursive directory creation.** A single call creates missing
  intermediate directories with a provided mode.
- **Unlimited size by default.** There is no hard-coded quota.
  Callers who want a cap impose one above the layer.
- **Byte-level fidelity.** Content written is retrieved identically;
  no normalization, no encoding translation.

### What it does not

- It is not a database. No transactions, no durability guarantees
  beyond the process lifetime unless the caller serialises.
- It is not thread-aware at the logical level. Concurrent mutations
  are safe (the implementation uses locks), but the semantics are
  "last writer wins" — there is no ordering primitive.
- It does not enforce ownership. There is no concept of "user X
  cannot write this file." If you hand the filesystem to an agent,
  the agent can touch everything in it.

### Path canonicalisation

Paths are POSIX strings (`/`-separated, leading `/` required for
absolutes). The layer normalises (`./`, `//`, trailing slashes) and
resolves symlinks when walking. On Windows, `\\` is *not* accepted
at the logical API — the filesystem's own paths are POSIX even when
the mount projects them under a drive letter. Translation happens at
the mount boundary, not inside the model.

## The mount

### What it does

Projects the in-memory filesystem as a real directory on the host
OS. Once mounted, any process — yours, the agent's, a shell,
`find` — sees the filesystem's contents at that directory and can
interact with it using normal OS calls.

### Two drivers

- **Linux / FUSE.** The kernel's FUSE interface. Mount point is an
  empty directory the layer owns. Read and write traffic crosses
  into user space and is served from the in-memory filesystem.
- **Windows / WinFSP.** The WinFSP user-mode filesystem framework.
  Mount point is an auto-selected drive letter. Same logical
  contract; the projection is through a Windows-native volume
  interface.

macOS is not supported. FUSE on macOS is a third-party adventure
that has not been judged worth the cost for the current audience.

### Mount guarantees

- **Bijection.** Every path in the in-memory filesystem has
  exactly one address through the mount. Writing through the mount
  and reading through the in-memory API returns the same bytes;
  writing through the in-memory API and reading through the mount
  does the same. No caching shenanigans, no ordering surprises.
- **Ephemeral.** The mount goes away when the workspace handle is
  dropped (or, on abnormal termination, when the OS reaps the
  mount-owning process). The in-memory filesystem exists
  independently; it can outlive any given mount.
- **Not a security boundary.** Processes launched against the
  mount run with your user's full privileges. They can read your
  real `$HOME`, open network sockets, etc. The mount bounds
  *filesystem-view*, not capability.

### Platform differences the caller must be aware of

- **Drive letters on Windows.** The mount point is chosen by the
  driver; the caller receives the path after mount. Callers that
  hard-code POSIX paths for the agent to use will fail on Windows.
- **Line endings.** The mount does not translate. If you put `\n`
  into the filesystem, the OS sees `\n`. On Windows this matters
  for tools that care.
- **Permissions.** WinFSP surfaces mode bits as best-effort; don't
  rely on POSIX mode fidelity through the Windows mount.

## The process launcher

### What it provides

A single entrypoint to spawn a host binary with its working
directory set to some path *inside the mount*, with a curated
environment, and with stdio wired to the caller.

The child process sees the mounted directory as a real filesystem.
It does not know (and does not need to know) that the bytes are
coming from an in-memory model.

### Curated environment

The launcher does not inherit the caller's environment wholesale.
It sets a deliberately short list: `HOME`, `USER`, `LOGNAME`,
`SHELL`, `TERM`, `PATH`. Everything else must be provided
explicitly by the caller.

The curated `HOME` is a POSIX path inside the mount. This is
deliberate: tools that write into `$HOME` (shell histories, cargo
caches, gitconfig probes) write into the mount, not into the real
home. **Caveat:** the subprocess inherits neither the mount
namespace nor any chroot — it's running with the host's real
filesystem beneath the mount. Tools that do absolute-path lookups
against paths that don't exist on the host (e.g., reading a
non-existent `/home/agent/.gitconfig`) will fail gracefully, but
anything that synthesises a host-real path from an env var may be
surprising. The `cargo build` scenario exercises this and documents
it openly.

### Stdio

The child's stdout, stderr, and stdin are exposed to the caller as
a PTY, not raw pipes. This matters for tools that detect a TTY
(colour output, progress bars, `less`-style paging). Callers can
read the combined output stream and interact with the child using
the terminal protocol.

## Serialization

### What you get

The in-memory filesystem serialises to an opaque byte blob and
deserialises back. The blob is the entire tree — inodes, content,
metadata. Size is proportional to the content.

### What this means

- **Checkpointing.** A caller can capture the full workspace state
  between operations and restore it later. This is how DevDev's
  task model implements resumability.
- **Transport.** The blob is a normal byte sequence; it can be sent
  over the network, written to disk, or handed to another
  process.
- **Versioning.** The blob format is not stable across major
  revisions of the crate. Callers that persist blobs across
  versions are on their own until the format is stabilised.

### What it does not cover

- **Mount state is not serialised.** A re-hydrated filesystem is
  unmounted. The caller remounts if they want a mount.
- **Running processes are not serialised.** If you snapshot with a
  process in flight, the blob captures the files, not the process.

## Expected usage shapes

### Throwaway workspace

Create an empty filesystem, mount it, run one process, drop.

### Seeded workspace

Create a filesystem, populate it with files the caller already has
(a git repo, a tarball contents, a fixture tree), mount, run
processes, optionally snapshot.

### Resumable workspace

Deserialise a stored blob into a fresh filesystem, mount, continue
work, re-snapshot. This is the pattern DevDev's task model uses.

### Snapshot-then-compare

Snapshot, run, snapshot again, compare the two blobs to observe
what the agent changed. This is possible but not explicitly
supported by the crate today; consumers roll their own diff.

## Roadmap from this layer's perspective

- **Process containment.** Today the subprocess is a normal host
  process. Real containment (namespaces on Linux, job objects on
  Windows) is a roadmap item. When it lands, the launcher contract
  grows but does not break.
- **macOS mount.** Not promised, not rejected. Would add a third
  driver behind the same mount contract.
- **Incremental serialisation.** Today's serialize is all-or-nothing.
  An incremental form would help very large workspaces.
- **Stable blob format.** Until declared stable, assume breaking
  changes between versions.
