//! Command-line parsing and validation. See README.md for the contract.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const DEFAULT_TIMEOUT_SECONDS: u32 = 120;

pub const USAGE: &str = "\
Usage: studio-run --script <path> [options]

Runs a Luau script in Roblox Studio on a hidden desktop.

Options:
  --script <path>      .lua or .luau file to run (required)
  --place <path>       .rbxl or .rbxlx file to open (default: empty baseplate)
  --timeout <seconds>  kill Studio after this long (default: 120)
  --log <path>         save Studio's complete output file here
  --visible            show Studio on the normal desktop
  --studio <path>      RobloxStudioBeta.exe to use (default: newest installed)
  --help               show this help

Exit codes:
  0    script ran to the end without an error
  1    script threw an error
  2    bad arguments, missing file, or Studio not found
  3    timeout reached and Studio killed
  4    Studio exited before the script finished
  130  cancelled with Ctrl+C
";

#[derive(Debug)]
pub struct Options {
    pub script: PathBuf,
    pub place: Option<PathBuf>,
    pub log: Option<PathBuf>,
    pub studio: PathBuf,
    pub timeout_seconds: u32,
    pub visible: bool,
}

#[derive(Debug)]
pub enum Command {
    Help,
    Run(Options),
}

#[derive(Debug, PartialEq)]
pub struct UsageError(pub String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Arguments as given, before any file checks.
#[derive(Debug, Default, PartialEq)]
struct RawOptions {
    script: Option<PathBuf>,
    place: Option<PathBuf>,
    log: Option<PathBuf>,
    studio: Option<PathBuf>,
    timeout_seconds: Option<u32>,
    visible: bool,
    help: bool,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, UsageError> {
    let raw = parse_raw(args)?;
    if raw.help {
        return Ok(Command::Help);
    }

    let script = raw
        .script
        .ok_or_else(|| UsageError("--script is required".into()))?;
    let script = require_file(&script, "--script", &["lua", "luau"])?;
    let place = raw
        .place
        .map(|path| require_file(&path, "--place", &["rbxl", "rbxlx"]))
        .transpose()?;
    let studio = match raw.studio {
        Some(path) => require_file(&path, "--studio", &["exe"])?,
        None => find_studio()?,
    };
    let log = raw.log.map(|path| require_log_path(&path)).transpose()?;

    Ok(Command::Run(Options {
        script,
        place,
        log,
        studio,
        timeout_seconds: raw.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS),
        visible: raw.visible,
    }))
}

fn parse_raw(args: impl IntoIterator<Item = OsString>) -> Result<RawOptions, UsageError> {
    let mut options = RawOptions::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let flag = arg.to_string_lossy().to_lowercase();
        let mut value = || {
            args.next()
                .ok_or_else(|| UsageError(format!("{} needs a value", arg.to_string_lossy())))
        };
        match flag.as_str() {
            "--help" | "-h" | "/?" => {
                options.help = true;
                return Ok(options);
            }
            "--visible" => options.visible = true,
            "--script" => options.script = Some(value()?.into()),
            "--place" => options.place = Some(value()?.into()),
            "--log" => options.log = Some(value()?.into()),
            "--studio" => options.studio = Some(value()?.into()),
            "--timeout" => {
                let text = value()?;
                let text = text.to_string_lossy();
                match text.trim().parse::<u32>() {
                    Ok(seconds) if seconds > 0 => options.timeout_seconds = Some(seconds),
                    _ => {
                        return Err(UsageError(format!(
                            "--timeout must be a positive whole number of seconds, got '{text}'"
                        )));
                    }
                }
            }
            _ => {
                return Err(UsageError(format!(
                    "unknown argument '{}'",
                    arg.to_string_lossy()
                )));
            }
        }
    }
    Ok(options)
}

fn absolute(path: &Path) -> Result<PathBuf, UsageError> {
    std::path::absolute(path)
        .map_err(|e| UsageError(format!("invalid path '{}': {e}", path.display())))
}

fn require_file(path: &Path, flag: &str, extensions: &[&str]) -> Result<PathBuf, UsageError> {
    let full = absolute(path)?;
    if !full.is_file() {
        return Err(UsageError(format!(
            "{flag} file not found: {}",
            full.display()
        )));
    }
    if !has_extension(&full, extensions) {
        let list: Vec<String> = extensions.iter().map(|e| format!(".{e}")).collect();
        return Err(UsageError(format!(
            "{flag} must be one of {}: {}",
            list.join(", "),
            full.display()
        )));
    }
    Ok(full)
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    let Some(extension) = path.extension() else {
        return false;
    };
    let extension = extension.to_string_lossy();
    extensions
        .iter()
        .any(|allowed| extension.eq_ignore_ascii_case(allowed))
}

fn require_log_path(path: &Path) -> Result<PathBuf, UsageError> {
    let full = absolute(path)?;
    let parent = full.parent().unwrap_or(Path::new(""));
    if !parent.is_dir() {
        return Err(UsageError(format!(
            "--log folder does not exist: {}",
            parent.display()
        )));
    }
    Ok(full)
}

/// Picks the most recently written `RobloxStudioBeta.exe` under
/// `%LOCALAPPDATA%\Roblox\Versions`. Folders that hold Roblox Player are skipped.
fn find_studio() -> Result<PathBuf, UsageError> {
    let versions = std::env::var_os("LOCALAPPDATA")
        .map(|local| PathBuf::from(local).join("Roblox").join("Versions"))
        .unwrap_or_else(|| PathBuf::from(r"%LOCALAPPDATA%\Roblox\Versions"));

    let mut newest: Option<(SystemTime, PathBuf)> = None;
    if let Ok(entries) = std::fs::read_dir(&versions) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("RobloxStudioBeta.exe");
            let Ok(written) = candidate.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            if candidate.is_file() && newest.as_ref().is_none_or(|(time, _)| written > *time) {
                newest = Some((written, candidate));
            }
        }
    }
    newest.map(|(_, path)| path).ok_or_else(|| {
        UsageError(format!(
            "Roblox Studio not found under {}; pass --studio <path>",
            versions.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(args: &[&str]) -> Result<RawOptions, UsageError> {
        parse_raw(args.iter().map(OsString::from))
    }

    #[test]
    fn flags_are_case_insensitive() {
        let options = raw(&["--SCRIPT", "a.luau", "--Visible", "--TimeOut", "60"]).unwrap();
        assert_eq!(options.script, Some(PathBuf::from("a.luau")));
        assert!(options.visible);
        assert_eq!(options.timeout_seconds, Some(60));
    }

    #[test]
    fn help_wins_once_reached() {
        assert!(raw(&["--script", "a.luau", "-h"]).unwrap().help);
        assert!(raw(&["/?", "--nope"]).unwrap().help);
        assert!(raw(&["--nope", "--help"]).is_err());
    }

    #[test]
    fn bad_timeouts_are_rejected() {
        for bad in ["0", "-5", "abc", "1.5", ""] {
            let error = raw(&["--timeout", bad]).unwrap_err();
            assert!(error.0.starts_with("--timeout must be"), "{bad}: {error}");
        }
    }

    #[test]
    fn missing_value_and_unknown_flag() {
        assert_eq!(
            raw(&["--script"]).unwrap_err(),
            UsageError("--script needs a value".into())
        );
        assert_eq!(
            raw(&["--nope"]).unwrap_err(),
            UsageError("unknown argument '--nope'".into())
        );
    }

    #[test]
    fn script_is_required() {
        let error = parse(Vec::<OsString>::new()).unwrap_err();
        assert_eq!(error, UsageError("--script is required".into()));
    }

    #[test]
    fn extension_check_ignores_case() {
        assert!(has_extension(Path::new("x.LuaU"), &["lua", "luau"]));
        assert!(!has_extension(Path::new("x.txt"), &["lua", "luau"]));
        assert!(!has_extension(Path::new("luau"), &["lua", "luau"]));
    }
}
