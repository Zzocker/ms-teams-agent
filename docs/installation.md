# Installation

[Back to README](../README.md)

Download the build for your OS from the [Actions](https://github.com/Zzocker/ms-teams-agent/actions) artifacts or [Releases](https://github.com/Zzocker/ms-teams-agent/releases).
Default builds link against GStreamer for audio calling. Install the dependencies for your OS
before launching or updating the app.

Release downloads include the version, OS and CPU architecture in their filenames.
Ubuntu and Arch builds target x86_64; Windows targets x86_64; macOS targets Apple Silicon (aarch64).
`SHA256SUMS` on each release contains checksums for the archives.

## Linux

Install runtime dependencies first:

```sh
# Arch
sudo pacman -S fontconfig libxkbcommon libxkbcommon-x11 wayland vulkan-icd-loader vulkan-swrast \
  gst-plugins-base gst-plugins-good gst-plugins-bad libnice
# Ubuntu 22.04+ / Debian with compatible system libraries
sudo apt install libfontconfig1 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libvulkan1 mesa-vulkan-drivers \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-nice gstreamer1.0-pulseaudio
```

Download the archive for your distribution, extract it, then run inside the extracted directory:

```sh
sh install.sh
```

The installer needs Python 3 and installs the executable, icon and launcher for your user,
without sudo. Open **Teams** from your application launcher, or run `~/.local/bin/ms-teams-agent`.
You can also run `./ms-teams-agent` directly from the extracted archive.

## Windows

Install the [GStreamer MSVC x86_64 runtime](https://gstreamer.freedesktop.org/download/#windows)
and add its `bin` folder to your user `PATH`. Extract the ZIP into a permanent folder and
run `ms-teams-agent.exe`.

## macOS

Install GStreamer and its ICE plugin with [Homebrew](https://brew.sh/).

```sh
brew install gstreamer libnice-gstreamer
```

Extract the archive and drag `Teams.app` into Applications. The Apple Silicon build is unsigned.
If macOS blocks it, clear quarantine on the downloaded app only if you trust the release:

```sh
xattr -dr com.apple.quarantine /Applications/Teams.app
```