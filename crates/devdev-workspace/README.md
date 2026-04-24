# devdev-workspace

**A virtual workspace for agents.** An in-memory, POSIX-ish filesystem
that you can mount at a real host path (FUSE on Linux, WinFSP on
Windows) and then `exec` real host binaries inside, under a PTY, with a
curated environment.

This crate is the workspace layer of [DevDev](https://github.com/goldenwitch/devdev),
extracted so it can be used standalone.

> ⚠️ **DevDev does not claim sandboxing.** The mount is a friendly
> host-path for tools that expect one. A process run via
> `Workspace::exec` still executes as your user, with your network, and
> can touch the host filesystem outside the mount if it wants to. True
> containment (namespaces, seccomp, capability gating) is on the
> roadmap. Treat this as "a virtual scratch directory you can snapshot
> and throw away", not "a jail".

## What it gives you

* **`Fs`** — an in-memory filesystem (inode table, not path-keyed
  blobs). Supports the POSIX operations real tools reach for: rename,
  hardlink, symlink, seek, truncate, `O_APPEND`, mode bits, atime/mtime.
  Serializable: `Fs::snapshot()` → `Snapshot` → `bincode` bytes, and
  back.
* **`Workspace::mount()`** — mounts `Fs` at a host tempdir via the
  platform FUSE/WinFSP driver. Returns a real `PathBuf` you can hand
  to any subprocess.
* **`Workspace::exec()`** — spawns a real host binary under a PTY,
  rooted inside the mount, with a curated environment (no inherited
  `PATH`, `HOME`, git config, etc. — just what you opt in to).
* **Snapshots** — serialize the entire state with `Fs::snapshot()`,
  round-trip through `bincode`, restore with `Fs::restore(snapshot)`.

Everything is deterministic: inode numbers are monotonic, timestamps
come from a clock you control, snapshots round-trip byte-for-byte.

## Minimal example

```rust
use devdev_workspace::Workspace;
use std::ffi::OsStr;

let mut ws = Workspace::new();

// Write a file into the virtual FS directly.
{
    let fs = ws.fs();
    let mut fs = fs.lock().unwrap();
    let ino = fs.create_file(devdev_workspace::ROOT_INO, b"hello.txt").unwrap();
    fs.write_all(ino, b"world\n").unwrap();
}

// Mount at a host tempdir.
let mount = ws.mount().expect("mount");
println!("mounted at {}", mount.display());

// Run a real binary inside the mount.
let mut out = Vec::new();
let code = ws.exec(
    OsStr::new("cat"),
    &[OsStr::new("hello.txt")],
    b"/",
    &mut out,
).expect("exec");
assert_eq!(code, 0);
assert_eq!(&out[..], b"world\r\n"); // PTY translates \n → \r\n
```

## Platform matrix

| Platform | Driver | Works out of the box |
|----------|--------|----------------------|
| Linux x86_64 | FUSE (via `fuser`) | yes — kernel FUSE is standard |
| Windows x86_64 | WinFSP | requires [WinFSP](https://github.com/winfsp/winfsp) installed (runtime + headers if building) |
| macOS | — | not supported in this pass |

On Windows without WinFSP installed, the crate still builds but mount
tests are gated `#[ignore]`. Unmounted operations (pure `Fs`, snapshot
round-trip) work everywhere.

## When to use this

* You're building an agent that needs a scratch filesystem it can
  inspect, snapshot, and discard.
* You want to run real tools (`cargo`, `git`, `rg`, a language server)
  in a directory you control the contents of, without polluting the
  user's home directory.
* You want a reproducible, serializable project state you can
  checkpoint between agent turns.

## When *not* to use this

* You need true process isolation. This crate does not provide it.
* You need macOS support today.
* You need to mount remote / network filesystems. This is an
  in-memory store.

## Design references

See the DevDev narrative:

* [`spirit/02-workspace-contract.md`](https://github.com/goldenwitch/devdev/blob/master/spirit/02-workspace-contract.md)
  — full contract, invariants, and serialization format.
* [`spirit/01-concept.md`](https://github.com/goldenwitch/devdev/blob/master/spirit/01-concept.md)
  — why a virtual workspace at all.

## License

MIT. See [LICENSE](https://github.com/goldenwitch/devdev/blob/master/LICENSE).
