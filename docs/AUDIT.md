# Audit (2026-10-31 engine and Linux cross-build, refreshed 2026-11-01)

Scope: whole workspace against `docs/ARCHITECTURE.md`, `docs/UI.md`, `docs/FIDELITY.md`, plus a Linux build to test the "thin shell" claim.

## Results
| Check | Result |
|---|---|
| `cargo test --workspace` | green: 383 core tests, 35 elsewhere (2026-11-01) |
| `scripts/ui-audit.py` | OK (exit 0) |
| `cargo audit` | 10 vulnerabilities (down from 13: zstd cleared), 5 warnings; the rest are `libcrux-*` via `davey`, `rsa` and `crypto-bigint` (below) |
| Linux x86_64 and aarch64 `cargo zigbuild` (glibc 2.31 floor) | core, etf, markdown, ffi build; debug, release and `--tests` |
| UniFFI Python and Kotlin generators on the Linux `.so` | generate; Python output compiles |
| `cargo test --workspace` on Debian 12 aarch64 in Docker (Colima) | passes (PORTING §7) |
| Windows | not run: `kernel_release` is `cfg(unix)` with an empty fallback; no audio backend off macOS |

Linux cross-build: `brew install zig`, `cargo install cargo-zigbuild`, `rustup target add aarch64-unknown-linux-gnu`, then `cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.31 -p unison-ffi`.

## Linux findings
The core and FFI needed no code changes for Linux. A second shell can start from `libunison_ffi.so` plus generated bindings.
1. Bindings need an unstripped library: `strip = "symbols"` removes the UniFFI metadata, so `uniffi-bindgen --library` on a stripped ELF generates nothing and prints no error. A Linux script should generate from a non-stripped build and fail on empty output.
2. No audio backend: `voice::audio::open` errors off macOS (PORTING #2).
3. Identity: Linux strings in `fidelity.rs` are `capture` guesses; `profile.default.json` and `update.rs` are macOS-shaped.
4. Host detection and thread priority are listed as seams in ARCHITECTURE §3.

## Security
- `zstd`: the direct dependency is zstd-safe 8; the old 0.13 line came in through `wreq-util` (certificate decompression of network data) and is replaced by `third_party/wreq-util` (zstd 0.14). Drop the patch when upstream bumps it.
- `davey =0.1.5` pins `libcrux-*` 0.0.x with advisories (AES-GCM tag check not constant time, panics on short input, SHAKE issues, an aarch64 constant-time select bug). DAVE is on the voice path. Only `0.2.0-beta.1` exists; bump when stable.
- `rsa 0.9.10` (Marvin timing attack, no fix): used only for the QR-login OAEP decrypt of a one-shot server ciphertext in `remote_auth.rs`; no decryption oracle is exposed. `crypto-bigint 0.5.5` via `p256`/`elliptic-curve`: verification only.
- Unmaintained: `audiopus_sys`, `instant`, `basic-toml`, `proc-macro-error2`.
- Release uses `panic = "abort"`, so an `unwrap` on network input is a crash. The `Error` enum is large (`result_large_err` warnings); box the big variants.

## Constitution check
Shell logic found by the first audit is in the core: friends tabs and pending count (`friends_view`), the unread list (`unread_list`), new-messages count (`new_since`), forum tag permission (`ForumTagItem.usable`), CDN resize URLs (`resized_media_url`), update-check timing (`UpdatePrefs`), call choices (`VoicePrefs`), native-calls switch. What stays in the shell is listed in PORTING §4.

## Navigation audit (2026-11)
Trigger: a Mentions row from a server opened under Direct Messages. Cause: REST messages carry no `guild_id`, and `navigate` read "guild nil" as "go Home". Fix: one core rule (`open_target::resolve`, 8 tests) and one shell path (`AppModel.open`), UX.md 8. Every surface that points at a message, channel, thread, DM or server now goes through `open`: Inbox, quick switcher and search, Saved, reminders and notifications, pins and threads popouts, forums, thread chips, reply lines, the unread bar, events, message requests, profiles, mutual servers, new servers, discord.com links, history and Alt-Up/Down. The sidebar reveals the selected row (category opened, row scrolled in) and a server in a closed folder opens its folder. A hidden, deleted or unknown channel says so and changes nothing. Checked offline with DevTools `opencheck` (copied cache, no token, network denied by a sandbox profile).

Not checked: the in-window highlight and `load_around` window (the cache held no messages), cross-account open (needs a second cached account), category expansion and scrolling (view state, checked by reading).

Not fixed: the Unreads tab lists only the open server's unread channels plus DMs (needs a core list across servers and a design decision); "Jump to present" re-selects the channel in place by design; user mention pills open the profile card, not a DM.

## Shell kit
`scripts/make-shell-kit.sh` builds `unison-devkit-<version>.zip` (engine source, Linux libraries, Python/Kotlin/Swift bindings, `API.md`, the constitution docs, the macOS shell as reference, `START_HERE.md` from `docs/shell-kit/`). Verified: unzipped, the Python bindings load the aarch64 `.so` on Debian 12 in Docker, `examples/smoke.py` passes, and the bundled `engine/` passes `cargo check --workspace --locked`. Colima does not share `/tmp`; mount from under `$HOME`.
