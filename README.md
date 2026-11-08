# studio-run

Runs a Luau script in Roblox Studio from the command line, on a hidden Windows desktop, and reports the result through a reliable exit code. It works with any test runner.

```powershell
rojo build test.project.json --output output.rbxl
studio-run --script game\ReplicatedStorage\Tests\run-tests.luau --place output.rbxl
```

Studio still starts in full, so this is headless-like rather than truly headless. A run usually takes 8–15 seconds. Scripts run in edit mode with command bar permissions, so `RunService:IsRunning()` is `false`.

## Requirements

- Windows 10 or 11
- Roblox Studio, installed and logged in

`studio-run.exe` is a single file with nothing else to install. It is meant for local use, not for CI.

## Options

| Option | Default | Purpose |
|---|---|---|
| `--script <path>` | Required | The `.lua` or `.luau` file to run |
| `--place <path>` | Empty baseplate | The `.rbxl` or `.rbxlx` file to open. Studio opens a temporary copy, so your file is never changed |
| `--timeout <seconds>` | `120` | Kills Studio after this long |
| `--log <path>` | Not saved | Saves Studio's complete output file. The folder must already exist |
| `--visible` | Off | Shows Studio on the normal desktop |
| `--studio <path>` | Newest installed | The `RobloxStudioBeta.exe` to use |

## Exit codes

| Code | Meaning |
|---|---|
| `0` | The script ran to the end without an error |
| `1` | The script threw an error or did not compile |
| `2` | Bad arguments, a missing file, or Studio not found |
| `3` | The timeout was reached and Studio was killed |
| `4` | Studio exited before the script finished |
| `130` | Cancelled with Ctrl+C |

A script passes if it finishes without throwing, so a test runner must call `error()` when tests fail. Errors in threads the script spawns, such as with `task.spawn`, do not count.

If Studio crashes or hangs after the script has finished, the result stands and a warning goes to `stderr`. Studio is killed if it hasn't quit 15 seconds after the script ends.

## Output

- **`stdout`:** what the script prints, streamed as Studio writes it.
- **`stderr`:** `studio-run` messages, plus the error and stack trace if the script throws.

## Known limitations

- **Plugin messages in `stdout`:** Studio's startup messages are removed, but messages from plugins while the script is running can't be told apart from script output. Your own plugins still load, and these messages are more likely if the script yields. They don't affect the exit code.
- **Syntax errors:** Studio reports nothing for them, so `studio-run` exits `1` with no line number. Use a linter such as `luau-lsp` for details.
- **Hidden dialogs:** a login prompt or error dialog can't be seen or clicked on the hidden desktop, so the run ends in a timeout. Use `--visible` to check.
- **Crash dumps:** Studio often crashes as it quits when run on the normal desktop, leaving dumps in `%LOCALAPPDATA%\Roblox\logs\crashes\reports`.
- **Undocumented Studio behavior:** this tool relies on `--outputFile` needing an absolute `.log` path and on the format of Studio's output file. A Studio update could change either. Tested on Studio 0.746.

## Building and testing

Install Rust with [rustup](https://rustup.rs) and the Visual Studio C++ Build Tools. `rust-toolchain.toml` selects the stable toolchain with `rustfmt` and `clippy`. The release build creates `target\release\studio-run.exe`, which links the C runtime statically so it runs on its own.

```powershell
cargo build --release
```

The unit tests don't need Studio.

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The integration tests open real Studio, so it must be installed and logged in.

```powershell
.\tests\run-integration.ps1
```
