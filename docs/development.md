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

## Experimental audio calling

Default builds include experimental outgoing audio calls. Install the GStreamer prerequisites
for your platform before building. See the [installation guide](installation.md).

```sh
cargo build --locked
```

Select **Start audio call** in a one-to-one chat to open the call screen. It shows the
contact, connection status, and elapsed time. Use **Mute** or press **Ctrl+Shift+M**
(**⌘⇧M** on macOS). Select **Chat** to go back to the conversation. The compact call bar
stays visible while you browse, with **Return to call**, mute, and hang-up controls.

Default builds embed the ringback recording for local playback while dialing.
[Microsoft Teams outgoing call tone](https://statics.teams.cdn.office.net/evergreen-assets/audio/tfl-tactile-sounds/desktop/default-consumer/v2/ringtones/outgoing/outgoing-ringtone-level30.mp3)

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

## Idle memory regression

The app releases image previews outside the current viewport when Rune reports the window idle
(normally after 10 seconds without a drawn frame, or when hidden). The visible previews, open
image, and pictures awaiting upload stay available. On Linux/glibc the same callback returns
freed heap pages to the OS; other platforms retain their usual allocator behavior.

Run the synthetic CPU-only image memory regression in isolation on Linux/glibc:

```sh
cargo test --locked images::tests::idle_cleanup_returns -- --ignored --test-threads=1 --nocapture
# Also capture the measured result in the demo UI (Vulkan, including software Vulkan, required):
IDLE_MEMORY_EVIDENCE=/tmp/idle.png cargo test --locked images::tests::idle_cleanup_returns -- --ignored --test-threads=1 --nocapture
```

The test generates 4096 small images, calls the app’s idle callback directly, checks that the
visible preview survives, and measures RSS before/after. The optional screenshot is rendered
after measurement, and its counters are also written to `/tmp/idle.png.json`. No Teams session
or network is used. This measures reclaimable image memory; live data and renderer memory have
their own baseline and are not expected to disappear.
