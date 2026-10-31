# Audit, 2026-10-31 (engine + Linux cross-build)

Scope: whole workspace against `docs/ARCHITECTURE.md`, `docs/UI.md`, `docs/FIDELITY.md`, plus a Linux build to test the "thin shell" claim. Run on the working tree including uncommitted work (38 modified files, some from another agent: `send_rules.rs`, `send_state.rs`, `Outgoing.swift`).

## Results
| Check | Result |
|---|---|
| `cargo test --workspace` | green: 248 core, 23 + 7 + 3 elsewhere |
| `scripts/ui-audit.py` | OK (exit 0) |
| `cargo clippy --workspace --all-targets` | 0 errors; ~330 warnings, 242 are `result_large_err` (below) |
| `cargo audit` | **12 vulnerabilities** + 15 warnings (below) |
| Linux x86_64 `cargo zigbuild` (glibc 2.31 floor, i.e. Debian 11+) | core, etf, markdown, ffi build; debug, release and `--tests` |
| Linux aarch64 release | builds |
| UniFFI Python and Kotlin generators on the Linux `.so` | generate; Python output compiles |
| Run on Debian | **not done**: no Docker/VM here (UTM is installed, no image) |

Toolchain installed for this: `brew install zig`, `cargo install cargo-zigbuild`, `rustup target add aarch64-unknown-linux-gnu`. Commands: `cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.31 -p unison-ffi`.

## Linux findings
The core and FFI needed **zero code changes** for Linux. A second shell can start from `libunison_ffi.so` + generated bindings.
1. **Bindings need an unstripped library.** `[profile.release] strip = "symbols"` removes the UniFFI metadata, so `uniffi-bindgen --library` on a release `.so` generates nothing and prints no error. Generate from a debug/`strip = "none"` build The macOS script generates from a release dylib and evidently works; I did not investigate why it differs from ELF. A Linux build script should use a non-stripped build for bindgen and fail if the output is empty.
2. **No audio backend**: `voice::audio::open` returns an error, so Linux voice is silent (known, PORTING #2). Next: PipeWire/PulseAudio behind that seam.
3. **Identity** (`fidelity.rs`): Linux strings are `capture` guesses, `profile.default.json` and `update.rs` ("Minimum macOS version") are macOS-shaped. `os_release` comes from `uname`, so Linux sends e.g. `6.1.0-18-amd64`; verify against the official Linux client.
4. **Windows would not compile**: `fidelity.rs` `kernel_release()` calls `libc::uname`/`utsname`, which do not exist on Windows (not run: no `cargo-xwin`). Gate it or move it to the shell's `HostInfo`.
5. **Rule 3.26/§4.6 drift**: `HostOs::current()` and `HostInfo::detect()` pick the OS with `cfg!(target_os)` inside the core, whereas ARCHITECTURE says the shell passes the profile in. `engine.rs` `lower_thread_qos` (`target_vendor = "apple"`) and `voice/connection.rs:508` QoS are also not in a listed seam. Add "thread priority" to §3 or pass in.

## Engine / security
- **zstd-safe 7.3.0 is a direct dependency** of `unison-core` and carries ~10 advisories (double free, UAF, OOB reads; fixed in >= 8.2.0, which is already in the tree via reqwest). Upgrade to `zstd-safe = "8"`; it decodes untrusted gateway data. Highest priority.
- `rsa 0.9.10` (Marvin timing attack, no fix) and `crypto-bigint 0.5.5` (via `p256`): check whether private-key operations ever run on attacker-influenced input; if only verification, document and ignore.
- `davey =0.1.5` pins `libcrux-*` 0.0.x with several advisories (AES-GCM tag check not constant time, panics on short input, SHAKE issues; DAVE is on the voice path and an aarch64 constant-time select bug applies to Apple Silicon). The exact pin blocks the fix: bump `davey` when upstream does.
- Unmaintained: `audiopus_sys`, `instant`, `basic-toml`, `proc-macro-error2`.
- `Error` enum is large (242 `result_large_err` warnings). Box the big variants or the payload once; mechanical.
- Panics: release uses `panic = "abort"`, so every `unwrap` on network input is a crash. Worth a grep pass over `gateway`/`rest` parsers.

## Constitution (shell logic that should be core)
Spot check of `apps/macos` (about 18k hand-written lines; the rest is generated). Mostly clean; candidates for the debt table in PORTING:
- `Views/UnreadBar.swift:12` counts messages newer than a marker by comparing ids; `Views/UnreadInbox.swift:78` decides what is unread and not muted. Unread logic is core by rule.
- `Views/FriendsActions.swift:47,57`: friend tab filtering and pending count.
- `Views/ForumPane.swift:371`: `!moderated || canModerate` is a permission rule.
- `ImagePipeline.swift:71`: builds CDN resize URLs (`width`, `height`, `format=webp`). That is a request shape; ARCHITECTURE §4.3 says request shapes live in `rest/` and are in FIDELITY.
- `Release.swift:131`: update-check timing in `UserDefaults` is behavioural persistence.
- `ChatPane.swift:314,317`, `MessageExtras.swift:37` (`isImage/isVideo`), `UnreadInbox`: filters over core data that another shell would repeat. `ForwardSheet`/`Pickers` filtering is already accepted as presentation.
- Uncommitted work introduces `Outgoing.swift` plus core `send_rules.rs`/`send_state.rs`: re-run the checklist when that lands.

## Docs
- `docs/PORTING.md` says cross-target `cargo check` could not run; it now does for Linux (this file). Windows still unrun.
- `docs/HANDOFF.md` test count (171, 183) is stale: 248.

## Suggested order for a Debian shell
1. Fix zstd-safe, rebuild.
2. Prove the FFI path from Python (login + message list) on a Debian VM: `libunison_ffi.so` + `unison_ffi.py`.
3. Capture the Linux identity profile; add the PipeWire audio backend.
4. Pick the toolkit (GTK4 via Python/Rust, or Qt) and package as `.deb` (`cargo deb` for the library, or a shell package depending on it).

## Follow-up (same day): fixes
- **zstd-safe 7 -> 8**: done (`unison-core/Cargo.toml`); `cargo audit` went from 12 to 10 vulnerabilities. Remaining: `libcrux-*` via `davey =0.1.5` (only a `0.2.0-beta.1` exists; bump when stable), `rsa` Marvin (used only for the QR-login OAEP decrypt of a one-shot server ciphertext in `remote_auth.rs`, no decryption oracle is exposed; accepted until a fixed release), `crypto-bigint 0.5.5` via `p256`/`elliptic-curve`.
- **Debian test run**: `cargo test --workspace` passes on Debian 12 aarch64 in Docker (Colima): 253 core tests, 0 failures. See PORTING §7 for the loop.
- **Windows blocker**: `kernel_release` is `cfg(unix)` with an empty fallback.
- **Seams**: ARCHITECTURE §3 now lists host detection and thread priority as seams.
- **Shell logic moved to core/FFI** (`unison-ffi/src/lists.rs`, `unison-core/src/media_url.rs`, tests added): friends tabs/filter/pending count (`friends_view`), unread list (`unread_list`), new-messages count (`new_since`, cap 50), forum tag permission (`ForumTagItem.usable`, additive), CDN resize URL (`resized_media_url`). Swift now only draws. macOS app builds.
- **Left as is**: `Release.swift` update-check timing (PORTING debt #6); `ChatPane`/`MessageExtras` filters are presentation over core data; `ForwardSheet`/`Pickers` already accepted.

## Shell kit (same day)
`scripts/make-shell-kit.sh` builds `unison-devkit-<version>.zip` (into `dist/`, or the release folder) (70 MB): engine source, Linux x86_64/aarch64 libraries, Python/Kotlin/Swift bindings, `API.md` (generated by `scripts/api-index.py`), the constitution docs, the macOS shell as reference and `START_HERE.md` (the agent brief, source in `docs/shell-kit/`). Verified: unzipped, Python bindings load the aarch64 `.so` on Debian 12 in Docker, `examples/smoke.py` passes (including a real QR-login event from the gateway), and the bundled `engine/` passes `cargo check --workspace --locked`. Colima does not share `/tmp`; mount from under `$HOME`.

# Navigation audit 2026-11

Trigger: in the Inbox, clicking a Mentions row that came from a server did not take you to that server.

## Root cause
`MentionItem.guild_id` comes from the REST message, and REST messages carry **no `guild_id`** (only gateway events do). So every server mention had `guildId == nil`. `AppModel.navigate` reads "guild nil and a server selected" as "go Home" and called `selectHome(preferChannel:)`: the channel opened, but under Direct Messages: the rail showed Home, the sidebar the DM list, the member list was gone. Contributing defects in the same path: `jump` re-derived the guild a second time on the *active* account; `navigate` did `switchAccount`, which first restored and acked the other account's last channel; `selectGuild(preferChannel:)` silently replaced a channel missing from the sidebar list (a thread, a hidden channel) with the server's first one and *marked it read*; `openChannel` and `jumpToMessage` were two racing requests (the latest page could land after the window around the message and replace it); popovers stayed in whatever state they had; nothing revealed the row (collapsed category, off-screen row, closed folder).
Fix: one core rule (`unison_core::open_target::resolve`, 8 tests including the REST-message case) and one shell path (`AppModel.open`), see UX.md 8.

## Surfaces
| Surface | Expected | What it did | Fix |
|---|---|---|---|
| Inbox, Mentions | right server, message loaded and highlighted | opened under Home (REST message has no guild) | `open`, guild resolved by the core |
| Inbox, Unreads | open the channel or DM, close the panel | selected directly, no reveal | `open` |
| Cross-account inbox view (`InboxView`, not wired to any button today) | right account, message | navigated, never jumped to the message | `open(account:, message:)` |
| Quick switcher: server, channel, DM, message, other account | right account and server, message highlighted | server result switched account and restored a channel; message result used `navigate` + `jump` (two paths) | `open` (account switch without restore) |
| Saved for Later rows | account, server, message | same two-step as Inbox; guild from a REST message | `open` |
| Reminder notification | open the saved message | opened the channel only | userInfo carries the message id, click uses `open` |
| Mention / DM notification | open the message | opened the channel only | message id carried, server re-resolved |
| Call notification | open the DM | worked | `open` |
| Search (local, Discord, Everywhere, inline) | jump, clear the search, close results | `jump` plus manual clearing in the view | `open` clears search (`dismissTransients`) |
| Pins popout | jump and close | `dismiss()` + `jump` | `open` |
| Threads popout | open thread and close the popover | popover stayed open over the thread | `open` + `transientEpoch` closes it |
| Forum posts, back to forum | open post / forum | `selectChannel` from the view | `open` |
| Thread chip in a message | open thread | `selectChannel` | `open` |
| Reply line, reply bar | scroll to the original | `jump`; from elsewhere two paths; reset a reply in progress | `open` (already-here keeps reply and edit) |
| `#channel` mention, message link | open, in another server too | `navigate` with the channel's guild (worked) | `open` |
| discord.com links in text, embeds, link buttons | open in the app | opened the browser | core `parse_link` + `link_target`, `AppModel.openLink` |
| Unread bar "Jump" | scroll to the first new message | `jump` | `open` |
| Event "Join" | open the voice/stage channel in that server | `selectChannel` assuming the same server | `open` |
| Message requests | open the DM, close the popover | `selectChannel` | `open` |
| Profile card / full profile "Message", friends list, group DM | open the DM, close the card | worked through `on_navigate` -> `navigate` | `on_navigate` -> `open` (card closed by `dismissTransients`) |
| Mutual servers (full profile) | open the server, close the sheet | `selectGuild` + manual close | `open(channel: nil)` |
| New server (join/create) | open it | `selectGuild` | `open(channel: nil)` |
| Server folders / rail | icon selected | a server in a **closed folder** showed no selection anywhere | the folder opens when its server is selected (any cause); rail scrolls to it |
| Sidebar | selected row visible | rows off screen or in a collapsed category stayed hidden after a jump, a history move or Alt-Up/Down | `sidebarReveal`: category opened (on `open`), row scrolled in for any selection |
| History back/forward, Alt-Up/Down, Cmd-Alt-Up/Down | selection stays visible | selection could leave the screen | same reveal |
| Account switch from `open` | no side effects | restored and acked the target account's last channel first | `switchAccount(restoreSelection: false)` |
| Hidden / deleted / unknown channel | say so | silent no-op (or opened the server's first channel and marked it read) | banner sentence, lands on the reachable server |
| Voice channel rows, member list, profile popovers | in place | fine | unchanged (sidebar rows are direct selectors; profile -> `open`) |
| Deep links (`unison://`) | n/a | none exist | not added |

## Verified offline (copied cache, no token, network denied by a sandbox profile)
DevTools `opencheck`: for two real servers a channel opened with no server hint lands in its own server (the Inbox bug), the Inbox popover closes, the sidebar is asked to reveal the row, the caret ends in the composer, a DM opens under Home, an unknown channel changes nothing and says so. `openat CHANNEL FAKEMESSAGE` from a server falls back cleanly (DM opened under Home). Core: 8 `open_target` tests (REST message without guild, store beats the hint, thread parent and category, unknown and hidden fallbacks, loaded-window check, link parsing including look-alike hosts).

## Reasoned, not run
This cache holds no messages, so the in-window highlight and the `load_around` window could not be exercised offline; they use the unchanged `MessageList` highlight code and the unchanged REST call. Cross-account open needs a second cached account. Category expansion, folder opening and scrolling are view state, checked by reading and by the reveal requests, not by a screenshot (the offline window is occluded).

## Not fixed (and why)
- The Unreads tab lists only the open server's unread channels (plus DMs). Needs a core list across servers and a design decision; not a navigation defect.
- ~~A mention in a thread this account never joined cannot be opened natively~~ Fixed (2026-11): the core fetches it once after the click (`Session::preview_thread`, FIDELITY "Unjoined threads"), the thread opens in its parent server with the parent revealed, and a "Join thread" bar above the composer is the only way in. Inbox, search, saved, reminders, notifications and discord.com links all go through `open`.
- "Jump to present" in the chat re-selects the channel in place (it resets the window, not the place); it stays a direct selector on purpose.
- User mention pills keep opening the profile card, not a DM.

