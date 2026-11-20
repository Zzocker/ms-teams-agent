# Using Teams

[Back to README](../README.md)

On first start it shows a device code and opens the Microsoft sign-in page. Sign in there and the app logs in.

Optional environment variables: `TEAMS_TENANT` (default `organizations`), `TEAMS_CLIENT_ID`.

Use **Chat** or **Teams** in the left rail. In Chat, choose **Chats** or **Unread**.
In Teams, choose **Channels** or **Unread**. Switching views or filters keeps the open
conversation and draft. Click a team to expand its channels. If loading fails, use
**Reload channels**.

Open a channel to see its **Posts**. Native Teams replies and quoted replies appear
under the original post when it’s loaded. If the original post is older, use the quote
or **Go to original post** to find it. Choose **Reply with quote** to quote a message in
your response.

Use the star in the conversation header to add or remove a favorite. Favorites are saved
on this device for your signed-in account. The sidebar button next to **Teams Agent**
hides or shows the list. Drag its right edge, or focus the divider and press Left or Right,
to resize it.

Press **Ctrl+E** to focus search. Search matches chat, team, and channel names across
sidebar filters. Press Enter to open the first result or Escape to dismiss the list.

New chat messages arrive over a persistent Teams WebSocket connection, with automatic reconnects
and session renewal. The chat moves to the top of the list. If you are viewing it in the focused
window, its messages update quietly; otherwise it gets an unread dot and a system notification.
Notifications include a short message preview and follow your operating system's notification settings.
Click a notification to reopen Teams and go directly to its chat.

Closing the window keeps Teams running in the background. Reopen it from the launcher or the tray's
**Open Teams** action; use **Quit Teams** in the tray to stop it. On macOS use the Dock to reopen and
the application menu to quit (Rune's tray support is Linux/Windows only). Linux needs a system tray
host; GNOME may need an AppIndicator extension. Connection errors appear above the chat list.

To mention a person, type `@` and part of their name or email in either message composer.
Choose a conversation member with Up/Down and Enter or Tab, or click their name. The chip
keeps that person's identity; Backspace removes it as a whole. Escape dismisses suggestions
and leaves the text in place. Ordinary `@text` still sends without mentioning anyone.
Suggestions show email or Teams ID to distinguish people with the same name. Member lookup
must succeed for the conversation; team/channel-wide tags are not included.

To attach a file, click **Attach file** beside the message box (or in the expanded editor)
and choose a file from your device. Repeat to attach more files; **Remove** takes a file
out of the draft. Click **Send** to upload and send them, with or without message text.
Files must be 100 MiB or smaller. Uploads run in the background; a failed send restores
the attachments for retry. Switching conversations discards the current draft's attachments.
Attachments can be added to new messages and replies, rather than edits of sent messages.

Chat attachments are uploaded to your OneDrive's **Microsoft Teams Chat Files** folder
and shared with the other chat members using read access, without sharing-notification emails.
File cards open in the browser, including the official Teams client's file cards.
The file picker is available in chats. Channel posts can include pasted images.
Attachments sent by the earlier media-store implementation must be resent to appear in official Teams.
Those older cards can still be downloaded through this app's Save dialog.
On Linux, the native file picker requires an XDG desktop portal with a FileChooser backend.
