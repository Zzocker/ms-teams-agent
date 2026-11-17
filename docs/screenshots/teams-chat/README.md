# Teams-style chat UI

All screenshots use the built-in synthetic scenes. No signed-in Teams instance,
account, messages, or network workload was used. The before image uses the prior
layout; the remaining images show this branch.

Visual reference: Microsoft's public [chat and channels guide](https://support.microsoft.com/en-us/teams/teams-channels/explore-the-new-chat-and-channels-experience-in-microsoft-teams).
This keeps the app's existing separate Chat/Teams navigation and supported actions.

Reproduce after building with `cargo build --locked`:

```sh
target/debug/ms-teams-agent --snap chat-light.png --scene main --theme light --size 1280x800 --scale 1
target/debug/ms-teams-agent --snap chat-dark.png --scene main --theme dark --size 1280x800 --scale 1
target/debug/ms-teams-agent --snap reply-light.png --scene reply --theme light --size 1000x680 --scale 1
target/debug/ms-teams-agent --snap files-dark.png --scene file --theme dark --size 1280x1100 --scale 1
```

The renderer needs Vulkan (software Vulkan is sufficient). Snapshot timestamps
are formatted in the local timezone. These screenshots show visual behavior;
they do not verify live Teams integration or native rendering on other platforms.
