# Using Teams

[Back to README](../README.md)

On first start it shows a device code and opens the Microsoft sign-in page. Sign in there and the app logs in.

Optional environment variables: `TEAMS_TENANT` (default `organizations`), `TEAMS_CLIENT_ID`.

Use **Chat** in the left rail for chats, or **Teams** for teams and their channels.
Click a team to expand or collapse its channels. Teams load separately when first opened;
failed requests show an error and a Retry button. Favorites are not shown.

New chat messages arrive over a persistent Teams WebSocket connection, with automatic reconnects
and session renewal. The chat moves to the top of the list. If you are viewing it in the focused
window, its messages update quietly; otherwise it gets an unread dot and a system notification.
Notifications include a short message preview and follow your operating system's notification settings.
Click a notification to reopen Teams and go directly to its chat.

Closing the window keeps Teams running in the background. Reopen it from the launcher or the tray's
**Open Teams** action; use **Quit Teams** in the tray to stop it. On macOS use the Dock to reopen and
the application menu to quit (Rune's tray support is Linux/Windows only). Linux needs a system tray
host; GNOME may need an AppIndicator extension. Connection errors appear above the chat list.
