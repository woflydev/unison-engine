# Voice, video and screen share: design and plan (M4–M5)

Status: **built and on by default** (Settings, Calls turns it off; the owner tested voice, video and screen sharing live on 2026-11-01). Values still unpinned against the official client are marked `capture` in FIDELITY.md (toffset and content-type on video, receiver-side RTCP, the RTCP APP payloads; IDENTIFY, op 4 flags and tracks, heartbeat, codec numbering, experiments, speaking op, audio RTP extensions and silence frames were checked on 2026-11-01 against the web bundle and by disassembling `discord_voice.node`); until pinned, native calls are a fidelity risk for the account. Sections marked **capture** depend on a recorded call with the official client (§9) before we can pin a value.

Goals, in order:
1. **Indistinguishable at the protocol level.** Every gateway payload, voice-gateway payload and RTP/RTCP packet matches what Discord 0.0.419+ sends. Where we can't read a value from shipped code, we capture it from a real call and pin it in the signed identity profile (as with `client_build_number` today).
2. **Hardware first, much less energy than Discord.** Opus, AES-GCM and the media pipeline run on Apple's fixed-function and vector hardware. Video stays on the GPU from capture to the wire.
3. **No automation.** Joining, unmuting, starting video or a stream, and every soundboard play need a `UserIntent`. Push-to-talk and the mic gate are local audio decisions, not API writes.

---

## 1. What the official client does (observed in web build 2026-10-29)

On desktop the **voice gateway is driven from JavaScript** (`RTCControlSocket`). Only media (UDP, codecs, DAVE frame crypto, audio devices) lives in the native `discord_voice` module. So every control payload below was read from the shipped bundle rather than inferred.

### 1.1 Main gateway (existing connection)

| Step | Payload | Notes |
|---|---|---|
| Join | op 4 `VOICE_STATE_UPDATE {guild_id, channel_id, self_mute, self_deaf, self_video, flags}` | **capture**: exact key set and `flags` value. |
| Server replies | `VOICE_STATE_UPDATE` (ours, carries `session_id`) then `VOICE_SERVER_UPDATE {token, guild_id, endpoint}` | The store already ignores these; they go to the voice session. |
| Leave | op 4 with `channel_id: null` | |
| Region ranking | `GET https://latency.discord.media/rtc?v=2` (v1 without port-aware testing), refetched on a schedule | Sent by the official client whether or not you're in a call. **capture**: schedule and whether desktop pings the returned IPs. |
| Go Live | op 18 `STREAM_CREATE {type, guild_id, channel_id, preferred_region}`, op 19 `STREAM_DELETE`, op 20 `STREAM_WATCH {stream_key}`, op 22 `STREAM_SET_PAUSED` | M5. |
| DM calls | op 13 `CALL_CONNECT {channel_id}` | M5. |

### 1.2 Voice gateway

URL `wss://{endpoint}?v=9` (`?v=10` when the SFU-update experiment is on), JSON text frames, `binaryType = arraybuffer` for DAVE's binary opcodes.

| Op | Direction | Payload (as sent) |
|---|---|---|
| 0 IDENTIFY | → | `{server_id, channel_id, user_id, session_id, token, max_dave_protocol_version, video, streams}` |
| 8 HELLO | ← | `{v, heartbeat_interval}`. For v ≥ 4 the client beats at `interval × modifier`; the web build also caps it at 5 s, the desktop build doesn't. |
| 3 HEARTBEAT | → | `{t: <monotonic ms>, seq_ack}` |
| 6 HEARTBEAT_ACK | ← | |
| 2 READY | ← | `{ssrc, ip, port, modes, streams, experiments}`; for v ≥ 6 the client answers with op 16 `{}` (voice backend version) |
| 1 SELECT_PROTOCOL | → | `{protocol: "udp", data: {address, port, mode}, address, port, mode, codecs, rtc_connection_id, experiments}` (yes, `address/port/mode` appear both inside `data` and at top level) |
| 4 SESSION_DESCRIPTION | ← | `{mode, secret_key, audio_codec, video_codec, media_session_id, keyframe_interval, dave_protocol_version, bandwidth_estimation_experiment}` |
| 5 SPEAKING | ↔ | → `{speaking: <bitflags>, delay: 0, ssrc}`; ← `{user_id, ssrc, speaking}` |
| 12 VIDEO | ↔ | → `{audio_ssrc, video_ssrc, rtx_ssrc, streams}` |
| 14 SESSION_UPDATE | → | `{codecs}` when the codec set changes |
| 7 RESUME / 9 RESUMED | ↔ | `{token, session_id, server_id, channel_id, seq_ack}` |
| 11 / 13 | ← | clients connect `{user_ids}` / disconnect `{user_id}` |
| 21–31 DAVE | ↔ | Seen in the bundle: 23 → transition ready `{transition_id}`, 26 → key package (binary), 28 → commit + welcome (binary), 31 → invalid commit/welcome `{transition_id}`. The rest follow Discord's published DAVE protocol: 21 prepare transition, 22 execute, 24 prepare epoch, 25 external sender, 27 proposals, 29 announce commit, 30 welcome. |
| 32 | → | `{}` "no route" |

Codec entries are `{name, type, priority: 1000 × p, payload_type, rtx_payload_type, encode, decode}`; stream entries `{type, rid, ssrc, active, quality, rtx_ssrc, max_bitrate, max_framerate, max_resolution: {type, width, height}}`. **Capture**: the exact codec list, priorities and payload types the native module advertises on macOS arm64 (expected: opus 120; H264 101/102, VP8, VP9, AV1 with RTX), and the `experiments` array.

Close codes the client acts on: 4006/4009 (session gone, rejoin), 4014/4015, **4803** (repeated invalid MLS), **4804** (refused DAVE downgrade), **4805** (invalid DAVE message). Backoff resets after a successful READY.

### 1.3 Media plane (native, so **capture** throughout)

- **UDP**: IP discovery (74-byte request/response), RTP with the SSRC from READY, RTCP sender/receiver reports, plus Discord's RTP header extensions (expected: audio level, abs-send-time / transport-wide CC, playout delay).
- **Transport encryption**: `aead_aes256_gcm_rtpsize` preferred, `aead_xchacha20_poly1305_rtpsize` as the fallback. The 4-byte nonce counter is appended after the payload. We offer modes in the same order as the native module.
- **DAVE (E2EE)**: MLS group key exchange through the voice gateway above, then per-frame AES-128-GCM with an 8-byte truncated tag, ULEB128 nonce, unencrypted ranges for codec headers, and the `0xFAFA` magic marker. Protocol v1. A downgrade to 0 is refused (4804), as the official client does.
- **Audio**: Opus 48 kHz stereo, 20 ms frames, in-band FEC driven by loss feedback, DTX off by default. **Capture**: whether desktop enables DTX and what bitrate it picks for the channel's `bitrate`. Five silence frames (`F8 FF FE`) are sent after speaking stops.

---

## 2. Architecture

```
          UI (SwiftUI)  ──  VoiceModel (@Observable)
                │ intents (join / mute / deafen / video / stream)
   ┌────────────┴─────────────── unison-voice (Rust) ───────────────────────┐
   │ VoiceSession: main-gateway op 4 ↔ voice gateway v9 (tokio-tungstenite) │
   │ UDP socket (tokio), IP discovery, RTP/RTCP, jitter buffer, NACK/RTX    │
   │ transport crypto (aes-gcm via ARMv8 crypto, xchacha20 fallback)        │
   │ DAVE: libdave (C++, MLS via mlspp) behind a small C shim              │
   │ Opus: libopus (static), NEON, FEC/PLC                                  │
   └──────────────▲────────────────────────────────────────────▲────────────┘
                  │ 20 ms PCM (lock-free ring)                  │ NAL units / frames
   ┌──────────────┴──────── media plane (Swift, macOS) ─────────┴────────────┐
   │ AVAudioEngine + voice processing (VPIO): AEC, AGC, NS, Voice Isolation  │
   │ ScreenCaptureKit ─ IOSurface ─ VTCompressionSession (H.264/HEVC/AV1)    │
   │ VTDecompressionSession ─ AVSampleBufferDisplayLayer (+ PiP)             │
   └──────────────────────────────────────────────────────────────────────────┘
```

- **One process.** Discord runs voice in a separate utility process plus the GPU process. We keep everything in-process, with audio on a real-time thread that the Rust side never blocks.
- **The audio thread is Swift/C only.** The VPIO render callback copies 10 ms frames into a lock-free SPSC ring. A Rust worker running at `QOS_CLASS_USER_INTERACTIVE` encodes them in pairs, so the real-time callback never sees an allocation, lock or syscall.
- **`unison-voice` is a new crate.** It reuses `transport.rs` for the WebSocket and the `fidelity` layer for the voice IDENTIFY and codec list, so identity values have one owner. UniFFI exposes a `VoiceController` object plus a `VoiceListener` callback.
- **The intent gate extends to voice.** `join`, `leave`, `set_self_mute`, `set_self_deaf`, `set_video`, `start_stream` and `play_sound` take a `UserIntent`. Speaking and SPEAKING op 5 follow the mic and VAD (that's what any client does) and need no intent. Background reconnects (RESUME) are allowed, because they continue a session the user started.

## 3. Audio

| Piece | Choice | Why |
|---|---|---|
| Capture and playout | `AVAudioEngine` input node with `setVoiceProcessingEnabled(true)` (the VoiceProcessingIO unit underneath) | Apple's AEC is tuned per device and uses the speaker reference properly. It's the same path FaceTime uses. |
| Noise suppression | VPIO's built-in suppression; the system **Voice Isolation / Wide Spectrum** mic modes (Control Center) are honoured. Optional later: a small DeepFilterNet model on the ANE. | Krisp is proprietary. VPIO's NS is nearly free. |
| AGC | VPIO AGC on, with an "automatic gain" toggle like Discord's | |
| Ducking | `voiceProcessingOtherAudioDuckingConfiguration` set to minimal, so other apps aren't muted while you're in a call | Discord's attenuation setting maps onto this. |
| VAD | Opus-independent energy + spectral VAD on the processed signal, with the official sensitivity slider (−100…0 dB), plus Discord's automatic mode | SPEAKING timing must match: the speaking flag goes up on the first voiced frame, and down after the same hang time the official client uses (**capture**). |
| Push-to-talk | Global shortcut via `CGEventTap` (needs Input Monitoring, asked only when PTT is enabled), with the PTT release delay | |
| Codec | libopus 1.5, 48 kHz stereo, 20 ms, `OPUS_APPLICATION_VOIP`, bitrate from the channel's `bitrate` (64 kbps default), in-band FEC with loss-driven `packet_loss_perc` | |
| Jitter buffer | Adaptive per SSRC (NetEQ-style: target 40–120 ms by measured jitter), Opus PLC/FEC for gaps | |
| Mixing | Per-user gain (the user-volume slider, 0–200 %), summed in float, soft-clipped, played through the VPIO output so AEC sees the true reference | |
| Devices | Default device follows the system; explicit choice by UID survives replugs | |

## 4. Video and screen share (M5)

| Piece | Choice |
|---|---|
| Screen capture | `SCContentSharingPicker` (the system picker, no Screen Recording prompt dance), then `SCStream` delivering IOSurface-backed `CVPixelBuffer`s in NV12 at the stream's frame rate, with `capturesAudio` for the app or system audio |
| Camera | `AVCaptureSession` → the same encoder; Center Stage, Portrait and Studio Light come from the system |
| Encode | `VTCompressionSession`, hardware only (`kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder`), `RealTime`, `AllowFrameReordering=false`, constant-QP-capped CBR with `DataRateLimits`, and keyframes on PLI/FIR or on the `keyframe_interval` Discord sends. H.264 is the default; AV1 on M3+ when the server negotiates it; HEVC only if Discord ever offers it. VP8/VP9 fall back to libvpx software encode, used only when a peer can't decode H.264 |
| Simulcast | The streams Discord's op 12 asks for (`rid`, `max_resolution`, `max_bitrate`), each its own VT session at a lower resolution, scaled with `VTPixelTransferSession` on the GPU |
| Packetization | RFC 6184 (H.264, FU-A), RFC 7741 (VP8) and the AV1 RTP spec, with RTX (RFC 4588) and NACK/PLI handling. DAVE encrypts the frame with codec-aware unencrypted ranges |
| Decode | `VTDecompressionSession` → `AVSampleBufferDisplayLayer`. Picture in Picture through `AVPictureInPictureController` with a sample-buffer content source |
| Grid | Each tile is its own display layer. Off-screen tiles are paused, and we tell the server through media-sink wants (op 15 in Discord's protocol; **capture** its shape) so it stops sending them. That's the official behaviour, and it saves bandwidth and decode work |

The CPU never touches a pixel: capture, scaling and encode all stay on the GPU and media engine in IOSurfaces.

## 5. Energy: how we'll beat Discord, and the targets

Where Discord spends energy in a call (to be confirmed with `powermetrics` in S6/S7):
- Separate voice and GPU processes, IPC for every frame, and Chromium's compositor for every video tile.
- WebRTC's software APM (AEC3, NS, AGC2) at about 10 ms granularity, plus Krisp when it's on.
- Software VP8/H.264 paths depending on the build, and pixel copies between capture, encode and the compositor.
- Renderer timers kept awake (`IntensiveWakeUpThrottling` disabled).

What Unison does instead: VPIO (hardware-tuned DSP), the media engine for encode and decode, IOSurface end to end, one process, and timers coalesced onto the 20 ms audio tick.

Targets. Each is measured on the same Mac, account and call, Unison vs. Discord 0.0.42x, via `tools/bench` (sudo `powermetrics --samplers cpu_power,gpu_power,ane_power,tasks`):

| Scenario | Metric | Target |
|---|---|---|
| S6a voice, 4 people, 30 % talking, 1 h | package power above idle | **≤ 40 % of Discord's**, and ≤ 1.5 % of one P-core |
| S6b voice, muted, listening only | package power above idle | **≤ 35 % of Discord's** |
| S6c voice while the window is hidden | wakeups/s | ≤ 60 (the 50 Hz audio tick plus networking), with no UI work |
| S7 share 1080p60 screen, 1 h | package power | **≥ 50 % lower** |
| S8 watch one 1080p60 stream | package power | **≥ 40 % lower** |
| S9 laptop loop with a 1 h call | battery used | **≥ 25 % less** than the same loop on Discord |
| Mouth-to-ear latency | measured with a loopback rig | no worse than Discord ± 10 ms |
| Audio glitches | xruns per hour | 0 at normal load |

A regression of more than 5 % on any of these fails CI, like the other §11 gates.

## 6. Fidelity checklist

1. Voice IDENTIFY, SELECT_PROTOCOL (including the duplicated `address/port/mode`), codec list, `experiments` and the stream descriptors come from one `fidelity::voice` module and are golden-tested against a capture.
2. The heartbeat cadence and `{t, seq_ack}` follow v ≥ 4 semantics; RESUME uses `seq_ack`.
3. Op 16 right after READY; op 14 only when codecs change; op 15 sink wants mirror what is on screen.
4. The encryption-mode offer order and the DAVE `max_dave_protocol_version` match the native module.
5. RTP: SSRCs from READY, the same header-extension IDs and order, Opus TOC and frame sizes, the silence frames, RTCP report intervals, and NACK/PLI behaviour. These get a **pcap diff** against an official call (the REA `inspect_pcap_capture` tool), since they can't be read from JS.
6. The region latency request (`latency.discord.media/rtc`) runs on the official schedule, so a client that joins voice without ever ranking regions doesn't stand out.
7. Telemetry stays off: no `/science` voice-quality events, no `RTC_*` analytics, and no `voice_connection` metrics unless the user enables analytics.
8. Krisp is absent. That shows up only as a missing analytics field, and analytics is already off.

## 7. Risks

- **libdave tracking.** It's C++ with mlspp and BoringSSL/OpenSSL. We vendor a pinned version, build it into the static library, and have the watcher flag changes to `dave_protocol_version`. If interop breaks after a Discord change, the kill switch routes voice to the web fallback.
- **Native-only values.** The codec list, RTP extensions and DTX/VAD timing need captures. Until each one is pinned, every unpinned value is marked `capture` in FIDELITY.md. We never guess on the wire. Native calls are on by default (tested live for voice, video and screen sharing); Settings, Calls can turn them off.
- **macOS audio quirks.** VPIO with Bluetooth headsets switches them to the HFP profile. Like Discord, we show the device's real input and offer "use the Mac's microphone" when AirPods would drop to HFP.
- **Two people sharing in one call.** Several VT sessions per stream at once. Budget: about 4 concurrent hardware encoders on M-series.

## 8. Stages

| Stage | Scope | Exit criteria |
|---|---|---|
| **V1 join, speak, hear** | op 4 join/leave, voice gateway v9 (identify, heartbeat, resume), IP discovery, `aead_aes256_gcm_rtpsize`, Opus encode/decode, VPIO capture and playout, jitter buffer, SPEAKING, mute and deafen, voice channel UI (participants, speaking rings, the control capsule), DAVE | A two-way call with the official client in an E2EE channel; golden tests for every voice-gateway payload; pcap diff with no unexplained differences |
| V2 polish | PTT, VAD sensitivity, per-user volume, device picker, reconnect/region change, soundboard playback, stage listen-only | S6 targets met |
| V3 video and streams (M5) | camera, Go Live (SCK + VT), watching streams, simulcast, PiP, sink wants | S7/S8 targets met |
| V4 | AV1, stage speaking, activities in voice (web bridge) | |

V1 is built in this order, each step testable on its own: (1) voice-gateway client with golden tests and the op 4 handshake; (2) UDP, IP discovery and transport crypto with RFC test vectors; (3) Opus and the RTP packetizer with loopback tests; (4) VPIO capture and playout behind a ring buffer; (5) DAVE with libdave test vectors; (6) UI; (7) a live call against the official client plus the pcap diff.

## 9. Before any code ships: the capture

To fill in every **capture** above, we need one recorded call:
1. The user joins a voice channel in the official desktop client on a second machine or account, with logging enabled (`--enable-logging` is not required; the voice gateway's JSON can be read through Discord's own debug logs or the REA Electron capture tools against the user's own install).
2. A `tcpdump` of the UDP flow (encrypted payloads are fine; we need headers, sizes, timing and RTCP).
3. Values recorded: the op 4 key set, the IDENTIFY/SELECT_PROTOCOL bodies, the codec list, `experiments`, the RTP header extensions, the silence and hang timing, and the latency-ping schedule.

The user runs the capture; Unison does not.

## 9. As built: how video and Go Live are wired (2026-10-30)

Mirrors how the official client separates media; every wire value not read from a capture is marked **capture** in FIDELITY.md.

- **One voice connection per media context.** The call connection (`Role::Call`) carries audio and the camera. Each Go Live stream, whether we publish it or watch it, is its **own voice connection** (`Role::StreamPublisher` / `StreamViewer`) with its own WebSocket, UDP socket, token and SSRCs, opened from the main gateway's `STREAM_CREATE` (`rtc_server_id`, `rtc_channel_id`) + `STREAM_SERVER_UPDATE` (`token`, `endpoint`). It identifies with `server_id = rtc_server_id`. Stream connections carry no microphone or playout, so sharing and watching never touch the call's audio path.
- **Gateway ops**: `start_stream` = op 18, `watch_stream` = op 20, `stop_stream` = op 19, pause = op 22; `STREAM_CREATE/SERVER_UPDATE/UPDATE/DELETE` drive the connections (`session/streams.rs`). Camera = op 4 `self_video` + op 12 on the call connection.
- **One nonce counter.** Audio, video and RTCP on a connection share a single sealing `Transport` (`stream::Media`); two sealers under one key would reuse AEAD nonces.
- **Send**: Swift hardware encoder (VideoToolbox) -> Annex B -> `push_video_frame` -> DAVE (`encrypt_video`, H.264) -> RFC 6184 packetizer -> seal -> UDP, all on the encoder's thread (no extra thread, no async hop). NACKs are answered from the history over RTX; PLI/FIR become a keyframe request back to the encoder; REMB/receiver reports drive the AIMD bitrate target; a new stream asks the encoder for a keyframe at once.
- **Receive** (`recv.rs`, `stream::RemoteStream`, `connection::receive`): UDP -> RTCP (opened with the 8-byte clear header) or RTP -> one decrypt per packet (extension body kept for the transport-wide sequence number and the video orientation, id 4) -> RTP padding stripped (the P bit; padding-only probes are dropped, and pad bytes left in break DAVE's trailer) -> per-SSRC reorder buffer (`recv::Reorder`: holds packets behind a hole, NACKs it every 30 ms up to 6 times, gives up after 160 ms and the depacketizer then asks for a keyframe, PLI at most one per 700 ms) -> depacketizer -> DAVE decrypt -> `on_remote_video` (the FFI attaches the video hooks on every call join, since the call connection takes them when it connects) -> `H264Display` (`AVSampleBufferDisplayLayer`, hardware decode, no CPU copy; turned by the sender's orientation, so a phone's portrait camera shows upright in a portrait tile). RTX copies (payload type 106, mapped to the video SSRC from op 12) fill holes without counting as received, so reports show real loss. Feedback we send for incoming video: a receiver report plus SDES CNAME every second (`recv::RecvStats`: loss fraction, jitter, last SR), transport-cc feedback every 100 ms for packets that carry extension id 5 (`rtcp::TccRecorder`), NACK, PLI. Once a second per remote tile the app logs `video display, last 1 s` (frames handed in, shown, skipped while hidden or waiting for a keyframe, decoder failures) next to `video rx`, so a stutter is placed on the network or the display; a decoder failure waits for a keyframe and asks for one instead of decoding P frames against nothing. A tile in a hidden window or off-screen pauses decode; shown again, it skips P frames and asks the sender for a keyframe (`request_video_keyframe` -> PLI), so the picture returns at once. Only the view that currently hosts a tile's layer reports its visibility, so a layout change (one tile to picture-in-picture, focus) never pauses it.
- **Codecs**: only H.264 is offered for encode/decode (op 1), so every peer picks the one codec we have hardware for. **capture**: whether the server accepts this list as-is, RTP header extensions (none sent), simulcast layers (one layer, rid `100`).
- **Power**: capture runs only while someone could watch (`VideoPower.cap`); with no viewers the encoder floors to 320x180@1 fps so a joining viewer is instant; battery, Low Power Mode and thermal state lower the ladder (event driven, no timers).
- **Not built yet**: stream audio (Go Live with sound), simulcast/quality picker, per-tile op 15 wants (everything is requested at 100 %), PiP, REMB (we send transport-cc and receiver reports instead; **capture**: which of them Discord's SFU actually uses toward a receiver). Unverified live: all of it. It needs a real call with a second client; DevTools cannot join or stream by design.


## 10. Manual test plan: Unison against the official client

Aim: see that a Unison call looks like an official one to the server and works both ways. Use a second Discord account you own (or a friend) on the official desktop client. Nothing needs a developer flag; run Unison normally and turn on debug logging for the voice part only.

Set up
1. In Unison: Settings, Calls is on (it is by default). Quit Unison, then start it from a terminal with `UNISON_LOG=$HOME/unison-call.log UNISON_LOG_FILTER=unison_core::voice=debug build/Unison.app/Contents/MacOS/Unison` (or the installed app's binary). Only voice lines are written, none with tokens or ids. Read them with `tail -f ~/unison-call.log` in a second terminal.
2. Put the two accounts in one voice channel of a server you own, or in a DM. The other side uses the official client.

Steps and what a pass looks like (in `~/unison-call.log`)
1. Audio only. Join from Unison, say a few words on each side. Pass: both hear each other; the log shows `audio rtp extensions in use ... audio-level 1, abs-send-time 3`, then `rtcp sender report sent (audio)` about every 5 seconds once you have spoken. The official side's speaking ring lights when you speak. Fail: no audio either way, a one-way call, or the official side sees you muted.
2. Stop talking for a few seconds, then talk again. Pass: the ring turns off and on again; nothing in the log but the 5 second reports.
3. Camera, Unison to the other side. Turn your camera on in Unison; the other side should see it within a few seconds. Pass: the log shows, in order, `camera toggled ... call_connection=true`, `video start requested`, `video sender created, op 12 sent`, `first video frame packetized and sent`, then `video tx, last 5 s` with `frames_sent` near 150 and `dave_refused=0 no_sender=0`, and `rtcp sender report sent (video)` about once a second. Fail: the first line that is missing is the step that broke. `call_connection=false` means the engine had no call; `dave_refused` above 0 means end-to-end encryption refused our frames (a DAVE problem, not video); `no_sender` above 0 with `frames_sent=0` means frames reach the engine but no sender was created (look for `video sender waits`); no `video tx` line at all means frames never leave the app (camera permission or capture).
3b. Camera, the other side to Unison. The official side turns the camera on. Pass: a steady picture; the log shows `video rx, last 1 s` once a second with `packets` around 100 to 300, `keyframe_requests` at most 1 or 2 at the start, `given_up` near 0, `dave_failed` and `no_sink` at 0 (frames that arrived but could not be decrypted, or had no app sink: the call connection was opened without video hooks) and, on a lossy network, `nacked` and `recovered` above 0 (repairs working). Also `incoming packets carry the transport-wide sequence number; sending transport-cc feedback` once. Fail: a blocky or frozen picture with `given_up` or `keyframe_requests` climbing every second, or `recovered` always 0 while `nacked` is high (the server does not answer NACKs; send me those lines).
4. Screen share. Start sharing from Unison, then have the official side watch. Then the official side shares and Unison watches. Pass: the picture is steady and in sync; the sender reports continue. Fail: the stream starts and stops, or the watcher never gets a first frame (look for a long run of lost keyframes, the viewer pressing for a new picture).
5. Leave and rejoin from Unison, twice. Pass: each join reaches Connected within a few seconds, audio and video return, the log starts a fresh `audio rtp extensions in use` line each time, and no error lines appear. Fail: joining sticks on Connecting, or the call drops after about 10 seconds.
6. Kill the network for 10 seconds on the Unison Mac (Wi-Fi off, then on). Pass: it reconnects by itself (a resume) without you rejoining; at worst you rejoin once.

What to send back if something fails: the terminal lines around the failing step (they hold only counts and extension names) and which step number failed. A server that dislikes our packets usually shows up as a call that connects and then silently drops audio or video, which steps 1 and 3 are written to catch.
