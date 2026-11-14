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
ms-teams-agent --list-scenes                 # login error code main reply image viewer teams loading search pick
ms-teams-agent --snap out.png --scene main   # render a screen to PNG
ms-teams-agent --tree tree.json --scene main # element tree: roles, labels, text, rects
```
