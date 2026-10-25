//! The local writer of folder downloads (§6.6 "Folders", §9.4): the only place that creates
//! user-visible local directories. A documented exception to "local opens go through
//! `local_path`", like the `.part` file of a single download: the destination comes from the
//! native folder dialog (`OsChosen`), and everything below it is reached relative to verified
//! directory handles, never by re-resolving a path another local user could redirect.
//!
//! - **Unix**: every directory is opened with `openat(O_DIRECTORY | O_NOFOLLOW)` from the
//!   job's root descriptor, created with `mkdirat` (mode 0700 while the job runs, the umask
//!   default afterwards), files with `openat(O_CREAT | O_EXCL | O_NOFOLLOW)`, renamed with
//!   `renameat` / a no-replace rename. A symlink planted in a shared parent (`/tmp`) can never
//!   redirect a write.
//! - **Windows**: every directory on the way is opened with
//!   `FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT` and without `FILE_SHARE_DELETE`
//!   (so it can be neither renamed nor replaced while the handle is held), a name-surrogate
//!   reparse point (symlink, junction, mount point) is refused, and its final path
//!   (`GetFinalPathNameByHandleW`) must be the expected path below the root. Each `.part` is
//!   checked the same way right after `CREATE_NEW`, before any byte is written; the rename is
//!   `SetFileInformationByHandle(FileRenameInfo)` on the `.part` handle relative to the pinned
//!   parent directory handle, replacing only when the conflict decision allows it.
//! - Created trees inherit the parent's ACL / umask like single-file downloads (decision D6).
//!
//! Names given to this module are single components that went through
//! `sftp::tree::sanitize_component` (no separators, no `.`/`..`, no reserved device names,
//! no trailing dots or spaces); they are checked again here and refused otherwise.

use std::collections::HashSet;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// Why a local tree operation failed.
#[derive(Debug)]
pub enum TreeError {
    /// A directory on the way is a symlink, junction or other name-surrogate reparse point, or
    /// not a directory, or its final path is not below the root.
    Untrusted,
    /// Something exists where a new entry was to be created (never replaced).
    Exists,
    /// A name or the whole path is too long for the file system.
    NameTooLong,
    DiskFull,
    PermissionDenied,
    NotFound,
    /// A directory is where a file was to be written.
    IsADirectory,
    /// A name that is not a single safe component (a bug in the caller).
    InvalidName,
    Io(io::Error),
}

impl TreeError {
    pub fn from_io(e: io::Error) -> Self {
        #[cfg(unix)]
        if let Some(code) = e.raw_os_error() {
            match code {
                libc::ELOOP | libc::ENOTDIR => return Self::Untrusted,
                libc::EEXIST => return Self::Exists,
                libc::ENAMETOOLONG => return Self::NameTooLong,
                libc::ENOSPC | libc::EDQUOT => return Self::DiskFull,
                libc::EISDIR => return Self::IsADirectory,
                _ => {}
            }
        }
        #[cfg(windows)]
        if let Some(code) = e.raw_os_error().and_then(|c| u32::try_from(c).ok()) {
            use windows_sys::Win32::Foundation as F;
            match code {
                F::ERROR_ALREADY_EXISTS | F::ERROR_FILE_EXISTS => return Self::Exists,
                F::ERROR_FILENAME_EXCED_RANGE => return Self::NameTooLong,
                F::ERROR_DISK_FULL | F::ERROR_HANDLE_DISK_FULL => return Self::DiskFull,
                F::ERROR_DIRECTORY => return Self::Untrusted,
                _ => {}
            }
        }
        match e.kind() {
            io::ErrorKind::AlreadyExists => Self::Exists,
            io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => Self::DiskFull,
            io::ErrorKind::InvalidFilename => Self::NameTooLong,
            io::ErrorKind::IsADirectory => Self::IsADirectory,
            io::ErrorKind::NotADirectory => Self::Untrusted,
            _ => Self::Io(e),
        }
    }
}

impl std::fmt::Display for TreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            other => write!(f, "{other:?}"),
        }
    }
}

/// What is at a local name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Missing,
    /// A regular file: size, mtime (Unix seconds), and whether this job created it.
    File {
        size: u64,
        mtime: Option<i64>,
        ours: bool,
    },
    /// A symlink, junction or other reparse point (replaced as an entry, never written through).
    /// `dir`: Windows junction or directory symlink (a directory entry: a file is never renamed
    /// over it); always `false` on Unix (`rename` replaces any symlink).
    Link {
        dir: bool,
    },
    Dir,
    /// A FIFO, socket, device.
    Other,
}

/// What exists at the destination root (`<parent>/<name>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootState {
    Missing,
    /// A real directory (merge possible).
    Dir,
    /// A file, link or anything else (only "keep both").
    Other,
}

/// A file's identity: `(dev, ino)` on Unix, `(volume serial, file index)` on Windows.
pub type FileId = (u64, u64);

/// The files a folder job created, kept across its retries (a retry never replaces them).
pub type CreatedSet = Arc<Mutex<HashSet<FileId>>>;

/// A verified directory of the tree.
#[derive(Debug)]
struct DirRef {
    #[cfg(unix)]
    fd: std::os::fd::OwnedFd,
    /// While it is open (without `FILE_SHARE_DELETE`) the directory can be neither renamed nor
    /// deleted, so the paths below it keep naming verified directories.
    #[cfg(windows)]
    handle: std::os::windows::io::OwnedHandle,
    /// Unix: for messages only. Windows: the verified final path (`\\?\…`), the base of every
    /// child path.
    path: PathBuf,
}

/// The directories from the root down to one directory. Windows keeps every handle (each
/// pins its directory); Unix only the last descriptor (`openat` needs no more).
struct Chain {
    root: Arc<DirRef>,
    dirs: Vec<DirRef>,
}

impl Chain {
    fn last(&self) -> &DirRef {
        self.dirs.last().unwrap_or(&self.root)
    }
}

/// A `.part` file being written below the root; [`LocalTree::commit`] or
/// [`LocalTree::discard`] ends it.
pub struct TreePart {
    chain: Chain,
    part_name: String,
    /// The `.part`'s identity (it counts as this job's file; removal checks it).
    id: FileId,
    /// For messages.
    pub path: PathBuf,
}

/// One folder download's local destination.
pub struct LocalTree {
    root: Arc<DirRef>,
    /// The chosen parent folder and the root's name in it (cancel removes an empty root).
    parent: DirRef,
    root_name: String,
    /// `<parent>/<name>` as shown in messages.
    shown: PathBuf,
    /// Files this job created (a later entry never replaces one of them), shared with the
    /// job's retries.
    created: CreatedSet,
    /// `.part` files being written (also never replaced or reported as a user's file).
    in_progress: Mutex<HashSet<FileId>>,
    /// Windows: the current user's SID (a new folder must be owned by it).
    #[cfg(windows)]
    user_sid: Option<String>,
}

impl std::fmt::Debug for LocalTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalTree").finish_non_exhaustive()
    }
}

/// A single safe path component (see the module docs).
fn check_name(name: &str) -> Result<(), TreeError> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0', ':'])
        || name.ends_with(['.', ' '])
        || super::local_path::is_reserved_device_name(name);
    if bad {
        Err(TreeError::InvalidName)
    } else {
        Ok(())
    }
}

/// The `.part` name of `name`: dot-prefixed with a random suffix, so a server name in the same
/// folder (`x.part` next to `x`) does not name another file's temporary file by accident
/// (and if one ever does, the identity check treats the `.part` as this job's file).
fn part_name(name: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let suffix = id.get(..8).unwrap_or("0");
    format!(".{name}.{suffix}.part")
}

/// Unix: the mode a new folder gets by default (`0o777 & !umask`), learned once per process
/// (Linux `/proc/self/status`, else a probe folder in the temp directory) instead of creating
/// the job's folders with it: they are created 0700 and get this mode at the end.
#[cfg(unix)]
fn default_dir_mode() -> u32 {
    static MODE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| {
        let from_proc = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("Umask:"))
                    .and_then(|v| u32::from_str_radix(v.trim(), 8).ok())
            });
        if let Some(umask) = from_proc {
            return 0o777 & !umask;
        }
        use std::os::unix::fs::PermissionsExt;
        let probe =
            std::env::temp_dir().join(format!(".shellfin-umask-{}", uuid::Uuid::new_v4().simple()));
        let mode = std::fs::create_dir(&probe)
            .and_then(|()| std::fs::symlink_metadata(&probe))
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o755);
        let _ = std::fs::remove_dir(&probe);
        mode
    })
}

/// `name (N)`: the "keep both" names of a destination root, `N` = 2…99.
pub fn keep_both_names(name: &str) -> impl Iterator<Item = String> + '_ {
    (2..=99).map(move |n| format!("{name} ({n})"))
}

impl LocalTree {
    /// What is at `<parent>/<name>` (`parent` comes from the folder dialog and is followed if
    /// it is a link; `name` is not).
    pub fn inspect(parent: &Path, name: &str) -> Result<RootState, TreeError> {
        check_name(name)?;
        let parent = sys::open_parent(parent)?;
        Ok(match sys::probe(&parent, name, &|_| false)? {
            Probe::Missing => RootState::Missing,
            Probe::Dir => RootState::Dir,
            _ => RootState::Other,
        })
    }

    /// Open the destination root `<parent>/<name>`: `create` = a new directory (`Exists` if
    /// anything is there), else an existing real directory (merge; `Untrusted` for a link,
    /// junction or file). `created` = the files this job wrote so far (empty for a new job).
    pub fn open(
        parent: &Path,
        name: &str,
        create: bool,
        created: CreatedSet,
    ) -> Result<Self, TreeError> {
        check_name(name)?;
        let parent_dir = sys::open_parent(parent)?;
        if create {
            sys::mkdir(&parent_dir, name)?;
        }
        let root = sys::open_child_dir(&parent_dir, name)?;
        let t = Self {
            root: Arc::new(root),
            parent: parent_dir,
            root_name: name.to_owned(),
            shown: parent.join(name),
            created,
            in_progress: Mutex::new(HashSet::new()),
            #[cfg(windows)]
            user_sid: crate::platform::win_security::current_user_sid().ok(),
        };
        if create {
            t.after_mkdir(&t.root)?;
        }
        Ok(t)
    }

    /// [`LocalTree::open`] for a new job (tests).
    #[cfg(test)]
    fn open_new(parent: &Path, name: &str, create: bool) -> Result<Self, TreeError> {
        Self::open(parent, name, create, CreatedSet::default())
    }

    /// Whether `id` is a file of this job (written, or a `.part` being written).
    fn is_ours(&self, created: &HashSet<FileId>, id: FileId) -> bool {
        created.contains(&id)
            || self
                .in_progress
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&id)
    }

    /// Close the tree and remove the root folder if it is empty (cancel of a job that created
    /// it; best effort, never recursive).
    pub fn remove_root_if_empty(self) {
        let Self {
            root,
            parent,
            root_name,
            ..
        } = self;
        drop(root);
        if let Err(e) = sys::rmdir_if_empty(&parent, &root_name) {
            tracing::debug!(error = %e, "kept a destination folder that is not empty");
        }
    }

    /// The path of `rel` below the root, for messages.
    pub fn shown(&self, rel: &[String]) -> PathBuf {
        let mut p = self.shown.clone();
        for c in rel {
            p.push(c);
        }
        p
    }

    fn chain(&self, rel: &[String]) -> Result<Chain, TreeError> {
        let mut chain = Chain {
            root: self.root.clone(),
            dirs: Vec::new(),
        };
        for c in rel {
            check_name(c)?;
            let next = sys::open_child_dir(chain.last(), c)?;
            #[cfg(unix)]
            chain.dirs.clear();
            chain.dirs.push(next);
        }
        Ok(chain)
    }

    /// A folder this job just created (Unix: 0700 until [`LocalTree::finish_dir`]) and opened
    /// again: it must belong to the current user, so another local user who removed it and
    /// put their own in its place (a non-sticky shared parent) is not written into. macOS: the
    /// quarantine attribute (Gatekeeper evaluates it on `.app` bundle directories).
    fn after_mkdir(&self, dir: &DirRef) -> Result<(), TreeError> {
        #[cfg(unix)]
        if !sys::owned_by_me(dir).map_err(TreeError::from_io)? {
            return Err(TreeError::Untrusted);
        }
        #[cfg(windows)]
        {
            let owner = crate::platform::win_security::security_of_file(&dir.handle)
                .map_err(TreeError::from_io)?
                .owner;
            // An elevated process creates objects owned by Administrators.
            let mine = owner
                .as_deref()
                .is_some_and(|o| Some(o) == self.user_sid.as_deref() || o == "S-1-5-32-544");
            if !mine {
                return Err(TreeError::Untrusted);
            }
        }
        #[cfg(target_os = "macos")]
        if let Err(e) = sys::quarantine(dir) {
            tracing::debug!(error = %e, "could not mark a downloaded folder");
        }
        Ok(())
    }

    /// Make sure the directory `rel` (≥ 1 component) exists below the root as a real directory:
    /// every existing component must be a directory (no link, junction or file). Returns whether
    /// the last component was created by this call.
    pub fn ensure_dir(&self, rel: &[String]) -> Result<bool, TreeError> {
        let Some((last, parents)) = rel.split_last() else {
            return Ok(false);
        };
        check_name(last)?;
        let chain = self.chain(parents)?;
        let created = match sys::mkdir(chain.last(), last) {
            Ok(()) => true,
            Err(TreeError::Exists) => false,
            Err(e) => return Err(e),
        };
        let dir = sys::open_child_dir(chain.last(), last)?;
        if created {
            self.after_mkdir(&dir)?;
        }
        Ok(created)
    }

    /// What is at `rel_dir/name`.
    pub fn probe(&self, rel_dir: &[String], name: &str) -> Result<Probe, TreeError> {
        check_name(name)?;
        let chain = self.chain(rel_dir)?;
        let created = self.created.lock().unwrap_or_else(PoisonError::into_inner);
        sys::probe(chain.last(), name, &|id| self.is_ours(&created, id))
    }

    /// Create `rel_dir/.<name>.<random>.part` exclusively, without following links.
    pub fn create_part(
        &self,
        rel_dir: &[String],
        name: &str,
    ) -> Result<(TreePart, File), TreeError> {
        check_name(name)?;
        let chain = self.chain(rel_dir)?;
        // Windows: a `.part` still pending deletion (a scanner holds it) refuses `CREATE_NEW`
        // with "access denied": try the next name, report the error only if none works.
        let mut denied = None;
        for _ in 0..100u32 {
            let part_name = part_name(name);
            match sys::create_file(chain.last(), &part_name) {
                Ok(file) => {
                    let id = match sys::file_id(&file) {
                        Ok(id) => id,
                        Err(e) => {
                            let _ = sys::unlink(chain.last(), &part_name, Some(&file), None);
                            return Err(TreeError::from_io(e));
                        }
                    };
                    self.in_progress
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(id);
                    let mut path = self.shown(rel_dir);
                    path.push(&part_name);
                    return Ok((
                        TreePart {
                            chain,
                            part_name,
                            id,
                            path,
                        },
                        file,
                    ));
                }
                Err(TreeError::Exists) => {}
                Err(TreeError::PermissionDenied) if cfg!(windows) => {
                    denied = Some(TreeError::PermissionDenied);
                }
                Err(e) => return Err(e),
            }
        }
        Err(denied.unwrap_or(TreeError::Exists))
    }

    /// Finish a `.part`: provenance mark, `times` (failure logged), then the rename to `name` in
    /// the same verified directory. `replace` = the conflict decision allows replacing an
    /// existing file (or a link, replaced as an entry); a directory is never replaced, nor a
    /// file this job created. Without `replace` an existing entry is `Exists`.
    pub fn commit(
        &self,
        part: &TreePart,
        file: &File,
        name: &str,
        replace: bool,
        times: Option<std::fs::FileTimes>,
    ) -> Result<(), TreeError> {
        check_name(name)?;
        let dir = part.chain.last();
        if let Err(e) = sys::mark(dir, &part.part_name, file) {
            tracing::warn!(error = %e, "could not mark a downloaded file");
        }
        if let Some(t) = times
            && let Err(e) = file.set_times(t)
        {
            tracing::info!(error = %e, "could not set the download's timestamps");
        }
        let id = part.id;
        // Held across the check, the rename and the insert: two files of this job whose names
        // collide past the case folding (NFC/NFD, the NTFS upcase table) cannot both pass.
        let mut created = self.created.lock().unwrap_or_else(PoisonError::into_inner);
        if replace {
            // A file of this job (written, or another file's `.part`) is never replaced.
            match sys::probe(dir, name, &|i| i != id && self.is_ours(&created, i))? {
                Probe::Dir | Probe::Link { dir: true } => return Err(TreeError::IsADirectory),
                Probe::File { ours: true, .. } => return Err(TreeError::Exists),
                _ => {}
            }
        }
        sys::rename(dir, &part.part_name, file, name, replace)?;
        created.insert(id);
        self.in_progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        Ok(())
    }

    /// The root folder's identity (a retry refuses a root that is no longer the same folder).
    pub fn root_id(&self) -> Option<(u64, u64)> {
        sys::dir_id(&self.root).ok()
    }

    /// Remove an unfinished `.part` (best effort).
    pub fn discard(&self, part: &TreePart, file: Option<&File>) {
        // Only the `.part` itself: Windows deletes through its handle, Unix checks that the
        // name still is that file.
        if let Err(e) = sys::unlink(part.chain.last(), &part.part_name, file, Some(part.id)) {
            tracing::debug!(error = %e, "could not remove a .part file");
        }
        self.in_progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&part.id);
    }

    /// Remove the directory `rel` if it is empty (never recursive; best effort).
    pub fn remove_if_empty(&self, rel: &[String]) {
        let Some((last, parents)) = rel.split_last() else {
            return;
        };
        if let Ok(chain) = self.chain(parents)
            && let Err(e) = sys::rmdir_if_empty(chain.last(), last)
        {
            tracing::debug!(error = %e, "kept a folder that is not empty");
        }
    }

    /// Unix: give a directory the job created its default mode (`0o777 & !umask`, plus a
    /// setgid bit inherited from the parent) at the end of the job (also after cancel or
    /// failure). `rel` empty = the root. No-op elsewhere.
    pub fn finish_dir(&self, rel: &[String]) {
        #[cfg(unix)]
        {
            let dir = match rel.split_last() {
                None => Ok(None),
                Some((last, parents)) => self
                    .chain(parents)
                    .and_then(|c| sys::open_child_dir(c.last(), last))
                    .map(Some),
            };
            let set = |d: &DirRef| {
                let sgid = sys::mode_of(d)? & 0o2000;
                sys::set_mode(d, default_dir_mode() | sgid)
            };
            let result = match &dir {
                Ok(Some(d)) => set(d),
                Ok(None) => set(&self.root),
                Err(_) => Ok(()),
            };
            if let Err(e) = result {
                tracing::debug!(error = %e, "could not set a folder's final mode");
            }
        }
        #[cfg(not(unix))]
        let _ = rel;
    }

    /// Free bytes for this user on the root's volume.
    pub fn free_space(&self) -> io::Result<u64> {
        sys::free_space(&self.root)
    }
}

#[cfg(unix)]
mod sys {
    //! `*at` system calls relative to verified directory descriptors.

    use std::ffi::CString;
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::path::Path;

    use super::{DirRef, FileId, Probe, TreeError};

    fn cstr(name: &str) -> Result<CString, TreeError> {
        CString::new(name).map_err(|_| TreeError::InvalidName)
    }

    fn last_err() -> TreeError {
        TreeError::from_io(io::Error::last_os_error())
    }

    pub(super) fn open_parent(path: &Path) -> Result<DirRef, TreeError> {
        use std::os::unix::fs::OpenOptionsExt;
        // The folder the user chose: links are followed (as for a chosen file); it must be a
        // directory.
        let f = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(path)
            .map_err(TreeError::from_io)?;
        Ok(DirRef {
            fd: OwnedFd::from(f),
            path: path.to_path_buf(),
        })
    }

    pub(super) fn open_child_dir(parent: &DirRef, name: &str) -> Result<DirRef, TreeError> {
        let c = cstr(name)?;
        // SAFETY: `c` is NUL-terminated; the descriptor is owned by `parent`.
        let fd = unsafe {
            libc::openat(
                parent.fd.as_raw_fd(),
                c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            // A symlink (ELOOP) or a file (ENOTDIR) where a directory belongs: `Untrusted`.
            return Err(last_err());
        }
        // SAFETY: `fd` is a new descriptor owned by nobody else.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        Ok(DirRef {
            fd,
            path: parent.path.join(name),
        })
    }

    pub(super) fn mkdir(parent: &DirRef, name: &str) -> Result<(), TreeError> {
        let c = cstr(name)?;
        // SAFETY: `c` is NUL-terminated; the descriptor is owned by `parent`.
        // 0700 from the start (no window with the default mode); `finish_dir` sets the default.
        let rc = unsafe { libc::mkdirat(parent.fd.as_raw_fd(), c.as_ptr(), 0o700) };
        if rc == 0 { Ok(()) } else { Err(last_err()) }
    }

    fn stat_at(parent: &DirRef, name: &str) -> Result<Option<libc::stat>, TreeError> {
        let c = cstr(name)?;
        // SAFETY: zeroed `stat` is a valid out-buffer.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `c` is NUL-terminated, `st` is writable; links are not followed.
        let rc = unsafe {
            libc::fstatat(
                parent.fd.as_raw_fd(),
                c.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc == 0 {
            return Ok(Some(st));
        }
        match last_err() {
            TreeError::NotFound => Ok(None),
            e => Err(e),
        }
    }

    #[allow(clippy::unnecessary_cast)] // the stat field types differ between Linux and macOS
    pub(super) fn probe(
        parent: &DirRef,
        name: &str,
        ours: &dyn Fn(FileId) -> bool,
    ) -> Result<Probe, TreeError> {
        let Some(st) = stat_at(parent, name)? else {
            return Ok(Probe::Missing);
        };
        Ok(match st.st_mode & libc::S_IFMT {
            libc::S_IFDIR => Probe::Dir,
            libc::S_IFLNK => Probe::Link { dir: false },
            libc::S_IFREG => Probe::File {
                size: u64::try_from(st.st_size).unwrap_or(0),
                mtime: Some(st.st_mtime as i64),
                ours: ours((st.st_dev as u64, st.st_ino as u64)),
            },
            _ => Probe::Other,
        })
    }

    pub(super) fn create_file(parent: &DirRef, name: &str) -> Result<File, TreeError> {
        let c = cstr(name)?;
        // SAFETY: `c` is NUL-terminated; mode as for a single download (`0o666 & !umask`).
        let fd = unsafe {
            libc::openat(
                parent.fd.as_raw_fd(),
                c.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o666 as libc::c_uint,
            )
        };
        if fd < 0 {
            return Err(last_err());
        }
        // SAFETY: `fd` is a new descriptor owned by nobody else.
        Ok(File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    #[allow(clippy::unnecessary_cast)]
    pub(super) fn file_id(file: &File) -> io::Result<FileId> {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata()?;
        Ok((m.dev() as u64, m.ino() as u64))
    }

    /// macOS reports a missing file-system feature as `ENOTSUP`, which differs from
    /// `EOPNOTSUPP` there (on Linux they are the same value).
    fn no_link_support(e: &io::Error) -> bool {
        #[cfg(target_os = "macos")]
        {
            e.raw_os_error() == Some(libc::ENOTSUP)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = e;
            false
        }
    }

    /// Rename without replacing: Linux `renameat2(RENAME_NOREPLACE)`, macOS
    /// `renameatx_np(RENAME_EXCL)`; where the file system does not support it, `linkat` +
    /// `unlinkat` (a hard link fails on an existing name).
    fn rename_noreplace(dir: &DirRef, from: &CString, to: &CString) -> Result<(), TreeError> {
        let fd = dir.fd.as_raw_fd();
        #[cfg(target_os = "linux")]
        {
            // The raw system call (Linux 3.15+): no glibc 2.28 / musl version requirement.
            // SAFETY: NUL-terminated names, descriptor owned by `dir`; the arguments match
            // `renameat2(int, const char *, int, const char *, unsigned int)`.
            let rc = unsafe {
                libc::syscall(
                    libc::SYS_renameat2,
                    fd,
                    from.as_ptr(),
                    fd,
                    to.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if rc == 0 {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            if !matches!(e.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS)) {
                return Err(TreeError::from_io(e));
            }
        }
        #[cfg(target_os = "macos")]
        {
            // SAFETY: NUL-terminated names, descriptor owned by `dir`.
            let rc = unsafe {
                libc::renameatx_np(fd, from.as_ptr(), fd, to.as_ptr(), libc::RENAME_EXCL)
            };
            if rc == 0 {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            if !matches!(e.raw_os_error(), Some(libc::EINVAL | libc::ENOTSUP)) {
                return Err(TreeError::from_io(e));
            }
        }
        // SAFETY: NUL-terminated names, descriptor owned by `dir`; links are not followed.
        let rc = unsafe { libc::linkat(fd, from.as_ptr(), fd, to.as_ptr(), 0) };
        if rc != 0 {
            let e = io::Error::last_os_error();
            if !matches!(
                e.raw_os_error(),
                Some(libc::EPERM | libc::EOPNOTSUPP | libc::EMLINK)
            ) && !no_link_support(&e)
            {
                return Err(TreeError::from_io(e));
            }
            // No hard links on this file system (FAT, some FUSE): check, then rename (a name
            // that appears in between is the only race left; documented in ARCHITECTURE §6.6).
            let name = to.to_str().map_err(|_| TreeError::InvalidName)?;
            if stat_at(dir, name)?.is_some() {
                return Err(TreeError::Exists);
            }
            // SAFETY: as above.
            let rc = unsafe { libc::renameat(fd, from.as_ptr(), fd, to.as_ptr()) };
            return if rc == 0 { Ok(()) } else { Err(last_err()) };
        }
        // SAFETY: as above.
        if unsafe { libc::unlinkat(fd, from.as_ptr(), 0) } != 0 {
            tracing::debug!(
                error = %io::Error::last_os_error(),
                "could not remove the .part name after linking"
            );
        }
        Ok(())
    }

    #[allow(clippy::unnecessary_cast)]
    pub(super) fn dir_id(dir: &DirRef) -> io::Result<FileId> {
        use std::os::unix::fs::MetadataExt;
        let f = std::mem::ManuallyDrop::new(
            // SAFETY: borrows the descriptor without taking ownership (never dropped).
            unsafe { File::from_raw_fd(dir.fd.as_raw_fd()) },
        );
        let m = f.metadata()?;
        Ok((m.dev() as u64, m.ino() as u64))
    }

    pub(super) fn rename(
        dir: &DirRef,
        from: &str,
        _file: &File,
        to: &str,
        replace: bool,
    ) -> Result<(), TreeError> {
        let (from, to) = (cstr(from)?, cstr(to)?);
        if !replace {
            return rename_noreplace(dir, &from, &to);
        }
        let fd = dir.fd.as_raw_fd();
        // SAFETY: NUL-terminated names, descriptor owned by `dir`. `renameat` replaces a link
        // as an entry and refuses a directory.
        let rc = unsafe { libc::renameat(fd, from.as_ptr(), fd, to.as_ptr()) };
        if rc == 0 {
            Ok(())
        } else {
            match last_err() {
                TreeError::Io(e) if e.raw_os_error() == Some(libc::ENOTEMPTY) => {
                    Err(TreeError::IsADirectory)
                }
                e => Err(e),
            }
        }
    }

    /// Remove `name` only if it still is the file `expected` (or `file`): a name another file
    /// took meanwhile is left alone.
    #[allow(clippy::unnecessary_cast)]
    pub(super) fn unlink(
        dir: &DirRef,
        name: &str,
        file: Option<&File>,
        expected: Option<FileId>,
    ) -> io::Result<()> {
        let want = match (expected, file) {
            (Some(id), _) => id,
            (None, Some(f)) => file_id(f)?,
            (None, None) => return Err(io::Error::from(io::ErrorKind::InvalidInput)),
        };
        let st = stat_at(dir, name)
            .map_err(|_| io::Error::from(io::ErrorKind::NotFound))?
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        if (st.st_dev as u64, st.st_ino as u64) != want {
            return Err(io::Error::other("the name is no longer this file"));
        }
        let c = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: NUL-terminated name, descriptor owned by `dir`.
        let rc = unsafe { libc::unlinkat(dir.fd.as_raw_fd(), c.as_ptr(), 0) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(super) fn rmdir_if_empty(dir: &DirRef, name: &str) -> io::Result<()> {
        let c = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: NUL-terminated name, descriptor owned by `dir`. `AT_REMOVEDIR` removes only an
        // empty directory and never a link.
        let rc = unsafe { libc::unlinkat(dir.fd.as_raw_fd(), c.as_ptr(), libc::AT_REMOVEDIR) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(super) fn mode_of(dir: &DirRef) -> io::Result<u32> {
        use std::os::unix::fs::PermissionsExt;
        let f = std::mem::ManuallyDrop::new(
            // SAFETY: borrows the descriptor without taking ownership (never dropped).
            unsafe { File::from_raw_fd(dir.fd.as_raw_fd()) },
        );
        Ok(f.metadata()?.permissions().mode() & 0o7777)
    }

    /// The directory belongs to the effective user.
    pub(super) fn owned_by_me(dir: &DirRef) -> io::Result<bool> {
        use std::os::unix::fs::MetadataExt;
        let f = std::mem::ManuallyDrop::new(
            // SAFETY: borrows the descriptor without taking ownership (never dropped).
            unsafe { File::from_raw_fd(dir.fd.as_raw_fd()) },
        );
        // SAFETY: `geteuid` has no preconditions.
        let me = unsafe { libc::geteuid() };
        Ok(f.metadata()?.uid() == me)
    }

    pub(super) fn set_mode(dir: &DirRef, mode: u32) -> io::Result<()> {
        // SAFETY: the descriptor is owned by `dir`.
        let rc = unsafe { libc::fchmod(dir.fd.as_raw_fd(), mode as libc::mode_t) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[allow(clippy::unnecessary_cast)]
    pub(super) fn free_space(dir: &DirRef) -> io::Result<u64> {
        // SAFETY: zeroed `statvfs` is a valid out-buffer.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: descriptor owned by `dir`, `st` writable.
        let rc = unsafe { libc::fstatvfs(dir.fd.as_raw_fd(), &mut st) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    }

    /// Provenance mark on the open `.part` (macOS quarantine with `fsetxattr`; nothing on
    /// Linux).
    pub(super) fn mark(_dir: &DirRef, _name: &str, file: &File) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            quarantine_fd(file.as_raw_fd())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = file;
            Ok(())
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn quarantine(dir: &DirRef) -> io::Result<()> {
        quarantine_fd(dir.fd.as_raw_fd())
    }

    #[cfg(target_os = "macos")]
    fn quarantine_fd(fd: libc::c_int) -> io::Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let value = format!("0081;{now:x};Shellfin;");
        let name = c"com.apple.quarantine";
        // SAFETY: `name` is NUL-terminated; `value` is valid for `value.len()` bytes.
        let rc =
            unsafe { libc::fsetxattr(fd, name.as_ptr(), value.as_ptr().cast(), value.len(), 0, 0) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(windows)]
mod sys {
    //! Handles opened without following reparse points, final-path checks, handle-relative
    //! rename and delete.

    use std::ffi::OsString;
    use std::fs::File;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::{Path, PathBuf};

    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW, DELETE,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_TAG_INFO, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_TRAVERSE, FileAttributeTagInfo, FileDispositionInfo, FileRenameInfo,
        GetDiskFreeSpaceExW, GetFileInformationByHandle, GetFileInformationByHandleEx,
        OPEN_EXISTING, READ_CONTROL, SYNCHRONIZE, SetFileInformationByHandle,
    };

    use super::{DirRef, FileId, Probe, TreeError};

    /// `IsReparseTagNameSurrogate`: symlinks, junctions, mount points (they name another
    /// location). Cloud placeholders and dedup are not name surrogates.
    const NAME_SURROGATE: u32 = 0x2000_0000;

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn last_err() -> TreeError {
        TreeError::from_io(io::Error::last_os_error())
    }

    fn open(
        path: &Path,
        access: u32,
        share: u32,
        disposition: u32,
        flags: u32,
    ) -> io::Result<OwnedHandle> {
        let w = wide(path);
        // SAFETY: `w` is NUL-terminated; no security attributes or template.
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                access,
                share,
                std::ptr::null(),
                disposition,
                flags,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `h` is a new handle owned by nobody else.
        Ok(unsafe { OwnedHandle::from_raw_handle(h) })
    }

    fn tag_info(h: HANDLE) -> io::Result<FILE_ATTRIBUTE_TAG_INFO> {
        let mut info = FILE_ATTRIBUTE_TAG_INFO {
            FileAttributes: 0,
            ReparseTag: 0,
        };
        // SAFETY: `info` is a writable buffer of the given size.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                h,
                FileAttributeTagInfo,
                (&raw mut info).cast(),
                u32::try_from(size_of::<FILE_ATTRIBUTE_TAG_INFO>()).unwrap_or(0),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info)
    }

    fn is_surrogate(info: &FILE_ATTRIBUTE_TAG_INFO) -> bool {
        info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            && info.ReparseTag & NAME_SURROGATE != 0
    }

    /// `true` if `got` is `expected` (case-insensitive, as NTFS compares names).
    fn same_path(got: &Path, expected: &Path) -> bool {
        let lower = |p: &Path| p.to_string_lossy().to_lowercase();
        lower(got) == lower(expected)
    }

    fn final_path(h: &OwnedHandle) -> Result<PathBuf, TreeError> {
        crate::platform::win_security::final_path(h).map_err(TreeError::from_io)
    }

    /// Directory access for the pinned handles: list, traverse, read attributes; shared for
    /// read and write but not delete, so the directory cannot be renamed or replaced meanwhile.
    /// (`READ_CONTROL`: the owner check of a folder this job created.)
    const DIR_ACCESS: u32 =
        FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE;
    const DIR_SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;

    pub(super) fn open_parent(path: &Path) -> Result<DirRef, TreeError> {
        // The folder the user chose: a link or junction is followed (as for a chosen file).
        let h = open(
            path,
            DIR_ACCESS,
            DIR_SHARE,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
        )
        .map_err(TreeError::from_io)?;
        let info = tag_info(h.as_raw_handle()).map_err(TreeError::from_io)?;
        if info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(TreeError::Untrusted);
        }
        let path = final_path(&h)?;
        Ok(DirRef { handle: h, path })
    }

    pub(super) fn open_child_dir(parent: &DirRef, name: &str) -> Result<DirRef, TreeError> {
        let expected = parent.path.join(name);
        let h = open(
            &expected,
            DIR_ACCESS,
            DIR_SHARE,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
        .map_err(TreeError::from_io)?;
        let info = tag_info(h.as_raw_handle()).map_err(TreeError::from_io)?;
        if is_surrogate(&info) || info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(TreeError::Untrusted);
        }
        let got = final_path(&h)?;
        if !same_path(&got, &expected) {
            return Err(TreeError::Untrusted);
        }
        Ok(DirRef {
            handle: h,
            path: got,
        })
    }

    pub(super) fn mkdir(parent: &DirRef, name: &str) -> Result<(), TreeError> {
        let w = wide(&parent.path.join(name));
        // SAFETY: `w` is NUL-terminated; default security (inherits the parent's ACL).
        let ok = unsafe { CreateDirectoryW(w.as_ptr(), std::ptr::null()) };
        if ok == 0 { Err(last_err()) } else { Ok(()) }
    }

    fn info_of(h: &OwnedHandle) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
        // SAFETY: zeroed is a valid out-buffer.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is writable.
        if unsafe { GetFileInformationByHandle(h.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info)
    }

    fn id_of(info: &BY_HANDLE_FILE_INFORMATION) -> FileId {
        (
            u64::from(info.dwVolumeSerialNumber),
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        )
    }

    pub(super) fn probe(
        parent: &DirRef,
        name: &str,
        ours: &dyn Fn(FileId) -> bool,
    ) -> Result<Probe, TreeError> {
        let expected = parent.path.join(name);
        let h = match open(
            &expected,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        ) {
            Ok(h) => h,
            Err(e) => {
                return match TreeError::from_io(e) {
                    TreeError::NotFound => Ok(Probe::Missing),
                    other => Err(other),
                };
            }
        };
        // A name that is another entry's 8.3 short name (`FOO~1.TXT`) opens that entry: never
        // treat (or later replace) it as this name.
        if !same_path(&final_path(&h)?, &expected) {
            return Err(TreeError::Untrusted);
        }
        let tag = tag_info(h.as_raw_handle()).map_err(TreeError::from_io)?;
        if tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            && tag.ReparseTag & NAME_SURROGATE != 0
        {
            // A junction or directory symlink is a directory entry: a file cannot be renamed
            // over it (it would be "access denied").
            return Ok(Probe::Link {
                dir: tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
            });
        }
        if tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            return Ok(Probe::Dir);
        }
        let info = info_of(&h).map_err(TreeError::from_io)?;
        let size = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
        let ft = info.ftLastWriteTime;
        let ticks = (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
        // 100 ns ticks since 1601 → Unix seconds.
        let mtime = i64::try_from(ticks / 10_000_000)
            .ok()
            .map(|s| s - 11_644_473_600);
        Ok(Probe::File {
            size,
            mtime,
            ours: ours(id_of(&info)),
        })
    }

    pub(super) fn create_file(parent: &DirRef, name: &str) -> Result<File, TreeError> {
        let expected = parent.path.join(name);
        let h = open(
            &expected,
            FILE_GENERIC_WRITE | FILE_GENERIC_READ | DELETE,
            FILE_SHARE_READ,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
        )
        .map_err(TreeError::from_io)?;
        // Before any byte is written: the new file is where it should be.
        let got = final_path(&h);
        let file = File::from(h);
        match got {
            Ok(got) if same_path(&got, &expected) => Ok(file),
            other => {
                let _ = delete_on_close(&file);
                Err(match other {
                    Err(e) => e,
                    Ok(_) => TreeError::Untrusted,
                })
            }
        }
    }

    pub(super) fn dir_id(dir: &DirRef) -> io::Result<FileId> {
        Ok(id_of(&info_of(&dir.handle)?))
    }

    pub(super) fn file_id(file: &File) -> io::Result<FileId> {
        // SAFETY: borrowed handle, not closed here.
        let h = std::mem::ManuallyDrop::new(unsafe {
            OwnedHandle::from_raw_handle(file.as_raw_handle())
        });
        Ok(id_of(&info_of(&h)?))
    }

    fn delete_on_close(file: &File) -> io::Result<()> {
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: `info` is a valid FILE_DISPOSITION_INFO; the handle has DELETE access.
        let ok = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileDispositionInfo,
                (&raw const info).cast(),
                u32::try_from(size_of::<FILE_DISPOSITION_INFO>()).unwrap_or(0),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Rename the open file to `to` inside `dir`. The target is the full path below the pinned
    /// (verified, not deletable or renameable) directory: `SetFileInformationByHandle` rejects a
    /// relative name with a `RootDirectory` handle (`ERROR_INVALID_PARAMETER`).
    pub(super) fn rename(
        dir: &DirRef,
        _from: &str,
        file: &File,
        to: &str,
        replace: bool,
    ) -> Result<(), TreeError> {
        let name: Vec<u16> = dir.path.join(to).as_os_str().encode_wide().collect();
        let name_bytes = name.len() * 2;
        // The struct (its one-element `FileName` holds the terminating NUL) plus the name.
        let total = size_of::<FILE_RENAME_INFO>() + name_bytes;
        // u64 words: aligned for FILE_RENAME_INFO.
        let mut buf = vec![0u64; total.div_ceil(8)];
        let info = buf.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        // SAFETY: `buf` holds `total` bytes, aligned to 8; the name (plus a NUL) fits behind
        // the header.
        unsafe {
            (*info).Anonymous.ReplaceIfExists = replace;
            (*info).RootDirectory = std::ptr::null_mut();
            (*info).FileNameLength = u32::try_from(name_bytes).unwrap_or(u32::MAX);
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                (&raw mut (*info).FileName).cast::<u16>(),
                name.len(),
            );
        }
        // SAFETY: `info` points to `total` initialised bytes; the handle has DELETE access.
        let ok = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileRenameInfo,
                info.cast_const().cast(),
                u32::try_from(total).unwrap_or(u32::MAX),
            )
        };
        if ok == 0 {
            return Err(match last_err() {
                TreeError::PermissionDenied if replace => {
                    // A directory at the target, or a file another program holds open.
                    match probe(dir, to, &|_| false) {
                        Ok(Probe::Dir) => TreeError::IsADirectory,
                        _ => TreeError::PermissionDenied,
                    }
                }
                e => e,
            });
        }
        Ok(())
    }

    pub(super) fn unlink(
        _dir: &DirRef,
        _name: &str,
        file: Option<&File>,
        _expected: Option<FileId>,
    ) -> io::Result<()> {
        // Through the `.part`'s own handle: nothing else can be deleted by mistake.
        match file {
            Some(f) => delete_on_close(f),
            None => Ok(()),
        }
    }

    pub(super) fn rmdir_if_empty(dir: &DirRef, name: &str) -> io::Result<()> {
        let expected = dir.path.join(name);
        let h = open(
            &expected,
            DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )?;
        // Exactly this folder (not one reached through an 8.3 alias).
        let got = crate::platform::win_security::final_path(&h)?;
        if !same_path(&got, &expected) {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        let tag = tag_info(h.as_raw_handle())?;
        if tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        // Deleting a non-empty directory fails (ERROR_DIR_NOT_EMPTY).
        delete_on_close(&File::from(h))
    }

    pub(super) fn free_space(dir: &DirRef) -> io::Result<u64> {
        let mut p: OsString = dir.path.as_os_str().to_owned();
        p.push("\\");
        let w: Vec<u16> = p.encode_wide().chain(std::iter::once(0)).collect();
        let mut free = 0u64;
        // SAFETY: `w` is NUL-terminated; only the first out-pointer is used.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                w.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(free)
    }

    /// Mark-of-the-Web on the `.part` (its directory is pinned, so the path names this file).
    pub(super) fn mark(dir: &DirRef, name: &str, _file: &File) -> io::Result<()> {
        crate::sftp::transfer::mark_of_the_web(&dir.path.join(name))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::platform::test_support::private_tempdir;

    fn rel(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_owned()).collect()
    }

    fn write_part(
        tree: &LocalTree,
        dir: &[String],
        name: &str,
        data: &[u8],
    ) -> Result<(), TreeError> {
        let (part, mut file) = tree.create_part(dir, name)?;
        std::io::Write::write_all(&mut file, data).unwrap();
        let r = tree.commit(&part, &file, name, false, None);
        if r.is_err() {
            tree.discard(&part, Some(&file));
        }
        r
    }

    #[test]
    fn creates_a_tree_and_never_replaces_without_permission() {
        let dir = private_tempdir();
        assert_eq!(
            LocalTree::inspect(dir.path(), "proj").unwrap(),
            RootState::Missing
        );
        let tree = LocalTree::open_new(dir.path(), "proj", true).unwrap();
        assert!(matches!(
            LocalTree::open_new(dir.path(), "proj", true),
            Err(TreeError::Exists)
        ));
        assert_eq!(
            LocalTree::inspect(dir.path(), "proj").unwrap(),
            RootState::Dir
        );
        assert!(tree.ensure_dir(&rel(&["a"])).unwrap());
        assert!(!tree.ensure_dir(&rel(&["a"])).unwrap(), "exists: merged");
        assert!(tree.ensure_dir(&rel(&["a", "b"])).unwrap());
        write_part(&tree, &rel(&["a", "b"]), "f.txt", b"one").unwrap();
        let f = dir.path().join("proj").join("a").join("b").join("f.txt");
        assert_eq!(std::fs::read(&f).unwrap(), b"one");
        assert!(!dir.path().join("proj/a/b/f.txt.part").exists());
        // A second write of the same name: never replaced without `replace`.
        assert!(matches!(
            write_part(&tree, &rel(&["a", "b"]), "f.txt", b"two"),
            Err(TreeError::Exists)
        ));
        assert_eq!(std::fs::read(&f).unwrap(), b"one");
        // Nor with `replace`: this job created it.
        let (part, file) = tree.create_part(&rel(&["a", "b"]), "f.txt").unwrap();
        assert!(matches!(
            tree.commit(&part, &file, "f.txt", true, None),
            Err(TreeError::Exists)
        ));
        tree.discard(&part, Some(&file));
        drop(file);
        assert_eq!(
            std::fs::read_dir(dir.path().join("proj/a/b"))
                .unwrap()
                .count(),
            1,
            "no .part left"
        );
        match tree.probe(&rel(&["a", "b"]), "f.txt").unwrap() {
            Probe::File { size, ours, .. } => assert_eq!((size, ours), (3, true)),
            other => panic!("{other:?}"),
        }
        assert_eq!(tree.probe(&rel(&["a"]), "b").unwrap(), Probe::Dir);
        assert_eq!(tree.probe(&rel(&["a"]), "zz").unwrap(), Probe::Missing);
        assert!(tree.free_space().unwrap() > 0);
    }

    /// A user's existing file is replaced only with `replace`; a directory never.
    #[test]
    fn replace_rules() {
        let dir = private_tempdir();
        std::fs::create_dir_all(dir.path().join("t/sub")).unwrap();
        std::fs::write(dir.path().join("t/old.txt"), b"user").unwrap();
        let tree = LocalTree::open_new(dir.path(), "t", false).unwrap();
        // The identity a retry compares: the same folder opened again.
        let again = LocalTree::open_new(dir.path(), "t", false).unwrap();
        assert!(tree.root_id().is_some());
        assert_eq!(tree.root_id(), again.root_id());
        drop(again);
        match tree.probe(&[], "old.txt").unwrap() {
            Probe::File { ours, .. } => assert!(!ours),
            other => panic!("{other:?}"),
        }
        let (part, mut file) = tree.create_part(&[], "old.txt").unwrap();
        std::io::Write::write_all(&mut file, b"new").unwrap();
        tree.commit(&part, &file, "old.txt", true, None).unwrap();
        drop(file);
        assert_eq!(std::fs::read(dir.path().join("t/old.txt")).unwrap(), b"new");
        let (part, file) = tree.create_part(&[], "sub").unwrap();
        assert!(matches!(
            tree.commit(&part, &file, "sub", true, None),
            Err(TreeError::IsADirectory)
        ));
        tree.discard(&part, Some(&file));
    }

    /// `x` and `x.part` in one folder (replace allowed, both in flight): each ends with its own
    /// content; a server name that equals another file's `.part` is this job's file (never
    /// replaced, never shown as a user's file); cancelling `x` keeps the finished `x.part`.
    #[test]
    fn temp_files_never_collide_with_server_names() {
        let dir = private_tempdir();
        let tree = LocalTree::open_new(dir.path(), "t", true).unwrap();
        let (px, mut fx) = tree.create_part(&[], "x").unwrap();
        let (pp, mut fp) = tree.create_part(&[], "x.part").unwrap();
        std::io::Write::write_all(&mut fx, b"content of x").unwrap();
        std::io::Write::write_all(&mut fp, b"content of x.part").unwrap();
        tree.commit(&pp, &fp, "x.part", true, None).unwrap();
        tree.commit(&px, &fx, "x", true, None).unwrap();
        drop((fx, fp));
        let read = |n: &str| std::fs::read(dir.path().join("t").join(n)).unwrap();
        assert_eq!(read("x"), b"content of x");
        assert_eq!(read("x.part"), b"content of x.part");

        // A server file named exactly like a running `.part`.
        let (px, mut fx) = tree.create_part(&[], "y").unwrap();
        std::io::Write::write_all(&mut fx, b"content of y").unwrap();
        let temp = px.path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(matches!(
            tree.probe(&[], &temp).unwrap(),
            Probe::File { ours: true, .. }
        ));
        let (pq, fq) = tree.create_part(&[], &temp).unwrap();
        assert!(matches!(
            tree.commit(&pq, &fq, &temp, true, None),
            Err(TreeError::Exists)
        ));
        tree.discard(&pq, Some(&fq));
        drop(fq);
        tree.commit(&px, &fx, "y", false, None).unwrap();
        drop(fx);
        assert_eq!(read("y"), b"content of y");

        // Cancel of `z` while a finished `z.part` exists: only the temporary file goes.
        let (pz, fz) = tree.create_part(&[], "z.part").unwrap();
        tree.commit(&pz, &fz, "z.part", false, None).unwrap();
        drop(fz);
        let (pz, fz) = tree.create_part(&[], "z").unwrap();
        tree.discard(&pz, Some(&fz));
        drop(fz);
        assert!(dir.path().join("t/z.part").exists());
        let names: Vec<String> = std::fs::read_dir(dir.path().join("t"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(!names.iter().any(|n| n.starts_with('.')), "{names:?}");
    }

    /// A retry shares the created set: a file the first pass wrote is never replaced.
    #[test]
    fn retries_keep_the_files_of_earlier_passes() {
        let dir = private_tempdir();
        let created = CreatedSet::default();
        let first = LocalTree::open(dir.path(), "t", true, created.clone()).unwrap();
        write_part(&first, &[], "a", b"first").unwrap();
        drop(first);
        let retry = LocalTree::open(dir.path(), "t", false, created).unwrap();
        assert!(matches!(
            retry.probe(&[], "a").unwrap(),
            Probe::File { ours: true, .. }
        ));
        let (p, f) = retry.create_part(&[], "a").unwrap();
        assert!(matches!(
            retry.commit(&p, &f, "a", true, None),
            Err(TreeError::Exists)
        ));
        retry.discard(&p, Some(&f));
        drop(f);
        assert_eq!(std::fs::read(dir.path().join("t/a")).unwrap(), b"first");
    }

    /// Cancel cleanup removes only empty directories.
    #[test]
    fn remove_if_empty_keeps_content() {
        let dir = private_tempdir();
        let tree = LocalTree::open_new(dir.path(), "t", true).unwrap();
        tree.ensure_dir(&rel(&["empty"])).unwrap();
        tree.ensure_dir(&rel(&["full"])).unwrap();
        write_part(&tree, &rel(&["full"]), "x", b"1").unwrap();
        tree.remove_if_empty(&rel(&["empty"]));
        tree.remove_if_empty(&rel(&["full"]));
        tree.finish_dir(&rel(&["full"]));
        tree.finish_dir(&[]);
        assert!(!dir.path().join("t/empty").exists());
        assert!(dir.path().join("t/full/x").exists());
        tree.remove_root_if_empty();
        assert!(dir.path().join("t/full/x").exists(), "not empty: kept");
        let tree = LocalTree::open_new(dir.path(), "e", true).unwrap();
        tree.remove_root_if_empty();
        assert!(!dir.path().join("e").exists(), "empty root removed");
    }

    /// Names that are not one safe component are refused here too.
    #[test]
    fn unsafe_names_are_refused() {
        let dir = private_tempdir();
        let tree = LocalTree::open_new(dir.path(), "t", true).unwrap();
        for bad in [
            "", ".", "..", "a/b", "a\\b", "a:b", "CON", "nul.txt", "x.", "x ", "a\0b",
        ] {
            assert!(
                matches!(tree.ensure_dir(&rel(&[bad])), Err(TreeError::InvalidName)),
                "{bad:?}"
            );
            assert!(
                matches!(tree.create_part(&[], bad), Err(TreeError::InvalidName)),
                "{bad:?}"
            );
        }
        assert!(matches!(
            LocalTree::open_new(dir.path(), "..", true),
            Err(TreeError::InvalidName)
        ));
        assert_eq!(
            keep_both_names("proj").take(2).collect::<Vec<_>>(),
            vec!["proj (2)".to_owned(), "proj (3)".to_owned()]
        );
    }

    /// An existing link in a merge destination is never followed: the subtree fails, the
    /// outside directory stays empty.
    #[cfg(unix)]
    #[test]
    fn existing_symlink_in_merge_is_refused() {
        let dir = private_tempdir();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::create_dir(dir.path().join("t")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("t/sub")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("rootlink")).unwrap();
        let tree = LocalTree::open_new(dir.path(), "t", false).unwrap();
        assert!(matches!(
            tree.ensure_dir(&rel(&["sub"])),
            Err(TreeError::Untrusted)
        ));
        assert!(matches!(
            tree.create_part(&rel(&["sub"]), "x"),
            Err(TreeError::Untrusted)
        ));
        assert_eq!(tree.probe(&[], "sub").unwrap(), Probe::Link { dir: false });
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        // A link as the destination root itself: refused for a merge.
        assert_eq!(
            LocalTree::inspect(dir.path(), "rootlink").unwrap(),
            RootState::Other
        );
        assert!(matches!(
            LocalTree::open_new(dir.path(), "rootlink", false),
            Err(TreeError::Untrusted)
        ));
    }

    /// A directory the job created is swapped for a symlink mid-job: later writes fail and
    /// nothing reaches the outside directory.
    #[cfg(unix)]
    #[test]
    fn swapped_directory_is_detected() {
        let dir = private_tempdir();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let tree = LocalTree::open_new(dir.path(), "t", true).unwrap();
        tree.ensure_dir(&rel(&["d"])).unwrap();
        std::fs::remove_dir(dir.path().join("t/d")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("t/d")).unwrap();
        assert!(matches!(
            tree.create_part(&rel(&["d"]), "f"),
            Err(TreeError::Untrusted)
        ));
        assert!(matches!(
            tree.ensure_dir(&rel(&["d", "e"])),
            Err(TreeError::Untrusted)
        ));
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        // A `.part` is a fresh dot-prefixed name, created exclusively without following links.
        tree.ensure_dir(&rel(&["e"])).unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, b"keep").unwrap();
        std::os::unix::fs::symlink(&victim, dir.path().join("t/e/f.part")).unwrap();
        let (part, mut f) = tree.create_part(&rel(&["e"]), "f").unwrap();
        let part_file = part
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            part_file.starts_with(".f.") && part_file.ends_with(".part"),
            "{part_file}"
        );
        std::io::Write::write_all(&mut f, b"data").unwrap();
        tree.commit(&part, &f, "f", false, None).unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
        assert_eq!(std::fs::read(dir.path().join("t/e/f")).unwrap(), b"data");
    }

    /// Discarding a `.part` whose name now belongs to another file leaves that file alone.
    #[cfg(unix)]
    #[test]
    fn discard_checks_the_identity() {
        let dir = private_tempdir();
        let tree = LocalTree::open_new(dir.path(), "t", true).unwrap();
        let (part, f) = tree.create_part(&[], "x").unwrap();
        drop(f);
        // Something else takes the `.part`'s name (it is moved away, another file comes).
        std::fs::rename(&part.path, dir.path().join("t/moved")).unwrap();
        std::fs::write(&part.path, b"someone else").unwrap();
        tree.discard(&part, None);
        assert_eq!(std::fs::read(&part.path).unwrap(), b"someone else");
    }

    /// Unix: new directories are 0700 while the job runs and get the umask default at the end.
    #[cfg(unix)]
    #[test]
    fn directory_modes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = private_tempdir();
        let tree = LocalTree::open_new(dir.path(), "t", true).unwrap();
        tree.ensure_dir(&rel(&["d"])).unwrap();
        let mode = |p: &str| {
            std::fs::metadata(dir.path().join(p))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("t/d"), 0o700);
        assert_eq!(mode("t"), 0o700);
        tree.finish_dir(&rel(&["d"]));
        tree.finish_dir(&[]);
        // The default of any new directory (`0o777 & !umask`).
        std::fs::create_dir(dir.path().join("plain")).unwrap();
        assert_eq!(mode("t/d"), mode("plain"));
        assert_eq!(mode("t"), mode("plain"));
    }

    /// Windows: a junction planted in the destination (to another folder, or a directory
    /// symlink) is never followed; the outside directory stays empty.
    #[cfg(windows)]
    #[test]
    #[allow(clippy::disallowed_methods)] // test-only: `mklink /J` needs cmd.exe
    fn planted_junction_is_refused() {
        let dir = private_tempdir();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let tree = LocalTree::open_new(dir.path(), "t", true).unwrap();
        tree.ensure_dir(&rel(&["d"])).unwrap();
        let junction = |link: &Path, target: &Path| {
            let st = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            assert!(st.status.success(), "mklink /J failed: {st:?}");
        };
        // Swap the created directory for a junction mid-job.
        std::fs::remove_dir(dir.path().join("t").join("d")).unwrap();
        junction(&dir.path().join("t").join("d"), &outside);
        assert!(matches!(
            tree.create_part(&rel(&["d"]), "f"),
            Err(TreeError::Untrusted)
        ));
        assert!(matches!(
            tree.ensure_dir(&rel(&["d", "e"])),
            Err(TreeError::Untrusted)
        ));
        assert_eq!(tree.probe(&[], "d").unwrap(), Probe::Link { dir: true });
        // Replace onto the junction: a clear "folder in the way", never "access denied".
        let (part, f) = tree.create_part(&[], "d").unwrap();
        assert!(matches!(
            tree.commit(&part, &f, "d", true, None),
            Err(TreeError::IsADirectory)
        ));
        tree.discard(&part, Some(&f));
        drop(f);
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        // A junction as the root of a merge.
        junction(&dir.path().join("rootj"), &outside);
        assert_eq!(
            LocalTree::inspect(dir.path(), "rootj").unwrap(),
            RootState::Other
        );
        assert!(matches!(
            LocalTree::open_new(dir.path(), "rootj", false),
            Err(TreeError::Untrusted)
        ));
        // A directory symlink to a network share that does not exist (junctions cannot point
        // to UNC paths): refused without being followed, so no SMB connection is attempted.
        // Creating it needs Developer Mode or elevation; skipped without.
        let unc = dir.path().join("t").join("unc");
        if let Err(e) =
            std::os::windows::fs::symlink_dir(r"\\shellfin-nonexistent.invalid\share", &unc)
        {
            eprintln!("skipping the UNC symlink case: {e}");
            return;
        }
        let started = std::time::Instant::now();
        assert!(matches!(
            tree.ensure_dir(&rel(&["unc"])),
            Err(TreeError::Untrusted)
        ));
        assert!(matches!(
            tree.create_part(&rel(&["unc"]), "f"),
            Err(TreeError::Untrusted)
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    /// Windows: the Mark-of-the-Web lands on the finished file, no other stream is created.
    #[cfg(windows)]
    #[test]
    fn mark_of_the_web_on_tree_files() {
        let dir = private_tempdir();
        let tree = LocalTree::open_new(dir.path(), "t", true).unwrap();
        write_part(&tree, &[], "setup.exe", b"MZ").unwrap();
        let mut ads = dir.path().join("t").join("setup.exe").into_os_string();
        ads.push(":Zone.Identifier");
        let zone = std::fs::read_to_string(PathBuf::from(ads)).unwrap();
        assert!(zone.contains("ZoneId=3"));
    }
}
