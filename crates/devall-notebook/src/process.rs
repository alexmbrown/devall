//! Spawning interpreters without console windows, and stopping a cell's whole process tree.
//!
//! - Windows: every process gets `CREATE_NO_WINDOW` and is put in a job object with
//!   `KILL_ON_JOB_CLOSE`, so interrupting (or the app exiting) ends everything the cell started.
//!   `taskkill /T /F` is the fallback when the job can't be created.
//! - Unix: every process leads its own process group; interrupt sends `SIGINT` to the group,
//!   kill sends `SIGKILL`.

use std::path::Path;

use tokio::process::{Child, Command};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// An async command for `program` that runs hidden, in its own process group / job.
pub(crate) fn command(program: &Path) -> Command {
    let mut c = Command::new(program);
    c.kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    #[cfg(unix)]
    c.process_group(0);
    c
}

/// A blocking command (interpreter detection), hidden on Windows.
pub(crate) fn std_command(program: &Path) -> std::process::Command {
    #[allow(unused_mut)]
    let mut c = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Runs a blocking command with a deadline; returns (exit success, stdout, stderr).
pub(crate) fn output_with_timeout(mut cmd: std::process::Command, timeout: std::time::Duration) -> Option<(bool, String, String)> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let mut out = child.stdout.take()?;
    let mut err = child.stderr.take()?;
    let reader = std::thread::spawn(move || {
        let mut o = Vec::new();
        let _ = out.read_to_end(&mut o);
        o
    });
    let err_reader = std::thread::spawn(move || {
        let mut e = Vec::new();
        let _ = err.read_to_end(&mut e);
        e
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let o = reader.join().unwrap_or_default();
    let e = err_reader.join().unwrap_or_default();
    let status = status?;
    Some((status.success(), String::from_utf8_lossy(&o).into_owned(), String::from_utf8_lossy(&e).into_owned()))
}

/// Handle on a spawned process and everything it starts.
pub(crate) struct ProcessTree {
    pid: Option<u32>,
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl ProcessTree {
    pub fn attach(child: &Child) -> Self {
        #[cfg(windows)]
        {
            let job = job::Job::new().filter(|j| child.raw_handle().is_some_and(|h| j.assign(h)));
            Self { pid: child.id(), job }
        }
        #[cfg(not(windows))]
        {
            Self { pid: child.id() }
        }
    }

    /// Ctrl-C equivalent: `SIGINT` to the process group (Unix). Windows has no console to signal,
    /// so this ends the tree.
    pub fn interrupt(&self) {
        #[cfg(unix)]
        self.signal(libc::SIGINT);
        #[cfg(windows)]
        self.kill();
    }

    /// Ends the process and all of its descendants.
    pub fn kill(&self) {
        #[cfg(unix)]
        self.signal(libc::SIGKILL);
        #[cfg(windows)]
        {
            if let Some(job) = &self.job {
                job.terminate();
            } else if let Some(pid) = self.pid {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("taskkill")
                    .args(["/T", "/F", "/PID", &pid.to_string()])
                    .creation_flags(CREATE_NO_WINDOW)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
        }
    }

    #[cfg(unix)]
    fn signal(&self, sig: i32) {
        if let Some(pid) = self.pid {
            // SAFETY: plain syscall; a negative pid addresses the process group we created.
            unsafe {
                libc::kill(-(pid as i32), sig);
            }
        }
    }
}

#[cfg(windows)]
mod job {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };

    /// A job object that kills its processes when terminated or when the handle closes.
    pub struct Job(HANDLE);

    // SAFETY: a job handle is a kernel object handle, usable from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new() -> Option<Self> {
            // SAFETY: FFI calls with valid pointers; the handle is closed on failure / drop.
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    CloseHandle(h);
                    return None;
                }
                Some(Self(h))
            }
        }

        pub fn assign(&self, process: std::os::windows::io::RawHandle) -> bool {
            // SAFETY: `process` is the live handle owned by tokio's Child.
            unsafe { AssignProcessToJobObject(self.0, process as HANDLE) != 0 }
        }

        pub fn terminate(&self) {
            // SAFETY: valid job handle.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: we own the handle; closing it kills remaining processes (KILL_ON_JOB_CLOSE).
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
