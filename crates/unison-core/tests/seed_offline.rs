//! Fills a COPY of an offline dev instance's account cache with a large, varied synthetic world
//! (many servers and channels, people with accents and CJK names, DMs, friends, unread and muted
//! places, and hundreds of thousands of messages) so the search surface can be judged and timed on
//! a big cache. Never point it at a real data directory.
//!
//! SEED_DB=/tmp/x/data/account-<id>.sqlite SEED_MESSAGES=200000 \
//!   cargo test -p unison-core --release --test seed_offline -- --ignored --nocapture

use unison_core::cache::Cache;
use unison_core::model::{Channel, Guild, Message, Relationship, User};
use unison_core::snowflake::Snowflake;

const EPOCH: u64 = 1_420_070_400_000;

fn flake(ms: u64, seq: u64) -> Snowflake {
    Snowflake(((ms - EPOCH) << 22) | (seq & 0x3fffff))
}

#[test]
#[ignore]
fn seed() {
    let path = std::env::var("SEED_DB").expect("SEED_DB");
    let want: u64 = std::env::var("SEED_MESSAGES").ok().and_then(|v| v.parse().ok()).unwrap_or(200_000);
    let cache = Cache::open(std::path::Path::new(&path)).unwrap();
    let mut store = cache.load_store().unwrap().expect("cached store");
    let me = store.me.as_ref().expect("me").id;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let template_guild: Guild = store.guilds.values().next().expect("a guild").clone();
    let template_channel: Channel = store.channels.values().find(|c| c.kind == 0 && c.guild_id.is_some()).expect("a text channel").clone();

    let first = ["Zoë", "Émile", "Łukasz", "Søren", "Ana", "Björn", "Chloé", "Dmitri", "Elif", "Fatima", "Gaëlle", "Hiro", "Ingrid", "Jürgen", "Kofi", "Léa", "Mateo", "Nikola", "Olu", "Pia", "田中", "山田", "Иван", "Мария", "김민", "Ștefan", "Renée", "Tomás"];
    let last = ["Müller", "García", "Nowak", "Okafor", "Sato", "Ivanov", "Dubois", "Kim", "Silva", "Rossi", "Andersson", "Haddad", "Lee", "Costa", "Novak", "Petrov"];
    let mut users: Vec<User> = Vec::new();
    for i in 0..400u64 {
        let name = format!("{} {}", first[i as usize % first.len()], last[(i as usize / 3) % last.len()]);
        let u = User { id: Snowflake(7_000_000_000_000_000 + i), username: format!("user{i}"), global_name: Some(name), ..Default::default() };
        store.users.insert(u.id, u.clone());
        users.push(u);
    }

    // Servers.
    let guild_names = [
        "Unison HQ", "東京ゲーム部", "Zürich Café Crew", "Привет мир", "Rust Lang Community Discord Server For Everyone Who Likes Crabs And Borrow Checkers", "Design Systems", "Müşteri Destek", "Photography Club", "Open Source Weekend", "Lo-fi Beats",
        "한국 개발자 모임", "Café de Paris", "Retro Games", "Linux Users", "Piano & Keys", "Ελληνική Κοινότητα", "Climbing Buddies", "Kitchen Table", "Pixel Art", "Robotics Lab",
        "Board Games Night", "Chess Club", "Anime Corner", "Film Society", "Book Nook", "Garden Gnomes", "Mechanical Keyboards", "Space Nerds", "Language Exchange", "Startup Founders",
    ];
    let chan_names = [
        "general", "off-topic", "announcements", "dev-talk", "design", "random", "memes", "music", "help", "showcase", "releases", "bugs", "ideas", "voice-chat", "café", "雑談", "обсуждение", "일반", "battery-life", "build-status", "reviews", "support", "introductions", "events", "resources",
        "photos", "gaming", "cooking", "travel", "pets",
    ];
    let mut text_channels: Vec<Id> = Vec::new();
    type Id = Snowflake;
    let mut seq = 1u64;
    for (gi, gname) in guild_names.iter().enumerate() {
        let gid = flake(now - 400 * 86_400_000 + gi as u64, 1000 + gi as u64);
        let mut g = template_guild.clone();
        g.id = gid;
        g.properties = None;
        g.inline.name = Some((*gname).to_string());
        g.inline.owner_id = Some(me);
        g.inline.icon = None;
        g.channels.clear();
        g.threads.clear();
        g.members.clear();
        g.voice_states.clear();
        let mut ids = Vec::new();
        for (ci, cname) in chan_names.iter().enumerate() {
            seq += 1;
            let cid = flake(now - 300 * 86_400_000 + seq, seq);
            let mut c = template_channel.clone();
            c.id = cid;
            c.guild_id = Some(gid);
            c.name = Some(if ci == 0 { "general".to_string() } else { (*cname).to_string() });
            c.parent_id = None;
            c.position = ci as i32;
            c.permission_overwrites.clear();
            c.last_message_id = Some(flake(now - (seq % 90) * 3_600_000, seq));
            if ci == 13 {
                c.kind = 2;
            }
            ids.push(cid);
            if c.kind == 0 {
                text_channels.push(cid);
            }
            store.channels.insert(cid, c);
        }
        store.guild_channels.insert(gid, ids);
        store.guilds.insert(gid, g);
    }
    // Unread, mentions, and one muted server.
    let ids: Vec<Id> = text_channels.clone();
    for (n, cid) in ids.iter().enumerate() {
        let last = store.channels[cid].last_message_id;
        let read = if n % 11 == 0 { Some(flake(now - 400 * 86_400_000, 1)) } else { last };
        let mentions = if n % 37 == 0 { (n as u32 % 5) + 1 } else { 0 };
        store.read_states.insert(*cid, unison_core::model::ReadStateEntry { id: *cid, last_message_id: read, mention_count: mentions, ..Default::default() });
    }
    if let Some(g) = guild_names.iter().position(|n| *n == "Lo-fi Beats").and_then(|i| store.guilds.keys().copied().filter(|k| k.0 > 0).nth(0).map(|_| i)) {
        let gid = *store.guilds.keys().find(|k| store.guilds[k].props().name.as_deref() == Some(guild_names[g])).unwrap();
        store.guild_settings.insert(gid, unison_core::model::UserGuildSettings { guild_id: Some(gid), muted: true, ..Default::default() });
    }

    // DMs and friends.
    let mut dm_template: Option<Channel> = None;
    for id in &store.private_channels {
        if let Some(c) = store.channels.get(id) {
            if c.kind == 1 {
                dm_template = Some(c.clone());
                break;
            }
        }
    }
    let dm_template = dm_template.unwrap_or_else(|| Channel { kind: 1, ..Default::default() });
    for (i, u) in users.iter().take(25).enumerate() {
        seq += 1;
        let cid = flake(now - 50 * 86_400_000 + seq, seq);
        let mut c = dm_template.clone();
        c.id = cid;
        c.kind = 1;
        c.guild_id = None;
        c.name = None;
        c.recipient_ids = vec![u.id];
        c.recipients = vec![u.clone()];
        c.last_message_id = Some(flake(now - (i as u64) * 7_200_000, seq));
        store.channels.insert(cid, c);
        store.private_channels.push(cid);
        text_channels.push(cid);
        if i % 4 == 0 {
            store.read_states.insert(cid, unison_core::model::ReadStateEntry { id: cid, last_message_id: Some(flake(now - 99 * 86_400_000, 2)), mention_count: (i as u32 % 3) + 1, ..Default::default() });
        }
    }
    for u in users.iter().skip(25).take(40) {
        store.relationships.push(Relationship { id: u.id, kind: 1, user_id: Some(u.id), user: Some(u.clone()), nickname: None });
    }

    // Messages.
    let words = [
        "battery", "garden", "release", "voice", "channel", "thread", "build", "server", "update", "crash", "design", "latency", "avatar", "reply", "sticker", "café", "résumé", "naïve", "über", "señor", "deploy", "review", "merge", "ship", "lunch", "weekend", "meeting", "photo", "music", "game",
        "今日は", "ありがとう", "привет", "спасибо", "고맙습니다", "the", "and", "with", "for", "you", "this", "that", "was", "are",
    ];
    let mut rng = 0x2545F4914F6CDD1Du64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let per = 500u64;
    let channels_to_fill = ((want / per) as usize).min(text_channels.len());
    let mut written = 0u64;
    for (k, cid) in text_channels.iter().take(channels_to_fill).enumerate() {
        let guild = store.channels[cid].guild_id;
        let batch: Vec<Message> = (0..per)
            .map(|i| {
                let author = &users[(next() % users.len() as u64) as usize];
                let mut text = (0..(3 + next() % 14)).map(|_| words[(next() % words.len() as u64) as usize]).collect::<Vec<_>>().join(" ");
                if next() % 17 == 0 {
                    text = format!("hey <@{}> {} <#{}> check this out", users[(next() % 50) as usize].id, text, cid);
                }
                if next() % 211 == 0 {
                    text = format!("{} {}", "lorem ipsum dolor sit amet ".repeat(14), text);
                }
                let ms = now - (per - i) * 600_000 - (k as u64) * 60_000;
                Message { id: flake(ms, next() % 4000), channel_id: *cid, content: text, author: author.clone(), ..Default::default() }
            })
            .collect();
        written += batch.len() as u64;
        cache.save_messages(*cid, &batch).unwrap();
        let _ = guild;
    }
    // A known needle in a known place, for visual checks.
    let general = text_channels[0];
    cache
        .save_messages(general, &[Message { id: flake(now - 1000, 1), channel_id: general, content: "the battery on <@7000000000000000003> went flat, résumé of the café meeting attached".into(), author: users[3].clone(), ..Default::default() }])
        .unwrap();
    cache.save_store(&store).unwrap();
    println!("seeded: {} guilds, {} channels, {} messages, {} users", store.guilds.len(), store.channels.len(), written, store.users.len());
}
