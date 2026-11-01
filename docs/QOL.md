# Quality-of-life rules

Every idea passes two gates:
1. **Fidelity**: no new request shape, header or gateway behaviour the official client lacks (`docs/FIDELITY.md`). Local-only behaviour (display, notifications, storage) is free; anything that talks to Discord reuses an official call, on a user gesture.
2. **No botting**: nothing sends, reacts, joins, edits or deletes on a timer, trigger, rule or in bulk (`UserIntent` stays one gesture per write). Reminders are OS notifications only.

## Built
- Uploads: tray with thumbnails, size, spoiler and alt text, shrink to the limit (setting: Automatic follows Nitro), location stripped from photos, local size checks before anything is requested, progress with speed and cancel, failed sends return text and files to the composer.
- Writing: tracking parameters removed from pasted links, `@everyone` and `@here` confirmation, over-length messages trimmed (the rest stays in the box, never sent as a second message), length counter.
- Notifications: keywords, quiet hours, per-server and per-channel overrides.
- Reading: long code blocks collapse to 10 lines, friend nicknames, attachments drag out to Finder, loaded messages export, header icons show a dot when Pins, Saved, Threads or Inbox have content (`Store::has_pins`, `has_active_threads`).
- Rules live in `unison-core/src/comfort.rs` (tested); the shell draws and runs ImageIO. No timers, no polling, no new dependencies.

## Not built yet
Local channel favourites, data saver on battery or a metered network.

## Not doing
Scheduled or delayed sends, auto-reply, auto-react, deleted-message logging, mass delete, leave or join, anything that spoofs presence or typing, token tools.
