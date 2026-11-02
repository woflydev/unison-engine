# Porting to a second platform

Rules are in `docs/ARCHITECTURE.md`; this file says where the repo stands against them and what a new shell must supply. No second shell exists yet; `docs/AUDIT.md` records the Linux build and `docs/shell-kit/` the agent brief.

## 1. Where the code is
| Layer | Size | Notes |
|---|---|---|
| Core (`unison-core`, `-etf`, `-markdown`) | ~30k lines Rust | protocol, store, cache, rules, voice/video engine |
| Shell API (`unison-ffi`) | ~9k lines Rust | UniFFI records, commands, listener (Swift is generated today; `cdylib` is built, other languages need their UniFFI generator) |
| macOS shell | ~24k lines Swift | SwiftUI + AppKit |

## 2. What is ready
- The workspace has no Apple-only crates except the audio backend, now target-gated: `coreaudio-rs` is a macOS-only dependency and `voice::audio::open` has a stub for other OSes, so `unison-core` should compile elsewhere (Linux: built with `cargo zigbuild --target x86_64-unknown-linux-gnu.2.31` and the whole workspace test suite passes on Debian 12 in Docker, see `docs/AUDIT.md`; Windows not yet run).
- Networking is `reqwest` + `rustls` with bundled roots, SQLite is bundled, Opus is static, DAVE is `davey`: no system TLS or keychain in the core. The engine receives tokens from the shell; the data directory is passed in.
- Everything the shell must implement is a callback or a plain data record. Navigation (history, remembered channel, stepping), forward targets, author grouping, group-DM capacity, autocomplete, slash commands, search execution, power policy and unread logic are in the core/FFI.
- Perf budgets and the harness (`perf/budgets.json`, `scripts/perf`) are macOS-driven but the budgets are platform-neutral numbers.

## 3. Blockers and fixes before a second shell starts
| # | Item | Why it blocks | Fix |
|---|---|---|---|
| 1 | Client identity: the OS now selects the platform strings (`HostOs`), but the Windows/Linux values are unverified and `profile.default.json`/`update.rs` still describe macOS builds | Fidelity requires the real OS and a real Discord desktop build for it | Capture Windows and Linux profiles (build numbers, `native_build_number`, `os_version` format) from the official client; add a per-OS update manifest |
| 2 | No audio backend for other OSes | Voice connects but is silent | Implement `voice::audio::open` for WASAPI/PulseAudio; echo cancellation needs its own answer (VoiceProcessingIO does it on macOS) |
| 3 | UniFFI generator for the target language | Swift is generated in `scripts/build-macos.sh` | Pick Kotlin (Android/JVM), C# (`uniffi-bindgen-cs`) or Python-based glue per platform; verify callback interfaces and async functions work in that generator before committing |
| 4 | Video: encode/decode/capture are shell-side (VideoToolbox, ScreenCaptureKit, AVFoundation) | Each OS needs its own hardware path | Keep the seam: shell hands Annex B access units to `push_video_frame` and receives them in `on_remote_video`. Windows: Media Foundation/D3D11; Linux: VAAPI/PipeWire |
| 5 | Row height measurement (`RowHeights.swift`) mirrors AppKit text layout | Every shell needs its own measurement; mismatches show as jumpy scrolling | Keep the `heightcheck` approach (compute vs fit) in each shell's DevTools; keep row structure in core data |

## 4. Architecture debt in the macOS shell
| # | Where | What | Move to |
|---|---|---|---|
| 1 | `Views/Pickers.swift` (masonry columns), `ForwardSheet.swift` (search filter) | List shaping | Acceptable as presentation; revisit only if a rule appears |
| 2 | `Localizable.xcstrings`; action titles come from the core in English | UI strings live in an Apple catalogue | Decide a shared string source (e.g. JSON in the repo compiled per platform) before translating |
| 3 | `AppModel.swift` `showMembers`, `density`, `useDiscordTheme` | UI-only prefs | Fine in the shell |
| 4 | Voice device ids (`voice.inputDevice`, `voice.outputDevice`) and the push-to-talk key (`voice.pttKeyCode`) in `UserDefaults` | Device ids and key codes differ per OS | Fine in the shell. Volumes, input mode, sensitivity and processing are the core's (`voice_prefs`) |
| 5 | `DevTools.swift`, `scripts/perf`, packaging scripts | macOS-only tooling | Per-platform by nature; keep the budgets shared |
| 6 | `UpdateInstaller.swift`, `update-swap.sh` | Mount, copy, codesign, swap and relaunch are OS work | Per-platform by nature; the rules are in `update_install.rs` |
| 7 | `AppModel.sidebarOpen`, `sidebarColumnWidth`, `Dock` and `ChatTitle` (`RootView.swift`, `DesignSystem.swift`) | Pane open/closed and width are UI-only prefs; dock and title layout are window chrome with macOS toolbar constants | Fine in the shell; another shell draws its own chrome. Keep the rule (one owned width, interruptible, chat follows the live edge) |
| 8 | Menu accelerators in `UnisonApp.swift` | The shortcut list is the core's (`keybinds.rs`, `keybind_table`), the binding to menu items is the shell's | Another shell translates `mod+shift+i` tokens to its modifier keys; custom bindings are not offered |
| 9 | `Views/Cosmetics.swift`, `Views/LottieSticker.swift` | Drawing only: the core parses and decides (`cosmetics.rs`, `sticker_motion.rs`) | Fine; Lottie needs a player (`lottie-ios`; web has lottie-web) |
| 10 | `AppModel+Settings.swift` `ProfileDraft`, `profileDirty`, `saveProfileDraft` | The Profiles pane's draft (display name, pronouns, about me) is kept and diffed in the shell, and the length limits (32/40/190) are applied there | Move to the core's settings draft (`Session::stage_*`) so the save bar covers it like the other settings |

Fixed on 2026-11-01: update-check timing and the beta channel (`UpdatePrefs`, `update_check_due`), the native-calls switch and the call choices (`VoicePrefs`), the zstd advisories (`third_party/wreq-util` moves zstd to 0.14 / zstd-safe 8).

Update and restart (what another shell implements). The core owns every rule and sentence (`update.rs`, `update_install.rs`, `unison-ffi/src/release.rs`); a shell supplies only the OS half:
1. Fetch the feed at `update_feed_url(base, channel)`, pass the body to `verify_appcast(json, version, build, UpdatePlatform{os, arch, os_version}, channel)`; it returns the asset for this OS and CPU (`macos`/`windows`/`linux`, `arm64`/`x86_64`) or nothing. Add the platform's assets to the feed (`assets[]`, see `docs/RELEASING.md`).
2. Download with progress into a temp file; check `update_digest_matches`; show the stage with `update_stage_text/action/busy` (store one `UpdateStage`).
3. `update_install_route(location_writable, read_only_location)`: `OpenInstaller` means open the download for the person and show the sentence.
4. Unpack/mount the download, copy the new app next to the installed one (same volume, so the swap is a rename), verify its code signature with the OS tool, read its `CodeIdentity` (identifier, version, signer; the running app's too) and call `update_check_candidate`; refuse with the returned sentence on error.
5. `update_restart_gate(in_call)`: never restart during a call; keep `ReadyToRestart` and let the person press Restart Now afterwards. Restart only from a click.
6. Hand off to a detached helper that outlives the app: wait for the app's pid to exit, move the old app aside, move the new one in, start it, confirm it runs, then delete the old one; on any failure put the old one back and start it. macOS: `apps/macos/Resources/update-swap.sh` (tested by `scripts/test-update-swap.sh`). Windows will need a helper process (the exe cannot replace itself while running; the same wait, swap, start, confirm, roll back sequence), Linux an AppImage/package-aware equivalent.
The update channel, auto-check switch and last-check time are core preferences (`updater_prefs`, `update_check_due`, `record_update_check`).

Moved to the core (done):
- Navigation history, remembered/last channel (was `UserDefaults`), channel/server stepping: `navstate.rs`, `shell_nav.rs`.
- Author grouping (`MessageItem.grouped`), forward destinations and fan-out, group-DM capacity, sensitive-media flag.
- Behavioural persistence (`localstate.rs`, `local.rs`): drafts (write-behind), recent reactions, reply ping, per-person call volume/mute, account order. Old `UserDefaults` values are not migrated.
- Search grammar (`search_syntax.rs`): filter list, query parsing, date bounds (the shell passes its UTC offset), suggestions.
- Video power policy: the shell reports raw readings, `video_power_cap` returns the cap.
- Notification decision: `Store::should_notify` (muted channels, muted servers, "nothing" level) gates the `Mentioned` delta; the shell only skips the focused channel.
- Action lists (`actions.rs`): `message_actions` (bar, shift bar, menu), `user_actions`, `dm_actions`, with permission rules (pin and delete need Manage Messages in servers). The shell maps ids to icons and handlers (`Views/ActionRunner.swift`).
- Client identity: `HostOs` in `HostInfo` drives `os`, `sec-ch-ua-platform` and the user agent. Windows and Linux strings are `capture` guesses.

## 5. What a new shell must implement
1. Engine lifecycle and the `EngineListener` callbacks (deltas batched per frame).
2. Secure token storage and the account list.
3. Views for every `*Item` record, following `docs/UI.md` in the platform's idiom.
4. Message list with measured rows, image pipeline (downsample off-main), animated media only when visible.
5. Composer: text editing with core-provided tokens/suggestions (`composer.rs`), pickers.
6. Voice and video seams (§3 of `ARCHITECTURE.md`); permissions prompts.
7. Notifications, menus/accelerators (see `docs/KEYBOARD.md`; the Go menu actions call the core's `nav_*`).
8. Power state reporting (`set_power_state`) from OS notifications, never from timers.
9. Packaging, signing and the update channel (`update.rs` verifies a signed manifest).
10. DevTools-style hooks for layout checks and a perf script that fills `perf/runs`.

## 6. Order
Capture per-platform identity profiles, prove the FFI path in the new language (login + message list), then build views.

## 7. Linux/Debian test loop
```
colima start && docker build -t unison-deb scripts/docker   # rust:1-bookworm + cmake
docker run --rm -v "$PWD":/src:ro -v unison-target:/target -e CARGO_TARGET_DIR=/target unison-deb cargo test --workspace
```
