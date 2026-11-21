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

## Set up Ask Me with Docker

Ask Me can use a model served on the same computer as Teams Agent. This setup runs
Qwen3.5 2B (`Q4_K_M`) with llama.cpp on the CPU. The model download is about 1.3 GB,
and Docker caps the container at 4 GiB of RAM. Keep enough memory available for the
app and your operating system. [Model file](https://huggingface.co/unsloth/Qwen3.5-2B-GGUF/blob/main/Qwen3.5-2B-Q4_K_M.gguf)

### Start the model server

Start Docker Engine or Docker Desktop. Run this command in a POSIX shell. In PowerShell,
join the lines into one command and omit the trailing backslashes.

```sh
docker run -d --name teams-ask-me-qwen-cpu \
  --memory 4g --memory-swap 4g \
  -p 127.0.0.1:11435:8080 \
  -v teams-ask-me-gguf:/models -e LLAMA_CACHE=/models \
  ghcr.io/ggml-org/llama.cpp:server \
  -hf unsloth/Qwen3.5-2B-GGUF:Q4_K_M --no-mmproj \
  --alias qwen3.5:2b-q4_K_M --ctx-size 2048 --parallel 1 \
  --n-gpu-layers 0 --reasoning off --host 0.0.0.0 --port 8080
```

The named volume keeps the downloaded model between container runs. Docker publishes
the API only on your computer's loopback address. [llama.cpp Docker documentation](https://github.com/ggml-org/llama.cpp/blob/master/docs/docker.md)

The first startup can take several minutes while the model downloads and loads. Check
the logs and health endpoint. In PowerShell, use `curl.exe`.

```sh
docker logs --tail 50 teams-ask-me-qwen-cpu
curl -fsS http://127.0.0.1:11435/health
```

Wait for the health check to return `{"status":"ok"}`. If port `11435` is in use,
choose a free host port in the `-p` option and use that same port in the API base URL.
[llama.cpp health endpoint](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md#get-health-returns-health-check-result)

### Connect Teams Agent

1. Open **Settings → Ask Me → Model connection**.
2. Set **API base URL** to `http://127.0.0.1:11435/v1`.
3. Set **Model ID** to `qwen3.5:2b-q4_K_M` and click **Test and save**.
4. Open **Ask Me** in the left rail. Send `Hi` to check for a direct reply, then ask
   about a detail in a recent Teams conversation. Click a citation to open the source message.

**Test and save** sends a short test prompt. When a question needs Teams information,
Ask Me sends matching message excerpts to the configured model to generate a cited answer.

### Stop or restart the model

Stop the model when you want to free its memory. Start the existing container again
when you need Ask Me. Both commands keep the cached model.

```sh
docker stop teams-ask-me-qwen-cpu
docker start teams-ask-me-qwen-cpu
```
