# Development and profiling

[Back to README](../README.md)

Run commands from the repository root.

## Headless tooling

Headless runs use built-in demo data and never touch the network:

```sh
ms-teams-agent --list-scenes                 # list available demo scenes
ms-teams-agent --snap out.png --scene main   # render a screen to PNG
ms-teams-agent --tree tree.json --scene main # element tree: roles, labels, text, rects
```

Reproduce the shortened-URL clipboard regression with synthetic messages and capture the actual
copy-and-paste result (requires a Vulkan adapter, including a software adapter):

```sh
COPY_LINK_EVIDENCE=/tmp/copy-link.png cargo test --locked ui::message::tests::copy_menu -- --nocapture
```

The test clicks **Copy text**, checks the clipboard, pastes into the composer, and captures that
app state. It also writes the pasted text to `/tmp/copy-link.png.txt`. The `copy-link` demo scene
shows the source message with its copy menu open.

## Profile the real app (Linux)

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
