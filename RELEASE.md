# Releasing studio-run

Releases are built and published by GitHub Actions when a version tag is pushed. The [release workflow](.github/workflows/release.yml) runs on `windows-latest`. You only need to prepare the version and push the tag.

## What a release contains

Each release has one asset, `studio-run-<version>-windows-x86_64.zip`, with `studio-run.exe` at the top level of the zip. Foreman, Rokit and Aftman pick a download by its name and expect this layout, so keep both if you change the workflow.

The exe links the C runtime statically (see [.cargo/config.toml](.cargo/config.toml)), so it runs without the Visual C++ Redistributable.

## Steps

1. **Pick the version.** Follow [semantic versioning](https://semver.org). A change to the options, exit codes or output format in the README is a breaking change.

2. **Bump the version** in `Cargo.toml`, then update `Cargo.lock`:

   ```powershell
   cargo build
   ```

3. **Run every check.** The workflow runs the unit tests but not the integration tests, which need a logged-in Studio. Run them locally first:

   ```powershell
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   cargo build --release
   .\tests\run-integration.ps1
   ```

4. **Update the version in the README** install snippets if they should point at the new release.

5. **Commit and push** to `main`:

   ```powershell
   git commit -am "Release v0.2.0"
   git push origin main
   ```

6. **Tag and push the tag.** The tag must be `v` followed by the exact `Cargo.toml` version, or the workflow fails:

   ```powershell
   git tag v0.2.0
   git push origin v0.2.0
   ```

7. **Check the run** on the repository's Actions tab. When it finishes, the release appears under Releases with notes generated from the commits since the previous tag. Edit the notes on GitHub if they need more detail.

## Using a new release

Projects pin an exact version, so they don't pick up a release until you change their config. In SparringArena's `foreman.toml`:

```toml
[tools]
studio-run = { github = "Gravemind1142/roblox-studio-run", version = "=0.2.0" }
```

Then run `foreman install`. If the repository is private, Foreman needs a GitHub token first (`foreman github-auth <token>`).

## When something goes wrong

- **Tag doesn't match `Cargo.toml`:** delete the tag, fix the version or the tag, and push again:

  ```powershell
  git tag -d v0.2.0
  git push origin :refs/tags/v0.2.0
  ```

- **Checks or build fail in the workflow:** no release is created. Fix the problem on `main`, delete and recreate the tag on the new commit, and push it again.

- **A bad release was published:** delete the release on GitHub and its tag, then publish a fixed version under a new number. Don't reuse a version number that has been published, since tool managers may have cached it.
