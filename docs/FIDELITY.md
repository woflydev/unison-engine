# Fidelity ledger

Every value Unison sends that identifies it as a client, where that value comes from, and how sure we are.
**Observed** means read from Discord's shipped code: the desktop host 0.0.420 (`app.asar`, Electron 44.9.3, Chrome 152.0.7977.167) and the web client build 655747 (re-read 2026-10-30). **Inferred** means derived from Chromium/Electron behaviour, not read from Discord's code. **Capture** means it needs a live request capture to confirm.

Run `cargo run --release -p unison-watcher -- snapshot tools/watcher/snapshots`, then `watcher diff` against the previous snapshot. Anything marked ⚠︎ below is re-checked automatically.

## Super-properties (`X-Super-Properties`, base64 JSON)

| Key, in order | Value | Status |
|---|---|---|
| `os` | `Mac OS X` | observed |
| `browser` | `Discord Client` | observed |
| `release_channel` | `stable` | observed |
| `client_version` | host version (profile) | observed ⚠︎. Installed Discord and the profile are both **0.0.420** as of 2026-10-30. |
| `os_version` | `os.release()` (Darwin kernel, e.g. `27.2.0`) | observed |
| `os_arch`, `app_arch` | `arm64` | observed |
| `system_locale` | `navigator.languages[0]` | observed |
| `has_client_mods` | `false` (truthful) | observed: the client sets `true` if jQuery, BetterDiscord or Vencord globals exist |
| `client_launch_id` | UUID per launch, per account | observed |
| `browser_user_agent`, `browser_version` | Electron UA / Electron version | observed |
| `client_build_number` | web `BUILD_NUMBER` | observed ⚠︎ |
| `native_build_number` | omitted on macOS | inferred (only set when the host reports one) |
| `client_event_source` | `null` | observed |
| `launch_signature` | `null` | **deliberate, and visible.** The official client computes it at boot with `libdiscore.generateLaunchSignature(globalThis)`, a WASM module that inspects the page's JavaScript environment, and the entry point awaits libdiscore before loading the app. A normal client therefore sends a non-null value; `null` only occurs when the WASM fails to load. We don't forge an environment attestation, so this field distinguishes Unison from a stock client. |
| `client_heartbeat_session_id` | UUID, rotates after 30 min idle | observed key; the rotation interval is **inferred** |
| `client_app_state` | `active` / `inactive` / `background` | observed (`AppStates` enum) |

The watcher's extracted key order matches `fidelity::tests::super_properties_key_order`.

## REST headers

| Header | Status |
|---|---|
| `Authorization`, `X-Super-Properties`, `X-Discord-Locale`, `X-Discord-Timezone` | observed (`ApiClient` interceptor) |
| `X-Debug-Options` | observed; only sent when non-empty (never for normal users) |
| `X-Fingerprint` | observed; only before login (from `/experiments`) |
| `X-Installation-ID` | observed; only when analytics are allowed. **Not sent** (analytics off by default). |
| `Accept-Language` | observed: Discord's own builder (`en-AU,en;q=0.9`) |
| `sec-ch-ua*`, `sec-fetch-*`, `priority`, `Origin`, `Referer` | inferred from Chromium 148 behaviour |
| Header order | inferred; **capture** |

## Gateway

| Item | Value | Status |
|---|---|---|
| URL | `wss://gateway.discord.gg/?encoding=etf&v=9&compress=zstd-stream` | observed (`GLOBAL_ENV`, `discord_erlpack`, `discord_zstd`) |
| IDENTIFY keys | `token, capabilities, properties, presence, compress, client_state, qos_token` | observed ⚠︎ (watcher-extracted, matches `identify_shape` test) |
| `capabilities` | `1734653` (`1767421` with channel obfuscation) | observed ⚠︎ |
| `properties` extras | `client_app_state`, `is_fast_connect: false`, `gateway_connect_reasons` (`COLD_START`, then `NO_REASONS`) | observed |
| `presence` | `{status:"unknown", since:0, activities:[], afk:false}` | inferred; **capture** |
| `qos_token` | protobuf `QosToken{client_provided{is_active}, derived}`, base64; `derived` = READY's `qos_token` | observed schema |
| Heartbeat | first beat at random × interval, then every interval; reconnect if no ACK | standard Discord behaviour |
| Lazy guild subscription | op 37 `{subscriptions:{guild:{…}}}` carrying only what changed: `typing`, `activities`, `threads` once per guild per session, then `channels` (LRU of 5, most recent first, `[[0,99]]`) when a channel's ranges change. Split above 15 360 bytes. Forums subscribe the guild only. | observed (`GuildSubscriptions`, `updateGuildSubscriptions`) |

## Forums

| Request | Shape | Status |
|---|---|---|
| Channel open | `GET /channels/{id}/messages?limit=50`, at once after a pause; opens within 400 ms of the previous one are debounced (core `OPEN_BURST`) | Unison pacing, not a wire change |
| Detached window grows (jumped to an old message, reader nears its end) | `GET /channels/{id}/messages?after={newest}&limit=50`, one at a time, driven by the reader's scroll only | `capture` (same query shape the official client sends when scrolling down a jumped window) |
| Archived posts | `GET /channels/{id}/threads/search?archived=true&sort_by=last_message_time&sort_order=desc&limit=25[&tag=a%2Cb]&tag_setting=match_some\|match_all&offset=N`, once per (sort, tags, setting) view and on scrolling to the end. The wire sort is always last-message time; the chosen order is applied locally. | observed (`loadArchivedThreads`) |
| Post previews | `POST /channels/{id}/post-data {thread_ids:[≤10]}`: the first 10 posts on open, then rows as they appear | observed (`FORUM_POSTS` batcher) |
| New post | `POST /channels/{id}/threads?use_nested_fields=true {name, auto_archive_duration, applied_tags, message:{content}}`; duration = the forum default ?? 4320 | observed (forum post form). Attachments not supported yet. |
| Forum unread counts | op 28 `REQUEST_FORUM_UNREADS {guild_id, channel_id, threads:[{thread_id, ack_message_id}]}` | observed; **not sent yet** (read-only; only for joined posts) |

## Voice, video and screen sharing (on by default; Settings, Calls)

| Item | Value | Status |
|---|---|---|
| Join / leave / mute | op 4 `{guild_id, channel_id, self_mute, self_deaf, self_video:false, flags:0}`; no `preferred_region(s)` (only sent after a latency test, which we don't run yet) | observed (`voiceStateUpdate`, `computeVoiceFlags`). `flags` is bit 1 clips enabled + bit 2 allow voice recording, both opt-in user settings that are off by default (clips are enabled only by a coach-mark click), so 0 is the default; checked in the web bundle 2026-10-30. Every desktop op 4 carries `tracks:[{type:"video",rid:"100",quality:100}]` (from `getVideoStreamParameters()` in the state, not only with the camera on); we now send it too, last in the object (`gateway::voice_state_payload`) |
| Voice gateway URL | `wss://{host}/?v=9`, port 443 dropped | observed (`RTCControlSocket`, `RTCConnection.endpoint`) |
| IDENTIFY | `{server_id, channel_id, user_id, session_id, token, max_dave_protocol_version:1, video:true, streams:[{type,rid:"100",quality:100}]}`; `type` is `video` on the call and `screen` on a Go Live connection | verified in the web bundle (`RTCControlSocket.identify`, `ez`, `getVideoStreamParameters(context)`: `video` = `supports(VIDEO)`, undefined keys dropped). The bundle's 100 for `quality` is the constant `Y4`; the READY fallback stream `{rid:"100", ssrc+1, rtx ssrc+2, quality:100, active:false}` matches ours |
| Heartbeat | server v >= 8: `{t, seq_ack}` with `seq_ack` -1 until a sequenced frame arrived (RESUME the same, `doResume`); before v8 the data is the bare `t`. Interval from HELLO; missed ACK closes 4000 and reconnects with backoff 1 s to 5 s, no more than 3 failures; close codes 4004, 4006, 4011, 4015 stop without retrying | verified in the web bundle (`sendHeartbeat`, `handleClose`, `handleHeartbeatTimeout`). We previously sent `null` for `seq_ack`; fixed |
| Op 16 | `{}` right after READY when the server is v ≥ 6 | observed |
| SELECT_PROTOCOL | `{protocol:"udp", data:{address,port,mode}, address, port, mode, codecs, rtc_connection_id, experiments}`. `codecs`: `opus` (priority 1000, PT 120, no rtx/encode/decode keys), then video at priority 1000 x (i+1), PT 101+2i, RTX one above, in the order AV1, H265, H264, VP8 on a Mac desktop (web bundle `RTCConnection.initialize`, `y`, `L`, `setupMediaEngine`: AV1 appears because the Mac sets `SIGNAL_AV1_DECODE`; H265 always; VP9 only in a denylist-override mode). We send those four at the official numbers (H264 = 105/106) but with `encode`/`decode` false for everything except H264, because we cannot do the others: the one deliberate difference. `experiments` are the client's own picks, not READY's: `fixed_keyframe_interval` (the native module declares that feature in `discord_voice/index.js`) | verified (web bundle + `discord_voice.node` features). **Still capture**: the exact `encode`/`decode` booleans the Mac engine reports (JS: H265 encode only on arm64 with a new enough macOS, decode true, AV1 decode only), and whether the remote-config experiments (`should_analyze_user_voice_volume`, `skip_silent_delay_estimator_ffts`) are on for a given user |
| Speaking | op 5 `{speaking, delay:0, ssrc}`; `speaking` a bit mask (1 voice, 2 soundshare, 4 priority). Desktop sends it only the first time it speaks (and again if the SSRC changes or the priority bit flips), never for each start and stop (`RTCConnection.shouldSendSpeaking`); `delay` is `vadLeading` on web in voice-activity mode and 0 on desktop (`getPacketDelay`). We now do the same (`SpeakingGate`) | verified in the web bundle. VAD defaults: threshold -60 with auto threshold on desktop, Krisp VAD on (activation 0.5), `vadLeading` 5, `vadTrailing` 25, PTT release delay 20 ms (web bundle `MediaEngineStore`; native `CreateDefaultVadSettings` stores 5 and 0x19 and 0.5f). The unit of leading/trailing is inferred as WebRTC's 10 ms frame, so about 50 ms and 250 ms; ours: 13 frames of 20 ms hang, no leading delay. The speaking-flag timing itself is local and has no wire shape beyond op 5 |
| Transport crypto | `aead_aes256_gcm_rtpsize`, else `aead_xchacha20_poly1305_rtpsize` | standard |
| E2EE | DAVE v1 via `davey` 0.1.5 (openmls); key package on the external sender, commit/welcome, transition ready, invalid-commit recovery | protocol observed; interop **needs a live call** |
| RTP | Opus PT 120. Outgoing audio carries one-byte header extensions (profile 0xBEDE) **id 1 `urn:ietf:params:rtp-hdrext:ssrc-audio-level`** (V bit + -dBov) and **id 3 `abs-send-time`**; we now send both (`rtp::audio_header_with_extensions`, extension body sealed with the payload). After speech the official client sends **10** silence packets (`AudioTransport::ShouldSendSilence`: counter < 10), each the previous header and extension block with `F8 FF FE` (`discord::SilencePacket`); we send 10 (was 5) | verified by disassembling `discord_voice.node` (arm64): `discord::media::RtpAudioExtensions` registers 1 audio-level, 3 abs-send-time, 9 `https://discord.com/#rtp-hdrext/2018-07-29/speaker` (received only), 10 sdes:mid, 11 sdes:rtp-stream-id, 12 repaired-rtp-stream-id. That the sender attaches exactly ids 1 and 3 is an inference from libwebrtc's sender (mid and stream ids need an SDP mid, which Discord does not use) |
| RTP video extensions | `RtpVideoExtensions` registers 2 `toffset`, 3 `abs-send-time`, 4 `urn:3gpp:video-orientation`, 5 `draft-holmer-rmcat-transport-wide-cc-extensions-01`, 6 `playout-delay`, 7 `video-content-type`, 8 `video-timing`, 10 sdes:mid, 11 sdes:rtp-stream-id, 12 repaired-rtp-stream-id (the init list says 12 entries; ten decoded). One-byte form (profile 0xBEDE) is what libwebrtc writes for ids up to 14. Every outgoing video and RTX packet of ours now carries **3 abs-send-time and 5 transport-wide sequence** (`rtp::video_extension_block`, counter per sender shared by video and RTX) | verified registration by disassembly; that the sender attaches exactly 3 and 5 (and also 2 toffset, 7 content type on screen share) is an inference from libwebrtc's egress. On receive we read **4 video-orientation** (low two bits x 90 degrees clockwise, kept per stream since libwebrtc sends it per frame, not per packet; mobile portrait cameras need it). **Not attached**: 2, 4, 6, 7, 8 (they need capture timing, rotation, content-type hints or an SDP mid), so **still capture** whether the official client adds toffset and, for screen share, content-type |
| RTCP | The native module sends libwebrtc's RTCP over the voice socket (log strings in `socket_client_adapter.cpp`: SR, RR, SDES, NACK, PLI), plus Discord's own: `SocketClient::SendNoopRTCPPacket(ssrc)` (an RTCP receiver report with no blocks, built with `webrtc::rtcp::ReceiverReport` and sent through `SendPacket`) and RTCP APP packets (`ProcessRttReportApp`, `ProcessReceiverReportApp`, `RtcpAppReceiverReportBlock::UnpackBitrate`). Feedback params in the module: `nack`, `goog-remb`, `transport-cc`, `ccm fir`, `pli`. We now send, while we send media, a **sender report with the CNAME SDES** as one compound packet every 1 s for video and 5 s for audio (`rtcp::sender_compound`, sealed like all RTCP: 8 bytes clear), parse transport-cc feedback and ignore it (counted in the debug log), and answer NACK with RTX and PLI/FIR with a keyframe as before | verified present (strings, symbols). The cadences are libwebrtc's defaults, an inference: the module's own are not decoded. **Still capture and not implemented**: receiver reports and report blocks for streams we receive, NACK we would send for a lost incoming video packet, REMB generation, the noop RR trigger, the RTCP APP payloads, and the CNAME format (random 16 letters and digits as libwebrtc's default) |
| Encryption modes | `discord::encryption::PacketCrypter::GetEncryptionModes` returns, in order: `aead_aes256_gcm_rtpsize` then `aead_aes256_gcm` (both only when `crypto_aead_aes256gcm_is_available`, i.e. AES hardware), then `aead_xchacha20_poly1305_rtpsize`, then `xsalsa20_poly1305_lite_rtpsize`, `_lite`, `_suffix` and plain `xsalsa20_poly1305` (enum 5, 4, 6, 3, 2, 1, 0; names from `ModeToString`, enum order taken from the string table). The JS hands the server's list to the module's `connect`. Ours: `aead_aes256_gcm_rtpsize`, falling back to `aead_xchacha20_poly1305_rtpsize` | verified for the module's own order; the module's final pick from the server's list is not decoded, but ours matches the head of that order on hardware with AES (every Apple silicon Mac). We do not implement the non-rtpsize modes |
| Call bitrates | JS transport options: `fec:true`, `packetLossRate:.3`, voice 64 kbps, call start 600 kbps, call min 16 kbps, call max 10 Mbps; per-stream video default max 3.5 Mbps (9 Mbps cap) | verified in the web bundle. We set Opus FEC on and the initial expected loss to 30 now; our video start/min/max come from the power policy (`VideoPower`), a product choice, not changed |
| Region latency test | `latency.discord.media/rtc?v=2` | **not sent yet** |

## Transport

Default: BoringSSL (`wreq`) with the TLS and HTTP/2 parameters of Chromium 149's profile (Discord 0.0.420 ships Chrome 152; the ClientHello shape is the same). Headers are never taken from the profile: they come from this ledger. `UNISON_TLS=rustls` selects the plain rustls stack. One stack per process; never fall back automatically after a block (two fingerprints on one session is worse than one).

| Item | Value | Status |
|---|---|---|
| REST ClientHello | GREASE, 15 ciphers (hash `8daaf6152771`), shuffled extensions incl. ALPS `44cd` and ECH `fe0d`, ALPN `h2, http/1.1`; JA4 `t13d1516h2_…` | inferred from the profile; checked locally by `transport::hello_tests` |
| WebSocket ClientHello | ALPN `http/1.1` only and **no ALPS** (Chromium adds ALPS only when h2 is offered); JA4 `t13d1515h1_…` | inferred; same tests |
| WebSocket upgrade request | HTTP/1.1, Title-Case names in Chromium's order (`Host, Connection, Pragma, Cache-Control, User-Agent, Upgrade, Origin, Sec-WebSocket-Version, Accept-Encoding, Accept-Language, Sec-WebSocket-Key`); test `websocket_request_is_chromium_ordered_and_title_cased`. The handshake is written by us (`Connection: Upgrade`, `Sec-WebSocket-Extensions` included), not the library; byte-for-byte checked on loopback. No known gaps | first live run (2026-10-31): REST 200, upgrade **403** while this request was all-lowercase and out of order; after the fix: upgrade accepted, session live, REST 200 (manual run, 2026-10-31) |
| Refused upgrade | any non-101 answer stops the gateway (`Error::UpgradeRefused`, fatal close); never retried in a loop | Unison policy |
| HTTP/2 SETTINGS / header order | the profile's, with our header order passed through | inferred; **capture** |

History: an earlier Chrome-impersonating attempt was answered by Cloudflare with "Sorry, you have been blocked" on the gateway upgrade. Cause not established (a WebSocket offering h2 or ALPS, or stock Chrome headers beside our own, are the suspects and are now handled). Confirmed once by a manual run on 2026-10-31 (gateway session established, REST 200); automation may not contact Discord, so any re-test is manual. If it is blocked, set `UNISON_TLS=rustls` and report the response body.

`permessage-deflate; client_max_window_bits` is offered on the gateway upgrade, as Chromium does, and inflated if the server accepts it (`wsraw.rs`, own RFC 6455 client; our sends are uncompressed, which the RFC allows). The rustls fallback does not offer it (tungstenite can't decode it).

## Behaviour

| Behaviour | Unison | Official |
|---|---|---|
| Acks (mark read) | Sent on opening a channel, and on your next scroll, keypress or click after new messages arrive | Sent automatically while the channel is focused |
| Typing | Throttled to one call per 8 s while composing | Same throttle |
| Message send body | `{mobile_network_type:"unknown", content, nonce, tts:false, message_reference?, allowed_mentions?, flags}`. A reply's reference is `{guild_id?, channel_id, message_id}` (no `type`; `guild_id` omitted in DMs). With the reply bar's "@ Off", `allowed_mentions:{parse:["users","roles","everyone"], replied_user:false}`; with "@ On", the key is absent. The toggle shows in servers only and never on your own message (`showMentionToggle`) | observed (web build 2026-10-29, `getSendMessageOptionsForReply`, `MessageQueue.handleSend`); `rest::tests::reply_body_matches_official_client` |
| Emoji favourites and frecency | `GET /users/@me/settings-proto/2` (FrecencyUserSettings), once per session when the reaction picker first opens; later changes come from `USER_SETTINGS_PROTO_UPDATE` (type 2). Read only: we never write frecency back, so reactions added here don't move Discord's "Frequently Used" | Same read via `UserSettingsProtoStore.loadIfUncached(FRECENCY_AND_FAVORITES_SETTINGS)` on picker open; the official client also PATCHes usage back. Observed (web build 2026-10-29) |
| DM mute / unmute (context menu) | `PATCH /users/@me/guilds/@me/settings` `{channel_overrides:{<id>:{muted:true, mute_config:{end_time:null, selected_time_window:-1}}}}`; unmute `{muted:false, mute_config:null}`. Endpoint and body envelope read from the web client (`USER_GUILD_SETTINGS(@me)`, `channel_overrides`); the mute object's exact shape is from the API's known form, **capture** before relying on it. Indefinite only (no duration picker yet) | read + **capture** |
| Start a DM call | op 4 voice state with `guild_id:null, channel_id:<dm>`, then (on `CALL_CREATE`) `POST /channels/{id}/call/ring` `{recipients:null}`; read from the web client's call actions | read |
| DM call state (ongoing / ringing) | Read-only from the gateway: `CALL_CREATE` `{channel_id, message_id, region, ongoing_rings, voice_states}`, `CALL_UPDATE` `{channel_id, message_id, region, ongoing_rings}` (no voice states: participants come from `VOICE_STATE_UPDATE`), `CALL_DELETE` `{channel_id, unavailable}`. `ongoing_rings` is a map keyed by user id (the web client uses `Object.keys`); an older `ringing` list is also accepted. A call is ongoing while someone is in it; we are never auto-joined by a ring | read (web build 2026-10-29) |
| Join an ongoing DM call / Accept | op 4 voice state `guild_id:null, channel_id:<dm>` only; no ring request (the ring is sent only when we start a call) | read |
| Decline / stop ringing | `POST /channels/{id}/call/stop-ringing` `{recipients:[<ids>]}` (`CALL_STOP_RINGING`, `stopRinging(channelId, recipients)`). The caller that builds the decline body is in a lazy chunk not read; we send `[<our id>]`, **capture** to confirm (vs `null`) | endpoint read, body **capture** |
| Friend / block from a user menu | add or accept: `PUT /users/@me/relationships/{id}` `{}`; block: same with `{type:2}`; unfriend, cancel, ignore, unblock: `DELETE` same path | read |
| Ack body | `{token, last_viewed}`, where `last_viewed = ceil((now − Discord epoch) / 1 day)` | observed |
| Create a poll | `POST /channels/{id}/messages` `{mobile_network_type, content:"", nonce, tts:false, flags:0, poll:{question:{text}, answers:[{poll_media:{text, emoji?}}], allow_multiselect, duration (hours: 1, 4, 8, 24, 72, 168, 336), layout_type:1}}`. Limits: question 300, answer 55, 2 to 10 answers (`poll_create.rs`); needs Send Polls in a server | `capture` (public API shape; the web modal's key order and the two-answer minimum are inferred) |
| Voice message | Upload `voice-message.ogg` (mono Opus in Ogg, 32 kbps, up to 5 min) through the normal attachment flow; the message has `content:""`, `flags: 1 << 13` and the attachment entry carries `duration_secs` and `waveform` (base64, at most 256 bytes); needs Send Voice Messages in a server (`voice_note.rs`) | `capture` (public API shape; bitrate and the waveform curve are ours) |
| Voice channel text chat | Same `GET/POST /channels/{voice id}/messages` as any channel; opened from the chat button on a voice row without joining | read from the API; **capture** that the official client sends nothing extra |
| Telemetry (`/science`, `/metrics`, Sentry) | not sent unless enabled in Privacy | sent |
| Captcha | shown to you in Discord's own page; never solved automatically | shown to the user |
| Automation | impossible: every write needs a `UserIntent` created by a UI gesture | — |

## User settings (Settings window)

Read from the web client bundle (`web.*.js`, `discord_protos.discord_users.v1.*`). Implemented in
`crates/unison-core/src/prefs.rs` with byte-level encoder/decoder tests.

| Write | Request | Body |
|---|---|---|
| Preloaded settings (theme, message display, animation, safety, notifications, status, locale) | `PATCH /users/@me/settings-proto/1` | `{settings: base64(partial PreloadedUserSettings)}`. The web client also sends `required_data_version` only for edits made offline; omitted online, as here. Response `{settings}` is the full proto. Sent once per Save Changes (UX.md 5d.34), never per toggle; per-server notification changes are one `PATCH /users/@me/guilds/{id}/settings` per changed server, 600 ms apart. |
| Display name | `PATCH /users/@me` | `{global_name}` (null clears). Web also sends `x-…md5` headers only for avatar changes. |
| Bio, pronouns | `PATCH /users/@me/profile` | `{bio, pronouns}`, only changed keys |
| Per-server notification default | `PATCH /users/@me/guilds/{id}/settings` | `{message_notifications: 0\|1\|2\|3}` |

PreloadedUserSettings fields used: `text_and_images`=6 (`inline_attachment_media`=9, `inline_embed_media`=10,
`gif_auto_play`=11, `render_embeds`=12, `render_reactions`=13, `animate_emoji`=14, `animate_stickers`=15 UInt32Value
(0 always, 1 on interaction, 2 never), `message_display_compact`=17, `convert_emoticons`=21,
`dm_spam_filter_v2`=27 enum (0 default, 1 disabled, 2 non-friends, 3 everyone)); `notifications`=7
(`show_in_app_notifications`=1, `enable_burst_reaction_notifications`=4, `reaction_notifications`=7 enum 0 on/1 DMs/2 off);
`privacy`=8 (`default_guilds_restricted`=4 plain bool, `friend_source_flags`=11 UInt32Value: 2 friends of friends,
4 server members, 8 everyone-else, 14 all); `status`=11 (`status`=1 StringValue, `custom_status`=2 {text=1, emoji_id=2
fixed64, emoji_name=3, expires_at_ms=4 fixed64}, `show_current_game`=3, `status_created_at_ms`=5 Int64Value);
`localization`=12 (`locale`=1 StringValue); `appearance`=13 (`theme`=1 enum 1 dark/2 light, `timestamp_hour_cycle`=9
enum 0 auto/1 12h/2 24h). Wrappers follow protobuf-ts: a zero value is an empty wrapper message.
Setting a status writes `status`, `status_created_at_ms` (now) and, as before, the gateway presence update.

More `PreloadedUserSettings` fields (read from the web bundle's protobuf-ts descriptors, 2026-11-01; the enum meanings of `explicit_content_filter` are `capture`): `voice_and_video`=5 (`afk_timeout`=6 UInt32Value seconds, `stream_notifications_enabled`=7 BoolValue); `text_and_images`=6 (`explicit_content_filter`=19 UInt32Value: 0 never, 1 non-friends, 2 everyone; `view_nsfw_guilds`=20, `view_nsfw_commands`=23); `privacy`=8 (`allow_activity_party_privacy_friends`=1, `allow_activity_party_privacy_voice_channel`=2, `inappropriate_conversation_warnings`=23, `recent_games_enabled`=24, `show_local_time`=31, all BoolValue). Defaults when absent follow the bundle (`recent_games_enabled`, `stream_notifications_enabled`, `inappropriate_conversation_warnings` on).

Local only, as in the official client (never synced): input/output device and volume, input mode, echo
cancellation, noise suppression, AGC, desktop-notification toggle. Stored under `voice.*` and `desktopNotifications`.

Not written (read-only with "change on Discord"): username, email, phone, password, 2FA, billing, avatar, banner,
accent colour, per-server DM overrides (`restricted_guild_ids`), explicit-content filter.

Unverified against a live capture: clearing a custom status is encoded as an empty `CustomStatus` message
(`0x5a 0x02 0x12 0x00`); the avatar upload path (needs the `buildHeadersForMd5` header) is deliberately not implemented.

## Composer autocomplete, emoji, stickers, GIFs

Verified in the web bundle (`web.*.js`, `vnd-emoji.*.js`):
- Member search: gateway op 8 `REQUEST_GUILD_MEMBERS` `{guild_id: [id], query, limit: 10, presences: true}`; reply `GUILD_MEMBERS_CHUNK` is absorbed into the member store. Sent only when the guild has more members than we hold, debounced 250 ms, once per (guild, query).
- GIF picker: `GET /gifs/trending`, `/gifs/trending-gifs`, `/gifs/search` with `media_format=webm`, `locale`, `limit`; `POST /gifs/select {id, q}` on pick. `locale=en-US` is hardcoded (capture: should follow client locale). A GIF is sent as its `url` in the message content.
- Sticker send: `POST /channels/{c}/messages` with `sticker_ids: [id]`, `content: ""`, `flags: 0`.
- Emoji catalog (categories, aliases, skin tones) from `vnd-emoji` via `tools/emoji/catalog.py`.
- Insertion: `<@id>`, `<@&id>`, `<#id>`, `<:name:id>`, `<a:name:id>`, `@everyone`/`@here` literal; on send `:name:` expands to unicode or a usable custom emoji (outside code spans).

Marked `capture` (behaviour, not a readable regex): trigger rules (`@`/`#` after whitespace, `@` query up to 32 chars with spaces, `:` needs 2+ chars, 10 rows); ranking (exact > prefix > word prefix > substring, then recent speakers, users < roles < everyone/here); premium rules (other-server and animated emoji need `premium_type > 0`, hidden in autocomplete and locked in the picker; @everyone/@here need MENTION_EVERYONE); skin tone kept locally; no standard sticker packs, favourite GIFs or Lottie stickers; GIF tiles animate on hover only.

## Video and Go Live (M5)

Read from the web client bundle (`discord.com/app`, `web.*.js`); implemented in `crates/unison-core/src/voice/video.rs`.

- Voice gateway op 12: `{audio_ssrc, video_ssrc, rtx_ssrc, streams:[{type, rid, ssrc, active, quality, rtx_ssrc, max_bitrate, max_framerate, max_resolution:{type,width,height}}]}`. Undefined fields are omitted. Stream `type` is `video` (camera) or `screen`; stopping video sends zero SSRCs and no streams.
- Voice gateway op 15 (media sink wants) is only sent when the voice server version is at least 5; payload is the wants object (`{any: n, "<ssrc>": n}`). **capture**: whether other keys accompany `any`.
- Main gateway opcodes: 18 STREAM_CREATE `{type, guild_id, channel_id, preferred_region}`, 19 STREAM_DELETE `{stream_key}`, 20 STREAM_WATCH `{stream_key}`, 21 STREAM_PING `{stream_key}`, 22 STREAM_SET_PAUSED `{stream_key, paused}`.
- Dispatches: STREAM_CREATE `{stream_key, region, viewer_ids, rtc_server_id, rtc_channel_id, paused}`, STREAM_SERVER_UPDATE `{stream_key, endpoint, token}`, STREAM_UPDATE `{stream_key, region, viewer_ids, paused}`, STREAM_DELETE `{stream_key, unavailable, reason}`.
- **capture**: stream key text (`guild:{guild}:{channel}:{owner}`, `call:{channel}:{owner}`), the `type` strings for create (`guild`/`call`), the exact simulcast `rid`/`quality`/`max_*` values for Go Live, the transport-cc and REMB header extensions in the negotiated list, and whether the native client uses H.264 for screen share by default.
- Unison choices that need no wire match: the encoder ladder and power policy in `video.rs`, NACK history size, one-keyframe-per-second PLI/FIR gate.
- Not yet wired: the voice connection does not yet open video SSRCs, seal video packets or send op 12/15, and no gateway client method sends ops 18 to 22. `VideoSender` and the payload builders are the seams.

## To confirm with a live capture

1. REST header order on an authenticated request.
2. The IDENTIFY `presence` object.
3. The pins endpoint response shape (`{items:[{message, pinned_at}]}` vs. an array; both are handled).

## Password sign-in (Package D)

Read from web build 652418 (`LoginActionCreators.login`, `loginMFAv2`, the captcha retry interceptor). Implemented in `crates/unison-core/src/rest/auth.rs` (response classification unit-tested).

| Step | Request | Status |
|---|---|---|
| Pre-login | `GET /experiments?with_guild_experiments=true` (fingerprint + cookies), then `X-Fingerprint` on the login POST | observed |
| Login | `POST /auth/login {login, password, undelete, login_source, gift_code_sku_id}`; we send `undelete:false`, the other two `null` | keys observed, values **capture** |
| Response | `{token}` or `{mfa:true, ticket, sms, totp, backup, webauthn, login_instance_id}` | observed |
| MFA | `POST /auth/mfa/{totp\|sms\|backup\|webauthn} {code, ticket, login_source, gift_code_sku_id, login_instance_id}` | observed |
| SMS code | `POST /auth/mfa/sms/send {ticket}` (endpoint observed, call site in an unread chunk) | body **capture** |
| Captcha | 400 with `captcha_key, captcha_sitekey, captcha_service (hcaptcha), captcha_rqdata, captcha_rqtoken, captcha_session_id`; retry carries `X-Captcha-Key`, `X-Captcha-Rqtoken`, `X-Captcha-Session-Id`. hCaptcha runs in a non-persistent WKWebView with `discord.com` as base URL; `rqdata` goes to `hcaptcha.setData` | headers observed; embedding and `setData` use **capture** |
| New location | error code string `ACCOUNT_LOGIN_VERIFICATION_EMAIL` under `errors.login._errors`; the user approves via the emailed `/authorize-ip` link, then retries | endpoint observed, error shape **capture** |
| Other codes | 70007 phone verification, 20011 scheduled for deletion, 20013 disabled, `suspended_user_token`: shown as messages, resolved on discord.com | observed |
| WebAuthn | not run natively; the user picks another method or uses QR | deliberate gap |

The password is held in memory only for the retry and cleared on success or cancel; never logged.

## Updates and diagnostics (Package D)

Unison's own update channel (not a Discord request): a signed appcast (Ed25519 over exact JSON bytes, like signed profiles) naming a DMG and SHA-256, verified in `unison_core::update`. Requests only on "Check for Updates" or, if opted in, once a day at launch; "Update and Restart" then downloads the DMG from the signed URL (no cookies, ephemeral session) and installs it only after a click (`docs/RELEASING.md`). Crash reports use MetricKit, stored locally, opt-in, exported by the user. Logs: `os_log` subsystem `app.unison.client`, no secrets or content. No telemetry.

## Voice engine settings (Package D)

Local only. Volumes, input mode, sensitivity, push-to-talk and per-user volume apply live; device and processing apply from the next join. Apple exposes no separate echo/noise switches, so VoiceProcessingIO is bypassed only when both are off; AGC maps to `kAUVoiceIOProperty_VoiceProcessingEnableAGC`. VPIO drives mic and speakers as one device. Auto sensitivity = tracked noise floor + 12 dB (clamped -60..-30 dBFS), 300 ms hang. Official thresholds/hang remain **capture**.

## Navigation, notifications and social (web build 2026-10-30)

| Action | Request | Status |
|---|---|---|
| QR sign-in ticket exchange | `POST /users/@me/remote-auth/login` `{ticket}` -> `{encrypted_token}`; may answer 400 with `captcha_key`/`captcha_sitekey`/`captcha_rqdata`/`captcha_rqtoken`/`captcha_session_id`, retried with `X-Captcha-Key`, `X-Captcha-Rqtoken`, `X-Captcha-Session-Id` as the password login is | **capture**: the retry is inferred from the web client's generic captcha handling; not seen in the cached bundle |
| Server Overview save | one `PATCH /guilds/{id}` `{name?, description?, icon?, banner?}` (`icon`/`banner` data URI or null) | documented API |
| Mark server/category/folder read | `POST /read-states/ack-bulk` (a folder is one list across its servers, not a request per server) `{read_states:[{channel_id,message_id,read_state_type:0}]}`, max 100 per request, 1 s apart (`BULK_ACK`) | observed |
| Mark channel read (menu) | `POST /channels/{c}/messages/{m}/ack` `{manual:true,mention_count:0}` | observed earlier; capture |
| Mentions inbox | `GET /users/@me/mentions?limit=25&before&guild_id&roles=true&everyone=true` (`MENTIONS`, `fetchRecentMentions`) | observed; default flags capture |
| Guild mute / level | `PATCH /users/@me/guilds/{g}/settings` `{muted, mute_config:{end_time,selected_time_window}}`, `{message_notifications}`, `{suppress_everyone}` | shape observed; window values 900/3600/10800/28800/86400/-1 capture |
| Channel/category override | same PATCH with `channel_overrides:{id:{muted,mute_config,message_notifications}}`; levels 0 all, 1 mentions, 2 none, 3 inherit | observed |
| Leave server | `DELETE /users/@me/guilds/{g}` `{lurking:false}` | observed (`GUILD_LEAVE`); body capture |
| Search | `GET /guilds/{g}/messages/search` or `/channels/{c}/messages/search`; keys author_id, channel_id, mentions, has, pinned, min_id, max_id, content, sort_by=timestamp, sort_order, offset; before/after/during map to max_id/min_id | keys observed; `include_nsfw` and `limit` not sent, capture |
| Add friend by username | `POST /users/@me/relationships` `{username, discriminator:null}` | capture |
| Group DM | `POST /users/@me/channels` `{recipients:[...]}`; `PUT/DELETE /channels/{c}/recipients/{u}` | capture |

Not implemented: channel invites, mute expiry tracking in the store (a timed mute shows as muted until the next settings sync), group DM member UI.

## Unjoined threads (a mention, search hit, saved item or link into a thread this account is not in)
Read from the web client bundle in `/tmp/bund` (build `web.ab385e8d0b568060.js`, the 2026-10-30 download), statically; nothing was run or requested. Names are the minified module names with what they do. The desktop host (`/Applications/Discord.app`, `app.asar`) only wraps the web client, so the logic is all in `web.*.js`; lazy chunks that were not in the download (the chat composer's thread bar) could not be read.

What the official client does, in call order:
1. **Route resolve** (`ensureChannelMatchesGuild`, router rewriter for `/channels/{guild}/{channel}[/{thread}]`): when connected and the id is a real snowflake (not a pseudo id, `jq`), and `ChannelStore.getChannel(id)` is empty, it calls `ThreadLoader.loadThread(id)` and then re-checks the route. If the channel is still unknown it redirects to `/channels/{guild}` (the server's default channel); if the channel is known but its `guild_id` differs from the route's, it redirects to the route's server. A known thread under the route's channel segment opens in the side panel (`closeChannelSidebar` when the parent differs). The same `loadThread` runs before voice joins (`await L.A.loadThread`).
2. **`ThreadLoader.loadThread(id)`** (module 353202): skipped when `id` is empty, a placeholder, already in the channel store, or the gateway is not connected. State per id (`LOADING` promise / `LOADED` / `NOT_FOUND`) is kept in memory and cleared on `CONNECTION_OPEN`, so the same id is requested once per connection. Request: `GET /channels/{id}` (`Endpoints.CHANNEL`), no query, no extra headers beyond the usual ones. Success: `LOADED`; only if the body's `type` is a thread type (`Le`: announcement, public, private thread) it dispatches `THREAD_CREATE {channel, messageId}` (the route's message id), which puts the thread in the channel and active-thread stores like a gateway event. Any error (403, 404, anything): `NOT_FOUND` and a local `CHANNEL_DELETE {id, guild_id from the route}` dispatch. There is also `loadThreadsBulk(ids)`: `POST /threads/bulk {thread_ids}` (`Endpoints.THREADS_BULK`), response `{items: [channel]}`, ids missing from `items` become `NOT_FOUND`; used for lists of many ids, not for a single open.
3. **Permissions of a thread** (`PermissionStore`, `x(thread, parentPerms, joined, guest)`): a private thread you have not joined has no permissions at all unless you hold Manage Threads on the parent (so it cannot be viewed); media threads need only Read History and View; otherwise sending needs Send Messages in Threads on the parent, and a locked thread also needs Manage Threads. The thread's own overwrites do not exist; it inherits the parent's.
4. **Messages**: the normal channel load, `GET /channels/{id}/messages?limit=50` (or `around=` for a jump). On failure it dispatches `LOAD_MESSAGES_FAILURE`, which marks the channel's message state `error` (no redirect, no retry).
5. **Gateway** (`updateGuildSubscriptions`, function `F(guildId, channelId, ranges)`): for a selected thread it calls `subscribeToGuild` (typing, activities, threads: once per guild per session, already sent) and then, for an *active* thread, `subscribeThreadMemberList(guild, thread)` (op 37 `thread_member_lists`); an announcement thread subscribes its parent's channel ranges; an archived thread subscribes nothing; no thread gets its own `channels` range. Joinedness is not checked. No thread-members request is made on opening.
6. **Unread and ack** (`ReadState`): `canTrackUnreads` is false for an archived thread; `canBeUnread` is false for a thread that is not joined (`_isJoinedThread`, from `JoinedThreadsStore.hasJoined` / the `member` of the channel body). So viewing an unjoined thread never makes it unread.
7. **Joining** (`ThreadActionCreators.joinThread(thread, location)`): `POST /channels/{id}/thread-members/@me?location=<analytics location>` (`THREAD_MEMBER(id)`, default member `@me`), no body. Called by Join Voice (`"Join Voice"`, after `unarchiveThreadIfNecessary`), by changing a thread's notification settings (`"Change Notification Settings"`, then `PATCH /channels/{id}/thread-members/@me/settings`) and by the Join button. Forum posts flip a local `THREAD_MEMBER_LOCAL_UPDATE` first and undo it on failure. Errors: code `TOO_MANY_THREAD_MEMBERS` shows a modal, anything else a generic error modal; the gateway answers with `THREAD_MEMBERS_UPDATE` / `THREAD_MEMBER_UPDATE`. Leaving is `DELETE` on the same path.

What Unison does (implemented):
- `Session::preview_thread` (`unison-core/src/session.rs`) for a channel id the store does not know: after a real gesture only, `GET /channels/{id}` once (`Rest::channel`), then `Store::admit_fetched_thread`, the equivalent of the `THREAD_CREATE` dispatch: it needs a thread type, the server and the parent channel loaded and the thread viewable (rule 3 above), and it keeps a thread member the gateway already gave us. Reads are de-duplicated (`reads` key `thread:{id}`, no second request while one is in flight) and a 403/404/not-a-thread/not-viewable answer is remembered for `MISS_TTL` (10 minutes, shorter than the official "until reconnect" because a person may be added to a private thread meanwhile). Nothing is joined, acked or written; no `thread-members` request. DevTools (no gesture) never fetches, and the automation switch blocks REST anyway.
- The messages load is the existing `open_channel` (`GET /channels/{id}/messages?limit=50`, or `around=`); the thread has no read state, so nothing is acked. The gateway subscription for a thread we are not in is the guild only, like the official client for a forum; a joined thread keeps the existing channel-range subscription (the official client sends `thread_member_lists` for it instead: left as is, see capture).
- Join: the shell's "Join thread" button calls the existing `POST /channels/{id}/thread-members/@me` (`Session::thread_membership`) with a click gesture; the local member flips when it succeeds.
- Thread permissions follow item 3 (`thread_permissions` in `store.rs`): `can_send` in a thread now needs Send Messages in Threads, and a locked thread needs Manage Threads.
- Errors are one plain sentence (`Error::user_message`), shown in the banner; "can't see it" for 403 and a private thread, "isn't available" for 404.

capture (not confirmable from the code that was available; the conservative variant is implemented):
- The Join button's own `?location=` value and its exact placement/wording (the thread-bar chunk was not in the download). We send the join without `location`, as before; the join-from-Threads-popout path is the same.
- Whether the server auto-joins a user who posts in a thread they have not joined (we rely on the gateway `THREAD_MEMBERS_UPDATE` either way; the bar disappears when it arrives), and whether the official client offers Join for an archived thread (joining one needs it unarchived; Join Voice unarchives first). Unison shows "This thread is archived." and no Join button.
- Whether the official client sends `thread_member_lists` for a selected active thread in our selected channel context (`F` passes the selected channel id as the third argument); we do not send it.
- A 403 versus 404 distinction in the official client: it treats every error the same (`NOT_FOUND`). We show two sentences but make the same single request.
- Rate-limit behaviour of `GET /channels/{id}`: handled by the shared REST layer; nothing special in the official code.

## Interactions, polls, threads (Package C)

Verified in the web bundle (`web.*.js`, 2026-10-30):
- `POST /interactions` (`Rsh.INTERACTIONS`). Component click body: `{type:3, nonce, guild_id, channel_id, message_flags, message_id, application_id, session_id, data:{component_type, custom_id, [type, values]}}`; nonce is a snowflake from `Date.now()`; selects add `type` and `values`; `guild_id` omitted in DMs. Command body: `{type:2, application_id, guild_id, channel_id, session_id, data, nonce, analytics_location, section_name, source}`. Autocomplete: `{type:4, application_id, guild_id, channel_id, session_id, data, nonce}`.
- Poll answers path: `/channels/{c}/polls/{m}/answers/@me`; expire `/polls/{m}/expire`. The web client uses PUT here (there is no POST/DELETE vote in the path table). capture: body `{answer_ids:[..]}` (the vote code sits in a lazy chunk we did not fetch).
- Threads: join `POST /channels/{t}/thread-members/@me`, leave `DELETE` same path (both with `?location=`, capture: value); create `POST /channels/{c}/messages/{m}/threads` or `/channels/{c}/threads` with `{name, type, auto_archive_duration, location}` (capture: location); archived list `GET /channels/{c}/threads/search?archived=true&sort_by=last_message_time&sort_order=desc&limit=25&tag_setting=..&offset=` (reused from the forum code).
- Gateway: `MESSAGE_POLL_VOTE_ADD/REMOVE`, `THREAD_MEMBERS_UPDATE`, `INTERACTION_MODAL_CREATE` handled in `store.rs`.

capture (guesses, not verified): modal submit body (`type:5`, `data:{id, custom_id, components}`), `analytics_location`/`section_name`/`source` on commands, the application id's location in `INTERACTION_MODAL_CREATE`, the `?location=` and `location` values above. Embed `video`/GIFV plays from `video.proxy_url` (fallback `url`).

## Presence, message requests, events, stages, moderation (2026-10-30)

Gateway (read): `PRESENCE_UPDATE` and the presence lists in READY / READY_SUPPLEMENTAL carry `activities[]` (`type` 0 playing, 1 streaming, 2 listening, 3 watching, 4 custom, 5 competing; custom status uses `state` + `emoji{id,name,animated}`); `GUILD_SCHEDULED_EVENT_CREATE/UPDATE/DELETE/USER_ADD/USER_REMOVE`; `STAGE_INSTANCE_CREATE/UPDATE/DELETE`; `guild_scheduled_events` and `stage_instances` inside `GUILD_CREATE`; DM channels carry `is_message_request` / `is_spam`.

| Action | Request | Status |
|---|---|---|
| Accept message request | `PUT /channels/{c}/recipients/@me` (no body); Ignore = the normal close (`DELETE /channels/{c}?silent=false`) | **capture** |
| Event interest | `PUT` / `DELETE /guilds/{g}/scheduled-events/{e}/users/@me` | **capture** (initial "interested" state is not in the payload, so it starts unknown until a USER_ADD/REMOVE for us) |
| Raise / lower hand | `PATCH /guilds/{g}/voice-states/@me` `{channel_id, request_to_speak_timestamp: iso\|null}` | documented API |
| Invite to speak / move to audience | `PATCH /guilds/{g}/voice-states/{u}` `{channel_id, suppress}` | documented API |
| Kick | `DELETE /guilds/{g}/members/{u}` | documented |
| Ban / unban | `PUT /guilds/{g}/bans/{u}` `{delete_message_seconds}` / `DELETE` same | documented; official-client extras (reason header, analytics) not sent, capture |
| Timeout | `PATCH /guilds/{g}/members/{u}` `{communication_disabled_until: iso\|null}` | documented |
| Nickname | `PATCH /guilds/{g}/members/{u\|@me}` `{nick}` | documented |
| Lists | `GET /guilds/{g}/bans?limit=200`, `/audit-logs?limit=50`, `/invites`; revoke `DELETE /invites/{code}` | documented; limits capture |
| Edit server | `PATCH /guilds/{g}` `{name?, description?}` | documented |
| Custom status | settings-proto field 11.2 (`prefs.rs`), expiry computed in the core | observed |

Not sent anywhere yet: `X-Audit-Log-Reason` (kick/ban/timeout have no reason field).

## Server setup (screening and onboarding) — all `capture`
- `GET /guilds/{id}/member-verification?with_guild=false` -> `{version, description, form_fields:[{field_type: TERMS|TEXT_INPUT|PARAGRAPH|MULTIPLE_CHOICE, label, values, required}]}`.
- `PUT /guilds/{id}/requests/@me` with the form echoed back, each field carrying `response` (`true` for TERMS, text, or the chosen index).
- `GET /guilds/{id}/onboarding` -> `{enabled, prompts:[{id, title, single_select, required, in_onboarding, options:[{id, title, description, emoji:{id,name}}]}]}`.
- `POST /guilds/{id}/onboarding-responses` `{onboarding_responses:[option ids], onboarding_prompts_seen:{prompt:ms}, onboarding_responses_seen:{option:ms}}`.
- Member `pending` (GUILD_CREATE / GUILD_MEMBER_UPDATE) and flags bit 1 (completed onboarding), bit 2 (bypasses verification) decide when the flow is offered.

## Friend nicknames and attachment details (read from web build 655747, 2026-10-30)
| Feature | Request | Status |
|---|---|---|
| Friend nickname | `PATCH /users/@me/relationships/{id}` body `{nickname}` (`updateRelationship`) | observed. `capture`: what the client sends to clear it (we send `null`). |
| Where a friend nickname shows | DMs and group DMs only (message author, DM list, DM member list, friends list, mentions in a DM); in a server the server nickname wins and the friend nickname is not used (`nick: friendNickname ?? name` only when the channel has no guild) | observed; `Store::display_name` follows it |
| Attachment entry in the send body | `{id, description?, filename, uploaded_filename, is_spoiler?: true, ...}` in that key order | observed; alt text is `description` (only when set), a spoiler is `is_spoiler: true` and the file name is **not** renamed |
Everything else added with these (link cleaning, `@everyone` confirm, keywords, quiet hours, export, location stripping, code collapsing, attachment drag-out) is local and sends nothing new.

## Message and upload limits (read from web build 655747, 2026-10-30)
| Rule | Value | Notes |
|---|---|---|
| Message length | 2000 UTF-16 units; 4000 with the increased-length perk | the perk is in the `TIER_2` (Nitro, `premium_type` 2) feature list only; Basic (3) and Classic (1) stay at 2000. Length is `textValue.length`. |
| Length counter | shown once the remainder is ≤ limit/10 (or over) | `showRemainingCharsAfterCount ?? max/10` |
| Files per message | 10 | `getMaxAttachmentsCount` |
| Total file bytes per message | 500 MiB (an experiment lifts it to 1 GiB; not assumed) | `getMaxTotalAttachmentSize`; sending over it is refused locally |
| Empty file | refused by the client (`ENTITY_EMPTY`) | refused locally |
| Per-file limit | 10 MiB free, 50 MiB (Basic/Classic or boost 2), 100 MiB (boost 3), 500 MiB Nitro | matches `upload_policy`; `capture` for a live 413 body |
Unison never splits an overlong message: "trim" sends the part that fits and leaves the rest in the composer.

## Events, channels and permissions (2026-10-31, all documented API shapes; extras of the official client are `capture`)
| Action | Request | Notes |
|---|---|---|
| Create / edit event | `POST` / `PATCH /guilds/{g}/scheduled-events[/{id}]` `{name, description, privacy_level:2, entity_type, channel_id, entity_metadata:{location}, scheduled_start_time, scheduled_end_time, image?}` | rules in `guild_events::EventDraft`; `recurrence_rule` and analytics fields not sent (**capture**) |
| Start / end / cancel | `PATCH .../scheduled-events/{id}` `{status}` (2 / 3 / 4) | transitions checked by `next_status` |
| Delete event | `DELETE .../scheduled-events/{id}` | |
| Create channel | `POST /guilds/{g}/channels` `{name, type, parent_id?, topic?}` | **capture** for official extras |
| Reorder channels / roles | `PATCH /guilds/{g}/channels` / `/roles` `[{id, position}]` | one step at a time; the core renumbers siblings by index (**capture**: official client may send `parent_id`/`lock_permissions`) |
| Channel permission overwrite | `PUT /channels/{c}/permissions/{id}` `{id, type:0|1, allow, deny}`, `DELETE` same path | bits I don't hold are kept as they were |

## Video receive feedback (2026-11-01)
What Unison sends for video it receives, modelled on libwebrtc defaults (not read from a capture; **capture** to confirm Discord's SFU uses it): receiver report with SDES CNAME every 1 s per video SSRC; transport-wide congestion control feedback every 100 ms when incoming packets carry header extension id 5; generic NACK (30 ms retry, 6 tries) answered over RTX; PLI when a hole is given up after 160 ms or no keyframe has arrived (at most one per 700 ms). No REMB. Sender SSRC in all of them is the connection's audio SSRC.

## Media (images, avatars, emoji)
Pictures on Discord's own hosts (`*.discordapp.com`, `*.discordapp.net`, `*.discord.com`, `discord.media`; `rest::media::is_media_url`) are fetched by the core (`Rest::fetch_media`, FFI `fetch_media`) on the same transport and client identity as REST, as the web client's `<img>` does: no token, no cookies, `Referer: https://discord.com/`, `sec-fetch-site: cross-site`, `sec-fetch-mode: no-cors`, `sec-fetch-dest: image`, `accept: image/avif,image/webp,...`, `priority: i`, Chromium header order from `ClientIdentity::media_headers` (`capture`: order and `priority` value inferred). Redirects only to other Discord media hosts. Other hosts (GIF providers, websites) never receive Discord headers or a Discord Referer; the shell loads them with its own stack. Not yet routed: video playback (AVPlayer) and drag/save/lightbox downloads, which still use the system stack.
Dedupe and freshness of all reads: ARCHITECTURE rule 10.


## Profile cosmetics (2026-11-01, `capture`)
Parsed in `unison-core/src/cosmetics.rs` from the user object (message authors, member list users, `GET /users/{id}/profile` `user`). Nothing is requested for them; every image is static.
| Cosmetic | Field | Image |
|---|---|---|
| Avatar decoration | `avatar_decoration_data.asset` | `cdn.discordapp.com/avatar-decoration-presets/{asset}.png?size=96&passthrough=false` (still frame) |
| Server tag | `primary_guild {identity_guild_id, identity_enabled, tag, badge}` | `cdn.discordapp.com/guild-tag-badges/{guild}/{badge}.png?size=64` |
| Nameplate | `collectibles.nameplate.asset` | `cdn.discordapp.com/assets/collectibles/{asset}static.png` |
| Display name style | `display_name_styles {font_id, effect_id, colors[]}` | colours only (gradient across the name); fonts and effects (neon, toon, pop, glow) are not drawn |
| Role icon | role `icon` | `cdn.discordapp.com/role-icons/{role}/{icon}.webp?size=40&quality=lossless` |
| Profile theme | profile `theme_colors` [primary, accent] (guild member profile wins) | banner gradient when there is no banner |
Not drawn: profile effects (animated layers from `user-profile-effects`), animated decorations.

## Server tag preview
The card a server tag opens (`unison_core::server_card`) is built from the store only: name, icon, banner (`banners/{guild}/{hash}`), description and member count for a server the account is in; the tag alone for one it is not in. The official client fetches a preview for servers you are not in; **capture** the request (fields, when it is made) before adding it. Web bundle (cached, 2026-11-02): the endpoint table has `GUILD_PREVIEW` = `GET /guilds/{id}/preview`; the call site that the tag card uses was not in the cached chunks, so whether it is that endpoint is unconfirmed. Nothing is requested today.

## Call idle disconnect
The official client leaves on its own (web bundle `CallIdleManager`, 2026-11-02): in a 1:1 DM call (private channel, at most one recipient) where you are the only voice state and no embedded activity is running, a 180 000 ms timer starts on every voice-state update (stopped when someone joins, the call closes or the connection closes). When it fires it posts a local bot notice (string `XYof5G`, `{number: 3}`, minutes) and leaves the call. Servers and group DMs never do this. An experiment (`2026-10-camp-in-dm`, variation 1) disables it. Unison does the same (`Session::watch_call_idle`, `Store::alone_in_dm_call`, `CALL_IDLE_LEAVE`): one timer, not restarted by later updates, and it reports "You were alone in the call for 3 minutes" through the error banner (the official notice is a local bot message; the exact wording is not captured). It is the one case where the core hangs up without a gesture.
