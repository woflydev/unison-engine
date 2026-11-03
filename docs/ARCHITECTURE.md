# Unison architecture constitution

Binding for every agent and contributor, next to `docs/UI.md` (look and feel) and `docs/FIDELITY.md` (wire shapes). **Read this file before writing any code**, and re-read the checklist in §5 before you call the work done. If a rule here and the code disagree, fix the code or amend this file in the same change.

Goal: one Rust engine, many thin native shells. macOS is the first shell; Windows, Linux and others will follow. Everything a second shell would have to rewrite is a bug in the first one.

## 1. The three layers
| Layer | Where | Owns | Must not contain |
|---|---|---|---|
| **Core** | `crates/unison-core`, `unison-etf`, `unison-markdown` | Protocol (REST, gateway, voice, DAVE, RTP/H.264 packetizing), the store, cache and persistence, every rule and policy (permissions, ordering, grouping, ranking, limits, unread/mention logic, navigation history, power policy, search grammar), request shapes, local behavioural settings | UI types, OS APIs (except behind a platform seam, §3) |
| **Shell API** | `crates/unison-ffi` | The one surface every shell calls: view-model records (`*Item`), commands, the listener. It composes core calls into what a screen needs (sidebar order, member rows, forward targets, `grouped` flags). Thin: it holds no state of its own | I/O or protocol knowledge of its own, per-platform branches |
| **Shell** | `apps/<platform>` | Rendering, input, accessibility, localisation of *presentation* (dates, numbers), OS integration (secure storage, notifications, menus and accelerators, file pickers, drag and drop, window chrome), hardware media (capture, codecs, display layers, audio devices), UI-only preferences (theme, density, pane widths) | Business rules, protocol shapes, ordering/filtering/ranking, persistence of anything that changes behaviour |

## 2. The decision test
Before writing a line in a shell ask: **"Would a Windows or Linux shell need the same code?"** If yes, it goes in the core (rules and state) or the shell API (shaping for a screen) and the shell only calls it. Typical hits: sorting, grouping, limits (e.g. "a group DM holds 10"), "can the user do X here" checks, which actions a message offers, search syntax, keyboard-navigation targets, history, "last channel in this server", drafts, recent emoji, volume per user, retry/backoff, throttling (typing 1 per 8 s), anything that decides *what* happens rather than *how it is drawn*.

A shell may keep **view state** (what is selected in a list, which popover is open, scroll offsets, hover) and **UI-only prefs**. It may keep a cache of core data to draw from (`AppModel` is such a cache); the core is the source of truth, and the shell re-reads after a delta.

## 3. Platform seams (the only places OS code is allowed)
Each is a narrow interface; a new platform implements it and nothing else changes.
- **Secure storage**: tokens live in the OS store, written and read by the shell, handed to the engine.
- **Audio device**: `voice::audio::open` (core, one `cfg(target_os)` implementation per OS).
- **Video capture, encode, decode and display**: the shell captures and uses the hardware codec; the core packetizes, encrypts, paces and negotiates (`voice/{h264,rtcp,video,stream}.rs`).
- **Notifications, menus, accelerators, dock/tray, file and image pickers, drag and drop, pasteboard, screen capture picker, permissions prompts.**
- **Text measurement** for the message list (row heights) and attributed text drawing. Rows are described by core data (`RichText`, spans); each shell lays them out.
- **Host detection and thread priority**: `fidelity.rs` (`HostOs::current`, `HostInfo::detect`, `kernel_release`) fills a default profile from the build target; a shell may overwrite it. `engine.rs::lower_thread_qos` and `voice/connection.rs` set QoS on Apple targets and are no-ops elsewhere.
- **Client identity**: the platform string in headers and super-properties must match the shell's real OS and a real Discord desktop build (see `docs/FIDELITY.md`); it is a profile the shell passes in, never a literal in the core.

## 4. Rules
1. **State and rules in the core, drawing in the shell.** A view body, a cell, a menu builder or an `AppModel` method that contains `if/else` business logic is a violation. Menus are built from a core answer ("these actions, enabled or not"), not re-derived per shell.
2. **Persist behaviour in the core.** Anything that changes behaviour across launches (drafts, history, remembered channel, recent reactions, per-user volume, reply-ping default) is stored by the core (SQLite `kv` or the account store). `UserDefaults`/registry/dconf are for UI-only prefs.
3. **One request shape, one place.** Endpoints, bodies and headers live in `unison-core/src/rest/` and are recorded in `docs/FIDELITY.md`; guesses are marked `capture`. A shell never builds a request.
4. **Intent gating.** Every state-changing call takes a `UserIntent` from a real user gesture (`UserGesture` through the FFI). Never loop or schedule a write.
5. **Records are the contract.** FFI records are additive: add fields, do not repurpose them. When you add one, update every shell's constructors (DevTools/test fixtures) in the same change.
6. **No platform names in shared code.** `target_os` appears only inside a seam in §3, with a stub for other OSes so the workspace always compiles everywhere.
7. **Performance rules apply to the core first.** No timers while idle, no polling, batch deltas per frame, no per-keystroke network. Budgets live in `perf/budgets.json`.
8. **Tests where the rule lives.** New rules get unit tests in the core (`cargo test --workspace`). Shell code is verified with DevTools (`heightcheck`, `snapall`, `synthcheck`).
10. **Reads are de-duplicated, in the core.** The same read is never in flight twice and is not repeated while its answer is fresh (`dedupe::Dedupe`: `begin` / `finish`, or a TTL cache for results the UI re-reads). New fetches state their freshness window in code; a shell never retries or re-asks on its own. Defaults: profile 60 s, notes 5 min, a channel already at its latest page 30 s. Accounts at launch connect staggered, the first at once (`Engine::restore_account`). Tests live next to the rule.
9. **Native feel is a shell concern, shared behaviour is not.** Each shell follows its platform's idioms (menus, shortcuts, motion, controls); it must still behave identically (same ordering, same limits, same defaults).

## 5. Checklist (run it before you finish)
- [ ] Did I add logic to Swift/C#/etc. that another shell would repeat? Move it to the core and call it.
- [ ] Is every new persisted value in the core (or a deliberate UI-only pref)?
- [ ] Are new FFI records additive and are all shell constructors updated?
- [ ] Is any `target_os` or platform literal outside a seam? Is there a stub for other OSes?
- [ ] Are new request bodies in `rest/` and noted in `docs/FIDELITY.md` (`capture` if guessed)?
- [ ] Core tests added; `cargo test --workspace` passes; the shell builds.
- [ ] If a shell exception was unavoidable, is it listed in the debt table of `docs/PORTING.md` with why?
- [ ] Where a place lives (server, category, thread parent, link target) is a core answer (`open_target`), not something a shell derives from a row or a hint (UX.md 8).
- [ ] **Whole-feature check (UX.md 0):** did I update every other implementation and every consumer of what I changed (other lists, panes, popovers, the other shell, fixtures, tests, docs), not just the spot I was looking at?

## 5b. No self-bot behaviour (binding)
Nothing Unison sends may look like a self-bot, in any build.
1. Every write needs a `UserIntent` from a real gesture (already enforced).
2. Automation never uses the network: while `rest::set_automation(true)` (turned on by the first DevTools command, or by a script) every REST call fails with `BlockedByPolicy("automation")`. Perf runs, layout checks, snapshots and agents work on the cache and the open gateway session.
3. Reads follow human cadence: opening a channel fetches at once only after a pause; fetches for opens are never closer than 400 ms (`open_pace`, used by `Session::open_channel`). The one prefetch is the official client's own: a read on mouse-down of a channel or DM row (`Session::preload_channel`, FIDELITY "Channel preload"). New fast paths (prefetching, background sweeps) need a reason in `docs/FIDELITY.md` and must be no faster than the official client would be.
4. At most one live perf run a day and one instance per token; never loop channel hops, scrolls or snapshots against a live account.
5. We do not forge integrity values (`launch_signature` stays null; the official client also sends null when its module is unavailable) and do not fake telemetry.

## 6. Adding a feature (the order)
1. Model and rules in `unison-core` (store, session, rest) with tests.
2. A command and a view-model record in `unison-ffi`.
3. UI in each shell from the record (follow `docs/UI.md`).
4. Docs: `docs/FIDELITY.md`, `docs/PARITY.md`, `docs/HANDOFF.md`.
