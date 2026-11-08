//! Follows Studio's output file as it grows and routes each line by run state.
//! Raw bytes are forwarded unchanged, so script output keeps its exact encoding.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

use crate::wrapper::Markers;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    WaitingForBegin,
    Running,
    CollectingError,
    Passed,
    Failed,
}

pub struct OutputFollower<'a, Out: Write, Err: Write> {
    path: PathBuf,
    markers: &'a Markers,
    stdout: Out,
    stderr: Err,
    partial_line: Vec<u8>,
    position: u64,
    pub state: RunState,
    /// Studio reports nothing for a syntax error: it echoes the source and never
    /// runs it. An echo with no first marker is therefore how a compile failure
    /// shows up.
    pub source_echoed: bool,
}

impl<'a, Out: Write, Err: Write> OutputFollower<'a, Out, Err> {
    pub fn new(path: PathBuf, markers: &'a Markers, stdout: Out, stderr: Err) -> Self {
        OutputFollower {
            path,
            markers,
            stdout,
            stderr,
            partial_line: Vec::new(),
            position: 0,
            state: RunState::WaitingForBegin,
            source_echoed: false,
        }
    }

    pub fn finished(&self) -> bool {
        matches!(self.state, RunState::Passed | RunState::Failed)
    }

    /// Reads whatever Studio has added since the last poll.
    pub fn poll(&mut self) {
        // Studio holds the file briefly while writing; a failed read is retried
        // on the next poll.
        let Ok(chunk) = self.read_new_bytes() else {
            return;
        };
        self.position += chunk.len() as u64;
        for line in chunk.split_inclusive(|&b| b == b'\n') {
            if let Some(line) = line.strip_suffix(b"\n") {
                self.partial_line.extend_from_slice(line);
                let complete = std::mem::take(&mut self.partial_line);
                self.handle_line(&complete);
            } else {
                self.partial_line.extend_from_slice(line);
            }
        }
    }

    /// Handles a last line that was written without a trailing newline.
    pub fn flush(&mut self) {
        if !self.partial_line.is_empty() {
            let line = std::mem::take(&mut self.partial_line);
            self.handle_line(&line);
        }
    }

    fn read_new_bytes(&self) -> io::Result<Vec<u8>> {
        let mut file: File = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(&self.path)?;
        let length = file.metadata()?.len();
        if length <= self.position {
            return Ok(Vec::new());
        }
        file.seek(SeekFrom::Start(self.position))?;
        let mut chunk = Vec::with_capacity((length - self.position) as usize);
        file.read_to_end(&mut chunk)?;
        Ok(chunk)
    }

    fn handle_line(&mut self, raw: &[u8]) {
        if self.finished() {
            return;
        }
        let decoded = String::from_utf8_lossy(raw);
        let line = decoded.trim_matches(['\r', ' ', '\u{FEFF}']);
        let markers = self.markers;
        match self.state {
            RunState::WaitingForBegin => {
                if line == markers.begin {
                    self.state = RunState::Running;
                } else if line.starts_with(&markers.source_echo) {
                    self.source_echoed = true;
                }
            }
            RunState::Running => {
                if line == markers.ok {
                    self.state = RunState::Passed;
                } else if line == markers.fail {
                    self.state = RunState::CollectingError;
                } else {
                    forward(&mut self.stdout, raw);
                }
            }
            RunState::CollectingError => {
                if line == markers.end {
                    self.state = RunState::Failed;
                } else {
                    forward(&mut self.stderr, raw);
                }
            }
            RunState::Passed | RunState::Failed => {}
        }
    }
}

fn forward(target: &mut impl Write, raw: &[u8]) {
    let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
    let mut line = Vec::with_capacity(raw.len() + 1);
    line.extend_from_slice(raw);
    line.push(b'\n');
    // A closed pipe, or bytes a console rejects, must not end the run: the exit
    // code still has to be correct.
    let _ = target.write_all(&line);
    let _ = target.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: PathBuf,
        markers: Markers,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("studio-run-unit-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Fixture {
                dir,
                markers: Markers::new("n0"),
            }
        }

        fn log(&self) -> PathBuf {
            self.dir.join("output.log")
        }

        fn append(&self, bytes: &[u8]) {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.log())
                .unwrap();
            file.write_all(bytes).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn routes_lines_by_state() {
        let fx = Fixture::new("routes");
        fx.append(
            b"engine noise\r\n\
              > print(\"__STUDIO_RUN_n0_\"..\"BEGIN__\");local x\r\n\
              __STUDIO_RUN_n0_BEGIN__\r\n\
              hello \xE2\x9C\x93\r\n\
              __STUDIO_RUN_n0_FAIL__\n\
              RunScript:2: boom\n\
              __STUDIO_RUN_n0_END__\n\
              after the end\n",
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut follower = OutputFollower::new(fx.log(), &fx.markers, &mut out, &mut err);
        follower.poll();
        assert_eq!(follower.state, RunState::Failed);
        assert!(follower.source_echoed);
        drop(follower);
        assert_eq!(out, b"hello \xE2\x9C\x93\n");
        assert_eq!(err, b"RunScript:2: boom\n");
    }

    #[test]
    fn keeps_partial_lines_between_polls() {
        let fx = Fixture::new("partial");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut follower = OutputFollower::new(fx.log(), &fx.markers, &mut out, &mut err);
        follower.poll(); // File does not exist yet.
        fx.append(b"__STUDIO_RUN_n0_BEG");
        follower.poll();
        assert_eq!(follower.state, RunState::WaitingForBegin);
        fx.append(b"IN__\npart");
        follower.poll();
        assert_eq!(follower.state, RunState::Running);
        fx.append(b"ial\n__STUDIO_RUN_n0_OK__");
        follower.poll();
        assert_eq!(follower.state, RunState::Running);
        follower.flush();
        assert_eq!(follower.state, RunState::Passed);
        drop(follower);
        assert_eq!(out, b"partial\n");
        assert!(err.is_empty());
    }

    #[test]
    fn echo_without_begin_is_detected() {
        let fx = Fixture::new("echo");
        fx.append(b"\xEF\xBB\xBF> print(\"__STUDIO_RUN_n0_\"..\"BEGIN__\");local = 1\n");
        let mut follower = OutputFollower::new(fx.log(), &fx.markers, io::sink(), io::sink());
        follower.poll();
        assert!(follower.source_echoed);
        assert_eq!(follower.state, RunState::WaitingForBegin);
    }
}
