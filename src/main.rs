//! studio-run: runs one Luau script in Roblox Studio on a hidden Windows desktop
//! and reports the result through its own exit code and output streams.
//!
//! See README.md for the command-line contract.

#[cfg(not(windows))]
compile_error!(
    "studio-run only supports Windows, because it drives Roblox Studio through Win32 APIs"
);

mod follower;
mod options;
mod studio;
mod wrapper;

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};
use windows_sys::core::BOOL;

use follower::{OutputFollower, RunState};
use options::{Command, Options};
use studio::StudioProcess;
use wrapper::Markers;

mod exit {
    pub const PASS: u8 = 0;
    pub const SCRIPT_FAILED: u8 = 1;
    pub const USAGE: u8 = 2;
    pub const TIMEOUT: u8 = 3;
    pub const STUDIO_FAILED: u8 = 4;
    pub const CANCELLED: u8 = 130;
}

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(15);

static CANCELLED: AtomicBool = AtomicBool::new(false);

fn main() -> ExitCode {
    // Every path returns a code instead of calling process::exit, so the Drop
    // cleanup of Studio, the job, the desktop and the temp folder always runs.
    let options = match options::parse(std::env::args_os().skip(1)) {
        Ok(Command::Run(options)) => options,
        Ok(Command::Help) => {
            let _ = io::stdout().write_all(options::USAGE.as_bytes());
            return ExitCode::from(exit::PASS);
        }
        Err(error) => {
            message(&error.0);
            message("run 'studio-run --help' for usage");
            return ExitCode::from(exit::USAGE);
        }
    };

    unsafe { SetConsoleCtrlHandler(Some(on_console_ctrl), 1) };

    let nonce = new_nonce();
    let work_dir = match TempDir::create(std::env::temp_dir().join(format!("studio-run-{nonce}"))) {
        Ok(dir) => dir,
        Err(error) => {
            message(&error);
            return ExitCode::from(exit::STUDIO_FAILED);
        }
    };
    let code = run(&options, &nonce, &work_dir.0).unwrap_or_else(|error| {
        message(&error);
        exit::STUDIO_FAILED
    });
    drop(work_dir);
    ExitCode::from(code)
}

fn run(options: &Options, nonce: &str, work_dir: &Path) -> Result<u8, String> {
    let markers = Markers::new(nonce);
    let script_path = work_dir.join("script.luau");
    let output_path = work_dir.join("output.log");

    let source = fs::read(&options.script)
        .map_err(|e| format!("could not read {}: {e}", options.script.display()))?;
    fs::write(&script_path, markers.wrap(&source))
        .map_err(|e| format!("could not write {}: {e}", script_path.display()))?;

    let mut command = OsString::new();
    push_quoted(&mut command, &options.studio);
    command.push(" --task RunScript --runScriptFile ");
    push_quoted(&mut command, &script_path);
    // Studio silently writes nothing unless this is an absolute .log path.
    command.push(" --outputFile ");
    push_quoted(&mut command, &output_path);
    command.push(" --quitAfterExecution");
    if let Some(place) = &options.place {
        // Studio opens a temporary copy so it can never change or lock the caller's place.
        let mut copy = work_dir.join("place");
        if let Some(extension) = place.extension() {
            copy.set_extension(extension);
        }
        fs::copy(place, &copy).map_err(|e| format!("could not copy {}: {e}", place.display()))?;
        command.push(" --localPlaceFile ");
        push_quoted(&mut command, &copy);
    }

    let desktop_name = (!options.visible).then(|| format!("studio-run-{nonce}"));
    let mut follower =
        OutputFollower::new(output_path.clone(), &markers, io::stdout(), io::stderr());
    let result = {
        let studio = StudioProcess::start(&command, work_dir, desktop_name.as_deref())
            .map_err(|e| e.to_string())?;
        supervise(&studio, &mut follower, options.timeout_seconds)
    };

    if let Some(log) = &options.log {
        save_log(&output_path, log);
    }
    Ok(result)
}

fn supervise<O: Write, E: Write>(
    studio: &StudioProcess,
    follower: &mut OutputFollower<'_, O, E>,
    timeout_seconds: u32,
) -> u8 {
    let started = Instant::now();
    let timeout = Duration::from_secs(timeout_seconds.into());
    loop {
        // Check for exit before reading, so the read after a detected exit is the final one.
        let exited = studio.has_exited();
        follower.poll();
        if follower.finished() {
            return finish_known_result(studio, follower);
        }
        if exited {
            follower.flush();
            if follower.finished() {
                return finish_known_result(studio, follower);
            }
            if follower.source_echoed && follower.state == RunState::WaitingForBegin {
                message(
                    "the script did not compile, probably a syntax error. Studio gives no details; \
                     check it with a linter such as luau-lsp.",
                );
                return exit::SCRIPT_FAILED;
            }
            message(&format!(
                "Studio exited with 0x{:08X} before the script finished. Rerun with --visible or --log to investigate.",
                studio.exit_code()
            ));
            return exit::STUDIO_FAILED;
        }
        if CANCELLED.load(Ordering::SeqCst) {
            studio.kill();
            follower.poll();
            message("cancelled");
            return exit::CANCELLED;
        }
        if started.elapsed() >= timeout {
            studio.kill();
            follower.poll();
            message(&format!(
                "timed out after {timeout_seconds}s and killed Studio. Studio may be stuck on a dialog; \
                 rerun with --visible to check."
            ));
            return exit::TIMEOUT;
        }
        sleep(POLL_INTERVAL);
    }
}

/// The script's result is final at this point. Studio's own shutdown cannot change it.
fn finish_known_result<O: Write, E: Write>(
    studio: &StudioProcess,
    follower: &OutputFollower<'_, O, E>,
) -> u8 {
    let grace = Instant::now();
    while !studio.has_exited()
        && !CANCELLED.load(Ordering::SeqCst)
        && grace.elapsed() < SHUTDOWN_GRACE
    {
        sleep(POLL_INTERVAL);
    }
    if !studio.has_exited() {
        if !CANCELLED.load(Ordering::SeqCst) {
            message(&format!(
                "Studio had not quit {}s after the script finished, so it was killed.",
                SHUTDOWN_GRACE.as_secs()
            ));
        }
        studio.kill();
    } else if studio.exit_code() != 0 {
        message(&format!(
            "warning: Studio exited with 0x{:08X} after the script finished; the script result is unaffected.",
            studio.exit_code()
        ));
    }
    if follower.state == RunState::Passed {
        exit::PASS
    } else {
        exit::SCRIPT_FAILED
    }
}

fn save_log(source: &Path, destination: &Path) {
    if !source.is_file() {
        message("warning: Studio did not write an output file, so --log was not saved.");
        return;
    }
    if let Err(error) = fs::copy(source, destination) {
        message(&format!("warning: could not save --log: {error}"));
    }
}

fn push_quoted(command: &mut OsString, path: &Path) {
    command.push("\"");
    command.push(path);
    command.push("\"");
}

fn message(text: &str) {
    let _ = writeln!(io::stderr(), "studio-run: {text}");
}

/// Handles Ctrl+C and Ctrl+Break so Studio is killed and the tool exits 130.
/// Other events, such as closing the console, fall through to the default
/// handler; the job object then ends Studio.
unsafe extern "system" fn on_console_ctrl(ctrl_type: u32) -> BOOL {
    if ctrl_type == CTRL_C_EVENT || ctrl_type == CTRL_BREAK_EVENT {
        CANCELLED.store(true, Ordering::SeqCst);
        1
    } else {
        0
    }
}

/// 32 hex characters, unique per run. `RandomState` is seeded from the OS's
/// random source; mixing in the time and process ID guards against reuse.
fn new_nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut nonce = String::with_capacity(32);
    for half in 0..2u8 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(nanos);
        hasher.write_u32(std::process::id());
        hasher.write_u8(half);
        let _ = write!(nonce, "{:016x}", hasher.finish());
    }
    nonce
}

/// The per-run temp folder. Studio may hold files briefly after it exits, so
/// deletion is retried and then given up on quietly.
struct TempDir(PathBuf);

impl TempDir {
    fn create(path: PathBuf) -> Result<Self, String> {
        fs::create_dir(&path).map_err(|e| format!("could not create {}: {e}", path.display()))?;
        Ok(TempDir(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        for _ in 0..10 {
            if !self.0.exists() || fs::remove_dir_all(&self.0).is_ok() {
                return;
            }
            sleep(Duration::from_millis(300));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonces_are_unique_hex() {
        let a = new_nonce();
        let b = new_nonce();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
