# ms-teams-agent

> [!WARNING]
> Very early stage. Expect missing features, bugs and breaking changes.

A lightweight Microsoft Teams desktop client written in Rust with [Rune](https://github.com/anomalyco/rune).
Supports chats, teams and channels, live messages, desktop notifications, and headless tooling for agents.

## Get started

Download a build from [Releases](https://github.com/Zzocker/ms-teams-agent/releases) and follow the
[installation guide](docs/installation.md) for Linux, Windows or macOS (Apple Silicon).

## Feature status

- [x] Microsoft sign-in and saved sessions
- [x] Read and send messages in existing chats
- [x] Browse teams and channels
- [x] Live message updates and unread indicators
- [x] Desktop notifications that open the chat
- [x] Reply to, edit and delete messages
- [x] Message formatting and code blocks
- [x] Paste and send images; view shared images
- [x] View shared file cards and open file links
- [x] Search loaded chats and channels by name
- [ ] Start new chats and create group chats
- [ ] Search message history
- [x] Attach documents and other device files in chats through OneDrive (up to 100 MiB each)
- [x] Message reactions (six quick choices; existing reaction counts)
- [ ] Presence and typing indicators
- [ ] Voice and video calls
- [ ] Calendar and meetings
- [ ] Favorites and pinned chats

## Documentation

- [Installation and building from source](docs/installation.md)
- [Using Teams and configuration](docs/usage.md)
- [Updating the app](docs/updates.md)
- [Making a release](docs/releasing.md)
- [Development, headless tooling and profiling](docs/development.md)
