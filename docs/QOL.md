# Quality-of-life candidates

Filter every idea through two gates before building it:
1. **Fidelity**: it adds no new request shape, header, or gateway behaviour the official client lacks (`docs/FIDELITY.md`). Anything local-only (display, notifications, storage) is free; anything that talks to Discord must reuse an existing official call, on a user gesture.
2. **No botting**: no automation of writes. Nothing sends, reacts, joins, edits or deletes on a timer, trigger, rule or in bulk (`UserIntent` stays one gesture per write). Reminders are OS notifications only.

## Safe, local-only (no new requests)
| Idea | Why it helps | Where |
|---|---|---|
| Upload tray: thumbnails, size per file, remove, reorder, "will be compressed to ~X" | see what you are about to send | core size/plan, shell tray |
| Spoiler toggle per file (`SPOILER_` prefix, what the official client does) and alt text | parity + accessibility | existing upload body |
| Strip location/EXIF from photos on upload (option, default on) | privacy | shell re-encode seam |
| Clean pasted links (utm_*, fbclid, share ids) | less tracking in chats | core text rule |
| Confirm before sending `@everyone`/`@here` in a big channel | stops accidents | core rule |
| Keyword highlights and quiet hours for local notifications | fewer interruptions | core, local notifier |
| Local nicknames for people/servers (display only) | readability | core kv |
| Pin/favourite channels locally, quick-switcher ranks them | speed | core kv |
| Drag attachments out to Finder, Save/Copy image, Open in Preview | native feel | shell |
| Export a channel from the local cache (no new fetching) | backups | core cache |
| Data saver on battery or metered network: still GIFs, smaller previews | battery | power engine |
| Collapse long code blocks and long messages | scan faster | shell |
| Window/state restore (last channels per account) | continuity | core kv (partly done) |

## Fine, but reuses an official call on a click
Mark-all-read, Mark Unread, bulk save-for-later, "copy message link", jump to date (official `around` fetch).

## Not doing
Scheduled or delayed sends, auto-reply, auto-react, message sniping/deleted-message logging, mass delete/leave/join, anything that spoofs presence or typing, token tools.

## Built (2026-10-30)
Friend nicknames (official feature), upload tray (preview, size, spoiler, alt text, shrink/too-big notes), location stripping, tracking-free pasted links, `@everyone`/`@here` confirmation, notification keywords, quiet hours, export of loaded messages. Rules and persistence: `unison-core/src/comfort.rs` (tested); FFI `unison-ffi/src/comfort.rs`; the shell draws and runs ImageIO. Also built: dragging an attachment out to Finder or another app (file promise, fetched on drop; `Media/AttachmentDrag.swift`) and collapsing code blocks over 16 lines to 10 plus a "… N more lines" note (core `comfort::collapse_code`; click to open; view state only). Not built yet: local channel favourites, data saver.
Cost: no timers, no polling, no new dependencies; keyword matching runs once per incoming message only when keywords exist.

## Sending (2026-10-31)
Progress row with speed and time left, cancel, shrink/upload/waiting stages; failed or cancelled sends return their text and files to the composer; plain-language errors everywhere (`Error::user_message`); length counter; trim setting (Automatic follows Nitro; the cut-off rest stays in the box, never sent as a second message); local checks for too many/too large/empty files and the 500 MiB total before anything is requested; the app stays out of App Nap while a send is going up.

## Header popup dots (2026-10-31)
Pins, Saved, Threads and Inbox icons show a small neutral dot when they have content. Rule in core (`Store::has_pins`, `has_active_threads`; FFI `header_dots`, tested); shell draws `ContentDotIcon`. No new requests: pins use the channel's last-pin stamp, so the dot can linger after the last pin is removed.
