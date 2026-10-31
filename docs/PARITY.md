# Parity audit (refreshed 2026-10-30, `main` at 46e7239)

Supersedes the 2026-10-29 audit (its backlog items 1-15 are all built; see HANDOFF for what is unverified live). Evidence is from greps over `crates/` and `apps/macos`; the feature inventory is from knowledge of the official client, so spot-check endpoint names with the watcher or a web capture before implementing. Rules for adding anything below: `docs/ARCHITECTURE.md` §6 (core first, then FFI, then shell).

Done since the first refresh (2026-10-30, unverified live): presence activities and custom status shown in member list, DM list and profiles; custom status editor; message requests; scheduled events panel; stage sheet; moderation (kick/ban/timeout/nickname with role hierarchy) and a Server Settings window.

Slash commands now cover autocomplete options (`slash_autocomplete`, gateway `APPLICATION_COMMAND_AUTOCOMPLETE_RESPONSE`, `capture`) and attachment options (uploaded with the command). Uploads over the limit are compressed per `upload_policy` (setting: Automatic/On/Off; Automatic follows Nitro); the shell only runs ImageIO/AVFoundation attempts. Pinned popout can unpin (with confirmation). The new-messages bar hides on reaching the latest message.

Server folders are editable: drag a server onto another to make a folder, into or out of a folder, reorder servers and folders; right-click a folder to rename/recolor; open state is remembered (`folders.rs`, `drop_on_rail`, `edit_folders`). The PATCH body (`encode_folders`) is `capture`: unverified whether the server replaces or appends the repeated folders field; check on a throwaway account first.

Added: audit-log reasons (`X-Audit-Log-Reason`, `capture`), member role assignment (core hierarchy rules), private notes, saved-for-later and reminders (device-local; Discord's synced list is not captured; reminders are OS-scheduled notifications), server admin (role editor with grant limits, channels, emoji and stickers, icon/banner; request bodies follow the public API).

Servers can be created and joined from the "+" at the bottom of the rail (`server_join.rs`, `create_server`, `join_server`, `preview_invite`). Request bodies are `capture`; joining may be answered with a captcha, which currently surfaces as an error (route to the web fallback if it happens).

Search (2026-10-30): toolbar search has core-defined scopes (this server / this channel or chat / everywhere; `search_scopes`, `Scope`), Return asks Discord only where that is possible; ⌘K also lists matching messages from every account's local history (`SwitchKind::Message`). Embeds flagged by the content scan are covered like attachments (`capture`: embed flag bit 4).

UI cleanup (2026-10-30): Friends panel, Add Friend, status editor, events, message requests moved onto design-system components (`StatusPill` added); window now enforces its minimum size (`contentMinSize`). Not yet checked by eye at minimum width.

Health: `cargo test --workspace` green (183 core tests); no `target_os` outside the §3 seams (audio, QoS, `HostOs`); perf budgets pass (`perf/runs/after2.md`: idle 0.23% CPU, 1.65 wakeups/s, scroll p99 10 ms, 150 MB).

## 1. Milestones

| Milestone | State | Remaining |
|---|---|---|
| M0 workspace, gateway, REST, fidelity, watcher, signed profile | done | golden corpus of captured requests (header order and `capture` bodies are guesses); bench S3-S9 are covered by `scripts/perf`, retire `tools/bench` |
| M1 store, cache, FTS, renderer, power engine | done | none |
| M2 chat core (send/edit/react/pin/typing/ack, uploads, threads, forums, search, DMs, friends, settings writes, notifications) | built | live verification of every write; forum list/create/sort/tags exist (`ForumPane.swift`) |
| M3 multi-account, inbox | done | Focus filters (deferred, not needed for parity) |
| M4 voice (V1, DM calls, PTT, per-user volume, DAVE, stage sheet) | built | live verification; soundboard; output-only device choice |
| M5 camera, Go Live, watch | built end to end | live verification; simulcast, screen-share audio, PiP, transport-cc/REMB negotiation |
| M6 web fallback, unknown-feature cards, kill-switch | done | Activities bridge; content-rule privacy list |
| M7 accessibility, themes, localisation, release | partial | VoiceOver audit, Dynamic Type, shared string source (PORTING #2), updater stays off until pubkey + appcast URL are set |
| Second shell | not started | PORTING §3 blockers: per-OS identity profiles, audio backend, UniFFI generator choice, cross-target `cargo check` (needs a C cross toolchain: zig or per-target gcc/MSVC; not run yet) |

## 2. Official-client features still absent

H = daily, M = regular, L = rare.

| Feature | Tag | Notes |
|---|---|---|
| Edit history, bulk delete | L | none |
| Lottie stickers | M | typed in the core (`StickerItem.lottie`, JSON URL) but not drawn: the shell needs the `lottie-ios` package, which needs your approval to add; animated PNG stickers draw as a still unless the image pipeline plays them (unchecked) |
| Server admin leftovers | L | sticker upload, server splash, move a channel between categories, member overwrites |
| Soundboard | L | none |
| Rich presence buttons, Spotify listen-along, activity invites | L | activities show as text + artwork only |
| Account settings (privacy, connections, 2FA), device/session list | L | route to web fallback |
| WebAuthn login | L | falls back to QR / other method |
| Menu-bar mini mode, split view, Live Text, translation | L | deferred, not needed for parity |

## 3. Next order of work

1. **Live verification pass** (user-driven, nothing in automation may send): the unverified list in HANDOFF, plus every `capture`-marked body in `docs/FIDELITY.md`. A wrong guess is the biggest fidelity risk, so convert guesses to captures before adding features.
2. **Efficiency**: re-run `scripts/perf/check.sh` on a release build after the recent feature merges (the last run predates forward/invite/group-DM/GIF work); watch hop latency (`reloadMessages` maps 50 rows on the main thread) and idle wakeups with voice/video panels open.
3. **Cheap parity**: embed NSFW cover; role editing; audit-log reasons.
4. **Porting readiness**: install a cross C toolchain and run `cargo check --target x86_64-pc-windows-msvc` / `x86_64-unknown-linux-gnu` on `unison-core`; pick the UniFFI generator for the first non-Apple shell.
5. **Larger features**: soundboard, Lottie, simulcast/PiP, event creation, stage creation/topic editing.
