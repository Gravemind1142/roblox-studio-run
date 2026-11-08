# studio-run: technical approach for a Rust port

`studio-run` runs one Luau script in Roblox Studio on a hidden Windows desktop and reports the result through its own exit code. This document holds what you need to rebuild it, including the Studio behavior that had to be discovered by testing.

A working C# version exists (`src/StudioRun.cs`, about 790 lines). Treat it as the reference and `tests/run-integration.ps1` as the acceptance test.

The Studio findings were tested on Studio 0.746.0.7461269 on Windows 11. The Rust advice in sections 5 and 8 has not been built or tested.

## 1. Command-line contract

```text
studio-run --script <path> [--place <path>] [--timeout <seconds>] [--log <path>] [--visible] [--studio <path>] [--help]
```

| Option | Default | Rules |
|---|---|---|
| `--script` | Required | Must exist; `.lua` or `.luau` |
| `--place` | Empty baseplate | Must exist; `.rbxl` or `.rbxlx`. Studio opens a temporary copy |
| `--timeout` | `120` | Positive whole seconds |
| `--log` | Not saved | Parent folder must already exist. Overwrites the file |
| `--visible` | Off | Uses the normal desktop instead of a hidden one |
| `--studio` | Newest installed | Must exist; `.exe` |
| `--help`, `-h`, `/?` | | Prints usage and exits `0` |

Flags are case-insensitive, and each value is a separate argument (`--timeout 60`). Relative paths are converted to absolute.

| Exit code | Meaning |
|---|---|
| `0` | The script ran to the end without an error |
| `1` | The script threw an error or did not compile |
| `2` | Bad arguments, a missing file, or Studio not found |
| `3` | The timeout was reached and Studio was killed |
| `4` | Studio exited before the script finished, or the tool could not start it |
| `130` | Cancelled with Ctrl+C |

- **`stdout`:** only what the script prints, streamed live as raw bytes.
- **`stderr`:** the tool's own messages, prefixed `studio-run: `, plus the script's error and traceback.
- **Pass rule:** a script passes if it finishes without throwing. Errors in spawned threads don't count.

## 2. The Studio command

Official docs: <https://create.roblox.com/docs/studio/cli>

```text
"<RobloxStudioBeta.exe>" --task RunScript --runScriptFile "<wrapper.luau>" --outputFile "<output.log>" --quitAfterExecution [--localPlaceFile "<place copy>"]
```

**Finding Studio:** look in `%LOCALAPPDATA%\Roblox\Versions\*\RobloxStudioBeta.exe`, and choose the file with the newest last-write time. Some version folders hold Roblox Player and have no Studio exe, so skip those.

## 3. Studio behavior the docs don't mention

The first two findings below are what shape the design.

1. **The exit code says nothing about the script.** On the normal desktop, 6 of 7 early runs crashed with `0xC0000005` as Studio quit, including passing scripts. A script that threw exited `0` once, and also on both hidden-desktop runs.
2. **Syntax errors are silent.** Studio echoes the source, never runs it, reports no error in either log, and exits `0`. No line number is available.
3. **`--outputFile` needs an absolute path ending in `.log`, with an existing parent folder.** Otherwise Studio writes nothing, and the only sign is an error in its own log:
   - `Output file must have a .log extension: C` appeared for a `.txt` path
   - `Output path's parent directory must exist: <path>` appeared for a relative path
4. **The output file has extra content.** It includes engine warnings, plugin messages, and an echo of the whole script source, with the first line prefixed by `> `. Script output lines have no prefix.
5. **The output file is UTF-8.** `✓` and similar characters must pass through as bytes.
6. **Studio sometimes doesn't quit.** It stayed open after the script finished in about 2 of 25 runs, so the tool needs a kill fallback.
7. **Scripts run in edit mode with command bar permissions.** `RunService:IsRunning()` is false, `IsEdit()` is true, and `IsServer()` and `IsClient()` are both true. `script` is `nil`. Physics stepping wasn't tested.
8. **Yielding works.** `task.wait` resumes, and Studio waits for the script to finish before quitting. `task.wait(math.huge)` falls back to 0, so use a loop in any hang test.
9. **`require` works with a place.** `require("@game/ReplicatedStorage/...")` works from the run script, including from inside the wrapper's function.
10. **User plugins still load.** Their messages can appear while the script runs. The binary contains the string `disableloaduserplugins`, which is undocumented and untested.
11. **Studio's logs go to `stderr` when redirected**, not `stdout` as the docs say. The tool doesn't use them.
12. **Studio's own log** is at `%LOCALAPPDATA%\Roblox\logs\*Studio*_last.log`, which helps with debugging. Crash dumps are in `logs\crashes\reports`.

## 4. Design

```text
parse args -> make temp folder -> write wrapper script -> copy place
           -> create hidden desktop and job -> start Studio suspended
           -> add to job -> resume -> poll output file and process
           -> map to exit code -> save --log -> clean up
```

### Script wrapper

The tool never runs the user's script directly. It wraps it to learn that it started and how it ended, using a new random hex nonce per run:

```lua
print("__STUDIO_RUN_<nonce>_".."BEGIN__");local __studioRunOk,__studioRunErr=xpcall(function(...) <USER SOURCE>
end,function(e) return debug.traceback(tostring(e),2) end);if __studioRunOk then print("__STUDIO_RUN_<nonce>_".."OK__") else print("__STUDIO_RUN_<nonce>_".."FAIL__");print(__studioRunErr);print("__STUDIO_RUN_<nonce>_".."END__") end
```

Each part of this is deliberate:

- **Split strings (`"a".."b"`):** the source echo contains the split form, so the finished marker can only appear when the code really runs.
- **Prefix stays on the user's first line:** error line numbers stay correct.
- **Suffix starts on a new line:** a comment on the user's last line can't swallow it.
- **Function body:** a top-level `return` in the user's script stays valid, which a footer appended to the file would not allow.
- **BOM:** strip any UTF-8 BOM from the source first.

### Reading the output

Poll the file every 100 ms, reading only new bytes. Open it with read, write and delete sharing, and treat a failed read as "retry on the next poll". Split on `\n`, and keep any incomplete line for the next read.

Compare each line to the markers after decoding as UTF-8 (lossy is fine) and trimming `\r`, spaces and BOM. Forward the original bytes, minus a trailing `\r`, followed by `\n`.

| State | Line | Action |
|---|---|---|
| Waiting for begin | BEGIN marker | Go to Running |
| Waiting for begin | Starts with `> print("__STUDIO_RUN_<nonce>_"` | Record that the source was echoed |
| Waiting for begin | Anything else | Discard |
| Running | OK marker | Result is pass |
| Running | FAIL marker | Go to Collecting error |
| Running | Anything else | Forward to `stdout` |
| Collecting error | END marker | Result is fail |
| Collecting error | Anything else | Forward to `stderr` |

### Supervision loop

Repeat every 100 ms, in this order:

1. Check whether Studio has exited, and remember the answer.
2. Read new output. Checking in this order means the read after a detected exit is complete.
3. **If there's a result:** wait up to 15s for Studio to quit, then kill it. If it exited nonzero, warn on `stderr`. Return `0` or `1`, whatever the crash status.
4. **If Studio has exited with no result:** process any incomplete last line and recheck. If the source was echoed but BEGIN never appeared, the script didn't compile, so print that and return `1`. Otherwise print Studio's exit code in hex and return `4`.
5. **If Ctrl+C was pressed:** kill Studio, do a final read, and return `130`.
6. **If the timeout has passed:** kill Studio, do a final read, print that it may be stuck on a dialog, and return `3`.

### Process launch

Standard process APIs can't choose a desktop, so this needs `CreateProcessW` directly:

1. `CreateDesktopW` with a unique name and `GENERIC_ALL`. Skip this for `--visible`.
2. `CreateJobObjectW`, then `SetInformationJobObject` with class 9 (`JobObjectExtendedLimitInformation`) and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
3. `CreateProcessW` with `CREATE_SUSPENDED`, no inherited handles, the temp folder as the working directory, and `lpDesktop` set to `winsta0\<name>`.
4. `AssignProcessToJobObject`, then `ResumeThread`. Starting suspended means Studio can't launch a child process before it's in the job.
5. To kill: `TerminateJobObject`, then wait up to 15s on the process.
6. To clean up, in this order: close the process handle, the job handle (which kills anything left), and the desktop.

The job object matters because a killed tool would otherwise leave an invisible Studio running.

### Temp files and cleanup

Use `%TEMP%\studio-run-<nonce>\`, containing `script.luau`, `output.log` and `place.<ext>`. Copy `output.log` to the `--log` path before deleting the folder. Studio may hold files briefly after exit, so retry the delete up to 10 times, 300 ms apart, and then ignore failure. A force-killed tool leaves this folder behind.

## 5. Rust notes

Not yet tested in Rust, so check these against current crate docs.

| Need | Suggestion |
|---|---|
| Windows calls | `windows-sys`, with features for `Win32_Foundation`, `Win32_Security`, `Win32_System_Threading`, `Win32_System_JobObjects`, `Win32_System_StationsAndDesktops` and `Win32_System_Console` |
| Arguments | Hand-written is good practice. `clap` is the usual choice, but check it can match the exit code `2` and case-insensitive flags |
| Nonce | The `uuid` crate, or anything producing unique hex |
| Ctrl+C | `SetConsoleCtrlHandler` setting an `AtomicBool`, or the `ctrlc` crate |
| File sharing | `OpenOptionsExt::share_mode` with read, write and delete flags |
| Cleanup | `Drop` types for the desktop, job, process and temp folder |

Things likely to catch you out:

- **Wide strings:** Windows calls need NUL-terminated UTF-16. Use `OsStr::encode_wide` and append a `0`.
- **Mutable command line:** `CreateProcessW` can modify its command-line buffer, so pass a `Vec<u16>` you own, not a constant.
- **Lifetimes:** the wide strings must stay alive for the whole call, so don't create them inline in a way that frees them early.
- **Raw byte output:** Rust's `stdout` rejects invalid UTF-8 when attached to a console, so don't let a write error end the run. Write bytes, flush after each line for live output, and ignore write failures such as a closed pipe, so the exit code stays correct.
- **Exit codes on `Drop`:** `std::process::exit` skips destructors. Return a code from an inner `run` function so cleanup still happens.
- **Exit code format:** `GetExitCodeProcess` returns a `u32`, so `0xC0000005` prints naturally in hex.
- **Windows-only build:** the toolchain choice affects only linking. GNU needs no Visual Studio install, while MSVC is the default on Windows and needs the Build Tools.

## 6. Tests

`tests/run-integration.ps1` runs the executable against real Studio, so Studio must be installed and logged in. Update its exe path for the Rust build. It takes about 2 minutes.

| Fixture | Contents | Expected |
|---|---|---|
| `pass.luau` | `print` and `warn` | `0`; `stdout` has the script's output and no markers, source echo or `Failed to apply` noise |
| `throw.luau` | `print`, then `error()` on line 2 | `1`; `stderr` has `RunScript:2:` and the message |
| `yield.luau` | `task.wait(0.5)`, then `print` | `0`; the print appears |
| `early-return.luau` | `print`, then `return true` | `0` |
| `syntax-error.luau` | `print`, then `local = 1` | `1`; `stderr` says it did not compile; the print doesn't appear |
| `unicode.luau` | Prints `✓ — héllo`; the last line is a comment with no newline | `0`; the bytes are intact |
| `hang.luau` | `while true do task.wait(1) end` with `--timeout 20` | `3` |

The script also checks usage errors (exit `2`), `--visible`, `--log`, and that no temp folders are left behind.

## 7. Verification status of the C# version

**Verified:**
- All the integration cases above passed.
- A real project suite built with Rojo exited `0` with 9/9 passing.
- The same suite with a deliberately failing test exited `1`, with the failing test shown.
- A passing and a failing suite run at the same time each returned the right code, in 15.4s total.
- Force-killing the tool closed Studio.

**Only partly verified:** the shutdown wait was cut from 30s to 15s after the last full test run, and only a passing script and the kill test were rerun.

**Not tested:**
- Ctrl+C. The handler is written, but it was never triggered.
- An expired login or a stuck dialog. Both are expected to end in a timeout.
- Rendering on the hidden desktop, such as screenshots.
- Whether hidden-desktop runs avoid the shutdown crash. Both early hidden runs exited `0`, which is a small sample.

## 8. Known limitations and options

- Plugin messages can appear in `stdout` while a script runs. This is best-effort filtering and doesn't affect the exit code.
- Syntax errors have no line number. Trying `loadstring` for details is untested, and it might change how `require` behaves.
- `lute tests` in the sample project reads `[SNAPSHOT_UPDATE:...]` blocks from `stdout`, so it could adopt this tool. That integration hasn't been built.
- For distribution, Foreman can install tools from GitHub releases, which suits a Rust version.
