# Making a release

[Back to README](../README.md)

Run commands from the repository root.

1. Update `version` in `Cargo.toml`, run `cargo check` to update `Cargo.lock`, and commit both with your changes.
2. Push a matching tag, for example `git tag v0.1.0` then `git push origin v0.1.0`.
3. Wait for the build workflow. It tests and packages every platform, then creates one **draft preview release** with all four archives, checksums and generated notes. A failed build prevents release creation.
4. Review the draft, download and smoke-test its packages, and edit the notes with known issues. Publish it from GitHub Releases when ready.

Only published releases appear in update checks. Use a new version and tag for each subsequent
release (for example `0.1.1` / `v0.1.1`). The workflow refuses tags that disagree with `Cargo.toml`
and never overwrites an existing release. To retry a failed release upload, remove its incomplete
draft before rerunning the release job. A published release should stay unchanged.

To package a local build without publishing:

```sh
cargo build --release --locked
python3 scripts/package.py ubuntu x86_64 target/release/ms-teams-agent --output /tmp/teams-packages
```
