# Unison engine

The Rust engine behind Unison, a native Discord client: protocol, state, rules, voice, and the `unison-ffi` surface that thin native shells call. GPL-3.0-or-later. Every release of Unison is one tag of this repository (`vX.Y.Z`), so you can build on the engine and take new releases with one line.

| Crate | What |
|---|---|
| `unison-core` | Protocol (REST, gateway, voice), store, cache, sessions, every rule |
| `unison-ffi` | The one surface a shell calls (UniFFI: Swift, Kotlin, Python bindings) |
| `unison-etf`, `unison-markdown` | Discord's term format, message markdown |

## Use it from Rust, pinned to a release
```toml
[dependencies]
unison-ffi = { git = "https://github.com/woflydev/unison-engine", tag = "v0.2.0" }
unison-core = { git = "https://github.com/woflydev/unison-engine", tag = "v0.2.0" }
```
To take a newer release, change the tag and run `cargo update -p unison-core -p unison-ffi`. To follow the latest release without editing, use `branch = "main"` (it always points at the newest tag) and run `cargo update`. Tags never move, so a pinned build is reproducible.

## Build a shell for a new platform
Read `docs/ARCHITECTURE.md` first (what belongs in the engine and what in a shell), then `docs/PORTING.md`. The releases page carries a devkit zip with prebuilt libraries, generated bindings and a guide written for coding agents.

## Rules that outrank everything
Never send a message, ring a call, join a server or change account state from automation or tests; every state-changing call needs a real user gesture. Never log or print tokens. Test with a throwaway account or offline.
