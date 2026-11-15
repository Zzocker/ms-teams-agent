# ms-teams-agent

> [!WARNING]
> Very early stage. Expect missing features, bugs and breaking changes.

A lightweight Microsoft Teams client written in Rust (UI on [Rune](https://github.com/anomalyco/rune)).

Easy for both people and agents to use: run it as a normal desktop app, or run it headless to render
screens to PNG and dump the element tree as JSON.

## Install

Download the build for your OS from the [Actions](../../actions) artifacts or [Releases](../../releases),
or build it yourself with [Rust](https://rustup.rs):

```sh
cargo install --git https://github.com/Zzocker/ms-teams-agent
```

### Linux

```sh
# Arch
sudo pacman -S fontconfig libxkbcommon vulkan-icd-loader
# Ubuntu / Debian
sudo apt install libfontconfig1 libxkbcommon0 libvulkan1

chmod +x ms-teams-agent && ./ms-teams-agent
```

For the app icon and a launcher entry, put `ms-teams-agent` on your `PATH` and, from a checkout:

```sh
install -Dm644 assets/icons/icon.svg ~/.local/share/icons/hicolor/scalable/apps/ms-teams-agent.svg
install -Dm644 assets/linux/ms-teams-agent.desktop ~/.local/share/applications/ms-teams-agent.desktop
```

### Windows

Unzip and run `ms-teams-agent.exe`. Building from source needs the MSVC toolchain (rustup's default).

### macOS

The build is for Apple Silicon and isn't signed, so clear the quarantine flag first:

```sh
xattr -d com.apple.quarantine ms-teams-agent
chmod +x ms-teams-agent && ./ms-teams-agent
```

## Use

On first start it shows a device code and opens the Microsoft sign-in page. Sign in there and the app logs in.

Optional environment variables: `TEAMS_TENANT` (default `organizations`), `TEAMS_CLIENT_ID`.

Use **Chat** in the left rail for chats, or **Teams** for teams and their channels.
Click a team to expand or collapse its channels. Teams load separately when first opened;
failed requests show an error and a Retry button. Favorites are not shown.

### For agents

Headless runs use built-in demo data and never touch the network:

```sh
ms-teams-agent --list-scenes                 # login error code main reply edit menu markdown editor image viewer teams loading search pick
ms-teams-agent --snap out.png --scene main   # render a screen to PNG
ms-teams-agent --tree tree.json --scene main # element tree: roles, labels, text, rects
```

### Profile the real app (Linux)

Sign in with the normal app first, then run from a graphical desktop session:

```sh
python3 scripts/profile.py
# Offscreen rendering with the same real Teams data and read-only workload:
python3 scripts/profile.py --headless
# Or choose a new results directory:
python3 scripts/profile.py --output /tmp/teams-profile-run-1
```

Final headless results — updated November 15, 2026, 12:00 PM IST.

| Measurement | Result |
|---|---:|
| Workload | 12 chat opens / 36 upward scroll actions |
| RSS before opening chats | 210.7 MiB |
| RSS after second pass, before cleanup | 277.6 MiB |
| Peak RSS | 277.8 MiB |
| Live Rust allocations before cleanup | 53.6 MiB |
| Preview cache before cleanup | 16 images / 19.9 MiB |
| Final RSS after diagnostic cache clearing and `malloc_trim` | 241.7 MiB |
| Average CPU (% of one core, including profiling overhead) | 96.0% |
