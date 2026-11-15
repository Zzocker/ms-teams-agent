# Installation

[Back to README](../README.md)

Download the build for your OS from the [Actions](https://github.com/Zzocker/ms-teams-agent/actions) artifacts or [Releases](https://github.com/Zzocker/ms-teams-agent/releases),
or build it yourself with [Rust](https://rustup.rs):

```sh
cargo install --git https://github.com/Zzocker/ms-teams-agent
```

Release downloads include the version, OS and CPU architecture in their filenames.
Ubuntu and Arch builds target x86_64; Windows targets x86_64; macOS targets Apple Silicon (aarch64).
`SHA256SUMS` on each release contains checksums for the archives.

## Linux

Install runtime dependencies first:

```sh
# Arch
sudo pacman -S fontconfig libxkbcommon libxkbcommon-x11 wayland vulkan-icd-loader vulkan-swrast
# Ubuntu 22.04+ / Debian with compatible system libraries
sudo apt install libfontconfig1 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libvulkan1 mesa-vulkan-drivers
```

Download the archive for your distribution, extract it, then run inside the extracted directory:

```sh
sh install.sh
```

The installer needs Python 3 and installs the executable, icon and launcher for your user,
without sudo. Open **Teams** from your application launcher, or run `~/.local/bin/ms-teams-agent`.
You can also run `./ms-teams-agent` directly from the extracted archive.

## Windows

Extract the ZIP into a permanent folder and run `ms-teams-agent.exe`.
Building from source needs the MSVC toolchain (rustup's default).

## macOS

Extract the archive and drag `Teams.app` into Applications. The Apple Silicon build is unsigned.
If macOS blocks it, clear quarantine on the downloaded app only if you trust the release:

```sh
xattr -dr com.apple.quarantine /Applications/Teams.app
```
