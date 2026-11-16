# Open issues: effort ranking and QA acceptance

Repository: [Zzocker/ms-teams-agent](https://github.com/Zzocker/ms-teams-agent/issues?q=is%3Aissue%20is%3Aopen)

Snapshot: 16 November 2026. All **10 open issues (#3–#12)** were fetched from GitHub, including their bodies and comments (none had comments). Pull requests are excluded. Code reviewed at `4056932e9dbdcaa53b4117de58954c2bba67cf76` (version 1.2.0).

This document tracks implementation and QA in ranked order. Per-issue status records fixes and user confirmation; unchecked QA items have no individually recorded result.

Workflow: resolve one issue at a time in ranked order, provide testing steps, and wait for user confirmation. When the user confirms an issue is fixed, mark it resolved here and commit its changes to `main`.

## Ranked from least to most effort

Order considers engineering effort first, then expected code change and uncertainty; it is not a severity ranking. Estimates cover investigation, implementation, focused automated checks, and developer verification by one engineer familiar with this code. One day means approximately eight working hours; external API or desktop-specific blockers can extend these ranges. Changed-line and file counts are rough planning estimates, including tests, not targets.

| Rank | Issue | Effort | Expected change | Main uncertainty |
| --- | --- | --- | --- | --- |
| 1 | [#10 Edit succeeds but reports JSON EOF](https://github.com/Zzocker/ms-teams-agent/issues/10) | 2–4 hours | Small: 20–60 lines, 1–2 files | Service response variants |
| 2 | [#12 Invisible text selection in light mode](https://github.com/Zzocker/ms-teams-agent/issues/12) | 2–6 hours | Small: 5–40 lines, 1–3 files | Whether Rune exposes selection styling |
| 3 | [#11 Search shortcut and dismissal](https://github.com/Zzocker/ms-teams-agent/issues/11) | 3–6 hours | Small: 30–90 lines, 2–3 files | Focus handling and desktop shortcut conflicts |
| 4 | [#6 Sent URLs are not hyperlinks](https://github.com/Zzocker/ms-teams-agent/issues/6) | 4–8 hours | Small–medium: 50–150 lines, 2–3 files | URL boundaries and Teams round trip |
| 5 | [#3 Notification sound does not work](https://github.com/Zzocker/ms-teams-agent/issues/3) | 0.5–1.5 days | Small–medium: 10–100 lines, 1–3 files | Desktop sound policy and platform behavior |
| 6 | [#9 Channel messages create chat rows](https://github.com/Zzocker/ms-teams-agent/issues/9) | 1–2 days | Medium: 70–200 lines, 3–4 files | Identifying channels before team metadata loads |
| 7 | [#7 Unread handling depends on message load](https://github.com/Zzocker/ms-teams-agent/issues/7) | 1–2 days | Medium: 80–220 lines, 3–4 files | Intended unread behavior and safe read horizons |
| 8 | [#8 Notification click and duplicate alert](https://github.com/Zzocker/ms-teams-agent/issues/8) | 1–3 days | Medium: 40–180 lines, 2–4 files | Native activation and source of second alert |
| 9 | [#4 Slow chat opening; preload and cache](https://github.com/Zzocker/ms-teams-agent/issues/4) | 2–4 days | Medium–large: 150–350 lines, 3–5 files | Cache consistency, network scheduling and races |
| 10 | [#5 Mention people while composing](https://github.com/Zzocker/ms-teams-agent/issues/5) | 3–6 days | Large: 250–600 lines, 5–8 files | Participant lookup and Teams mention wire format |

## 1. #10 — Editing reports an error after succeeding

**Status:** Resolved — user confirmed the fix on 16 November 2026. Successful edit responses no longer require a JSON body; reads and failure responses retain their existing handling. Automated validation: 63 tests passed, 2 ignored. Individual manual checklist results were not supplied.

**Reported:** `Not edited: chat service 200 OK: json: EOF while parsing a value at line column 0`; the server actually updates the message.

**Code evidence and required fix:** In `src/teams.rs`, `edit()` passes the PUT response to `answer()`, which always reads JSON, including successful empty responses. Accept successful empty edit responses without requiring JSON. Preserve authentication and HTTP failure reporting; keep JSON validation for reads that require a response body. Check the shared helper's callers (`get`, `send`, `edit`, `older`, and other service requests) before changing its contract. Verify the edit completion path in `src/actions.rs` and `src/main.rs`.

**QA acceptance:**

- [ ] Edit an existing message. The text updates once, editing closes, and no false error appears; reopening the chat shows the same edit.
- [ ] Cover empty successful responses (200 and 204) and successful JSON responses in a focused response-handling check.
- [ ] An expired session, HTTP rejection, or network failure still reports failure and retains recoverable edit content.
- [ ] Edit a message containing a quote or image; those remain intact. Sending and loading messages still work.

## 2. #12 — Text selection is invisible in light mode

**Status:** Resolved — user confirmed the fix on 16 November 2026. Message text uses Rune's Chromium selection style, with explicit active/inactive highlight and foreground colors. Automated validation: 63 tests passed, 2 ignored. Individual manual checklist results were not supplied.

**Reported:** Selected message text becomes white and cannot be distinguished in light mode.

**Code evidence and required fix:** `message_text()` in `src/ui/message.rs` enables selection without explicit selection paint. Message colors use `tok::theme(dark())`; selection rendering may come from Rune. Inspect the framework's styling API before deciding whether this needs a local style change or a dependency fix. Ensure both selection background and foreground remain readable.

**QA acceptance:**

- [ ] In light mode, select part and all of a message: selected and unselected regions are clearly distinguishable, and selected text stays readable.
- [ ] Repeat for sent and received messages, links, multiline text, and code blocks.
- [ ] Copy returns the selected text unchanged; switching between light, dark, and system theme preserves visible selection.
- [ ] Dark-mode selection and keyboard selection remain usable.

## 3. #11 — Search shortcut and exit behavior

**Status:** Resolved — user confirmed the fix after retesting on 16 November 2026. Ctrl+Alt+E focuses search; Escape and outside click clear the query and dismiss results. The dropdown stays above the main content. Automated validation: 64 tests passed, 2 ignored, including character-by-character filtering and result visibility. Individual manual checklist results were not supplied.

**Reported:** Add `Ctrl+Alt+E`; Escape or clicking outside the dropdown must leave search mode. The issue's `eac` is interpreted as Escape.

**Code evidence and required fix:** Search input and query-driven results live in `src/ui/main_view.rs`; selecting a result clears the query, but there is no search-specific shortcut or dismissal handler there. Add window-level shortcut handling and focus the existing search field. Dismiss results and leave search mode on Escape/outside click, using existing overlay/focus patterns. Do not expand this into message-content search: current search filters chat/channel names.

**QA acceptance:**

- [ ] `Ctrl+Alt+E` focuses search from the chat list and composer; typing immediately updates results without inserting `E` into the draft.
- [ ] Escape closes results and exits search mode. Outside click does the same; subsequent normal interaction works.
- [ ] Clicking a result opens the correct conversation and dismisses search; clicking within the input/dropdown does not dismiss it prematurely.
- [ ] Test empty results, repeated shortcut use, and Teams/channel view. Other overlays keep their expected Escape behavior.

## 4. #6 — Sent URLs do not become hyperlinks

**Reported:** A URL in a sent message is displayed as plain text.

**Code evidence and required fix:** `text_to_html()` in `src/teams/html.rs` escapes plain text without adding anchors; incoming link ranges are obtained from HTML anchors by `html_to_rich()`. `src/ui/message.rs` already renders and opens those link ranges. Generate safe HTTP(S) anchors for outgoing URLs and ensure any local display path gets consistent link data. Apply to normal send, reply, and edit through the shared HTML conversion; preserve code blocks and escaping.

**QA acceptance:**

- [ ] Send HTTP and HTTPS URLs: both sender and recipient see clickable links immediately and after reopening the chat, including in an official Teams client.
- [ ] Multiple links, query strings containing `&`, Unicode surrounding text, line breaks, and trailing punctuation preserve correct text and destination.
- [ ] Editing or replying with a URL produces the same behavior; copied text remains correct.
- [ ] Code samples are not unintentionally linked. HTML-like input is escaped, and unsafe schemes such as `javascript:` do not become clickable.

## 5. #3 — Notification sound does not work

**Reported:** No further reproduction details were supplied.

**Code evidence and required fix:** `start()` in `src/notifications.rs` creates `notify-rust` notifications with title, body, icon, and action, but no explicit sound configuration. This alone does not prove the cause: the notification daemon and operating system may supply or suppress sound. Reproduce with desktop sound enabled, determine whether a supported sound hint suffices, and verify notification suppression/deduplication still applies. Any platform-specific solution must follow the supported desktop behavior.

**QA acceptance:**

- [ ] With system notification sound enabled, a new message in another chat produces one alert and one audible sound.
- [ ] Own messages, replayed events, edits, and messages in the currently visible focused chat do not produce extra sounds.
- [ ] Test background/minimized/tray states and several consecutive messages without duplicated audio.
- [ ] System mute and Do Not Disturb remain respected. Record tested OS/desktop, notification daemon, and sound settings; verify each supported platform before claiming platform-wide completion.

## 6. #9 — Channel messages create entries in Chat

**Reported:** New Teams/channel messages create a new chat.

**Code evidence and required fix:** `message_update()` in `src/teams/events.rs` wraps conversation events as `Chat`; `receive()` in `src/notifications.rs` inserts unknown conversations into `Screen::Main.chats`. Teams/channels are held separately in `s.teams`. Route events by conversation type and update the existing channel state instead of inserting channel IDs into the direct/group chat list. Handle metadata not yet loaded and reconnect snapshots as well as live events; do not classify every unknown conversation as a channel.

**QA acceptance:**

- [ ] Receive a channel message while viewing Chat: no new channel row appears in the chat list, and the message is available in its correct channel.
- [ ] Repeat before opening Teams, after opening Teams, with a collapsed team, and after reconnecting.
- [ ] Channel preview/unread state updates in the correct location; an open channel receives the update once.
- [ ] A genuinely new direct/group conversation still appears in Chat. Replayed events and edits do not create duplicates.
- [ ] Channel notifications, if delivered, open the correct channel without switching it into the Chat list; coordinate this with #8.

## 7. #7 — Unread handling must be independent of history loading

**Reported:** “unread message are marked, after messages are loaded for that chat. make it independent.”

**Interpretation:** Decouple unread/read-state handling from fetching history. The wording is ambiguous about marking unread versus clearing unread. Existing push handling already sets unread for inactive chats; reproduce that separately. QA should verify both immediate incoming unread indicators and correct acknowledgement when viewing a conversation.

**Code evidence and required fix:** `open_chat()` in `src/actions.rs` fetches messages and only then calls `teams::mark_read()`. `mark_selected_read()` also requires a loaded page, and `mark_read()` derives the horizon from message IDs in that page. Retain enough trusted latest-message metadata to update read state independently where available. Preserve focus/visibility rules, explicit failure handling, and ordering so a newer message cannot be cleared by an older acknowledgement. Relevant files: `src/actions.rs`, `src/notifications.rs`, `src/teams.rs`, `src/main.rs`, and potentially chat parsing/state.

**QA acceptance:**

- [ ] Throttle or fail history loading. A new message in an inactive chat shows unread immediately without opening that chat.
- [ ] Opening a focused, visible unread conversation initiates read handling independently when valid latest-message metadata is available; it does not wait behind the history request.
- [ ] Hidden/minimized/unfocused conversations are not marked read merely because their data is loaded or prefetched.
- [ ] A message arriving during an older read request remains unread until acknowledged through its own horizon.
- [ ] Read API failure is visible and retryable; missing IDs never cause fabricated or overly broad read acknowledgements. Confirm persisted status after reopening/reconnecting.

## 8. #8 — Notification click does not open app; duplicate alert

**Reported:** Clicking a notification does not open the application. Two alerts appear: the actual event and “Teams is ready.” Both symptoms must be resolved.

**Code evidence and required fix:** `src/notifications.rs` already registers a default action, waits for it, checks the session/chat, calls `open_chat()`, and calls `window.open()`. Reproduce native action delivery and window activation before changing this path. There is no obvious production “Teams is ready” alert in the inspected notification path; an ignored desktop notification test has different readiness text. Trace the second alert's actual source instead of assuming it is the same bug. Activation should restore/focus the existing app and navigate to the right conversation; channel routing must align with #9.

**QA acceptance:**

- [ ] Click a real message notification with the app unfocused, minimized, and hidden in the tray: the existing window becomes visible/focused and opens the correct conversation.
- [ ] Repeat with another chat selected, search open, and Teams view active. Channel notifications navigate to the channel after #9.
- [ ] One incoming event produces one message alert and no extra “Teams is ready” alert. Reconnects and clicks do not add readiness alerts.
- [ ] A stale notification after sign-out/account change cannot open content from the previous session; a removed conversation is handled safely.
- [ ] Verify actual desktop action behavior on each claimed supported OS/desktop. A headless state test alone does not establish completion.

## 9. #4 — Slow opening; preload first 10 chats and cache

**Reported:** New chats take too long to load. The body proposes loading initial messages for the first 10 chats in the background and caching them.

**Code evidence and required fix:** `open_chat()` clears `s.messages` and always requests history; `State` in `src/main.rs` stores only the selected conversation's page. Add a bounded cache of initial message pages, preload the first ten chats without blocking the foreground request, display cached data promptly, and refresh it safely. Update/invalidate cached data for push events, edits, deletes, sends, and reconnects. Scope cached data to the signed-in session and clear on sign-out. In-memory caching is the initial assumption; persistence across restarts is not specified by the issue.

**QA acceptance:**

- [ ] After the list loads, initial history for up to the first ten chats loads in the background; fewer than ten chats causes no errors or repeated requests.
- [ ] Opening a prefetched chat displays cached messages promptly, without waiting for another network round trip. Capture before/after timings on the same account/network.
- [ ] Opening a noncached chat remains responsive and takes priority over background work; background failures do not prevent normal loading/retry.
- [ ] Rapidly switch A → B → A under slow network: messages, drafts, errors, and pagination always belong to the selected conversation.
- [ ] New messages, edits, deletes, sends, older-history loading, and reconnects do not leave stale or duplicated content in the cache.
- [ ] Prefetching does not mark chats read or emit message notifications; memory and background concurrency stay bounded.
- [ ] Sign out and sign into another account: no previous account's cached content is displayed.

## 10. #5 — Mention people while composing

**Reported:** Add support for tagging people while writing a message; no detailed interaction or recipient scope is specified.

**Code evidence and required fix:** Composer state currently holds plain text (`s.draft`), and `send()` in `src/teams.rs` sends generated HTML without mention metadata. `html_to_rich()` explicitly lacks mention support. Implement participant lookup, an accessible `@` suggestion/selection flow, stable mention identity in draft state, Teams-compatible mention serialization, and readable received mentions. Cover both normal and expanded composers (`src/ui/main_view.rs`, `src/ui/composer.rs`), state/actions, HTML conversion, parsing, and send/edit paths. Validate the protocol against an actual Teams recipient; visible `@Name` alone is insufficient.

**QA acceptance:**

- [ ] Type `@` and part of a participant name: candidates appear; select using keyboard or mouse, or dismiss with Escape without losing the draft.
- [ ] Duplicate display names resolve to the selected person's actual identity. Multiple mentions, Unicode names, and text around mentions survive cursor movement and deletion.
- [ ] Send to a direct/group chat: the intended recipient sees a real Teams mention, with recipient-side mention behavior under their notification settings; others are not accidentally mentioned.
- [ ] Reopen the message and inspect it in an official Teams client: identity and visible text remain correct.
- [ ] Reply, edit, send failure/retry, expanded composer, chat switching, and sign-out do not corrupt mentions or leak candidates between conversations/accounts.
- [ ] Ordinary `@` text remains sendable; unavailable lookup fails gracefully. Channel participant mentions must be verified if included in the claimed scope; team/channel-wide tags are not specified by this issue.

## Shared QA setup and completion evidence

Use two accounts (sender and recipient), a direct chat, a group chat, and at least one Teams channel. Use distinct messages for each test, enabled desktop notifications, both themes, and throttled/offline network scenarios where listed. For notification tests, record OS/desktop and whether the app was focused, minimized, or hidden.

For each issue record: tested build/commit, environment, steps and actual result, checklist results, and relevant screenshot/log/timing evidence with credentials and private message content removed. A proposed fix is ready to close only after its acceptance checks pass; blocked platform/API checks remain explicitly unverified.

Coordinate #3 and #8 because they share notification delivery; #9 and #8 because channel activation needs correct routing; and #7 and #4 because prefetch must never mark unseen messages read. These dependencies may make grouped implementation cheaper than the standalone estimates above.
