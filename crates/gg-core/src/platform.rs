//! What differs between the operating systems where the app runs other programs and looks
//! at files: finding a program on a `PATH`, the process tree a child and everything it
//! starts form (so that a time limit, a Stop or the app's exit ends all of it), whether a
//! process still runs, and what tells one file from another without reading it.
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

#[cfg(windows)]
mod job {
    //! The Windows calls behind [`super::ProcessTree`] and friends.

    use std::ffi::c_void;
    use std::io;
    use std::os::windows::io::AsRawHandle as _;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
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
    pub(super) fn file_identity(path: &Path) -> Option<u128> {
        let file = std::fs::File::open(path).ok()?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the file handle is open (held by `file`); `info` is a valid out pointer.
        let ok =
            unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &raw mut info) };
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
