use serde_json::{Value, json};

use super::*;

fn term(v: Value) -> Vec<u8> {
    // Strip the version byte: dispatch data is a sub-term.
    unison_etf::encode_value(&v)[1..].to_vec()
}

fn ready() -> Value {
    json!({
        "v": 9,
        "session_id": "s",
        "user": {"id": "1", "username": "me"},
        "users": [{"id": "2", "username": "alice", "global_name": "Alice"}],
        "guilds": [{
            "id": "100",
            "properties": {"name": "Guild", "owner_id": "2"},
            "joined_at": "2024-01-01T00:00:00Z",
            "roles": [
                {"id": "100", "name": "@everyone", "permissions": (1u64 << 10 | 1 << 11).to_string(), "position": 0},
                {"id": "300", "name": "mod", "permissions": "0", "position": 1, "color": 0xff0000}
            ],
            "channels": [
                {"id": "10", "type": 4, "name": "TEXT", "position": 0},
                {"id": "12", "type": 2, "name": "voice", "position": 0, "parent_id": "10"},
                {"id": "11", "type": 0, "name": "general", "position": 1, "parent_id": "10", "last_message_id": "500"},
                {"id": "13", "type": 0, "name": "secret", "position": 2, "parent_id": "10",
                 "permission_overwrites": [{"id": "100", "type": 0, "allow": "0", "deny": (1u64 << 10).to_string()}]},
                {"id": "14", "type": 0, "name": "mods", "position": 3, "parent_id": "10",
                 "permission_overwrites": [
                    {"id": "100", "type": 0, "allow": "0", "deny": (1u64 << 10).to_string()},
                    {"id": "300", "type": 0, "allow": (1u64 << 10).to_string(), "deny": "0"}
                 ]}
            ]
        }],
        "merged_members": [[{"user_id": "1", "roles": ["300"]}]],
        "private_channels": [
            {"id": "20", "type": 1, "recipient_ids": ["2"], "last_message_id": "400"},
            {"id": "21", "type": 1, "recipient_ids": ["2"], "last_message_id": "600"}
        ],
        "read_state": {"entries": [{"id": "11", "last_message_id": "499", "mention_count": 0}], "partial": false, "version": 1},
        "user_guild_settings": {"entries": [], "partial": false, "version": 1},
        "relationships": []
    })
}

pub(super) fn store() -> Store {
    let mut s = Store::default();
    let d = s.apply("READY", &term(ready())).unwrap();
    assert!(d.contains(&Delta::Ready));
    s
}

#[test]
fn ready_builds_sorted_visible_channels() {
    let s = store();
    let chs = &s.guild_channels[&Snowflake(100)];
    // Category first (position 0, not voice), then text by position, voice last.
    assert_eq!(chs.iter().map(|c| c.0).collect::<Vec<_>>(), vec![10, 11, 13, 14, 12]);
    let visible: Vec<u64> = chs.iter().filter(|c| s.can_view(&s.channels[c])).map(|c| c.0).collect();
    assert_eq!(visible, vec![10, 11, 14, 12], "#secret hidden, #mods visible through role");
    assert!(s.is_unread(&s.channels[&Snowflake(11)]));
    assert_eq!(s.private_channels, vec![Snowflake(21), Snowflake(20)], "DMs newest first");
    assert_eq!(s.name_color(Some(Snowflake(100)), Snowflake(1)), Some(0xff0000));
}

#[test]
fn messages_mentions_and_pending_echo() {
    let mut s = store();
    let ch = Snowflake(11);
    s.set_history_latest(ch, vec![], false);
    let nonce = Snowflake::nonce_now();
    s.push_pending(ch, "hi".into(), nonce, None);
    assert_eq!(s.history[&ch].messages.len(), 1);

    let mine = json!({"id": "501", "channel_id": "11", "guild_id": "100", "author": {"id": "1", "username": "me"},
                      "content": "hi", "timestamp": "t", "nonce": nonce.to_string()});
    s.apply("MESSAGE_CREATE", &term(mine)).unwrap();
    assert_eq!(s.history[&ch].messages.len(), 1, "echo replaced");
    assert_eq!(s.history[&ch].messages[0].id, Snowflake(501));

    let ping = json!({"id": "502", "channel_id": "11", "guild_id": "100", "author": {"id": "2", "username": "alice"},
                      "content": "<@1>", "timestamp": "t", "mentions": [{"id": "1", "username": "me"}]});
    let d = s.apply("MESSAGE_CREATE", &term(ping)).unwrap();
    assert!(d.contains(&Delta::Mentioned { channel: ch, message: Snowflake(502) }));
    assert_eq!(s.mention_count(ch), 1);

    let d = s.apply("MESSAGE_ACK", &term(json!({"channel_id": "11", "message_id": "502", "mention_count": 0}))).unwrap();
    assert_eq!(d, vec![Delta::ReadState { channel: ch }]);
    assert!(!s.is_unread(&s.channels[&ch]));
}

#[test]
fn a_replys_echo_has_its_reply_line_from_the_start() {
    let mut s = store();
    let ch = Snowflake(11);
    let orig = json!({"id": "7", "channel_id": "11", "author": {"id": "2", "username": "alice"}, "content": "", "timestamp": "t",
                      "attachments": [{"id": "9", "filename": "a.png", "content_type": "image/png", "size": 1, "url": "u", "proxy_url": "u"}]});
    s.set_history_latest(ch, vec![serde_json::from_value(orig).unwrap()], false);
    s.push_pending(ch, "nice".into(), Snowflake::nonce_now(), Some(Snowflake(7)));
    let echo = s.history[&ch].messages.back().unwrap();
    assert_eq!(echo.id, Snowflake(0));
    assert_eq!(echo.kind, message_type::REPLY);
    assert_eq!(echo.referenced_message.as_ref().map(|r| r.id), Some(Snowflake(7)));
    // An original that is not loaded: a plain echo (the confirmed message brings the line).
    s.push_pending(ch, "x".into(), Snowflake::nonce_now(), Some(Snowflake(999)));
    assert!(s.history[&ch].messages.back().unwrap().referenced_message.is_none());
}

#[test]
fn reactions_count_and_me() {
    let mut s = store();
    let ch = Snowflake(11);
    let m = json!({"id": "1", "channel_id": "11", "author": {"id": "2"}, "content": "x", "timestamp": "t"});
    s.set_history_latest(ch, vec![serde_json::from_value(m).unwrap()], false);
    let r = |u: &str| term(json!({"user_id": u, "channel_id": "11", "message_id": "1", "emoji": {"name": "👍"}}));
    s.apply("MESSAGE_REACTION_ADD", &r("2")).unwrap();
    s.apply("MESSAGE_REACTION_ADD", &r("1")).unwrap();
    let rx = &s.history[&ch].messages[0].reactions[0];
    assert_eq!((rx.count, rx.me), (2, true));
    s.apply("MESSAGE_REACTION_REMOVE", &r("1")).unwrap();
    let rx = &s.history[&ch].messages[0].reactions[0];
    assert_eq!((rx.count, rx.me), (1, false));
}

#[test]
fn own_reaction_shows_at_once_and_the_echo_is_not_counted_twice() {
    let mut s = store();
    let ch = Snowflake(11);
    let m = json!({"id": "1", "channel_id": "11", "author": {"id": "2"}, "content": "x", "timestamp": "t",
                   "reactions": [{"emoji": {"name": "🔥"}, "count": 2, "me": false}]});
    s.set_history_latest(ch, vec![serde_json::from_value(m).unwrap()], false);
    let msg = Snowflake(1);
    // A new reaction and one more on an existing chip.
    assert_eq!(s.apply_own_reaction(ch, msg, "👍", true), vec![Delta::MessageUpdated { channel: ch, message: msg }]);
    assert_eq!(s.apply_own_reaction(ch, msg, "🔥", true).len(), 1);
    let rx = &s.history[&ch].messages[0].reactions;
    assert_eq!((rx[0].count, rx[0].me, rx[1].count, rx[1].me), (3, true, 1, true));
    // Again: nothing to change. The gateway's echo of the same reaction changes nothing either.
    assert!(s.apply_own_reaction(ch, msg, "🔥", true).is_empty());
    let echo = term(json!({"user_id": "1", "channel_id": "11", "message_id": "1", "emoji": {"name": "🔥"}}));
    assert!(s.apply("MESSAGE_REACTION_ADD", &echo).unwrap().is_empty());
    assert_eq!(s.history[&ch].messages[0].reactions[0].count, 3);
    // A refused request is taken back.
    assert_eq!(s.apply_own_reaction(ch, msg, "👍", false).len(), 1);
    assert_eq!(s.history[&ch].messages[0].reactions.len(), 1);
    // A custom emoji by `name:id`.
    assert_eq!(s.apply_own_reaction(ch, msg, "blob:42", true).len(), 1);
    assert_eq!(s.history[&ch].messages[0].reactions[1].emoji.id, Some(Snowflake(42)));
}

#[test]
fn reaction_add_many_folds_users() {
    let mut s = store();
    let ch = Snowflake(11);
    let m = json!({"id": "1", "channel_id": "11", "author": {"id": "2"}, "content": "x", "timestamp": "t",
                   "reactions": [{"emoji": {"name": "👍"}, "count": 1, "me": false}]});
    s.set_history_latest(ch, vec![serde_json::from_value(m).unwrap()], false);
    let many = json!({"channel_id": "11", "message_id": "1", "guild_id": "100", "reactions": [
        {"emoji": {"name": "👍"}, "users": ["1", "3"]},
        {"emoji": {"name": "🔥", "id": null}, "users": ["4", "5", "6"]}
    ]});
    let d = s.apply("MESSAGE_REACTION_ADD_MANY", &term(many)).unwrap();
    assert_eq!(d, vec![Delta::MessageUpdated { channel: ch, message: Snowflake(1) }]);
    let rx = &s.history[&ch].messages[0].reactions;
    assert_eq!((rx[0].count, rx[0].me), (3, true));
    assert_eq!((rx[1].count, rx[1].me), (3, false));
    assert!(s.unknown_events.is_empty());
}

#[test]
fn rail_honours_folders() {
    let mut s = store();
    s.apply("GUILD_CREATE", &term(json!({"id": "101", "properties": {"name": "B"}, "joined_at": "2025-01-01"}))).unwrap();
    s.folders = vec![
        GuildFolder { id: None, name: None, color: None, guild_ids: vec![Snowflake(100)] },
        GuildFolder { id: Some(7), name: Some("F".into()), color: None, guild_ids: vec![Snowflake(101), Snowflake(999)] },
    ];
    let rail = s.rail();
    assert_eq!(rail[0], RailEntry::Guild(Snowflake(100)));
    match &rail[1] {
        RailEntry::Folder(f) => assert_eq!(f.guild_ids, vec![Snowflake(101)], "unknown guilds dropped"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn member_list_sync_and_ops() {
    let mut s = store();
    let item = |id: &str| json!({"member": {"user": {"id": id, "username": id}, "roles": [], "presence": {"status": "online"}}});
    let u = json!({"guild_id": "100", "id": "everyone", "member_count": 3, "online_count": 2, "groups": [],
                   "ops": [{"op": "SYNC", "range": [0, 99], "items": [{"group": {"id": "online", "count": 2}}, item("2"), item("5")]},
                           {"op": "DELETE", "index": 2}]});
    s.apply("GUILD_MEMBER_LIST_UPDATE", &term(u)).unwrap();
    let l = &s.member_lists[&Snowflake(100)];
    assert_eq!(l.items.len(), 2);
    assert_eq!(s.users[&Snowflake(2)].username, "2", "members absorbed into user cache");
}

#[test]
fn unknown_events_are_counted_not_fatal() {
    let mut s = store();
    assert!(s.apply("SOME_FUTURE_EVENT", &term(json!({"x": 1}))).unwrap().is_empty());
    assert_eq!(s.unknown_events["SOME_FUTURE_EVENT"], 1);
}

#[test]
fn documented_dispatch_events_are_known() {
    let mut s = store();
    for e in ["GUILD_INTEGRATIONS_UPDATE", "INTEGRATION_UPDATE", "INTEGRATION_CREATE", "INTEGRATION_DELETE", "GUILD_BAN_ADD", "GUILD_MEMBER_REMOVE", "INVITE_CREATE", "WEBHOOKS_UPDATE"] {
        assert!(s.apply(e, &term(json!({"guild_id": "100"}))).unwrap().is_empty());
    }
    assert!(s.unknown_events.is_empty(), "{:?}", s.unknown_events);
}

#[test]
fn new_fields_do_not_break_decoding() {
    let mut s = store();
    let m = json!({"id": "9", "channel_id": "11", "author": {"id": "2", "brand_new": [1, 2]}, "content": "x",
                   "timestamp": "t", "type": 99, "future_field": {"a": true}});
    s.set_history_latest(Snowflake(11), vec![], false);
    s.apply("MESSAGE_CREATE", &term(m)).unwrap();
    assert_eq!(s.history[&Snowflake(11)].messages[0].kind, 99);
}

#[test]
fn member_list_role_groups_are_integers_over_etf() {
    // Seen live: hoisted-role groups arrive with the role id as an ETF integer.
    let mut s = store();
    let u = json!({"guild_id": "100", "id": "everyone", "member_count": 1, "online_count": 1,
                   "groups": [{"id": 555942858461282344u64, "count": 1}],
                   "ops": [{"op": "SYNC", "range": [0, 99], "items": [{"group": {"id": 555942858461282344u64, "count": 7}}]}]});
    s.apply("GUILD_MEMBER_LIST_UPDATE", &term(u)).unwrap();
    let l = &s.member_lists[&Snowflake(100)];
    assert_eq!(l.items[0].group.as_ref().unwrap().id, "555942858461282344");
    assert_eq!(l.items[0].group.as_ref().unwrap().count, 7);
}

#[test]
fn invisible_names_fall_back_to_username() {
    assert!(crate::model::is_blank_name("\u{1CBC}\u{1CBC}\u{1CBC}"));
    assert!(crate::model::is_blank_name("\u{3164} \u{200B}"));
    assert!(!crate::model::is_blank_name("\u{20DF} \u{20DF}"), "enclosing diamonds draw visibly");
    let u = crate::model::User { username: "alice".into(), global_name: Some("\u{1CBC}\u{1CBC}".into()), ..Default::default() };
    assert_eq!(u.display_name(), "alice");
}

#[test]
fn mentioned_users_resolve_without_membership() {
    let mut s = store();
    s.set_history_latest(Snowflake(11), vec![], false);
    let m = json!({"id": "9", "channel_id": "11", "author": {"id": "2", "username": "a"}, "content": "<@77>",
                   "timestamp": "t", "type": 0, "mentions": [{"id": "77", "username": "modmail"}]});
    s.apply("MESSAGE_CREATE", &term(m)).unwrap();
    assert_eq!(s.display_name(None, Snowflake(77)), "modmail");
}

#[test]
fn leaving_a_channel_trims_its_history() {
    let mut s = store();
    let msgs: Vec<_> = (1..=200u64).rev().map(|i| serde_json::from_value(json!({"id": i.to_string(), "channel_id": "11", "author": {"id": "2"}, "content": "x", "timestamp": "t"})).unwrap()).collect();
    s.set_history_latest(Snowflake(11), msgs, true);
    s.trim_history(Snowflake(11), 50);
    let h = &s.history[&Snowflake(11)];
    assert_eq!(h.messages.len(), 50);
    assert_eq!(h.messages.back().unwrap().id, Snowflake(200), "newest kept");
    assert!(h.has_more_before);
}

#[test]
fn store_survives_json_round_trip() {
    // The launch paint reads the store back from the cache; a failed decode means a blank window
    // until READY.
    let s = store();
    let json = serde_json::to_vec(&s).unwrap();
    let back: Store = match serde_json::from_slice(&json) {
        Ok(b) => b,
        Err(e) => {
            let col = e.column();
            let txt = String::from_utf8_lossy(&json);
            panic!("store does not round-trip: {e}\n…{}…", &txt[col.saturating_sub(300)..(col + 50).min(txt.len())])
        }
    };
    assert_eq!(back.rail(), s.rail());
    assert_eq!(back.private_channels, s.private_channels);
    assert_eq!(back.guild_channels.len(), s.guild_channels.len());
}

fn proto_b64(status: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(crate::model::settings_proto::encode_status(status))
}

#[test]
fn ready_status_comes_from_saved_setting_over_sessions() {
    let mut r = ready();
    r["sessions"] = json!([{"session_id": "all", "status": "online"}]);
    r["user_settings_proto"] = json!(proto_b64("dnd"));
    let mut s = Store::default();
    s.apply("READY", &term(r.clone())).unwrap();
    assert_eq!(s.my_status, "dnd");

    // No saved status: fall back to the sessions aggregate, then to online.
    r["user_settings_proto"] = json!(null);
    r["sessions"] = json!([{"session_id": "all", "status": "idle"}]);
    s.apply("READY", &term(r.clone())).unwrap();
    assert_eq!(s.my_status, "idle");
    r["sessions"] = json!([]);
    s.apply("READY", &term(r)).unwrap();
    assert_eq!(s.my_status, "online");
}

#[test]
fn other_device_status_change_updates_us() {
    let mut s = store();
    let upd = |st: &str| term(json!({"settings": {"type": 1, "proto": proto_b64(st)}, "partial": false}));
    let d = s.apply("USER_SETTINGS_PROTO_UPDATE", &upd("invisible")).unwrap();
    assert!(d.contains(&Delta::OwnStatus));
    assert_eq!(s.my_status, "invisible");
    let d = s.apply("USER_SETTINGS_PROTO_UPDATE", &upd("invisible")).unwrap();
    assert!(!d.contains(&Delta::OwnStatus), "unchanged");
    // A status-only update must not wipe the folder layout.
    s.folders = vec![GuildFolder::default()];
    s.apply("USER_SETTINGS_PROTO_UPDATE", &upd("idle")).unwrap();
    assert_eq!(s.folders.len(), 1);

    let d = s.apply("SESSIONS_REPLACE", &term(json!([{"session_id": "all", "status": "online"}]))).unwrap();
    assert_eq!(d, vec![Delta::OwnStatus]);
    assert_eq!(s.my_status, "online");
}

#[test]
fn dm_presence_from_supplemental_and_updates() {
    let mut s = store();
    let alice = Snowflake(2);
    assert_eq!(s.presence(alice), "offline");

    let d = s
        .apply(
            "READY_SUPPLEMENTAL",
            &term(json!({
                "merged_presences": {"friends": [{"user_id": "2", "status": "idle"}], "guilds": [[{"user_id": "9", "status": "online"}]]},
                "merged_members": [], "guilds": []
            })),
        )
        .unwrap();
    assert!(d.contains(&Delta::PrivateChannelsChanged));
    assert_eq!(s.presence(alice), "idle");
    assert_eq!(s.presence(Snowflake(9)), "online");

    let upd = |st: &str| term(json!({"user": {"id": "2"}, "status": st, "client_status": {"desktop": st}, "activities": []}));
    assert_eq!(s.apply("PRESENCE_UPDATE", &upd("dnd")).unwrap(), vec![Delta::PrivateChannelsChanged, Delta::UserUpdated { user: alice }]);
    assert_eq!(s.presence(alice), "dnd");
    assert!(s.apply("PRESENCE_UPDATE", &upd("dnd")).unwrap().is_empty(), "no change, no refresh");
    assert_eq!(s.apply("PRESENCE_UPDATE", &upd("offline")).unwrap(), vec![Delta::PrivateChannelsChanged, Delta::UserUpdated { user: alice }]);
    assert_eq!(s.presence(alice), "offline");
    // A user who isn't a DM recipient changes nothing visible in the DM list.
    let other = term(json!({"user": {"id": "9"}, "status": "idle"}));
    assert_eq!(s.apply("PRESENCE_UPDATE", &other).unwrap(), vec![Delta::UserUpdated { user: Snowflake(9) }]);
    assert_eq!(s.presence(Snowflake(9)), "idle");
    // Never track ourselves from presence events.
    s.apply("PRESENCE_UPDATE", &term(json!({"user": {"id": "1"}, "status": "idle"}))).unwrap();
    assert_eq!(s.presence(Snowflake(1)), "offline");
    // Every surface asks `status_of`: someone known only from a DM or the friends list (no member list) shows
    // what they really are, not "offline" (the profile card said so while the DM list said online).
    assert_eq!(s.status_of(Snowflake(9), None), "idle");
    assert_eq!(s.status_of(Snowflake(9), Some(Snowflake(100))), "idle", "a server with no member list for them falls back to what is known");
}

#[test]
fn closing_a_dm_removes_it_and_only_dms() {
    let mut s = store();
    assert!(s.remove_private_channel(Snowflake(11)).is_empty(), "guild channels are never removed this way");
    assert!(s.channels.contains_key(&Snowflake(11)));
    assert_eq!(s.remove_private_channel(Snowflake(20)), vec![Delta::PrivateChannelsChanged]);
    assert_eq!(s.private_channels, vec![Snowflake(21)]);
    let d = s.apply("CHANNEL_DELETE", &term(json!({"id": "21", "type": 1}))).unwrap();
    assert_eq!(d, vec![Delta::PrivateChannelsChanged]);
    assert!(s.private_channels.is_empty());
}

// ----- Forums -----

fn forum_store() -> Store {
    let mut s = store();
    let forum = json!({"id": "30", "type": 15, "guild_id": "100", "name": "help", "position": 5,
        "default_sort_order": 1, "default_auto_archive_duration": 1440,
        "available_tags": [{"id": "1", "name": "bug", "moderated": false, "emoji_name": "🐛"}, {"id": "2", "name": "idea"}]});
    s.apply("CHANNEL_CREATE", &term(forum)).unwrap();
    s
}

fn thread(id: &str, last: &str, tags: &[&str], archived: bool) -> Value {
    json!({"id": id, "type": 11, "guild_id": "100", "parent_id": "30", "name": format!("post {id}"),
           "last_message_id": last, "applied_tags": tags, "message_count": 3, "owner_id": "2",
           "thread_metadata": {"archived": archived, "locked": false}})
}

#[test]
fn forum_channel_decodes_tags_and_defaults() {
    let s = forum_store();
    let f = &s.channels[&Snowflake(30)];
    assert!(f.is_forum_like());
    assert_eq!(f.available_tags.len(), 2);
    assert_eq!(f.available_tags[0].emoji_name.as_deref(), Some("🐛"));
    assert_eq!(f.default_sort_order, Some(1));
    assert_eq!(f.default_auto_archive_duration, Some(1440));
}

#[test]
fn thread_list_sync_fills_active_posts_and_membership() {
    let mut s = forum_store();
    let sync = json!({"guild_id": "100", "threads": [thread("40", "41", &["1"], false), thread("42", "90", &[], false)],
                      "members": [{"id": "42", "user_id": "1", "join_timestamp": "t", "flags": 1}]});
    let d = s.apply("THREAD_LIST_SYNC", &term(sync)).unwrap();
    assert!(d.contains(&Delta::ForumChanged { channel: Snowflake(30) }));
    let q = ForumQuery::default();
    assert_eq!(s.forum_posts(Snowflake(30), &q), vec![Snowflake(42), Snowflake(40)]);
    assert!(s.channels[&Snowflake(42)].member.is_some(), "joined post");
    assert!(s.channels[&Snowflake(40)].member.is_none());
    // THREAD_UPDATE without a member keeps the membership.
    s.apply("THREAD_UPDATE", &term(thread("42", "95", &[], false))).unwrap();
    assert!(s.channels[&Snowflake(42)].member.is_some());
}

#[test]
fn archived_pages_append_and_carry_first_messages() {
    let mut s = forum_store();
    let q = ForumQuery::default();
    let page: crate::forum::ThreadSearch = serde_json::from_value(json!({
        "threads": [thread("50", "51", &[], true), thread("52", "53", &[], true)],
        "members": [], "has_more": true,
        "first_messages": [{"id": "50", "channel_id": "50", "author": {"id": "2", "username": "a"}, "content": "hello", "timestamp": "t"}]
    })).unwrap();
    s.absorb_thread_search(Snowflake(30), &q, 0, page);
    let list = s.forum_list(Snowflake(30), &q).unwrap();
    assert_eq!((list.next_offset, list.has_more, list.loaded), (2, true, true));
    assert_eq!(s.forum_posts(Snowflake(30), &q), vec![Snowflake(50), Snowflake(52)]);
    assert_eq!(s.previews.first[&Snowflake(50)].as_ref().unwrap().content, "hello");
    assert!(s.previews.first[&Snowflake(52)].is_none(), "searched without a first message: known empty");
    // Archived posts stay out of the sidebar.
    assert!(s.channels[&Snowflake(50)].is_archived());
    assert!(s.previews.missing(&[Snowflake(50), Snowflake(52), Snowflake(60)]) == vec![Snowflake(60)]);
}

#[test]
fn post_data_fills_previews_and_clears_requests() {
    let mut s = forum_store();
    s.previews.requested.insert(Snowflake(40));
    s.previews.requested.insert(Snowflake(41));
    let data: crate::forum::PostData = serde_json::from_value(json!({"threads": {
        "40": {"first_message": {"id": "40", "channel_id": "40", "author": {"id": "2"}, "content": "first", "timestamp": "t"}}
    }})).unwrap();
    s.absorb_post_data(Snowflake(30), &[Snowflake(40), Snowflake(41)], data);
    assert_eq!(s.previews.first[&Snowflake(40)].as_ref().unwrap().content, "first");
    assert!(s.previews.first[&Snowflake(41)].is_none());
    assert!(s.previews.requested.is_empty());
}

#[test]
fn replies_bump_post_counts_and_first_message_edits_reach_preview() {
    let mut s = forum_store();
    s.apply("THREAD_CREATE", &term(thread("40", "40", &[], false))).unwrap();
    s.previews.first.insert(Snowflake(40), Some(serde_json::from_value(json!({"id": "40", "channel_id": "40", "author": {"id": "2"}, "content": "v1", "timestamp": "t"})).unwrap()));
    let reply = json!({"id": "70", "channel_id": "40", "guild_id": "100", "author": {"id": "2", "username": "a"}, "content": "r", "timestamp": "t"});
    let d = s.apply("MESSAGE_CREATE", &term(reply)).unwrap();
    assert!(d.contains(&Delta::ForumChanged { channel: Snowflake(30) }));
    assert_eq!(s.channels[&Snowflake(40)].message_count, Some(4));
    assert_eq!(s.channels[&Snowflake(40)].last_message_id, Some(Snowflake(70)));
    s.apply("MESSAGE_UPDATE", &term(json!({"id": "40", "channel_id": "40", "content": "v2"}))).unwrap();
    assert_eq!(s.previews.first[&Snowflake(40)].as_ref().unwrap().content, "v2");
    s.apply("MESSAGE_DELETE", &term(json!({"id": "40", "channel_id": "40"}))).unwrap();
    assert!(s.previews.first[&Snowflake(40)].is_none());
}

#[test]
fn created_post_is_joined_and_listed() {
    let mut s = forum_store();
    let post: crate::model::Channel = serde_json::from_value(thread("80", "80", &["2"], false)).unwrap();
    let first = serde_json::from_value(json!({"id": "80", "channel_id": "80", "author": {"id": "1"}, "content": "body", "timestamp": "t"})).unwrap();
    let d = s.insert_created_post(post, Some(first));
    assert!(d.contains(&Delta::ForumChanged { channel: Snowflake(30) }));
    assert!(s.channels[&Snowflake(80)].member.is_some());
    assert_eq!(s.forum_posts(Snowflake(30), &ForumQuery::default())[0], Snowflake(80));
}

#[test]
fn dm_mute_is_local_then_confirmed_by_settings_update() {
    let mut s = store();
    let dm = s.channels[&Snowflake(20)].clone();
    assert!(!s.is_channel_muted(&dm));
    assert_eq!(s.set_dm_muted(Snowflake(20), true), vec![Delta::PrivateChannelsChanged]);
    assert!(s.is_channel_muted(&dm));
    s.set_dm_muted(Snowflake(20), false);
    assert!(!s.is_channel_muted(&dm));
    let upd = term(json!({"guild_id": null, "channel_overrides": [{"channel_id": "20", "muted": true}]}));
    assert_eq!(s.apply("USER_GUILD_SETTINGS_UPDATE", &upd).unwrap(), vec![Delta::PrivateChannelsChanged]);
    assert!(s.is_channel_muted(&dm));
}

#[test]
fn stale_status_confirmations_do_not_flicker_a_just_chosen_status() {
    let mut s = store();
    s.my_status = "invisible".into();
    s.set_my_status_local("idle");
    let sessions = |st: &str| term(json!([{"session_id": "all", "status": st}]));
    assert!(s.apply("SESSIONS_REPLACE", &sessions("invisible")).unwrap().is_empty(), "stale: ignored");
    assert_eq!(s.my_status, "idle");
    assert!(s.apply("SESSIONS_REPLACE", &sessions("idle")).unwrap().is_empty(), "agrees: nothing to change");
    // Confirmed: later changes from other devices apply again.
    assert_eq!(s.apply("SESSIONS_REPLACE", &sessions("dnd")).unwrap(), vec![Delta::OwnStatus]);
    assert_eq!(s.my_status, "dnd");
}

#[test]
fn dm_call_participants_follow_call_and_voice_state_events() {
    let mut s = store();
    let create = term(json!({"channel_id": "20", "voice_states": [{"user_id": "1", "channel_id": "20"}], "ringing": ["2"]}));
    assert_eq!(s.apply("CALL_CREATE", &create).unwrap(), vec![Delta::Voice]);
    assert_eq!(s.dm_voice[&Snowflake(20)].len(), 1);
    let join = term(json!({"user_id": "2", "channel_id": "20", "session_id": "x", "self_mute": true}));
    s.apply("VOICE_STATE_UPDATE", &join).unwrap();
    assert_eq!(s.dm_voice[&Snowflake(20)].len(), 2);
    let leave = term(json!({"user_id": "2", "channel_id": null, "session_id": "x"}));
    s.apply("VOICE_STATE_UPDATE", &leave).unwrap();
    assert_eq!(s.dm_voice[&Snowflake(20)].len(), 1);
    s.apply("CALL_DELETE", &term(json!({"channel_id": "20"}))).unwrap();
    assert!(s.dm_voice.is_empty());
}

#[test]
fn alone_in_dm_call_only_for_a_one_to_one_dm_with_nobody_else() {
    let mut s = store();
    let me = me_id(&s);
    s.channels.insert(Snowflake(20), Channel { id: Snowflake(20), kind: channel_type::DM, ..Default::default() });
    s.channels.insert(Snowflake(21), Channel { id: Snowflake(21), kind: channel_type::GROUP_DM, ..Default::default() });
    assert!(!s.alone_in_dm_call(), "not in a call");
    s.voice.channel = Some(Snowflake(20));
    assert!(s.alone_in_dm_call());
    s.apply("VOICE_STATE_UPDATE", &term(json!({"user_id": me.0.to_string(), "channel_id": "20", "session_id": "x"}))).unwrap();
    assert!(s.alone_in_dm_call(), "our own state does not count");
    s.apply("VOICE_STATE_UPDATE", &term(json!({"user_id": "2", "channel_id": "20", "session_id": "y"}))).unwrap();
    assert!(!s.alone_in_dm_call());
    s.voice.channel = Some(Snowflake(21));
    assert!(s.alone_in_dm_call() == false, "a group DM never idles out");
}

fn me_id(s: &Store) -> Snowflake {
    s.my_id().expect("test store has a current user")
}

#[test]
fn call_ringing_us_is_incoming_and_never_joins() {
    let mut s = store();
    let me = me_id(&s).to_string();
    let create = term(json!({"channel_id": "20", "message_id": "9", "voice_states": [{"user_id": "2", "channel_id": "20"}], "ongoing_rings": {me.clone(): null}}));
    let d = s.apply("CALL_CREATE", &create).unwrap();
    assert!(d.contains(&Delta::IncomingCall { channel: Snowflake(20) }));
    assert_eq!(s.incoming_calls(), vec![Snowflake(20)]);
    assert!(s.dm_call_ongoing(Snowflake(20)));
    assert!(s.voice.channel.is_none(), "a ring never joins by itself");
    // A repeat of the same ring does not notify twice.
    let again = term(json!({"channel_id": "20", "ongoing_rings": {me.clone(): null}}));
    assert!(!s.apply("CALL_UPDATE", &again).unwrap().contains(&Delta::IncomingCall { channel: Snowflake(20) }));
}

#[test]
fn call_update_keeps_participants_and_drops_answered_rings() {
    let mut s = store();
    let me = me_id(&s);
    let create = term(json!({"channel_id": "20", "voice_states": [{"user_id": "2", "channel_id": "20"}], "ongoing_rings": {me.to_string(): null, "3": null}}));
    s.apply("CALL_CREATE", &create).unwrap();
    // CALL_UPDATE has no voice states: participants must survive it.
    s.apply("CALL_UPDATE", &term(json!({"channel_id": "20", "ongoing_rings": {me.to_string(): null, "3": null}}))).unwrap();
    assert_eq!(s.dm_call_participants(Snowflake(20)), vec![Snowflake(2)]);
    // User 3 answers: they join and stop ringing.
    s.apply("VOICE_STATE_UPDATE", &term(json!({"user_id": "3", "channel_id": "20", "session_id": "x"}))).unwrap();
    assert_eq!(s.dm_call_ringing(Snowflake(20)), vec![me]);
    // The server empties the ring list.
    s.apply("CALL_UPDATE", &term(json!({"channel_id": "20", "ongoing_rings": {}}))).unwrap();
    assert!(s.dm_call_ringing(Snowflake(20)).is_empty());
    assert!(s.incoming_calls().is_empty());
}

#[test]
fn call_stays_ongoing_until_the_last_person_leaves() {
    let mut s = store();
    s.apply("CALL_CREATE", &term(json!({"channel_id": "20", "voice_states": [{"user_id": "2", "channel_id": "20"}, {"user_id": "3", "channel_id": "20"}]}))).unwrap();
    // The one who started it leaves; the call goes on for the other.
    s.apply("VOICE_STATE_UPDATE", &term(json!({"user_id": "2", "channel_id": null, "session_id": "x"}))).unwrap();
    assert!(s.dm_call_ongoing(Snowflake(20)));
    assert!(s.incoming_calls().is_empty(), "not ringing us");
    s.apply("VOICE_STATE_UPDATE", &term(json!({"user_id": "3", "channel_id": null, "session_id": "y"}))).unwrap();
    assert!(!s.dm_call_ongoing(Snowflake(20)));
    // CALL_DELETE clears the ring state too.
    s.apply("CALL_CREATE", &term(json!({"channel_id": "20", "voice_states": [{"user_id": "2", "channel_id": "20"}], "ongoing_rings": {"3": null}}))).unwrap();
    s.apply("CALL_DELETE", &term(json!({"channel_id": "20"}))).unwrap();
    assert!(s.dm_calls.is_empty() && !s.dm_call_ongoing(Snowflake(20)));
}

#[test]
fn declining_clears_our_ring_locally_and_old_ringing_lists_still_parse() {
    let mut s = store();
    let me = me_id(&s);
    s.apply("CALL_CREATE", &term(json!({"channel_id": "20", "voice_states": [{"user_id": "2", "channel_id": "20"}], "ringing": [me.to_string()]}))).unwrap();
    assert_eq!(s.incoming_calls(), vec![Snowflake(20)]);
    s.stop_ringing_local(Snowflake(20), me);
    assert!(s.incoming_calls().is_empty());
    assert!(s.dm_call_ongoing(Snowflake(20)), "declining does not end the call");
}

#[test]
fn poll_votes_fold_into_message() {
    let mut s = store();
    let ch = Snowflake(11);
    let m = json!({"id": "1", "channel_id": "11", "author": {"id": "2"}, "content": "", "timestamp": "t",
                   "poll": {"question": {"text": "Q"}, "answers": [{"answer_id": 1, "poll_media": {"text": "A"}}]}});
    s.set_history_latest(ch, vec![serde_json::from_value(m).unwrap()], false);
    let ev = |u: &str| term(json!({"user_id": u, "channel_id": "11", "message_id": "1", "answer_id": 1}));
    s.apply("MESSAGE_POLL_VOTE_ADD", &ev("1")).unwrap();
    s.apply("MESSAGE_POLL_VOTE_ADD", &ev("2")).unwrap();
    let v = crate::interactions::parse_poll(s.history[&ch].messages[0].poll.as_ref().unwrap()).unwrap();
    assert_eq!((v.answers[0].count, v.answers[0].me), (2, true));
    s.apply("MESSAGE_POLL_VOTE_REMOVE", &ev("1")).unwrap();
    let v = crate::interactions::parse_poll(s.history[&ch].messages[0].poll.as_ref().unwrap()).unwrap();
    assert_eq!((v.answers[0].count, v.answers[0].me), (1, false));
}

#[test]
fn muted_channels_count_mentions_but_do_not_notify() {
    let mut s = store();
    let ch = Snowflake(11);
    s.set_history_latest(ch, vec![], false);
    s.guild_settings.insert(
        Snowflake(100),
        UserGuildSettings {
            guild_id: Some(Snowflake(100)),
            channel_overrides: vec![ChannelOverride { channel_id: ch, muted: true, ..Default::default() }],
            ..Default::default()
        },
    );
    let ping = json!({"id": "502", "channel_id": "11", "guild_id": "100", "author": {"id": "2", "username": "alice"},
                      "content": "<@1>", "timestamp": "t", "mentions": [{"id": "1", "username": "me"}]});
    let d = s.apply("MESSAGE_CREATE", &term(ping)).unwrap();
    assert!(!d.iter().any(|x| matches!(x, Delta::Mentioned { .. })));
    assert_eq!(s.mention_count(ch), 1, "the badge still counts it");

    // A muted server lets a direct @mention through the channel level only when the channel isn't muted.
    s.guild_settings.get_mut(&Snowflake(100)).unwrap().channel_overrides.clear();
    s.guild_settings.get_mut(&Snowflake(100)).unwrap().muted = true;
    let everyone = json!({"id": "503", "channel_id": "11", "guild_id": "100", "author": {"id": "2", "username": "alice"},
                          "content": "@everyone", "timestamp": "t", "mention_everyone": true});
    let d = s.apply("MESSAGE_CREATE", &term(everyone)).unwrap();
    assert!(!d.iter().any(|x| matches!(x, Delta::Mentioned { .. })));
    let direct = json!({"id": "504", "channel_id": "11", "guild_id": "100", "author": {"id": "2", "username": "alice"},
                        "content": "<@1>", "timestamp": "t", "mentions": [{"id": "1", "username": "me"}]});
    let d = s.apply("MESSAGE_CREATE", &term(direct)).unwrap();
    assert!(d.iter().any(|x| matches!(x, Delta::Mentioned { .. })));
}

#[test]
fn a_role_ping_counts_only_for_members_of_the_role() {
    use crate::model::{Member, Message, UserGuildSettings};
    let mut s = Store::default();
    let (g, role, other) = (Snowflake(100), Snowflake(7), Snowflake(8));
    s.me = Some(User { id: Snowflake(1), ..Default::default() });
    s.my_members.insert(g, Member { roles: vec![role], ..Default::default() });
    let msg = |roles: Vec<Snowflake>| Message {
        id: Snowflake(500),
        channel_id: Snowflake(11),
        guild_id: Some(g),
        author: User { id: Snowflake(2), ..Default::default() },
        mention_roles: roles,
        ..Default::default()
    };
    assert!(s.pings_me(&msg(vec![role])), "a role I hold pings me");
    assert!(!s.pings_me(&msg(vec![other])), "a role I do not hold does not");
    assert!(!s.pings_me(&msg(vec![])), "a plain message does not");
    let mut mine = msg(vec![role]);
    mine.author.id = Snowflake(1);
    assert!(!s.pings_me(&mine), "my own message never pings");
    s.guild_settings.insert(g, UserGuildSettings { suppress_roles: true, ..Default::default() });
    assert!(!s.pings_me(&msg(vec![role])), "suppressed roles do not ping");
}

#[test]
fn a_ping_loaded_over_rest_still_pings() {
    use crate::model::{Channel, Member, Message};
    let mut s = Store::default();
    let (g, role) = (Snowflake(100), Snowflake(7));
    s.me = Some(User { id: Snowflake(1), ..Default::default() });
    s.my_members.insert(g, Member { roles: vec![role], ..Default::default() });
    s.channels.insert(Snowflake(11), Channel { id: Snowflake(11), guild_id: Some(g), ..Default::default() });
    // A REST page has no `guild_id` on its messages.
    let rest = |roles: Vec<Snowflake>, everyone: bool| Message {
        id: Snowflake(500),
        channel_id: Snowflake(11),
        author: User { id: Snowflake(2), ..Default::default() },
        mention_roles: roles,
        mention_everyone: everyone,
        ..Default::default()
    };
    assert!(s.pings_me(&rest(vec![role], false)), "a role ping from history");
    assert!(s.pings_me(&rest(vec![], true)), "@everyone from history");
    assert!(!s.pings_me(&rest(vec![], false)));
}

#[test]
fn activities_follow_presence_updates() {
    let mut s = Store::default();
    s.me = Some(User { id: Snowflake(1), ..Default::default() });
    let upd = |acts: serde_json::Value| term(json!({"user": {"id": "2"}, "status": "online", "activities": acts}));
    let d = s.apply("PRESENCE_UPDATE", &upd(json!([{"type": 0, "name": "Factorio"}]))).unwrap();
    assert!(d.contains(&Delta::UserUpdated { user: Snowflake(2) }));
    assert_eq!(crate::activity::row_text(s.user_activities(Snowflake(2))).as_deref(), Some("Playing Factorio"));
    assert!(s.apply("PRESENCE_UPDATE", &upd(json!([{"type": 0, "name": "Factorio"}]))).unwrap().is_empty());
    s.apply("PRESENCE_UPDATE", &upd(json!([]))).unwrap();
    assert!(s.user_activities(Snowflake(2)).is_empty());
}

#[test]
fn message_requests_are_separate_until_accepted() {
    let mut s = Store::default();
    s.channels.insert(Snowflake(10), Channel { id: Snowflake(10), kind: channel_type::DM, is_message_request: true, ..Default::default() });
    s.channels.insert(Snowflake(11), Channel { id: Snowflake(11), kind: channel_type::DM, is_message_request: true, is_spam: true, ..Default::default() });
    s.channels.insert(Snowflake(12), Channel { id: Snowflake(12), kind: channel_type::DM, ..Default::default() });
    s.private_channels = vec![Snowflake(12), Snowflake(11), Snowflake(10)];
    assert_eq!(s.message_requests(false), vec![Snowflake(10)]);
    assert_eq!(s.message_requests(true), vec![Snowflake(11)]);
    assert_eq!(s.accept_message_request(Snowflake(10)), vec![Delta::PrivateChannelsChanged]);
    assert!(s.message_requests(false).is_empty());
    assert!(s.accept_message_request(Snowflake(12)).is_empty(), "not a request");
}

#[test]
fn scheduled_events_and_stages_follow_the_gateway() {
    let mut s = Store::default();
    s.me = Some(User { id: Snowflake(1), ..Default::default() });
    let ev = |st: u8| term(json!({"id": "50", "guild_id": "7", "name": "Movie night", "scheduled_start_time": "2026-11-01T10:00:00+00:00", "status": st, "entity_type": 3, "user_count": 2}));
    assert_eq!(s.apply("GUILD_SCHEDULED_EVENT_CREATE", &ev(1)).unwrap(), vec![Delta::GuildEvents { guild: Snowflake(7) }]);
    assert_eq!(s.guild_events(Snowflake(7)).len(), 1);
    let add = |u: &str, n: &str| term(json!({"guild_scheduled_event_id": "50", "guild_id": "7", "user_id": u, "_": n}));
    s.apply("GUILD_SCHEDULED_EVENT_USER_ADD", &add("1", "")).unwrap();
    assert!(s.event_interest.contains(&Snowflake(50)));
    assert_eq!(s.guild_events(Snowflake(7))[0].user_count, Some(3));
    s.apply("GUILD_SCHEDULED_EVENT_USER_REMOVE", &add("1", "")).unwrap();
    assert!(!s.event_interest.contains(&Snowflake(50)));
    s.apply("GUILD_SCHEDULED_EVENT_UPDATE", &ev(3)).unwrap();
    assert!(s.guild_events(Snowflake(7)).is_empty(), "completed events are hidden");
    s.apply("GUILD_SCHEDULED_EVENT_DELETE", &ev(3)).unwrap();
    assert!(s.scheduled_events[&Snowflake(7)].is_empty());

    let inst = term(json!({"id": "60", "guild_id": "7", "channel_id": "8", "topic": "AMA", "privacy_level": 2}));
    s.apply("STAGE_INSTANCE_CREATE", &inst).unwrap();
    assert_eq!(s.stages[&Snowflake(8)].topic, "AMA");
    let vs = |u: &str, suppress: bool, hand: Option<&str>| term(json!({"guild_id": "7", "channel_id": "8", "user_id": u, "session_id": "x", "suppress": suppress, "request_to_speak_timestamp": hand}));
    s.apply("VOICE_STATE_UPDATE", &vs("2", false, None)).unwrap();
    s.apply("VOICE_STATE_UPDATE", &vs("3", true, Some("2026-11-01T10:00:00Z"))).unwrap();
    s.apply("VOICE_STATE_UPDATE", &vs("4", true, None)).unwrap();
    let (sp, hands, aud) = s.stage_roles(Snowflake(7), Snowflake(8));
    assert_eq!((sp.len(), hands.len(), aud.len()), (1, 1, 1));
    s.apply("STAGE_INSTANCE_DELETE", &inst).unwrap();
    assert!(s.stages.is_empty());
}

#[test]
fn friend_nickname_replaces_every_name_and_clears() {
    let mut s = Store::default();
    s.me = Some(User { id: Snowflake(1), ..Default::default() });
    s.users.insert(Snowflake(2), User { id: Snowflake(2), username: "alice".into(), ..Default::default() });
    let rel = |kind: u8, nick: serde_json::Value| term(json!({"id": "2", "type": kind, "nickname": nick, "user": {"id": "2", "username": "alice"}}));
    s.apply("RELATIONSHIP_ADD", &rel(1, json!("Ally"))).unwrap();
    assert_eq!(s.display_name(None, Snowflake(2)), "Ally");
    assert_eq!(s.display_name(Some(Snowflake(100)), Snowflake(2)), "alice", "servers never use a friend nickname");
    // A nickname on a blocked or pending relationship does not apply.
    s.apply("RELATIONSHIP_UPDATE", &rel(3, json!("Ally"))).unwrap();
    assert_eq!(s.display_name(None, Snowflake(2)), "alice");
    s.apply("RELATIONSHIP_UPDATE", &rel(1, json!("   "))).unwrap();
    assert_eq!(s.display_name(None, Snowflake(2)), "alice", "a blank nickname is no nickname");
    // Local edit, then removal.
    assert!(!s.set_friend_nick_local(Snowflake(2), Some("Al".into())).is_empty());
    assert_eq!(s.display_name(None, Snowflake(2)), "Al");
    s.apply("RELATIONSHIP_REMOVE", &term(json!({"id": "2", "type": 1}))).unwrap();
    assert_eq!(s.display_name(None, Snowflake(2)), "alice");
    assert!(s.set_friend_nick_local(Snowflake(2), Some("x".into())).is_empty(), "not a friend any more");
}

#[test]
fn people_are_found_by_any_name_they_answer_to() {
    let mut s = Store::default();
    s.me = Some(User { id: Snowflake(1), ..Default::default() });
    s.users.insert(Snowflake(2), User { id: Snowflake(2), username: "alice_w".into(), ..Default::default() });
    s.users.insert(Snowflake(3), User { id: Snowflake(3), username: "bob".into(), ..Default::default() });
    s.apply("RELATIONSHIP_ADD", &term(json!({"id": "2", "type": 1, "nickname": "Ally", "user": {"id": "2", "username": "alice_w"}}))).unwrap();
    let first = |q: &str| s.users_matching(q).first().map(|(id, _)| *id);
    assert_eq!(first("ally"), Some(Snowflake(2)), "friend nickname");
    assert_eq!(first("alice"), Some(Snowflake(2)), "username prefix");
    assert_eq!(first("alyl"), Some(Snowflake(2)), "one typo still finds the friend");
    assert_eq!(first("zzzz"), None);
    assert_eq!(first("bpb"), None);
    assert_eq!(first("bob"), Some(Snowflake(3)));
    assert!(s.users_matching("").is_empty());
}

#[test]
fn keywords_notify_without_a_badge() {
    let mut s = store();
    s.notify_keywords = vec!["rust".into()];
    s.set_history_latest(Snowflake(11), vec![], true);
    let msg = |id: &str, text: &str| term(json!({"id": id, "channel_id": "11", "guild_id": "100", "author": {"id": "2", "username": "alice"},
                                                  "content": text, "timestamp": "t"}));
    let d = s.apply("MESSAGE_CREATE", &msg("600", "anyone using Rust?")).unwrap();
    assert!(d.iter().any(|x| matches!(x, Delta::Mentioned { .. })));
    assert_eq!(s.mention_count(Snowflake(11)), 0, "a keyword is not a mention");
    let d = s.apply("MESSAGE_CREATE", &msg("601", "trust me")).unwrap();
    assert!(!d.iter().any(|x| matches!(x, Delta::Mentioned { .. })));
}

#[test]
fn a_cache_from_an_older_build_still_loads() {
    // A store saved before a field existed must not be discarded (that forces a full READY on launch).
    let mut v = serde_json::to_value(Store::default()).unwrap();
    let o = v.as_object_mut().unwrap();
    assert!(o.remove("folders").is_some());
    o.remove("guild_positions");
    let s: Store = serde_json::from_value(v).expect("older cache loads");
    assert!(s.folders.is_empty());
}

#[test]
fn old_quiet_threads_leave_the_sidebar() {
    use crate::model::{Channel, ReadStateEntry, channel_type};
    let mut s = Store::default();
    let now = crate::fidelity::DISCORD_EPOCH_MS + 5_000_000_000u64;
    let at = |ms_ago: u64| crate::snowflake::Snowflake((now - ms_ago - crate::fidelity::DISCORD_EPOCH_MS) << 22);
    let thread = |id: u64, last: Option<Id>, joined: bool| Channel {
        id: crate::snowflake::Snowflake(id),
        kind: channel_type::PUBLIC_THREAD,
        last_message_id: last,
        member: joined.then(|| json!({})),
        ..Default::default()
    };
    let day = THREAD_SIDEBAR_WINDOW_MS;
    let fresh = thread(1, Some(at(1_000)), true);
    let stale = thread(2, Some(at(3 * day)), true);
    let stale_unjoined = thread(3, Some(at(1_000)), false);
    assert!(s.thread_in_sidebar(&fresh, now, None));
    assert!(!s.thread_in_sidebar(&stale, now, None), "quiet for days: hidden");
    assert!(!s.thread_in_sidebar(&stale_unjoined, now, None), "not joined: hidden");
    assert!(s.thread_in_sidebar(&stale, now, Some(crate::snowflake::Snowflake(2))), "the open one stays");
    s.read_states.insert(crate::snowflake::Snowflake(2), ReadStateEntry { id: crate::snowflake::Snowflake(2), last_message_id: Some(at(4 * day)), ..Default::default() });
    assert!(s.thread_in_sidebar(&stale, now, None), "unread: shown");
    let mut archived = thread(4, Some(at(1_000)), true);
    archived.thread_metadata = Some(json!({ "archived": true }));
    assert!(!s.thread_in_sidebar(&archived, now, None), "archived never lists");
    assert!(s.thread_in_sidebar(&archived, now, Some(crate::snowflake::Snowflake(4))), "except the one you are viewing");
}

#[test]
fn header_popups_know_when_they_have_content() {
    use crate::model::{Channel, channel_type};
    use crate::snowflake::Snowflake as S;
    let mut s = Store::default();
    let parent = S(10);
    s.channels.insert(parent, Channel { id: parent, kind: channel_type::GUILD_TEXT, ..Default::default() });
    assert!(!s.has_pins(parent));
    assert!(!s.has_active_threads(parent));
    s.channels.get_mut(&parent).unwrap().last_pin_timestamp = Some("2026-10-31T00:00:00+00:00".into());
    assert!(s.has_pins(parent));
    let t = |id: u64, archived: bool| Channel {
        id: S(id),
        kind: channel_type::PUBLIC_THREAD,
        parent_id: Some(parent),
        thread_metadata: Some(json!({ "archived": archived })),
        ..Default::default()
    };
    s.channels.insert(S(11), t(11, true));
    assert!(!s.has_active_threads(parent), "archived threads do not count");
    s.channels.insert(S(12), t(12, false));
    assert!(s.has_active_threads(parent));
    assert!(!s.has_active_threads(S(99)), "other channels' threads do not count");
}
