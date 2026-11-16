//! What differs between the operating systems where the app runs other programs and looks
//! at files: finding a program on a `PATH`, the process tree a child and everything it
//! starts form (so that a time limit, a Stop or the app's exit ends all of it), whether a
//! process still runs, whether a git process works in a repository, and what tells one file
//! from another without reading it.
//!
//! On Unix a child leads a session of its own (no controlling terminal) and so a process
//! group, which a signal ends as a whole. On Windows a child is started without a console
//! window (`CREATE_NO_WINDOW`: nothing it starts flashes one either) and suspended, put in
//! a job object of its own that kills what is in it when its last handle closes, then
//! resumed: everything it starts is in the job from the first instruction on. The app
//! holds the handle while the child runs, so its exit (even a crash) ends the tree too.
//!
//! The parts with logic of their own (the Windows program lookup) take everything they
//! depend on as arguments, so they are tested on every platform.

use std::path::{Path, PathBuf};

/// What looking a program up on a `PATH` found.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum Lookup {
    /// The program, by its full path.
    Program(PathBuf),
    /// No program, but a script of that name that Windows would run through another program
    /// (`git.cmd`, a `.bat` shim: `cmd.exe`), which the app never does: it runs `.exe` (and
    /// `.com`) programs only, since `cmd.exe` would parse the arguments again (R19).
    ScriptOnly(PathBuf),
    /// Nothing of that name.
    Missing,
}

impl Lookup {
    /// The program found, if any.
    pub(crate) fn program(self) -> Option<PathBuf> {
        match self {
            Self::Program(p) => Some(p),
            Self::ScriptOnly(_) | Self::Missing => None,
        }
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
/// The extensions Windows tries for a name without one when `PATHEXT` is not set.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

#[cfg_attr(not(windows), allow(dead_code))]
/// Of the extensions Windows runs, those of programs, which the app may start itself.
const PROGRAM_EXTENSIONS: [&str; 2] = [".exe", ".com"];

/// The first program called `name` (a bare name, no folder in it) in an absolute folder of
/// `path` (a `PATH` value; the app's own when `None`). Never in the current folder or
/// through a relative entry (`.`, `bin`, an empty one): the app's children run in
/// repositories, where such an entry would find a program of the repository's.
///
/// Unix: an executable file of exactly that name. Windows: as Windows' own lookup does
/// it, `name` as it is when its extension is one of `PATHEXT`'s, else `name` with each of
/// them in turn; only a program (`.exe`, `.com`) is taken, and a script of the name found
/// on the way (`gh.cmd`) is reported when no program is (see [`Lookup::ScriptOnly`]).
pub(crate) fn find_program(name: &str, path: Option<&std::ffi::OsStr>) -> Lookup {
    let owned;
    let path = match path {
        Some(p) => p,
        None => match std::env::var_os("PATH") {
            Some(p) => {
                owned = p;
                owned.as_os_str()
            }
            None => return Lookup::Missing,
        },
    };
    #[cfg(windows)]
    {
        let pathext = std::env::var("PATHEXT").ok();
        windows_lookup(
            name,
            &path.to_string_lossy(),
            pathext.as_deref(),
            &|candidate| {
                Path::new(candidate)
                    .symlink_metadata()
                    .is_ok_and(|m| !m.is_dir())
            },
        )
    }
    #[cfg(not(windows))]
    {
        std::env::split_paths(path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(name))
            .find(|candidate| executable_file(candidate))
            .map_or(Lookup::Missing, Lookup::Program)
    }
}

/// Whether `candidate` is a file Unix would run.
#[cfg(unix)]
fn executable_file(candidate: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    candidate
        .metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(any(unix, windows)))]
fn executable_file(candidate: &Path) -> bool {
    candidate.is_file()
}

#[cfg_attr(not(windows), allow(dead_code))]
/// Windows' program lookup (see [`find_program`]) over `path` (a `PATH` value, `;` between
/// folders, a folder possibly in double quotes) with extensions `pathext` (`PATHEXT`; its
/// default when unset or empty), `exists` telling whether a file is there (anything that is
/// not a folder: Windows Terminal's `wt.exe` is an alias the file system cannot read).
/// Text only, so that it runs (and is tested) anywhere.
pub(crate) fn windows_lookup(
    name: &str,
    path: &str,
    pathext: Option<&str>,
    exists: &dyn Fn(&str) -> bool,
) -> Lookup {
    let extensions: Vec<String> = pathext
        .filter(|p| !p.trim().is_empty())
        .unwrap_or(DEFAULT_PATHEXT)
        .split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            if e.starts_with('.') {
                e
            } else {
                format!(".{e}")
            }
        })
        .collect();
    let own = name
        .rfind('.')
        .map(|at| name[at..].to_ascii_lowercase())
        .filter(|ext| extensions.contains(ext));
    let names: Vec<String> = match own {
        Some(_) => vec![name.to_owned()],
        None => extensions.iter().map(|e| format!("{name}{e}")).collect(),
    };
    let mut script = None;
    for dir in windows_path_dirs(path) {
        for file in &names {
            let candidate = format!("{}\\{file}", dir.trim_end_matches(['\\', '/']));
            if !exists(&candidate) {
                continue;
            }
            let ext = file
                .rfind('.')
                .map(|at| file[at..].to_ascii_lowercase())
                .unwrap_or_default();
            if PROGRAM_EXTENSIONS.contains(&ext.as_str()) {
                return Lookup::Program(PathBuf::from(candidate));
            }
            script.get_or_insert_with(|| PathBuf::from(&candidate));
        }
    }
    script.map_or(Lookup::Missing, Lookup::ScriptOnly)
}

#[cfg_attr(not(windows), allow(dead_code))]
/// The absolute folders of Windows `PATH` value `path`, in order: split at `;` outside double
/// quotes, the quotes taken off; a relative entry (`bin`, `.`, `\tools`, `C:tools`) and an
/// empty one are left out.
pub(crate) fn windows_path_dirs(path: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    let mut entry = String::new();
    let mut quoted = false;
    for c in path.chars().chain(std::iter::once(';')) {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                let dir = std::mem::take(&mut entry);
                if windows_absolute(&dir) && !dirs.contains(&dir) {
                    dirs.push(dir);
                }
            }
            c => entry.push(c),
        }
    }
    dirs
}

#[cfg_attr(not(windows), allow(dead_code))]
/// Whether `path` is a full Windows path: `C:\…` (or `C:/…`), or a share `\\server\…`.
pub(crate) fn windows_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    let drive =
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    let share = b.len() > 2 && matches!(b[0], b'\\' | b'/') && b[1] == b[0];
    drive || share
}

/// A child's process tree: what ends it and everything it started. On Unix the process
/// group it leads (its pid; the child is made a session leader at its start); on Windows the
/// job object it was put in before it ran.
pub struct ProcessTree {
    #[cfg(unix)]
    group: Option<i32>,
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl ProcessTree {
    /// Asks the tree to end. Unix: SIGTERM to the group, so git removes its lock files.
    /// Windows has no such request for a program without a window: the tree ends at once
    /// (a lock file git held may stay behind; the app offers to remove a stale
    /// `index.lock`).
    pub(crate) fn terminate(&self) {
        #[cfg(unix)]
        self.signal(libc::SIGTERM);
        #[cfg(windows)]
        self.kill();
    }

    /// Ends the tree at once.
    pub(crate) fn kill(&self) {
        #[cfg(unix)]
        self.signal(libc::SIGKILL);
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.terminate();
        }
    }

    /// The child ended by itself: what it started and left running (a credential cache
    /// daemon, say) lives on, as on Unix, where nothing is sent to the group then. (Dropped
    /// without this, the Windows tree is ended.)
    pub(crate) fn release(self) {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.release();
        }
    }

    #[cfg(unix)]
    fn signal(&self, signal: libc::c_int) {
        if let Some(group) = self.group {
            // SAFETY: killpg only sends a signal; a group that is gone is ESRCH.
            unsafe {
                libc::killpg(group, signal);
            }
        }
    }
}

/// Starts `cmd` (made with [`child_flags`] applied) and returns it with its process tree.
/// On Windows it starts suspended, is put in a job of its own and is then resumed; if that
/// cannot be done it is killed and this fails: a child the app could not end with all it
/// starts is never left running.
pub(crate) fn spawn(
    cmd: &mut tokio::process::Command,
) -> std::io::Result<(tokio::process::Child, ProcessTree)> {
    #[cfg(windows)]
    {
        cmd.creation_flags(job::CREATE_NO_WINDOW | job::CREATE_SUSPENDED);
        let mut child = cmd.spawn()?;
        match job::contain(&child) {
            Ok(job) => Ok((child, ProcessTree { job: Some(job) })),
            Err(e) => {
                let _ = child.start_kill();
                Err(std::io::Error::new(
                    e.kind(),
                    format!("could not start it in a job object of its own: {e}"),
                ))
            }
        }
    }
    #[cfg(not(windows))]
    {
        let child = cmd.spawn()?;
        let tree = ProcessTree {
            #[cfg(unix)]
            group: child.id().and_then(|pid| i32::try_from(pid).ok()),
        };
        Ok((child, tree))
    }
}

/// Sets what every child of the app's gets from the start (see the module docs): on Unix a
/// session of its own, so no controlling terminal and its own process group (R15, R41); on
/// Windows no console window. [`spawn`] adds the rest.
pub(crate) fn child_flags(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    // SAFETY: the closure runs in the forked child before exec and only calls setsid, which
    // is async-signal-safe and touches no memory of the parent.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    #[cfg(windows)]
    cmd.creation_flags(job::CREATE_NO_WINDOW);
}

/// Whether process `pid` is known to be gone (`false` when it runs or cannot be told).
pub(crate) fn process_gone(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // Signal 0 checks only: ESRCH, no such process. (EPERM: one runs, another user's.)
        // SAFETY: signal 0 sends nothing.
        let alive = unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
        !alive
    }
    #[cfg(windows)]
    {
        job::process_gone(pid)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

/// Whether `a` and `b` name the same file or folder. When both exist, compared with links
/// resolved (which also gives each the case it has on disk, on macOS and Windows); when
/// neither exists, their nearest existing folders resolved and the rest compared as written,
/// ignoring case where the file system does (Windows; a macOS volume that says it is
/// case-insensitive). One that exists and one that does not are different.
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (dunce::canonicalize(a), dunce::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        (Err(_), Err(_)) => {
            let (a, b) = (resolved_as_far_as_exists(a), resolved_as_far_as_exists(b));
            a == b
                || (case_insensitive_at(&a)
                    && a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase())
        }
        _ => false,
    }
}

/// `path` with its nearest existing folder resolved and the rest as written.
fn resolved_as_far_as_exists(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut at = path;
    loop {
        if let Ok(real) = dunce::canonicalize(at) {
            return rest.iter().rev().fold(real, |p, name| p.join(name));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                at = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Whether names that differ only in case are one name where `path` (or its nearest
/// existing folder) is: always on Windows; on macOS as the volume says; never elsewhere.
fn case_insensitive_at(path: &Path) -> bool {
    #[cfg(windows)]
    {
        let _ = path;
        true
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let existing = path.ancestors().find(|p| p.exists()).unwrap_or(path);
        let Ok(c) = std::ffi::CString::new(existing.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: a NUL-terminated path; pathconf only reads it.
        unsafe { libc::pathconf(c.as_ptr(), libc::_PC_CASE_SENSITIVE) == 0 }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = path;
        false
    }
}

/// Gives `from` the name `to` in one step unless something has that name already (then an
/// error, `AlreadyExists` where the platform says so): never replaces `to`, not even one
/// made a moment before. Windows `MoveFileExW` without replacing, macOS `renamex_np` with
/// `RENAME_EXCL`, Linux `renameat2` with `RENAME_NOREPLACE`; where the file system cannot do
/// that, a check and a rename.
pub(crate) fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        job::move_no_replace(from, to)
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = |p: &Path| {
            std::ffi::CString::new(p.as_os_str().as_bytes())
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
        };
        let (cfrom, cto) = (c(from)?, c(to)?);
        // SAFETY: both are NUL-terminated paths that live through the call.
        #[cfg(target_os = "macos")]
        let done = unsafe { libc::renamex_np(cfrom.as_ptr(), cto.as_ptr(), libc::RENAME_EXCL) };
        // SAFETY: as above; AT_FDCWD makes both paths relative to the current folder.
        #[cfg(target_os = "linux")]
        let done = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                cfrom.as_ptr(),
                libc::AT_FDCWD,
                cto.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if done == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        let unsupported = matches!(
            e.raw_os_error(),
            Some(libc::EINVAL | libc::ENOTSUP | libc::ENOSYS)
        );
        if !unsupported {
            return Err(e);
        }
        if std::fs::symlink_metadata(to).is_ok() {
            return Err(std::io::ErrorKind::AlreadyExists.into());
        }
        std::fs::rename(from, to)
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        if std::fs::symlink_metadata(to).is_ok() {
            return Err(std::io::ErrorKind::AlreadyExists.into());
        }
        std::fs::rename(from, to)
    }
}

/// What tells file `path` (with metadata `meta`) from another file that took its name: the
/// device and inode on Unix, the volume and file index on Windows. `None` where it cannot
/// be read (then size and time alone tell versions apart).
pub(crate) fn file_identity(path: &Path, meta: &std::fs::Metadata) -> Option<u128> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = path;
        Some((u128::from(meta.dev()) << 64) | u128::from(meta.ino()))
    }
    #[cfg(windows)]
    {
        let _ = meta;
        job::file_identity(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, meta);
        None
    }
}

/// What a look for git processes working in a repository found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
pub(crate) enum GitScan {
    /// One is running there (its pid).
    Running(u32),
    /// None is (of those this user may look at).
    NoneRunning,
    /// It could not be told: not on this platform, or the process list was not readable.
    Unknown,
}

/// Whether a program called `name` (a file name, as a process list gives it) is git on
/// `os` (`std::env::consts::OS`).
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
pub(crate) fn is_git_program(os: &str, name: &str) -> bool {
    if os == "windows" {
        name.eq_ignore_ascii_case("git.exe") || name.eq_ignore_ascii_case("git")
    } else {
        name == "git"
    }
}

/// Looks, as well as this user may, for a `git` process other than the app's own whose
/// working folder is in one of `dirs` (canonical folders: a worktree, its git dir). Linux:
/// `/proc`; macOS: libproc; elsewhere [`GitScan::Unknown`]. Another user's processes cannot
/// be looked into and count as none.
pub(crate) fn git_running_in(dirs: &[PathBuf]) -> GitScan {
    let inside = |cwd: &Path| dirs.iter().any(|d| cwd.starts_with(d));
    #[cfg(target_os = "linux")]
    {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return GitScan::Unknown;
        };
        let own = std::process::id();
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            if pid == own {
                continue;
            }
            let dir = entry.path();
            let comm = std::fs::read_to_string(dir.join("comm")).unwrap_or_default();
            let exe = std::fs::read_link(dir.join("exe")).ok();
            let git = is_git_program("linux", comm.trim())
                || exe
                    .as_deref()
                    .and_then(Path::file_name)
                    .is_some_and(|n| is_git_program("linux", &n.to_string_lossy()));
            if git
                && let Ok(cwd) = std::fs::read_link(dir.join("cwd"))
                && inside(&cwd)
            {
                return GitScan::Running(pid);
            }
        }
        GitScan::NoneRunning
    }
    #[cfg(target_os = "macos")]
    {
        macos::git_running_in(&inside)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = inside;
        GitScan::Unknown
    }
}

#[cfg(target_os = "macos")]
mod macos {
    //! The libproc calls behind [`super::git_running_in`] (no child process is started).

    use std::ffi::{CStr, c_int, c_void};
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::Path;

    use super::GitScan;

    pub(super) fn git_running_in(inside: &dyn Fn(&Path) -> bool) -> GitScan {
        // SAFETY: with no buffer, proc_listallpids only counts.
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        let Ok(count) = usize::try_from(count) else {
            return GitScan::Unknown;
        };
        // Room for processes started meanwhile.
        let mut pids = vec![0 as libc::pid_t; count + 64];
        let bytes = c_int::try_from(pids.len() * size_of::<libc::pid_t>()).unwrap_or(c_int::MAX);
        // SAFETY: the buffer holds `bytes` bytes of pids.
        let got = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast::<c_void>(), bytes) };
        let Ok(got) = usize::try_from(got) else {
            return GitScan::Unknown;
        };
        let own = libc::pid_t::try_from(std::process::id()).unwrap_or(0);
        for &pid in pids.iter().take(got.min(pids.len())) {
            if pid <= 0 || pid == own {
                continue;
            }
            let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
            // SAFETY: the buffer is PROC_PIDPATHINFO_MAXSIZE bytes long.
            let len = unsafe {
                libc::proc_pidpath(pid, path.as_mut_ptr().cast::<c_void>(), path.len() as u32)
            };
            let Ok(len) = usize::try_from(len) else {
                continue;
            };
            let program = Path::new(std::ffi::OsStr::from_bytes(&path[..len.min(path.len())]));
            let git = program
                .file_name()
                .is_some_and(|n| super::is_git_program("macos", &n.to_string_lossy()));
            if !git {
                continue;
            }
            // SAFETY: an all-zero proc_vnodepathinfo is a valid value of it.
            let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
            let size = c_int::try_from(size_of::<libc::proc_vnodepathinfo>()).unwrap_or(c_int::MAX);
            // SAFETY: `info` is the structure PROC_PIDVNODEPATHINFO fills, of its size.
            let filled = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDVNODEPATHINFO,
                    0,
                    (&raw mut info).cast::<c_void>(),
                    size,
                )
            };
            if filled != size {
                continue; // another user's, or gone
            }
            // SAFETY: the path is a NUL-terminated C string within its MAXPATHLEN bytes
            // (laid out as 32 rows of 32).
            let raw: &[u8; 1024] = unsafe { &*(&raw const info.pvi_cdir.vip_path).cast() };
            let Ok(cwd) = CStr::from_bytes_until_nul(raw) else {
                continue;
            };
            if inside(Path::new(std::ffi::OsStr::from_bytes(cwd.to_bytes()))) {
                return GitScan::Running(u32::try_from(pid).unwrap_or(0));
            }
        }
        GitScan::NoneRunning
    }
}

#[cfg(windows)]
mod job {
    //! The Windows calls behind [`super::ProcessTree`] and friends.

    use std::ffi::c_void;
    use std::io;
    use std::os::windows::io::AsRawHandle as _;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FileIdInfo,
        GetFileInformationByHandle, GetFileInformationByHandleEx, MoveFileExW,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, OpenThread, PROCESS_QUERY_LIMITED_INFORMATION,
        ResumeThread, THREAD_SUSPEND_RESUME,
    };

    pub(super) use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};

    /// A handle that is closed when dropped.
    struct Owned(HANDLE);

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: the handle is open and owned here.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    /// A job object that kills what is in it when its handle closes, until released.
    pub(in crate::platform) struct Job(Owned);

    // SAFETY: a job handle may be used and closed from any thread.
    unsafe impl Send for Job {}
    // SAFETY: as above; the calls made through `&Job` are thread-safe kernel calls.
    unsafe impl Sync for Job {}

    impl Job {
        fn new() -> io::Result<Self> {
            // SAFETY: no attributes, no name: a new unnamed job.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(Owned(handle));
            job.kill_on_close(true)?;
            Ok(job)
        }

        fn kill_on_close(&self, on: bool) -> io::Result<()> {
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            if on {
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            }
            // SAFETY: `info` is the structure this information class takes, of its size.
            let ok = unsafe {
                SetInformationJobObject(
                    self.0.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast::<c_void>(),
                    u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                        .unwrap_or(u32::MAX),
                )
            };
            if ok == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        pub(in crate::platform) fn terminate(&self) {
            // SAFETY: the job handle is open; a job with nothing left in it is fine.
            unsafe {
                TerminateJobObject(self.0.0, 1);
            }
        }

        pub(in crate::platform) fn release(&self) {
            let _ = self.kill_on_close(false);
        }
    }

    /// Puts `child`, started suspended, in a new job and resumes it.
    pub(super) fn contain(child: &tokio::process::Child) -> io::Result<Job> {
        let (Some(handle), Some(pid)) = (child.raw_handle(), child.id()) else {
            return Err(io::Error::other("the program ended at once"));
        };
        let job = Job::new()?;
        // SAFETY: both handles are open (the child's is held by `child`).
        if unsafe { AssignProcessToJobObject(job.0.0, handle as HANDLE) } == 0 {
            return Err(io::Error::last_os_error());
        }
        resume(pid)?;
        Ok(job)
    }

    /// Resumes the threads of process `pid`, which was started suspended (its main thread).
    fn resume(pid: u32) -> io::Result<()> {
        // SAFETY: a snapshot of every thread of the system; closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = Owned(snapshot);
        let mut entry = THREADENTRY32 {
            dwSize: u32::try_from(size_of::<THREADENTRY32>()).unwrap_or(u32::MAX),
            ..Default::default()
        };
        let mut resumed = false;
        // SAFETY: `entry` is a THREADENTRY32 with its size set, as both calls require.
        let mut more = unsafe { Thread32First(snapshot.0, &raw mut entry) } != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: opens the thread by id, only to resume it; closed right after.
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if !thread.is_null() {
                    let thread = Owned(thread);
                    // SAFETY: the thread handle is open with THREAD_SUSPEND_RESUME.
                    resumed |= unsafe { ResumeThread(thread.0) } != u32::MAX;
                }
            }
            // SAFETY: as above.
            more = unsafe { Thread32Next(snapshot.0, &raw mut entry) } != 0;
        }
        if resumed {
            Ok(())
        } else {
            Err(io::Error::other("its main thread could not be resumed"))
        }
    }

    /// Whether process `pid` has exited (or never ran); `false` when it runs or cannot be
    /// asked (another user's).
    pub(super) fn process_gone(pid: u32) -> bool {
        // SAFETY: opens the process by id only to read its exit code; closed right after.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            // ERROR_INVALID_PARAMETER: no such process. Access denied: one runs.
            return io::Error::last_os_error().raw_os_error() == Some(87);
        }
        let handle = Owned(handle);
        let mut code = 0u32;
        // SAFETY: the handle is open with the right to query; `code` is a valid out pointer.
        let ok = unsafe { GetExitCodeProcess(handle.0, &raw mut code) } != 0;
        ok && code != STILL_ACTIVE as u32
    }

    /// The volume serial number and file index of `path`.
    /// `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`: fails when `to` exists.
    pub(super) fn move_no_replace(from: &Path, to: &Path) -> io::Result<()> {
        use std::os::windows::ffi::OsStrExt as _;
        let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain([0]).collect() };
        let (from, to) = (wide(from), wide(to));
        // SAFETY: both are NUL-terminated wide strings that live through the call.
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// The file's 128-bit id (`FileIdInfo`: unique on ReFS too) mixed with its volume's
    /// serial number, so on NTFS (where the id's upper half is zero) it is the volume and the
    /// file index; where `FileIdInfo` cannot be read (FAT), the volume and the 64-bit file
    /// index. A folder is opened too (`FILE_FLAG_BACKUP_SEMANTICS`).
    pub(super) fn file_identity(path: &Path) -> Option<u128> {
        use std::os::windows::fs::OpenOptionsExt as _;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .ok()?;
        let handle = file.as_raw_handle() as HANDLE;
        let mut id = FILE_ID_INFO::default();
        // SAFETY: the handle is open (held by `file`); `id` is a valid out buffer of the
        // size given.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                (&raw mut id).cast::<c_void>(),
                u32::try_from(std::mem::size_of::<FILE_ID_INFO>()).ok()?,
            )
        };
        if ok != 0 {
            return Some(
                u128::from_le_bytes(id.FileId.Identifier)
                    ^ (u128::from(id.VolumeSerialNumber) << 64),
            );
        }
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: as above; `info` is a valid out pointer.
        let ok = unsafe { GetFileInformationByHandle(handle, &raw mut info) };
        (ok != 0).then(|| {
            (u128::from(info.dwVolumeSerialNumber) << 64)
                | (u128::from(info.nFileIndexHigh) << 32)
                | u128::from(info.nFileIndexLow)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paths are compared as the file system does: a folder spelled in another case is the
    /// same folder where names ignore case (macOS by default, Windows), whether it exists
    /// or not (a worktree removed outside the app).
    /// A rename that must not replace never does, and works when the name is free.
    #[test]
    fn a_rename_without_replacing_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        assert!(rename_no_replace(&a, &b).is_err());
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
        std::fs::remove_file(&b).unwrap();
        rename_no_replace(&a, &b).unwrap();
        assert!(!a.exists());
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "a");
    }

    #[test]
    fn paths_are_compared_as_the_file_system_does() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("Probe"), "").unwrap();
        let ignores_case = root.join("probe").exists();

        let there = root.join("Worktree");
        std::fs::create_dir(&there).unwrap();
        assert!(same_path(&there, &root.join("Worktree")));
        std::fs::create_dir(root.join("x")).unwrap();
        assert!(same_path(
            &there,
            &root.join("x").join("..").join("Worktree")
        ));
        assert_eq!(same_path(&there, &root.join("worktree")), ignores_case);

        let gone = root.join("Gone").join("Wt");
        assert!(same_path(&gone, &root.join("Gone/Wt")));
        assert_eq!(
            same_path(&gone, &root.join("gone").join("wt")),
            ignores_case
        );
        assert!(!same_path(&gone, &root.join("Gone").join("Other")));
        assert!(!same_path(&gone, &there));
    }

    /// The files that "exist" in a lookup test.
    fn on_disk(files: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |candidate| files.iter().any(|f| f.eq_ignore_ascii_case(candidate))
    }

    #[test]
    fn windows_finds_a_program_by_pathext_in_path_order() {
        let path = r"C:\Windows\System32;C:\Program Files\Git\cmd;C:\Users\u\bin";
        let disk = on_disk(&[
            r"C:\Program Files\Git\cmd\git.exe",
            r"C:\Users\u\bin\git.exe",
        ]);
        assert_eq!(
            windows_lookup("git", path, None, &disk),
            Lookup::Program(PathBuf::from(r"C:\Program Files\Git\cmd\git.exe"))
        );
        // A name with an extension of PATHEXT is looked for as it is.
        assert_eq!(
            windows_lookup("git.exe", r"C:\Users\u\bin", None, &disk),
            Lookup::Program(PathBuf::from(r"C:\Users\u\bin\git.exe"))
        );
        // PATHEXT's own order and spelling (no dot, lower case) are taken.
        let com = on_disk(&[r"C:\t\x.com", r"C:\t\x.exe"]);
        assert_eq!(
            windows_lookup("x", r"C:\t", Some("exe;.COM"), &com),
            Lookup::Program(PathBuf::from(r"C:\t\x.exe"))
        );
        assert_eq!(
            windows_lookup("nothing", path, None, &disk),
            Lookup::Missing
        );
    }

    #[test]
    fn windows_never_runs_a_script_and_says_when_only_one_is_found() {
        // Scoop-style: a batch shim earlier on PATH, the program later: the program.
        let disk = on_disk(&[r"C:\shims\gh.cmd", r"C:\Program Files\GitHub CLI\gh.exe"]);
        let path = r"C:\shims;C:\Program Files\GitHub CLI";
        assert_eq!(
            windows_lookup("gh", path, None, &disk),
            Lookup::Program(PathBuf::from(r"C:\Program Files\GitHub CLI\gh.exe"))
        );
        // Only the shim: said, never run.
        let shim = on_disk(&[r"C:\shims\gh.cmd", r"C:\other\gh.bat"]);
        assert_eq!(
            windows_lookup("gh", path, None, &shim),
            Lookup::ScriptOnly(PathBuf::from(r"C:\shims\gh.cmd"))
        );
        // Asked for by its script name, still not a program.
        assert_eq!(
            windows_lookup("gh.cmd", path, None, &shim),
            Lookup::ScriptOnly(PathBuf::from(r"C:\shims\gh.cmd"))
        );
        // A PATHEXT of scripts only finds no program.
        assert_eq!(
            windows_lookup("gh", r"C:\shims", Some(".CMD;.BAT"), &shim),
            Lookup::ScriptOnly(PathBuf::from(r"C:\shims\gh.cmd"))
        );
    }

    #[test]
    fn windows_never_looks_in_a_relative_or_empty_path_entry() {
        // A repository's own git.exe (the current folder, a relative bin, a drive-relative
        // or root-relative entry) is never found.
        let disk = on_disk(&[
            r".\git.exe",
            r"bin\git.exe",
            r"\git.exe",
            r"C:git.exe",
            r"git.exe",
            r"D:\real\git.exe",
        ]);
        assert_eq!(
            windows_lookup("git", r".;bin;;\;C:;D:\real", None, &disk),
            Lookup::Program(PathBuf::from(r"D:\real\git.exe"))
        );
        assert_eq!(
            windows_lookup("git", r".;bin;;C:", None, &disk),
            Lookup::Missing
        );
    }

    #[test]
    fn windows_path_entries_may_be_quoted_and_end_with_a_separator() {
        assert_eq!(
            windows_path_dirs(r#"C:\a;"C:\with;semicolon";C:\b\;C:/c/;\\server\share\tools;C:\a"#),
            [
                r"C:\a",
                r"C:\with;semicolon",
                r"C:\b\",
                "C:/c/",
                r"\\server\share\tools"
            ]
        );
        let disk = on_disk(&[
            r"C:\with;semicolon\x.exe",
            r"C:\b\y.exe",
            r"C:/c\z.exe",
            r"C:\w.exe",
        ]);
        let path = r#""C:\with;semicolon";C:\b\;C:/c/;C:\"#;
        for (name, file) in [
            ("x", r"C:\with;semicolon\x.exe"),
            ("y", r"C:\b\y.exe"),
            ("z", r"C:/c\z.exe"),
            ("w", r"C:\w.exe"),
        ] {
            assert_eq!(
                windows_lookup(name, path, None, &disk),
                Lookup::Program(file.into())
            );
        }
    }

    #[test]
    fn full_windows_paths_are_told_from_relative_ones() {
        for full in [r"C:\", r"c:/x", r"\\server\share", "//server/share"] {
            assert!(windows_absolute(full), "{full}");
        }
        for relative in ["", "C:", "C:x", r"\x", "/x", "x", ".", r"..\x"] {
            assert!(!windows_absolute(relative), "{relative}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_finds_an_executable_file_in_absolute_entries_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let tool = bin.join("tool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        let plain = bin.join("plain");
        std::fs::write(&plain, "").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([Path::new("relative"), &bin]).unwrap();
        assert_eq!(find_program("tool", Some(&path)), Lookup::Program(tool));
        assert_eq!(find_program("plain", Some(&path)), Lookup::Missing);
        let relative = std::ffi::OsString::from("bin");
        assert_eq!(find_program("tool", Some(&relative)), Lookup::Missing);
    }

    #[test]
    fn a_file_keeps_its_identity_until_another_takes_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "one").unwrap();
        let id = |p: &Path| file_identity(p, &std::fs::metadata(p).unwrap());
        let first = id(&file).expect("readable here");
        std::fs::write(&file, "two").unwrap();
        assert_eq!(id(&file), Some(first), "written in place: the same file");
        let other = dir.path().join("b.txt");
        std::fs::write(&other, "two").unwrap();
        std::fs::rename(&other, &file).unwrap();
        assert_ne!(id(&file), Some(first), "replaced: another file");
    }

    #[test]
    fn git_is_known_by_its_program_name_per_platform() {
        assert!(is_git_program("macos", "git"));
        assert!(is_git_program("linux", "git"));
        assert!(!is_git_program("linux", "Git"));
        assert!(!is_git_program("linux", "git-remote-https"));
        assert!(!is_git_program("macos", "gitk"));
        assert!(is_git_program("windows", "git.exe"));
        assert!(is_git_program("windows", "GIT.EXE"));
        assert!(!is_git_program("windows", "git-bash.exe"));
    }

    #[test]
    fn no_git_runs_in_a_new_folder() {
        let dir = tempfile::tempdir().unwrap();
        let scan = git_running_in(&[dunce::canonicalize(dir.path()).unwrap()]);
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert_eq!(scan, GitScan::NoneRunning);
        } else {
            assert_eq!(scan, GitScan::Unknown);
        }
    }

    /// Killing the tree ends what the child started too: `ping`, started by `cmd`, holds the
    /// output pipe; the read ends at once only if it was ended with `cmd`.
    #[cfg(windows)]
    #[tokio::test]
    async fn the_whole_tree_ends_with_the_job() {
        use tokio::io::AsyncReadExt as _;
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.args(["/d", "/c", "ping -n 30 127.0.0.1"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true);
        child_flags(&mut cmd);
        let (mut child, tree) = spawn(&mut cmd).unwrap();
        let mut out = child.stdout.take().unwrap();
        let mut first = [0u8; 1];
        out.read_exact(&mut first).await.unwrap();
        let started = std::time::Instant::now();
        tree.kill();
        let mut rest = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            out.read_to_end(&mut rest),
        )
        .await
        .expect("the pipe closed: ping ended with the tree")
        .unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        let _ = child.wait().await;
    }

    #[test]
    fn this_process_runs_and_a_finished_one_is_gone() {
        assert!(!process_gone(std::process::id()));
        let mut child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                &["/c", "exit 0"][..]
            } else {
                &[][..]
            })
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(process_gone(pid));
    }
}
