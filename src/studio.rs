//! Starts Studio inside a kill-on-close job, optionally on its own hidden desktop.
//! The job means Studio cannot outlive this tool, even if the tool itself is killed.

use std::ffi::{OsStr, c_void};
use std::fmt;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, CreateDesktopW, HDESK};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, GetExitCodeProcess, PROCESS_INFORMATION, ResumeThread,
    STARTUPINFOW, WaitForSingleObject,
};

const KILL_WAIT_MILLISECONDS: u32 = 15_000;

/// A failed Windows call, with what the tool was trying to do.
#[derive(Debug)]
pub struct LaunchError {
    context: &'static str,
    source: io::Error,
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.context, self.source)
    }
}

fn check(succeeded: bool, context: &'static str) -> Result<(), LaunchError> {
    if succeeded {
        Ok(())
    } else {
        Err(LaunchError {
            context,
            source: io::Error::last_os_error(),
        })
    }
}

/// NUL-terminated UTF-16, as Windows calls expect.
fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

pub struct StudioProcess {
    desktop: HDESK,
    job: HANDLE,
    process: HANDLE,
}

impl StudioProcess {
    /// Starts `command_line` suspended, adds it to the job, then resumes it, so
    /// Studio cannot start a child process outside the job.
    pub fn start(
        command_line: &OsStr,
        working_directory: &Path,
        hidden_desktop_name: Option<&str>,
    ) -> Result<Self, LaunchError> {
        // Fields are filled in as each handle is created, so Drop cleans up
        // whatever exists if a later step fails.
        let mut studio = StudioProcess {
            desktop: null_mut(),
            job: null_mut(),
            process: null_mut(),
        };

        let mut startup = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        // Declared out here because lpDesktop points into it during CreateProcessW below.
        let mut desktop_path: Vec<u16>;
        if let Some(name) = hidden_desktop_name {
            let name_wide = wide(OsStr::new(name));
            studio.desktop = unsafe {
                CreateDesktopW(name_wide.as_ptr(), null(), null(), 0, GENERIC_ALL, null())
            };
            check(!studio.desktop.is_null(), "could not create hidden desktop")?;
            desktop_path = wide(OsStr::new(&format!("winsta0\\{name}")));
            startup.lpDesktop = desktop_path.as_mut_ptr();
        }

        studio.job = unsafe { CreateJobObjectW(null(), null()) };
        check(!studio.job.is_null(), "could not create job object")?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                studio.job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast::<c_void>(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        check(configured != 0, "could not configure job object")?;

        // CreateProcessW may write to the command-line buffer, so it must be owned.
        let mut command = wide(command_line);
        let directory = wide(working_directory.as_os_str());
        let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let created = unsafe {
            CreateProcessW(
                null(),
                command.as_mut_ptr(),
                null(),
                null(),
                0,
                CREATE_SUSPENDED,
                null(),
                directory.as_ptr(),
                &startup,
                &mut info,
            )
        };
        check(created != 0, "could not start Roblox Studio")?;
        studio.process = info.hProcess;

        let resumed = (|| {
            let assigned = unsafe { AssignProcessToJobObject(studio.job, studio.process) };
            check(assigned != 0, "could not add Roblox Studio to job object")?;
            let previous = unsafe { ResumeThread(info.hThread) };
            check(previous != u32::MAX, "could not resume Roblox Studio")
        })();
        unsafe { CloseHandle(info.hThread) };
        resumed?;
        Ok(studio)
    }

    pub fn has_exited(&self) -> bool {
        unsafe { WaitForSingleObject(self.process, 0) == WAIT_OBJECT_0 }
    }

    pub fn exit_code(&self) -> u32 {
        let mut code = 0;
        unsafe { GetExitCodeProcess(self.process, &mut code) };
        code
    }

    /// Ends Studio and everything it started, then waits for it to go.
    pub fn kill(&self) {
        unsafe {
            TerminateJobObject(self.job, 1);
            WaitForSingleObject(self.process, KILL_WAIT_MILLISECONDS);
        }
    }
}

impl Drop for StudioProcess {
    fn drop(&mut self) {
        unsafe {
            if !self.process.is_null() {
                CloseHandle(self.process);
            }
            if !self.job.is_null() {
                // Kill-on-close ends anything still running in the job.
                CloseHandle(self.job);
            }
            if !self.desktop.is_null() {
                CloseDesktop(self.desktop);
            }
        }
    }
}
