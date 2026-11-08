# Contributing to studio-run

## Setup

Install Rust with [rustup](https://rustup.rs) and the Visual Studio C++ Build Tools. `rust-toolchain.toml` selects the stable toolchain with `rustfmt` and `clippy`.

## Building

```powershell
cargo build --release
```

This creates `target\release\studio-run.exe`. It links the C runtime statically (see [.cargo/config.toml](.cargo/config.toml)), so it runs without the Visual C++ Redistributable installed.

## Testing

The unit tests don't need Studio.

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The integration tests open real Studio, so it must be installed and logged in. They run `target\release\studio-run.exe`, so build the release first.

```powershell
.\tests\run-integration.ps1
```

The release workflow runs the unit tests, formatting and lint checks, but not the integration tests. Run those yourself before a change goes in.

## Releasing

See [RELEASE.md](RELEASE.md).
