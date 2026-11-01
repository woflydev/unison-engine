# Parity with the official client (2026-11-01)

Gap list against the official desktop client. Evidence is from reading `crates/` and `apps/macos`; the feature inventory comes from the official client's web bundle, so check endpoint shapes against `docs/FIDELITY.md` (`capture` marks a guess) before relying on one. New work follows `docs/ARCHITECTURE.md` §6.

## Milestones
| Milestone | State | Remaining |
|---|---|---|
| Gateway, REST, fidelity profile, store, cache, FTS, renderer, power policy | done | golden corpus of captured requests (header order and `capture` bodies are guesses) |
| Chat (send, edit, react, pin, typing, ack, uploads, polls, voice messages, threads, forums, search, DMs, friends, notifications) | built | live check of every write |
| Multi-account, inbox, saved, reminders, server folders | done | none |
| Voice, video, Go Live, stages, DM calls, text chat in voice channels | built | live check; soundboard; simulcast, screen-share audio, PiP |
| Server admin, moderation, events | built | recurring events, member-level channel overwrites |
| Settings | built | see below |
| Profile cosmetics | decorations, server tags, nameplates, name colours, role icons, profile themes | profile effects, animated decorations, name-style effects and fonts |
| Stickers | image and Lottie | sticker upload |
| Accessibility, themes, localisation | partial | VoiceOver audit, shared string source (PORTING #2) |
| Second shell | not started | PORTING §3 |

## Settings not built
Wired (`prefs.rs`, shapes in FIDELITY): theme, message display, media and embed toggles, animation, DM scan and spam filters, friend requests, new-server defaults, age-restricted content, activity privacy, local time, status, language and time format, notifications (reactions, super reactions, streams, per-server and per-channel overrides), AFK timeout. Not wired because they need a password, a code or a payment, or a feature we do not have: username, email, password, 2FA, billing, connections, devices, data requests, blocked users list, Advanced (developer mode), Streamer mode, game overlay, activity registration, per-app volume, soundboard volume. Proto fields known but not used yet: spoiler reveal mode, image descriptions, command suggestions, TTS, thread sidebar, profile visibility. Keybinds: the table is the core's (`keybinds.rs`), custom bindings are not offered.

## Absent features
| Feature | Notes |
|---|---|
| Edit history, bulk delete | rare |
| Server admin leftovers | sticker upload, splash, move a channel between categories |
| Soundboard | none |
| Rich presence buttons, Spotify listen-along, activity invites | activities show as text and artwork |
| WebAuthn login | falls back to QR or another method |
| Menu-bar mini mode, split view, Live Text, translation | not needed for parity |

## Next
1. Live verification by the user (nothing in automation may send): the list in `docs/HANDOFF.md` and every `capture` body in `docs/FIDELITY.md`. Convert guesses to captures before adding features.
2. Re-run `scripts/perf/check.sh` on a release build after the recent feature merges.
3. Porting readiness: run `cargo check` for Windows; pick the UniFFI generator for the first non-Apple shell.
4. Soundboard, simulcast, PiP, stage creation.
