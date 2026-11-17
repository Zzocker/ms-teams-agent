# Message state verification

All images render this Rust app with built-in, synthetic data. No actual Teams instance,
account, session, real messages, or live service was used. These are UI state fixtures;
the delete/undo action sequence is separately exercised against a local HTTP mock server.

| Screenshot | State |
| --- | --- |
| `edited-seen-typing-dark.png` | Edited labels, confirmed Seen, incoming typing; dark theme |
| `edited-seen-typing-light.png` | Same states; light theme |
| `deleted-undo-light.png` | Deleted message placeholder and Undo action |
| `restored-dark.png` | Restored message appearance (synthetic fixture) |
| `restore-error-light.png` | Restore failure keeps the placeholder and retry action |
| `sending-dark.png` | Pending send at 1000 × 680 |

Reproduce from the repository root (Vulkan adapter, including software Vulkan, required):

```sh
cargo build --locked --offline
./target/debug/ms-teams-agent --snap /tmp/states-dark.png --scene message-states --theme dark --size 1200x800 --scale 1
./target/debug/ms-teams-agent --snap /tmp/states-light.png --scene message-states --theme light --size 1200x800 --scale 1
./target/debug/ms-teams-agent --snap /tmp/deleted.png --scene message-deleted --theme light --size 1200x800 --scale 1
./target/debug/ms-teams-agent --snap /tmp/restored.png --scene message-restored --theme dark --size 1200x800 --scale 1
./target/debug/ms-teams-agent --snap /tmp/restore-error.png --scene message-restore-error --theme light --size 1200x800 --scale 1
./target/debug/ms-teams-agent --snap /tmp/sending.png --scene message-sending --theme dark --size 1000x680 --scale 1
cargo test --locked --offline -- --test-threads=1
cargo check --locked --offline --no-default-features
```

The suite checks metadata parsing, typing start/stop/self filtering and expiry, chat-scoped
read receipts, pending/sent/seen labels, deleted-message action visibility, and the Undo
button against a loopback mock. The mock checks soft-delete, restore's DELETE JSON body,
403 preservation and retry, restoration from an older history page, receipt URL/parsing,
and typing POST format. Layout checks cover both themes at 640 × 400, 1000 × 680, and
1280 × 800. Local loopback sockets must be permitted to run the mock tests.

Read positions refresh every 15 seconds only while the selected conversation is visible
and focused. Seen means at least one other participant's confirmed horizon reaches the
latest outgoing message; it does not claim everyone in a group has read it. Disabled or
unavailable receipts fall back to Sent. Incoming typing expires after eight seconds or
clears on a stop event/message; outgoing typing is throttled to once per four seconds.

The protocol uses Teams' undocumented chat service. Tenant policy, live delivery,
receipt availability, and native Windows/macOS rendering were not verified.
