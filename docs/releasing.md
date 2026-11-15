# Making a release

[Back to README](../README.md)

Run commands from the repository root.

1. Update `version` in `Cargo.toml`, run `cargo check` to update `Cargo.lock`, and commit both with your changes. The first release uses the existing `0.1.0` version.
2. Open a release PR from `main` into `release` and wait for PR tests and release builds to pass.
3. Merge the PR. The push to `release` tests and packages every platform. After all builds succeed, CI creates the matching tag (for example `v0.1.0`) on that exact commit and creates one **draft preview release** with all four archives, checksums and generated notes. No manual tagging is needed.
4. Review the draft, download and smoke-test its packages, and edit the notes with known issues. Publish it from GitHub Releases when ready.

Only published releases appear in update checks. Bump the version for every subsequent release
(for example `0.1.1`, automatically tagged as `v0.1.1`). CI refuses to move an existing tag to a
different commit and never overwrites an existing release. A failed platform build prevents both
tagging and release creation. Pull requests run tests and builds without creating tags or releases.

If release creation fails after tagging, rerun the failed release job: CI accepts an existing tag
only when it points to the same tested commit. If the failed upload left an incomplete draft,
remove that draft before retrying. A published release should stay unchanged.

You can also run the build workflow manually from GitHub Actions with the `release` branch
selected. Manual runs on other branches build packages without creating a tag or release.
Pushing a tag manually does not trigger this workflow.

To package a local build without publishing:

```sh
cargo build --release --locked
python3 scripts/package.py ubuntu x86_64 target/release/ms-teams-agent --output /tmp/teams-packages
```
