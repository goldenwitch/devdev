//! In-memory inode-centric filesystem — backing store for the
//! FUSE/WinFSP driver.
//!
//! All operations are keyed by inode number (`Ino`), not path, because
//! the kernel FS protocols (FUSE, WinFSP) are inode-centric. A thin
//! path-resolution layer sits on top for the convenience API exposed
//! via `Workspace`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Inode number. Monotonic per mount lifetime; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Ino(pub u64);

/// The root inode. Always allocated at `new()`.
pub const ROOT_INO: Ino = Ino(1);

/// Default memory cap: 2 GiB of file content.
pub const DEFAULT_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

/// Snapshot format magic: `DDWS\0` + version byte.
pub const SNAPSHOT_MAGIC: &[u8; 6] = b"DDWS\0\x01";

/// Current snapshot version.
pub const SNAPSHOT_VERSION: u8 = 1;

/// Maximum number of symlink hops before ELOOP.
const MAX_SYMLINK_HOPS: u32 = 40;

/// Maximum single name length (bytes), matches POSIX `NAME_MAX`.
const NAME_MAX: usize = 255;

/// Inode kinds we support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    File,
    Directory,
    Symlink,
}

/// POSIX-style timespec (seconds + nanos since UNIX epoch).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timespec {
    pub secs: i64,
    pub nanos: u32,
}

impl Timespec {
    fn now() -> Self {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => Self {
                secs: d.as_secs() as i64,
                nanos: d.subsec_nanos(),
            },
            Err(_) => Self::default(),
        }
    }
}

/// Inode attributes exposed to the FS driver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InodeAttr {
    pub ino: Ino,
    pub kind: Kind,
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub size: u64,
    pub atime: Timespec,
    pub mtime: Timespec,
    pub ctime: Timespec,
    pub crtime: Timespec,
}

/// Attributes to modify via `setattr`.
#[derive(Debug, Clone, Default)]
pub struct SetAttr {
    pub mode: Option<u16>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub size: Option<u64>,
    pub atime: Option<Timespec>,
    pub mtime: Option<Timespec>,
    pub ctime: Option<Timespec>,
}

/// POSIX-style error codes. Driver maps these to its native errno type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum Errno {
    #[error("no such file or directory")]
    NoEnt,
    #[error("file exists")]
    Exist,
    #[error("is a directory")]
    IsDir,
    #[error("not a directory")]
    NotDir,
    #[error("directory not empty")]
    NotEmpty,
    #[error("too many links")]
    Mlink,
    #[error("permission denied")]
    Acces,
    #[error("invalid argument")]
    Inval,
    #[error("no space left on device")]
    NoSpc,
    #[error("input/output error")]
    Io,
    #[error("file name too long")]
    NameTooLong,
    #[error("bad file descriptor")]
    BadF,
    #[error("operation not supported")]
    NoSys,
}

/// Serializable snapshot of the filesystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u8,
    pub next_ino: u64,
    pub bytes_used: u64,
    pub bytes_limit: u64,
    pub inodes: BTreeMap<u64, SnapshotInode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInode {
    pub attr: InodeAttr,
    pub body: SnapshotBody,
}

/// Directory entries are stored as raw bytes because POSIX names need not
/// be UTF-8. This diverges from the original scaffold which used `String`
/// keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SnapshotBody {
    File { content: Vec<u8> },
    Directory { entries: Vec<(Vec<u8>, u64)> },
    Symlink { target: Vec<u8> },
}

/// A directory entry produced by `readdir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub ino: Ino,
    pub kind: Kind,
    pub name: Vec<u8>,
}

#[derive(Debug, Clone)]
enum Body {
    File(Vec<u8>),
    Dir(BTreeMap<Vec<u8>, Ino>),
    Symlink(Vec<u8>),
}

#[derive(Debug, Clone)]
struct Inode {
    attr: InodeAttr,
    body: Body,
    /// Parent directory. Tracked only for directories (used for rename
    /// loop detection and for readdir's virtual `..` entry). Root's
    /// parent is itself.
    parent: Option<Ino>,
}

/// The in-memory filesystem.
pub struct Fs {
    inodes: HashMap<Ino, Inode>,
    next_ino: u64,
    bytes_used: u64,
    bytes_limit: u64,
    time_source: Arc<dyn Fn() -> Timespec + Send + Sync>,
}

impl std::fmt::Debug for Fs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fs")
            .field("inodes", &self.inodes.len())
            .field("next_ino", &self.next_ino)
            .field("bytes_used", &self.bytes_used)
            .field("bytes_limit", &self.bytes_limit)
            .finish()
    }
}

impl Default for Fs {
    fn default() -> Self {
        Self::new()
    }
}

impl Fs {
    /// Construct an empty filesystem with a single root directory
    /// (mode `0o755`, nlink=2).
    pub fn new() -> Self {
        Self::with_limit(DEFAULT_LIMIT)
    }

    /// Same as `new` but with a custom byte cap for file content.
    pub fn with_limit(bytes_limit: u64) -> Self {
        let time_source: Arc<dyn Fn() -> Timespec + Send + Sync> = Arc::new(Timespec::now);
        let now = time_source();
        let mut inodes = HashMap::new();
        let root = Inode {
            attr: InodeAttr {
                ino: ROOT_INO,
                kind: Kind::Directory,
                mode: 0o755,
                uid: 0,
                gid: 0,
                nlink: 2,
                size: 0,
                atime: now,
                mtime: now,
                ctime: now,
                crtime: now,
            },
            body: Body::Dir(BTreeMap::new()),
            parent: Some(ROOT_INO),
        };
        inodes.insert(ROOT_INO, root);
        Self {
            inodes,
            next_ino: ROOT_INO.0 + 1,
            bytes_used: 0,
            bytes_limit,
            time_source,
        }
    }

    /// Override the time source (tests use this for determinism).
    pub fn set_time_source<F>(&mut self, f: F)
    where
        F: Fn() -> Timespec + Send + Sync + 'static,
    {
        self.time_source = Arc::new(f);
    }

    pub fn bytes_used(&self) -> u64 {
        self.bytes_used
    }

    pub fn bytes_limit(&self) -> u64 {
        self.bytes_limit
    }

    fn now(&self) -> Timespec {
        (self.time_source)()
    }

    fn alloc_ino(&mut self) -> Ino {
        let n = self.next_ino;
        self.next_ino += 1;
        Ino(n)
    }

    fn get(&self, ino: Ino) -> Result<&Inode, Errno> {
        self.inodes.get(&ino).ok_or(Errno::NoEnt)
    }

    fn get_mut(&mut self, ino: Ino) -> Result<&mut Inode, Errno> {
        self.inodes.get_mut(&ino).ok_or(Errno::NoEnt)
    }

    // ---- Inode introspection ---------------------------------------------

    pub fn getattr(&self, ino: Ino) -> Result<InodeAttr, Errno> {
        Ok(self.get(ino)?.attr.clone())
    }

    pub fn setattr(&mut self, ino: Ino, attr: SetAttr) -> Result<InodeAttr, Errno> {
        if let Some(new_size) = attr.size {
            self.truncate(ino, new_size)?;
        }
        let now = self.now();
        let node = self.get_mut(ino)?;
        if let Some(m) = attr.mode {
            node.attr.mode = m;
        }
        if let Some(u) = attr.uid {
            node.attr.uid = u;
        }
        if let Some(g) = attr.gid {
            node.attr.gid = g;
        }
        if let Some(t) = attr.atime {
            node.attr.atime = t;
        }
        if let Some(t) = attr.mtime {
            node.attr.mtime = t;
        }
        if let Some(t) = attr.ctime {
            node.attr.ctime = t;
        } else {
            node.attr.ctime = now;
        }
        Ok(node.attr.clone())
    }

    // ---- Lookup ----------------------------------------------------------

    pub fn lookup(&self, parent: Ino, name: &[u8]) -> Result<InodeAttr, Errno> {
        validate_name(name)?;
        let p = self.get(parent)?;
        let entries = match &p.body {
            Body::Dir(e) => e,
            _ => return Err(Errno::NotDir),
        };
        let ino = entries.get(name).copied().ok_or(Errno::NoEnt)?;
        Ok(self.get(ino)?.attr.clone())
    }

    // ---- File I/O --------------------------------------------------------

    pub fn read(&self, ino: Ino, offset: u64, size: u32) -> Result<Vec<u8>, Errno> {
        let node = self.get(ino)?;
        let content = match &node.body {
            Body::File(c) => c,
            Body::Dir(_) => return Err(Errno::IsDir),
            Body::Symlink(_) => return Err(Errno::Inval),
        };
        let len = content.len() as u64;
        if offset > len {
            return Ok(Vec::new());
        }
        let start = offset as usize;
        let end = (offset.saturating_add(size as u64)).min(len) as usize;
        Ok(content[start..end].to_vec())
    }

    pub fn write(&mut self, ino: Ino, offset: u64, data: &[u8]) -> Result<u32, Errno> {
        let now = self.now();
        let node = self.get(ino)?;
        match node.body {
            Body::File(_) => {}
            Body::Dir(_) => return Err(Errno::IsDir),
            Body::Symlink(_) => return Err(Errno::Inval),
        }
        let old_len = node.attr.size;
        let new_end = offset.saturating_add(data.len() as u64);
        let new_len = old_len.max(new_end);
        let delta = new_len.saturating_sub(old_len);
        if self
            .bytes_used
            .checked_add(delta)
            .is_none_or(|u| u > self.bytes_limit)
        {
            return Err(Errno::NoSpc);
        }
        let node = self.get_mut(ino)?;
        let content = match &mut node.body {
            Body::File(c) => c,
            _ => unreachable!(),
        };
        if (new_len as usize) > content.len() {
            content.resize(new_len as usize, 0);
        }
        let start = offset as usize;
        content[start..start + data.len()].copy_from_slice(data);
        node.attr.size = new_len;
        node.attr.mtime = now;
        node.attr.ctime = now;
        self.bytes_used += delta;
        Ok(data.len() as u32)
    }

    pub fn truncate(&mut self, ino: Ino, size: u64) -> Result<(), Errno> {
        let now = self.now();
        let node = self.get(ino)?;
        match node.body {
            Body::File(_) => {}
            Body::Dir(_) => return Err(Errno::IsDir),
            Body::Symlink(_) => return Err(Errno::Inval),
        }
        let old_len = node.attr.size;
        if size > old_len {
            let grow = size - old_len;
            if self
                .bytes_used
                .checked_add(grow)
                .is_none_or(|u| u > self.bytes_limit)
            {
                return Err(Errno::NoSpc);
            }
            self.bytes_used += grow;
        } else {
            self.bytes_used -= old_len - size;
        }
        let node = self.get_mut(ino)?;
        let content = match &mut node.body {
            Body::File(c) => c,
            _ => unreachable!(),
        };
        content.resize(size as usize, 0);
        node.attr.size = size;
        node.attr.mtime = now;
        node.attr.ctime = now;
        Ok(())
    }

    // ---- Directory ops ---------------------------------------------------

    pub fn mkdir(&mut self, parent: Ino, name: &[u8], mode: u16) -> Result<InodeAttr, Errno> {
        validate_name(name)?;
        self.ensure_dir_no_entry(parent, name)?;
        let now = self.now();
        let ino = self.alloc_ino();
        let node = Inode {
            attr: InodeAttr {
                ino,
                kind: Kind::Directory,
                mode,
                uid: 0,
                gid: 0,
                nlink: 2,
                size: 0,
                atime: now,
                mtime: now,
                ctime: now,
                crtime: now,
            },
            body: Body::Dir(BTreeMap::new()),
            parent: Some(parent),
        };
        self.inodes.insert(ino, node);
        self.insert_entry(parent, name.to_vec(), ino, now)?;
        // parent gains a child directory → nlink++
        let p = self.get_mut(parent)?;
        p.attr.nlink += 1;
        let attr = self.get(ino)?.attr.clone();
        Ok(attr)
    }

    pub fn rmdir(&mut self, parent: Ino, name: &[u8]) -> Result<(), Errno> {
        validate_name(name)?;
        let target = self.child_ino(parent, name)?;
        let node = self.get(target)?;
        match &node.body {
            Body::Dir(entries) => {
                if !entries.is_empty() {
                    return Err(Errno::NotEmpty);
                }
            }
            Body::File(_) | Body::Symlink(_) => return Err(Errno::NotDir),
        }
        let now = self.now();
        self.remove_entry(parent, name, now)?;
        // Parent loses a subdir → nlink--
        let p = self.get_mut(parent)?;
        p.attr.nlink -= 1;
        self.inodes.remove(&target);
        Ok(())
    }

    pub fn readdir(&self, ino: Ino, offset: u64) -> Result<Vec<DirEntry>, Errno> {
        let node = self.get(ino)?;
        let entries = match &node.body {
            Body::Dir(e) => e,
            _ => return Err(Errno::NotDir),
        };
        let parent = node.parent.unwrap_or(ROOT_INO);
        let mut all = Vec::with_capacity(entries.len() + 2);
        all.push(DirEntry {
            ino,
            kind: Kind::Directory,
            name: b".".to_vec(),
        });
        all.push(DirEntry {
            ino: parent,
            kind: Kind::Directory,
            name: b"..".to_vec(),
        });
        for (name, child_ino) in entries {
            let kind = self.get(*child_ino)?.attr.kind;
            all.push(DirEntry {
                ino: *child_ino,
                kind,
                name: name.clone(),
            });
        }
        if offset as usize >= all.len() {
            return Ok(Vec::new());
        }
        Ok(all.split_off(offset as usize))
    }

    // ---- File create/unlink ---------------------------------------------

    pub fn create(&mut self, parent: Ino, name: &[u8], mode: u16) -> Result<InodeAttr, Errno> {
        validate_name(name)?;
        self.ensure_dir_no_entry(parent, name)?;
        let now = self.now();
        let ino = self.alloc_ino();
        let node = Inode {
            attr: InodeAttr {
                ino,
                kind: Kind::File,
                mode,
                uid: 0,
                gid: 0,
                nlink: 1,
                size: 0,
                atime: now,
                mtime: now,
                ctime: now,
                crtime: now,
            },
            body: Body::File(Vec::new()),
            parent: None,
        };
        self.inodes.insert(ino, node);
        self.insert_entry(parent, name.to_vec(), ino, now)?;
        Ok(self.get(ino)?.attr.clone())
    }

    pub fn unlink(&mut self, parent: Ino, name: &[u8]) -> Result<(), Errno> {
        validate_name(name)?;
        let target = self.child_ino(parent, name)?;
        let node = self.get(target)?;
        if matches!(node.body, Body::Dir(_)) {
            return Err(Errno::IsDir);
        }
        let now = self.now();
        self.remove_entry(parent, name, now)?;
        let node = self.get_mut(target)?;
        node.attr.nlink = node.attr.nlink.saturating_sub(1);
        if node.attr.nlink == 0 {
            let body_bytes = match &node.body {
                Body::File(c) => c.len() as u64,
                Body::Symlink(t) => t.len() as u64,
                _ => 0,
            };
            self.bytes_used = self.bytes_used.saturating_sub(body_bytes);
            self.inodes.remove(&target);
        }
        Ok(())
    }

    // ---- Symlink / hardlink ---------------------------------------------

    pub fn symlink(
        &mut self,
        parent: Ino,
        name: &[u8],
        target: &[u8],
    ) -> Result<InodeAttr, Errno> {
        validate_name(name)?;
        self.ensure_dir_no_entry(parent, name)?;
        let add = target.len() as u64;
        if self
            .bytes_used
            .checked_add(add)
            .is_none_or(|u| u > self.bytes_limit)
        {
            return Err(Errno::NoSpc);
        }
        let now = self.now();
        let ino = self.alloc_ino();
        let node = Inode {
            attr: InodeAttr {
                ino,
                kind: Kind::Symlink,
                mode: 0o777,
                uid: 0,
                gid: 0,
                nlink: 1,
                size: target.len() as u64,
                atime: now,
                mtime: now,
                ctime: now,
                crtime: now,
            },
            body: Body::Symlink(target.to_vec()),
            parent: None,
        };
        self.inodes.insert(ino, node);
        self.insert_entry(parent, name.to_vec(), ino, now)?;
        self.bytes_used += add;
        Ok(self.get(ino)?.attr.clone())
    }

    pub fn readlink(&self, ino: Ino) -> Result<Vec<u8>, Errno> {
        let node = self.get(ino)?;
        match &node.body {
            Body::Symlink(t) => Ok(t.clone()),
            Body::Dir(_) => Err(Errno::IsDir),
            Body::File(_) => Err(Errno::Inval),
        }
    }

    pub fn link(
        &mut self,
        ino: Ino,
        new_parent: Ino,
        new_name: &[u8],
    ) -> Result<InodeAttr, Errno> {
        validate_name(new_name)?;
        let src = self.get(ino)?;
        if matches!(src.body, Body::Dir(_)) {
            // No hardlinks to directories.
            return Err(Errno::IsDir);
        }
        self.ensure_dir_no_entry(new_parent, new_name)?;
        let now = self.now();
        self.insert_entry(new_parent, new_name.to_vec(), ino, now)?;
        let node = self.get_mut(ino)?;
        node.attr.nlink += 1;
        node.attr.ctime = now;
        Ok(node.attr.clone())
    }

    // ---- Rename ----------------------------------------------------------

    pub fn rename(
        &mut self,
        parent: Ino,
        name: &[u8],
        new_parent: Ino,
        new_name: &[u8],
    ) -> Result<(), Errno> {
        validate_name(name)?;
        validate_name(new_name)?;

        // Same-name same-dir is a no-op, but the source must still exist.
        if parent == new_parent && name == new_name {
            self.child_ino(parent, name)?;
            return Ok(());
        }

        let src = self.child_ino(parent, name)?;

        // new_parent must be a directory.
        match self.get(new_parent)?.body {
            Body::Dir(_) => {}
            _ => return Err(Errno::NotDir),
        }

        let src_is_dir = matches!(self.get(src)?.body, Body::Dir(_));

        // Loop detection: if src is a directory, new_parent must not be
        // src itself nor a descendant of src.
        if src_is_dir && self.is_ancestor_or_self(src, new_parent)? {
            return Err(Errno::Inval);
        }

        // Handle overwrite of destination.
        if let Ok(dst) = self.child_ino(new_parent, new_name) {
            if dst == src {
                // Same inode via hardlink; just remove the old name.
                let now = self.now();
                self.remove_entry(parent, name, now)?;
                return Ok(());
            }
            let dst_is_dir = matches!(self.get(dst)?.body, Body::Dir(_));
            match (src_is_dir, dst_is_dir) {
                (false, true) => return Err(Errno::IsDir),
                (true, false) => return Err(Errno::NotDir),
                (true, true) => self.rmdir(new_parent, new_name)?,
                (false, false) => self.unlink(new_parent, new_name)?,
            }
        }

        let now = self.now();
        self.remove_entry(parent, name, now)?;
        self.insert_entry(new_parent, new_name.to_vec(), src, now)?;

        // Fix up parent pointer + nlink for dir moves across parents.
        if src_is_dir {
            if let Some(node) = self.inodes.get_mut(&src) {
                node.parent = Some(new_parent);
                node.attr.ctime = now;
            }
            if parent != new_parent {
                if let Some(p) = self.inodes.get_mut(&parent) {
                    p.attr.nlink = p.attr.nlink.saturating_sub(1);
                }
                if let Some(np) = self.inodes.get_mut(&new_parent) {
                    np.attr.nlink += 1;
                }
            }
        } else if let Some(node) = self.inodes.get_mut(&src) {
            node.attr.ctime = now;
        }
        Ok(())
    }

    /// Is `ancestor` the same inode as `descendant`, or any ancestor of it?
    fn is_ancestor_or_self(&self, ancestor: Ino, descendant: Ino) -> Result<bool, Errno> {
        let mut cur = descendant;
        for _ in 0..=self.inodes.len() {
            if cur == ancestor {
                return Ok(true);
            }
            let n = self.get(cur)?;
            let p = n.parent.unwrap_or(ROOT_INO);
            if p == cur {
                return Ok(false);
            }
            cur = p;
        }
        // Cycle in parent chain — shouldn't happen; bail out safely.
        Err(Errno::Io)
    }

    // ---- Path helpers ----------------------------------------------------

    /// Resolve an absolute path to an inode. Follows symlinks on
    /// intermediate components but NOT on the final component.
    pub fn resolve(&self, path: &[u8]) -> Result<Ino, Errno> {
        self.resolve_inner(path, false, 0)
    }

    fn resolve_follow(&self, path: &[u8]) -> Result<Ino, Errno> {
        self.resolve_inner(path, true, 0)
    }

    fn resolve_inner(&self, path: &[u8], follow_final: bool, depth: u32) -> Result<Ino, Errno> {
        if depth > MAX_SYMLINK_HOPS {
            return Err(Errno::Io);
        }
        if path.is_empty() || path[0] != b'/' {
            return Err(Errno::Inval);
        }
        let comps = split_path(path);
        if comps.is_empty() {
            return Ok(ROOT_INO);
        }
        let mut cur = ROOT_INO;
        let last = comps.len() - 1;
        for (i, comp) in comps.iter().enumerate() {
            cur = self.step(cur, comp)?;
            let is_last = i == last;
            if !is_last || follow_final {
                let mut hops = 0u32;
                while let Body::Symlink(target) = &self.get(cur)?.body {
                    hops += 1;
                    if hops > MAX_SYMLINK_HOPS || depth + hops > MAX_SYMLINK_HOPS {
                        return Err(Errno::Io);
                    }
                    if target.starts_with(b"/") {
                        cur = self.resolve_inner(target, true, depth + hops)?;
                    } else {
                        // Relative target: resolve against the directory
                        // that contained this symlink (i.e. the prefix of
                        // `path` up to component `i`).
                        let parent_path = prefix_path(path, i);
                        let mut new_path =
                            Vec::with_capacity(parent_path.len() + 1 + target.len());
                        if parent_path.is_empty() {
                            new_path.push(b'/');
                        } else {
                            new_path.extend_from_slice(&parent_path);
                            new_path.push(b'/');
                        }
                        new_path.extend_from_slice(target);
                        cur = self.resolve_inner(&new_path, true, depth + hops)?;
                    }
                }
            }
        }
        Ok(cur)
    }

    /// Take one step along a component. Handles `.` and `..`.
    fn step(&self, cur: Ino, comp: &[u8]) -> Result<Ino, Errno> {
        if comp == b"." {
            return Ok(cur);
        }
        if comp == b".." {
            let node = self.get(cur)?;
            return Ok(node.parent.unwrap_or(ROOT_INO));
        }
        let node = self.get(cur)?;
        match &node.body {
            Body::Dir(entries) => entries.get(comp).copied().ok_or(Errno::NoEnt),
            Body::Symlink(_) | Body::File(_) => Err(Errno::NotDir),
        }
    }

    pub fn read_path(&self, path: &[u8]) -> Result<Vec<u8>, Errno> {
        let ino = self.resolve_follow(path)?;
        let attr = self.getattr(ino)?;
        self.read(ino, 0, attr.size as u32)
    }

    /// Write `bytes` to `path`, creating the file if missing and
    /// truncating it if present. Parent directory must already exist.
    pub fn write_path(&mut self, path: &[u8], bytes: &[u8]) -> Result<(), Errno> {
        let (parent_path, name) = split_parent(path)?;
        let parent = if parent_path.is_empty() {
            ROOT_INO
        } else {
            self.resolve_follow(&parent_path)?
        };
        let ino = match self.lookup(parent, &name) {
            Ok(a) => {
                if a.kind == Kind::Directory {
                    return Err(Errno::IsDir);
                }
                self.truncate(a.ino, 0)?;
                a.ino
            }
            Err(Errno::NoEnt) => self.create(parent, &name, 0o644)?.ino,
            Err(e) => return Err(e),
        };
        if !bytes.is_empty() {
            self.write(ino, 0, bytes)?;
        }
        Ok(())
    }

    pub fn mkdir_p(&mut self, path: &[u8], mode: u16) -> Result<(), Errno> {
        if path.is_empty() || path[0] != b'/' {
            return Err(Errno::Inval);
        }
        let mut cur = ROOT_INO;
        for comp in split_path(path) {
            if comp == b"." {
                continue;
            }
            if comp == b".." {
                let n = self.get(cur)?;
                cur = n.parent.unwrap_or(ROOT_INO);
                continue;
            }
            match self.lookup(cur, comp) {
                Ok(a) => {
                    if a.kind != Kind::Directory {
                        return Err(Errno::NotDir);
                    }
                    cur = a.ino;
                }
                Err(Errno::NoEnt) => {
                    let a = self.mkdir(cur, comp, mode)?;
                    cur = a.ino;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    // ---- Snapshot / restore ---------------------------------------------

    pub fn to_snapshot(&self) -> Snapshot {
        let mut inodes = BTreeMap::new();
        for (ino, node) in &self.inodes {
            let body = match &node.body {
                Body::File(c) => SnapshotBody::File { content: c.clone() },
                Body::Dir(e) => SnapshotBody::Directory {
                    entries: e.iter().map(|(n, i)| (n.clone(), i.0)).collect(),
                },
                Body::Symlink(t) => SnapshotBody::Symlink { target: t.clone() },
            };
            inodes.insert(
                ino.0,
                SnapshotInode {
                    attr: node.attr.clone(),
                    body,
                },
            );
        }
        Snapshot {
            version: SNAPSHOT_VERSION,
            next_ino: self.next_ino,
            bytes_used: self.bytes_used,
            bytes_limit: self.bytes_limit,
            inodes,
        }
    }

    pub fn from_snapshot(snap: Snapshot) -> Result<Self, Errno> {
        if snap.version != SNAPSHOT_VERSION {
            return Err(Errno::Inval);
        }
        let mut inodes: HashMap<Ino, Inode> = HashMap::new();
        for (raw, sn) in snap.inodes.iter() {
            let ino = Ino(*raw);
            let body = match &sn.body {
                SnapshotBody::File { content } => Body::File(content.clone()),
                SnapshotBody::Directory { entries } => {
                    let mut map = BTreeMap::new();
                    for (name, child) in entries {
                        map.insert(name.clone(), Ino(*child));
                    }
                    Body::Dir(map)
                }
                SnapshotBody::Symlink { target } => Body::Symlink(target.clone()),
            };
            inodes.insert(
                ino,
                Inode {
                    attr: sn.attr.clone(),
                    body,
                    parent: None,
                },
            );
        }
        if !inodes.contains_key(&ROOT_INO) {
            return Err(Errno::Inval);
        }
        if let Some(r) = inodes.get_mut(&ROOT_INO) {
            r.parent = Some(ROOT_INO);
        }
        // Reconstruct directory parent pointers from directory entries.
        let dir_inos: Vec<Ino> = inodes
            .iter()
            .filter(|(_, n)| matches!(n.body, Body::Dir(_)))
            .map(|(i, _)| *i)
            .collect();
        let mut parent_fixups: Vec<(Ino, Ino)> = Vec::new();
        for d in &dir_inos {
            if let Some(node) = inodes.get(d)
                && let Body::Dir(entries) = &node.body
            {
                for child in entries.values() {
                    if let Some(c) = inodes.get(child)
                        && matches!(c.body, Body::Dir(_))
                    {
                        parent_fixups.push((*child, *d));
                    }
                }
            }
        }
        for (c, p) in parent_fixups {
            if let Some(n) = inodes.get_mut(&c) {
                n.parent = Some(p);
            }
        }
        Ok(Self {
            inodes,
            next_ino: snap.next_ino,
            bytes_used: snap.bytes_used,
            bytes_limit: snap.bytes_limit,
            time_source: Arc::new(Timespec::now),
        })
    }

    pub fn serialize(&self) -> Vec<u8> {
        let snap = self.to_snapshot();
        let mut out = Vec::new();
        out.extend_from_slice(SNAPSHOT_MAGIC);
        let body = bincode::serialize(&snap).expect("snapshot bincode serialize");
        out.extend_from_slice(&body);
        out
    }

    pub fn deserialize(bytes: &[u8]) -> Result<Self, Errno> {
        if bytes.len() < SNAPSHOT_MAGIC.len() {
            return Err(Errno::Inval);
        }
        let (magic, rest) = bytes.split_at(SNAPSHOT_MAGIC.len());
        if magic != SNAPSHOT_MAGIC {
            return Err(Errno::Inval);
        }
        let snap: Snapshot = bincode::deserialize(rest).map_err(|_| Errno::Io)?;
        Self::from_snapshot(snap)
    }

    // ---- Internal helpers -----------------------------------------------

    fn ensure_dir_no_entry(&self, parent: Ino, name: &[u8]) -> Result<(), Errno> {
        let p = self.get(parent)?;
        let entries = match &p.body {
            Body::Dir(e) => e,
            _ => return Err(Errno::NotDir),
        };
        if entries.contains_key(name) {
            return Err(Errno::Exist);
        }
        Ok(())
    }

    fn child_ino(&self, parent: Ino, name: &[u8]) -> Result<Ino, Errno> {
        let p = self.get(parent)?;
        let entries = match &p.body {
            Body::Dir(e) => e,
            _ => return Err(Errno::NotDir),
        };
        entries.get(name).copied().ok_or(Errno::NoEnt)
    }

    fn insert_entry(
        &mut self,
        parent: Ino,
        name: Vec<u8>,
        child: Ino,
        now: Timespec,
    ) -> Result<(), Errno> {
        let p = self.get_mut(parent)?;
        match &mut p.body {
            Body::Dir(e) => {
                e.insert(name, child);
                p.attr.size = e.len() as u64;
                p.attr.mtime = now;
                p.attr.ctime = now;
                Ok(())
            }
            _ => Err(Errno::NotDir),
        }
    }

    fn remove_entry(&mut self, parent: Ino, name: &[u8], now: Timespec) -> Result<Ino, Errno> {
        let p = self.get_mut(parent)?;
        match &mut p.body {
            Body::Dir(e) => {
                let ino = e.remove(name).ok_or(Errno::NoEnt)?;
                p.attr.size = e.len() as u64;
                p.attr.mtime = now;
                p.attr.ctime = now;
                Ok(ino)
            }
            _ => Err(Errno::NotDir),
        }
    }
}

fn validate_name(name: &[u8]) -> Result<(), Errno> {
    if name.is_empty() || name == b"." || name == b".." {
        return Err(Errno::Inval);
    }
    if name.len() > NAME_MAX {
        return Err(Errno::NameTooLong);
    }
    if name.iter().any(|b| *b == b'/' || *b == 0) {
        return Err(Errno::Inval);
    }
    Ok(())
}

/// Split an absolute path into its non-empty components.
fn split_path(path: &[u8]) -> Vec<&[u8]> {
    path.split(|b| *b == b'/')
        .filter(|c| !c.is_empty())
        .collect()
}

/// Build the path prefix up to (but not including) component `i`.
/// Returns a path string like `/a/b` (no trailing slash). For `i == 0`,
/// returns empty (meaning: the root directory).
fn prefix_path(path: &[u8], i: usize) -> Vec<u8> {
    let comps = split_path(path);
    if i == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for c in comps.iter().take(i) {
        out.push(b'/');
        out.extend_from_slice(c);
    }
    out
}

/// Split `/a/b/c` → (`/a/b`, `c`). Root (`/`) and empty are errors.
fn split_parent(path: &[u8]) -> Result<(Vec<u8>, Vec<u8>), Errno> {
    if path.is_empty() || path[0] != b'/' {
        return Err(Errno::Inval);
    }
    let comps = split_path(path);
    if comps.is_empty() {
        return Err(Errno::Inval);
    }
    let name = comps.last().unwrap().to_vec();
    let mut parent = Vec::new();
    for c in comps.iter().take(comps.len() - 1) {
        parent.push(b'/');
        parent.extend_from_slice(c);
    }
    Ok((parent, name))
}

// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn attr_of(fs: &Fs, ino: Ino) -> InodeAttr {
        fs.getattr(ino).expect("getattr")
    }

    #[test]
    fn empty_fs_root_is_dir() {
        let fs = Fs::new();
        let a = attr_of(&fs, ROOT_INO);
        assert_eq!(a.kind, Kind::Directory);
        assert_eq!(a.nlink, 2);
        assert_eq!(a.mode, 0o755);
        let ents = fs.readdir(ROOT_INO, 0).unwrap();
        assert_eq!(ents.len(), 2);
        assert_eq!(ents[0].name, b".");
        assert_eq!(ents[0].ino, ROOT_INO);
        assert_eq!(ents[1].name, b"..");
        assert_eq!(ents[1].ino, ROOT_INO);
    }

    #[test]
    fn create_write_read_file() {
        let mut fs = Fs::new();
        let a = fs.create(ROOT_INO, b"hello.txt", 0o644).unwrap();
        assert_eq!(a.kind, Kind::File);
        assert_eq!(a.nlink, 1);
        let n = fs.write(a.ino, 0, b"hello world").unwrap();
        assert_eq!(n, 11);
        let data = fs.read(a.ino, 0, 1024).unwrap();
        assert_eq!(data, b"hello world");
        assert_eq!(fs.bytes_used(), 11);
        assert_eq!(fs.read(a.ino, 6, 1024).unwrap(), b"world");
        assert_eq!(fs.read(a.ino, 6, 3).unwrap(), b"wor");
        assert_eq!(fs.read(a.ino, 99, 1024).unwrap(), b"");
    }

    #[test]
    fn write_at_offset_zero_fills() {
        let mut fs = Fs::new();
        let a = fs.create(ROOT_INO, b"f", 0o644).unwrap();
        fs.write(a.ino, 5, b"XY").unwrap();
        let data = fs.read(a.ino, 0, 100).unwrap();
        assert_eq!(data, b"\0\0\0\0\0XY");
        assert_eq!(fs.bytes_used(), 7);
    }

    #[test]
    fn mkdir_rmdir() {
        let mut fs = Fs::new();
        let d = fs.mkdir(ROOT_INO, b"sub", 0o755).unwrap();
        assert_eq!(d.kind, Kind::Directory);
        assert_eq!(attr_of(&fs, ROOT_INO).nlink, 3);
        let ents = fs.readdir(ROOT_INO, 0).unwrap();
        assert_eq!(ents.len(), 3);
        assert_eq!(ents[2].name, b"sub");
        fs.create(d.ino, b"f", 0o644).unwrap();
        assert_eq!(fs.rmdir(ROOT_INO, b"sub"), Err(Errno::NotEmpty));
        fs.unlink(d.ino, b"f").unwrap();
        fs.rmdir(ROOT_INO, b"sub").unwrap();
        assert_eq!(attr_of(&fs, ROOT_INO).nlink, 2);
    }

    #[test]
    fn unlink_file_unlink_dir_fails() {
        let mut fs = Fs::new();
        let f = fs.create(ROOT_INO, b"f", 0o644).unwrap();
        fs.write(f.ino, 0, b"abc").unwrap();
        fs.unlink(ROOT_INO, b"f").unwrap();
        assert_eq!(fs.bytes_used(), 0);
        assert_eq!(fs.lookup(ROOT_INO, b"f"), Err(Errno::NoEnt));
        fs.mkdir(ROOT_INO, b"d", 0o755).unwrap();
        assert_eq!(fs.unlink(ROOT_INO, b"d"), Err(Errno::IsDir));
        fs.create(ROOT_INO, b"g", 0o644).unwrap();
        assert_eq!(fs.rmdir(ROOT_INO, b"g"), Err(Errno::NotDir));
    }

    #[test]
    fn symlink_readlink_resolve() {
        let mut fs = Fs::new();
        let t = fs.create(ROOT_INO, b"target", 0o644).unwrap();
        fs.write(t.ino, 0, b"payload").unwrap();
        let s = fs.symlink(ROOT_INO, b"link", b"target").unwrap();
        assert_eq!(s.kind, Kind::Symlink);
        assert_eq!(fs.readlink(s.ino).unwrap(), b"target");
        assert_eq!(fs.resolve(b"/link").unwrap(), s.ino);
        assert_eq!(fs.read_path(b"/link").unwrap(), b"payload");
        let la = fs.lookup(ROOT_INO, b"link").unwrap();
        assert_eq!(la.kind, Kind::Symlink);
    }

    #[test]
    fn symlink_loop_elops() {
        let mut fs = Fs::new();
        fs.symlink(ROOT_INO, b"a", b"b").unwrap();
        fs.symlink(ROOT_INO, b"b", b"a").unwrap();
        assert_eq!(fs.read_path(b"/a"), Err(Errno::Io));
    }

    #[test]
    fn hardlink() {
        let mut fs = Fs::new();
        let a = fs.create(ROOT_INO, b"a", 0o644).unwrap();
        fs.write(a.ino, 0, b"same").unwrap();
        let b = fs.link(a.ino, ROOT_INO, b"b").unwrap();
        assert_eq!(b.ino, a.ino);
        assert_eq!(attr_of(&fs, a.ino).nlink, 2);
        assert_eq!(fs.read_path(b"/b").unwrap(), b"same");
        fs.unlink(ROOT_INO, b"a").unwrap();
        assert_eq!(fs.read_path(b"/b").unwrap(), b"same");
        assert_eq!(attr_of(&fs, a.ino).nlink, 1);
        assert_eq!(fs.bytes_used(), 4);
        fs.unlink(ROOT_INO, b"b").unwrap();
        assert_eq!(fs.bytes_used(), 0);
        fs.mkdir(ROOT_INO, b"d", 0o755).unwrap();
        let d_ino = fs.lookup(ROOT_INO, b"d").unwrap().ino;
        assert_eq!(fs.link(d_ino, ROOT_INO, b"d2"), Err(Errno::IsDir));
    }

    #[test]
    fn rename_file_and_overwrite() {
        let mut fs = Fs::new();
        let a = fs.create(ROOT_INO, b"a", 0o644).unwrap();
        fs.write(a.ino, 0, b"A").unwrap();
        fs.create(ROOT_INO, b"b", 0o644).unwrap();
        fs.write_path(b"/b", b"BBBB").unwrap();
        fs.rename(ROOT_INO, b"a", ROOT_INO, b"b").unwrap();
        assert_eq!(fs.lookup(ROOT_INO, b"a"), Err(Errno::NoEnt));
        assert_eq!(fs.read_path(b"/b").unwrap(), b"A");
        assert_eq!(fs.bytes_used(), 1);
    }

    #[test]
    fn rename_same_name_same_dir_noop() {
        let mut fs = Fs::new();
        fs.create(ROOT_INO, b"f", 0o644).unwrap();
        fs.rename(ROOT_INO, b"f", ROOT_INO, b"f").unwrap();
        fs.lookup(ROOT_INO, b"f").unwrap();
    }

    #[test]
    fn rename_dir_overwrite_empty_and_reject_non_empty() {
        let mut fs = Fs::new();
        fs.mkdir(ROOT_INO, b"src", 0o755).unwrap();
        fs.mkdir(ROOT_INO, b"dst", 0o755).unwrap();
        fs.rename(ROOT_INO, b"src", ROOT_INO, b"dst").unwrap();
        assert_eq!(fs.lookup(ROOT_INO, b"src"), Err(Errno::NoEnt));
        fs.mkdir(ROOT_INO, b"src2", 0o755).unwrap();
        let dst = fs.lookup(ROOT_INO, b"dst").unwrap().ino;
        fs.create(dst, b"file", 0o644).unwrap();
        assert_eq!(
            fs.rename(ROOT_INO, b"src2", ROOT_INO, b"dst"),
            Err(Errno::NotEmpty)
        );
    }

    #[test]
    fn rename_dir_into_descendant_fails() {
        let mut fs = Fs::new();
        fs.mkdir(ROOT_INO, b"a", 0o755).unwrap();
        let a = fs.lookup(ROOT_INO, b"a").unwrap().ino;
        fs.mkdir(a, b"b", 0o755).unwrap();
        let b = fs.lookup(a, b"b").unwrap().ino;
        fs.mkdir(b, b"c", 0o755).unwrap();
        assert_eq!(fs.rename(ROOT_INO, b"a", b, b"x"), Err(Errno::Inval));
    }

    #[test]
    fn rename_dir_across_parents_updates_nlink() {
        let mut fs = Fs::new();
        fs.mkdir(ROOT_INO, b"p1", 0o755).unwrap();
        fs.mkdir(ROOT_INO, b"p2", 0o755).unwrap();
        let p1 = fs.lookup(ROOT_INO, b"p1").unwrap().ino;
        let p2 = fs.lookup(ROOT_INO, b"p2").unwrap().ino;
        fs.mkdir(p1, b"d", 0o755).unwrap();
        assert_eq!(attr_of(&fs, p1).nlink, 3);
        assert_eq!(attr_of(&fs, p2).nlink, 2);
        fs.rename(p1, b"d", p2, b"d").unwrap();
        assert_eq!(attr_of(&fs, p1).nlink, 2);
        assert_eq!(attr_of(&fs, p2).nlink, 3);
        let d = fs.lookup(p2, b"d").unwrap().ino;
        let ents = fs.readdir(d, 0).unwrap();
        assert_eq!(ents[1].name, b"..");
        assert_eq!(ents[1].ino, p2);
    }

    #[test]
    fn truncate_grow_shrink() {
        let mut fs = Fs::new();
        let f = fs.create(ROOT_INO, b"f", 0o644).unwrap();
        fs.write(f.ino, 0, b"hello").unwrap();
        fs.truncate(f.ino, 8).unwrap();
        assert_eq!(fs.read(f.ino, 0, 100).unwrap(), b"hello\0\0\0");
        assert_eq!(fs.bytes_used(), 8);
        fs.truncate(f.ino, 3).unwrap();
        assert_eq!(fs.read(f.ino, 0, 100).unwrap(), b"hel");
        assert_eq!(fs.bytes_used(), 3);
    }

    #[test]
    fn memory_cap_enforced() {
        let mut fs = Fs::with_limit(4);
        let f = fs.create(ROOT_INO, b"f", 0o644).unwrap();
        fs.write(f.ino, 0, b"abcd").unwrap();
        let before = fs.read(f.ino, 0, 100).unwrap();
        assert_eq!(fs.write(f.ino, 4, b"e"), Err(Errno::NoSpc));
        let after = fs.read(f.ino, 0, 100).unwrap();
        assert_eq!(before, after);
        assert_eq!(fs.bytes_used(), 4);
        fs.write(f.ino, 0, b"WXYZ").unwrap();
        assert_eq!(fs.read(f.ino, 0, 100).unwrap(), b"WXYZ");
    }

    #[test]
    fn mkdir_p_creates_all() {
        let mut fs = Fs::new();
        fs.mkdir_p(b"/a/b/c", 0o755).unwrap();
        let c = fs.resolve(b"/a/b/c").unwrap();
        assert_eq!(attr_of(&fs, c).kind, Kind::Directory);
        fs.mkdir_p(b"/a/b/c", 0o755).unwrap();
    }

    #[test]
    fn write_path_requires_parent() {
        let mut fs = Fs::new();
        assert_eq!(fs.write_path(b"/missing/file", b"x"), Err(Errno::NoEnt));
        fs.mkdir_p(b"/d", 0o755).unwrap();
        fs.write_path(b"/d/file", b"hello").unwrap();
        assert_eq!(fs.read_path(b"/d/file").unwrap(), b"hello");
        fs.write_path(b"/d/file", b"hi").unwrap();
        assert_eq!(fs.read_path(b"/d/file").unwrap(), b"hi");
        assert_eq!(fs.bytes_used(), 2);
    }

    #[test]
    fn path_edge_cases() {
        let mut fs = Fs::new();
        assert_eq!(fs.resolve(b"/").unwrap(), ROOT_INO);
        fs.mkdir(ROOT_INO, b"foo", 0o755).unwrap();
        let foo = fs.resolve(b"//foo").unwrap();
        assert_eq!(fs.resolve(b"/foo/").unwrap(), foo);
        assert_eq!(fs.resolve(b"/./foo").unwrap(), foo);
        assert_eq!(fs.resolve(b"/foo/..").unwrap(), ROOT_INO);
        assert_eq!(fs.resolve(b""), Err(Errno::Inval));
        assert_eq!(fs.resolve(b"foo"), Err(Errno::Inval));
    }

    #[test]
    fn bad_names_rejected() {
        let mut fs = Fs::new();
        assert_eq!(fs.create(ROOT_INO, b"", 0o644).unwrap_err(), Errno::Inval);
        assert_eq!(fs.create(ROOT_INO, b".", 0o644).unwrap_err(), Errno::Inval);
        assert_eq!(fs.create(ROOT_INO, b"..", 0o644).unwrap_err(), Errno::Inval);
        assert_eq!(
            fs.create(ROOT_INO, b"a/b", 0o644).unwrap_err(),
            Errno::Inval
        );
        assert_eq!(
            fs.create(ROOT_INO, b"a\0b", 0o644).unwrap_err(),
            Errno::Inval
        );
        let long = vec![b'a'; 256];
        assert_eq!(
            fs.create(ROOT_INO, &long, 0o644).unwrap_err(),
            Errno::NameTooLong
        );
    }

    #[test]
    fn snapshot_round_trip() {
        let mut fs = Fs::with_limit(1024);
        fs.mkdir_p(b"/a/b", 0o755).unwrap();
        fs.write_path(b"/a/hello.txt", b"hello!").unwrap();
        fs.write_path(b"/a/b/data.bin", &[0xDE, 0xAD, 0xBE, 0xEF])
            .unwrap();
        fs.symlink(ROOT_INO, b"lnk", b"/a/hello.txt").unwrap();
        let used = fs.bytes_used();

        let bytes = fs.serialize();
        let fs2 = Fs::deserialize(&bytes).unwrap();
        assert_eq!(fs2.bytes_used(), used);
        assert_eq!(fs2.bytes_limit(), 1024);
        assert_eq!(fs2.read_path(b"/a/hello.txt").unwrap(), b"hello!");
        assert_eq!(
            fs2.read_path(b"/a/b/data.bin").unwrap(),
            &[0xDE, 0xAD, 0xBE, 0xEF]
        );
        assert_eq!(fs2.read_path(b"/lnk").unwrap(), b"hello!");
        let root_ents = fs2.readdir(ROOT_INO, 0).unwrap();
        let names: Vec<_> = root_ents.iter().map(|e| e.name.clone()).collect();
        assert!(names.iter().any(|n| n == b"a"));
        assert!(names.iter().any(|n| n == b"lnk"));
    }

    #[test]
    fn snapshot_bad_magic_and_version() {
        let mut bad = vec![b'X'; 8];
        bad.extend_from_slice(
            &bincode::serialize(&Snapshot {
                version: SNAPSHOT_VERSION,
                next_ino: 2,
                bytes_used: 0,
                bytes_limit: 0,
                inodes: BTreeMap::new(),
            })
            .unwrap(),
        );
        assert_eq!(Fs::deserialize(&bad).unwrap_err(), Errno::Inval);

        let mut v = Vec::new();
        v.extend_from_slice(SNAPSHOT_MAGIC);
        v.extend_from_slice(
            &bincode::serialize(&Snapshot {
                version: 99,
                next_ino: 2,
                bytes_used: 0,
                bytes_limit: 0,
                inodes: BTreeMap::new(),
            })
            .unwrap(),
        );
        assert_eq!(Fs::deserialize(&v).unwrap_err(), Errno::Inval);

        let mut v = Vec::new();
        v.extend_from_slice(SNAPSHOT_MAGIC);
        v.extend_from_slice(&[0xFF; 4]);
        assert_eq!(Fs::deserialize(&v).unwrap_err(), Errno::Io);
    }

    #[test]
    fn setattr_changes_mode_and_size() {
        let mut fs = Fs::new();
        let f = fs.create(ROOT_INO, b"f", 0o644).unwrap();
        fs.write(f.ino, 0, b"abcdef").unwrap();
        let a = fs
            .setattr(
                f.ino,
                SetAttr {
                    mode: Some(0o600),
                    size: Some(3),
                    ..SetAttr::default()
                },
            )
            .unwrap();
        assert_eq!(a.mode, 0o600);
        assert_eq!(a.size, 3);
        assert_eq!(fs.read(f.ino, 0, 100).unwrap(), b"abc");
    }

    #[test]
    fn bad_inode_is_noent() {
        let fs = Fs::new();
        assert_eq!(fs.getattr(Ino(9999)).unwrap_err(), Errno::NoEnt);
    }

    #[test]
    fn dup_name_exist() {
        let mut fs = Fs::new();
        fs.create(ROOT_INO, b"a", 0o644).unwrap();
        assert_eq!(fs.create(ROOT_INO, b"a", 0o644).unwrap_err(), Errno::Exist);
        assert_eq!(fs.mkdir(ROOT_INO, b"a", 0o755).unwrap_err(), Errno::Exist);
        assert_eq!(fs.symlink(ROOT_INO, b"a", b"x").unwrap_err(), Errno::Exist);
    }

    #[test]
    fn read_write_on_dir_is_isdir() {
        let mut fs = Fs::new();
        fs.mkdir(ROOT_INO, b"d", 0o755).unwrap();
        let d = fs.lookup(ROOT_INO, b"d").unwrap().ino;
        assert_eq!(fs.read(d, 0, 1).unwrap_err(), Errno::IsDir);
        assert_eq!(fs.write(d, 0, b"x").unwrap_err(), Errno::IsDir);
        assert_eq!(fs.truncate(d, 0).unwrap_err(), Errno::IsDir);
    }

    #[test]
    fn deterministic_time_source() {
        let mut fs = Fs::new();
        fs.set_time_source(|| Timespec { secs: 42, nanos: 7 });
        let a = fs.create(ROOT_INO, b"f", 0o644).unwrap();
        assert_eq!(a.mtime, Timespec { secs: 42, nanos: 7 });
    }
}
