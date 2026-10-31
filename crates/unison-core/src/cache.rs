//! Per-account SQLite cache: the last session's state (for an instant cold start),
//! recent message history, a full-text index for local search, and cookies.

use std::path::Path;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{Id, Message};
use crate::snowflake::Snowflake;
use crate::store::Store;
use crate::Result;

/// Messages kept on disk per channel.
const DISK_MESSAGES_PER_CHANNEL: i64 = 500;

/// Bump when the text index layout changes; `migrate_fts` rebuilds it.
const FTS_VERSION: &[u8] = b"3";

pub struct Cache {
    conn: Mutex<Connection>,
    /// Hash of the last bytes written per snapshot key: an unchanged snapshot costs no compression,
    /// no SQLite write and no WAL growth.
    written: Mutex<std::collections::HashMap<&'static str, u64>>,
}

fn hash_bytes(b: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub channel: Id,
    pub message: Id,
    pub author: String,
    pub snippet: String,
}

/// Past this many matches the count reads "1000+" instead of walking the whole index.
pub const TOTAL_CAP: i64 = 1000;

/// What to look for in the local text index.
#[derive(Debug, Clone, Default)]
pub struct LocalQuery {
    /// Words typed; each matches as a prefix.
    pub terms: Vec<String>,
    /// Only these channels (`None`: all).
    pub channels: Option<Vec<Id>>,
    pub min_id: Option<u64>,
    pub max_id: Option<u64>,
    /// Author display-name fragments.
    pub authors: Vec<String>,
    pub offset: u32,
    pub limit: u32,
    pub want_total: bool,
}

#[derive(Debug, Clone)]
pub struct LocalHit {
    pub channel: Id,
    pub message: Id,
    pub author: String,
    pub content: String,
    /// The stored message (author id and avatar, attachments) when it is still on disk.
    pub full: Option<Message>,
}

#[derive(Debug, Clone, Default)]
pub struct LocalPage {
    pub hits: Vec<LocalHit>,
    /// Matches in all (counted only when `want_total`), at most `TOTAL_CAP`.
    pub total: u32,
    pub capped: bool,
}

impl Cache {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA mmap_size=67108864;
             PRAGMA temp_store=MEMORY;
             CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v BLOB NOT NULL);
             CREATE TABLE IF NOT EXISTS messages (
                channel INTEGER NOT NULL, id INTEGER NOT NULL, data BLOB NOT NULL,
                PRIMARY KEY (channel, id)) WITHOUT ROWID;",
        )?;
        Self::migrate_fts(&conn)?;
        Ok(Self { conn: Mutex::new(conn), written: Mutex::new(Default::default()) })
    }

    /// The text index is keyed by message id (its rowid), so deleting and ordering newest-first
    /// are index lookups, and it carries 1-3 letter prefix tables so the first keystroke is a
    /// lookup, not a scan. Older caches are rebuilt once from the stored messages.
    fn migrate_fts(conn: &Connection) -> Result<()> {
        let version: Option<Vec<u8>> = conn.query_row("SELECT v FROM kv WHERE k = 'fts_version'", [], |r| r.get(0)).optional()?;
        if version.as_deref() == Some(FTS_VERSION) {
            return Ok(());
        }
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(
            "DROP TABLE IF EXISTS messages_fts;
             CREATE VIRTUAL TABLE messages_fts USING fts5(
                content, author UNINDEXED, channel,
                tokenize='unicode61 remove_diacritics 2', prefix='1 2 3');",
        )?;
        {
            let mut ins = tx.prepare("INSERT INTO messages_fts (rowid, content, author, channel) VALUES (?1, ?2, ?3, ?4)")?;
            let mut all = tx.prepare("SELECT channel, id, data FROM messages")?;
            let mut rows = all.query([])?;
            while let Some(r) = rows.next()? {
                let (channel, id, data): (i64, i64, Vec<u8>) = (r.get(0)?, r.get(1)?, r.get(2)?);
                if let Ok(m) = serde_json::from_slice::<Message>(&data) {
                    if !m.content.is_empty() {
                        ins.execute(params![id, m.content, m.author.display_name(), channel])?;
                    }
                }
            }
        }
        tx.execute("INSERT OR REPLACE INTO kv (k, v) VALUES ('fts_version', ?1)", [FTS_VERSION])?;
        tx.commit()?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.conn.lock().query_row("SELECT v FROM kv WHERE k = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn put(&self, key: &str, value: &[u8]) -> Result<()> {
        self.conn.lock().execute("INSERT OR REPLACE INTO kv (k, v) VALUES (?1, ?2)", params![key, value])?;
        Ok(())
    }

    /// `put`, skipped when the same bytes were the last ones written under `key` by this process.
    pub fn put_if_changed(&self, key: &'static str, value: &[u8]) -> Result<bool> {
        let h = hash_bytes(value);
        if self.written.lock().get(key) == Some(&h) {
            return Ok(false);
        }
        self.put(key, value)?;
        self.written.lock().insert(key, h);
        Ok(true)
    }

    pub fn get_string(&self, key: &str) -> Result<Option<String>> {
        Ok(self.get(key)?.map(|v| String::from_utf8_lossy(&v).into_owned()))
    }

    /// Snapshot of guilds/channels/read states, zstd-compressed JSON.
    pub fn save_store(&self, store: &Store) -> Result<()> {
        let json = serde_json::to_vec(store)?;
        let h = hash_bytes(&json);
        if self.written.lock().get("store") == Some(&h) {
            return Ok(());
        }
        let mut out = vec![0u8; zstd_safe::compress_bound(json.len())];
        let n = zstd_safe::compress(&mut out[..], &json, 3).map_err(|c| crate::Error::Cache(zstd_safe::get_error_name(c).into()))?;
        out.truncate(n);
        self.put("store", &out)?;
        self.written.lock().insert("store", h);
        Ok(())
    }

    pub fn load_store(&self) -> Result<Option<Store>> {
        let Some(blob) = self.get("store")? else { return Ok(None) };
        let size = zstd_safe::get_frame_content_size(&blob).ok().flatten().unwrap_or(64 << 20) as usize;
        let mut out = vec![0u8; size];
        let n = zstd_safe::decompress(&mut out[..], &blob).map_err(|c| crate::Error::Cache(zstd_safe::get_error_name(c).into()))?;
        out.truncate(n);
        // An undecodable store (e.g. after a model change) just means no launch paint; say so.
        match serde_json::from_slice(&out) {
            Ok(s) => Ok(Some(s)),
            Err(e) => {
                tracing::warn!("cached store unreadable, waiting for READY: {e}");
                Ok(None)
            }
        }
    }

    pub fn save_messages(&self, channel: Id, messages: &[Message]) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut put = tx.prepare_cached("INSERT OR REPLACE INTO messages (channel, id, data) VALUES (?1, ?2, ?3)")?;
            let mut del_fts = tx.prepare_cached("DELETE FROM messages_fts WHERE rowid = ?1")?;
            let mut fts = tx.prepare_cached("INSERT INTO messages_fts (rowid, content, author, channel) VALUES (?1, ?2, ?3, ?4)")?;
            for m in messages.iter().filter(|m| m.id.0 != 0) {
                put.execute(params![channel.0 as i64, m.id.0 as i64, serde_json::to_vec(m)?])?;
                del_fts.execute([m.id.0 as i64])?;
                if !m.content.is_empty() {
                    fts.execute(params![m.id.0 as i64, m.content, m.author.display_name(), channel.0 as i64])?;
                }
            }
        }
        tx.execute(
            "DELETE FROM messages WHERE channel = ?1 AND id NOT IN
               (SELECT id FROM messages WHERE channel = ?1 ORDER BY id DESC LIMIT ?2)",
            params![channel.0 as i64, DISK_MESSAGES_PER_CHANNEL],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_message(&self, channel: Id, id: Id) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM messages WHERE channel = ?1 AND id = ?2", params![channel.0 as i64, id.0 as i64])?;
        conn.execute("DELETE FROM messages_fts WHERE rowid = ?1", [id.0 as i64])?;
        Ok(())
    }

    /// Newest first, like the REST endpoint.
    pub fn load_messages(&self, channel: Id, limit: u32) -> Result<Vec<Message>> {
        let conn = self.conn.lock();
        let mut q = conn.prepare_cached("SELECT data FROM messages WHERE channel = ?1 ORDER BY id DESC LIMIT ?2")?;
        let rows = q.query_map(params![channel.0 as i64, limit], |r| r.get::<_, Vec<u8>>(0))?;
        Ok(rows.filter_map(|r| r.ok()).filter_map(|b| serde_json::from_slice(&b).ok()).collect())
    }

    /// The stored messages around `message` (up to `half` before it and `half` after), newest
    /// first, when `message` itself is stored; empty otherwise. A jump to a message we have seen
    /// before can paint this window at once instead of waiting for `?around=`.
    pub fn load_around(&self, channel: Id, message: Id, half: u32) -> Result<Vec<Message>> {
        let conn = self.conn.lock();
        let read = |sql: &str, limit: u32| -> Result<Vec<Message>> {
            let mut q = conn.prepare_cached(sql)?;
            let rows = q.query_map(params![channel.0 as i64, message.0 as i64, limit], |r| r.get::<_, Vec<u8>>(0))?;
            Ok(rows.filter_map(|r| r.ok()).filter_map(|b| serde_json::from_slice(&b).ok()).collect())
        };
        let at_and_before = read("SELECT data FROM messages WHERE channel = ?1 AND id <= ?2 ORDER BY id DESC LIMIT ?3", half + 1)?;
        if at_and_before.first().map(|m| m.id) != Some(message) {
            return Ok(Vec::new());
        }
        let mut after = read("SELECT data FROM messages WHERE channel = ?1 AND id > ?2 ORDER BY id ASC LIMIT ?3", half)?;
        after.reverse();
        after.extend(at_and_before);
        Ok(after)
    }

    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>> {
        let q = LocalQuery { terms: query.split_whitespace().map(str::to_string).collect(), limit, ..Default::default() };
        Ok(self.search_page(&q)?.hits.into_iter().map(|h| SearchHit { channel: h.channel, message: h.message, author: h.author, snippet: h.content }).collect())
    }

    /// One page of local matches, newest first: every word is a prefix match (so "batt" finds
    /// "battery", accents and case ignored), narrowed by channel, id range and author. Cost is an
    /// index walk, not a scan, so it stays fast on a large cache.
    pub fn search_page(&self, q: &LocalQuery) -> Result<LocalPage> {
        let terms: Vec<String> = q
            .terms
            .iter()
            .filter(|t| t.chars().any(char::is_alphanumeric))
            .map(|t| format!("\"{}\"*", t.replace('"', "")))
            .collect();
        let mut wh: Vec<String> = Vec::new();
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        // The channel is an indexed column too, so a scope narrows the index walk itself.
        let mut m: Vec<String> = Vec::new();
        if !terms.is_empty() {
            m.push(format!("content : ({})", terms.join(" ")));
        } else if q.channels.is_none() && q.authors.is_empty() && q.min_id.is_none() && q.max_id.is_none() {
            return Ok(LocalPage::default());
        }
        if let Some(cs) = &q.channels {
            if cs.is_empty() {
                return Ok(LocalPage::default());
            }
            m.push(format!("channel : ({})", cs.iter().map(|c| format!("\"{}\"", c.0)).collect::<Vec<_>>().join(" OR ")));
        }
        if !m.is_empty() {
            wh.push("messages_fts MATCH ?".into());
            args.push(m.join(" AND ").into());
        }
        if let Some(i) = q.min_id {
            wh.push("rowid >= ?".into());
            args.push((i as i64).into());
        }
        if let Some(i) = q.max_id {
            wh.push("rowid <= ?".into());
            args.push((i as i64).into());
        }
        for a in &q.authors {
            wh.push("author LIKE ? ESCAPE '\\'".into());
            args.push(format!("%{}%", a.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")).into());
        }
        let wh = if wh.is_empty() { String::new() } else { format!(" WHERE {}", wh.join(" AND ")) };
        let conn = self.conn.lock();
        let mut st = conn.prepare_cached(&format!("SELECT rowid, channel, author, content FROM messages_fts{wh} ORDER BY rowid DESC LIMIT {} OFFSET {}", q.limit, q.offset))?;
        let rows = st.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok(LocalHit { message: Snowflake(r.get::<_, i64>(0)? as u64), channel: Snowflake(r.get::<_, i64>(1)? as u64), author: r.get(2)?, content: r.get(3)?, full: None })
        })?;
        let mut hits: Vec<LocalHit> = rows.filter_map(|r| r.ok()).collect();
        let mut by_pk = conn.prepare_cached("SELECT data FROM messages WHERE channel = ?1 AND id = ?2")?;
        for h in &mut hits {
            h.full = by_pk
                .query_row(params![h.channel.0 as i64, h.message.0 as i64], |r| r.get::<_, Vec<u8>>(0))
                .optional()?
                .and_then(|b| serde_json::from_slice(&b).ok());
        }
        let (mut total, mut capped) = (hits.len() as u32 + q.offset, false);
        if q.want_total {
            let n: i64 = conn.query_row(&format!("SELECT count(*) FROM (SELECT 1 FROM messages_fts{wh} LIMIT {})", TOTAL_CAP + 1), rusqlite::params_from_iter(args.iter()), |r| r.get(0))?;
            capped = n > TOTAL_CAP;
            total = n.min(TOTAL_CAP) as u32;
        }
        Ok(LocalPage { hits, total, capped })
    }

    pub fn wipe(&self) -> Result<()> {
        self.conn.lock().execute_batch("DELETE FROM kv; DELETE FROM messages; DELETE FROM messages_fts;")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::User;

    fn msg(id: u64, text: &str) -> Message {
        Message {
            id: Snowflake(id),
            channel_id: Snowflake(1),
            content: text.into(),
            author: User { id: Snowflake(9), username: "bob".into(), ..Default::default() },
            ..Default::default()
        }
    }

    #[test]
    fn messages_round_trip_and_search() {
        let c = Cache::in_memory().unwrap();
        c.save_messages(Snowflake(1), &[msg(1, "hello battery world"), msg(2, "zen garden")]).unwrap();
        let loaded = c.load_messages(Snowflake(1), 10).unwrap();
        assert_eq!(loaded.iter().map(|m| m.id.0).collect::<Vec<_>>(), vec![2, 1]);
        let hits = c.search("batt", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message, Snowflake(1));
        assert!(c.search("\" OR 1=1", 10).unwrap().is_empty());
        c.delete_message(Snowflake(1), Snowflake(1)).unwrap();
        assert!(c.search("battery", 10).unwrap().is_empty());
    }

    fn msg_in(channel: u64, id: u64, who: &str, text: &str) -> Message {
        Message { id: Snowflake(id), channel_id: Snowflake(channel), content: text.into(), author: User { id: Snowflake(9), username: who.into(), ..Default::default() }, ..Default::default() }
    }

    fn q(words: &str) -> LocalQuery {
        LocalQuery { terms: words.split_whitespace().map(str::to_string).collect(), limit: 25, want_total: true, ..Default::default() }
    }

    #[test]
    fn search_is_prefix_diacritic_blind_and_newest_first() {
        let c = Cache::in_memory().unwrap();
        c.save_messages(Snowflake(1), &[msg_in(1, 10, "bob", "Café au lait"), msg_in(1, 30, "bob", "cafe racer"), msg_in(1, 20, "ann", "coffee")]).unwrap();
        let ids = |p: LocalPage| p.hits.iter().map(|h| h.message.0).collect::<Vec<_>>();
        assert_eq!(ids(c.search_page(&q("caf")).unwrap()), [30, 10], "prefix, newest first, accents folded");
        assert_eq!(ids(c.search_page(&q("CAFE au")).unwrap()), [10], "all words must match");
        assert!(c.search_page(&q("zzz")).unwrap().hits.is_empty());
        assert!(c.search_page(&q("")).unwrap().hits.is_empty(), "no words and no filter: nothing");
        assert!(c.search_page(&q("\"*(")).unwrap().hits.is_empty(), "syntax is never injected");
    }

    #[test]
    fn search_scopes_filters_and_pages() {
        let c = Cache::in_memory().unwrap();
        let a: Vec<Message> = (1..=60).map(|i| msg_in(1, i * 10, "bob", "hello there")).collect();
        let b: Vec<Message> = (1..=5).map(|i| msg_in(2, i * 10 + 5, "ann", "hello again")).collect();
        c.save_messages(Snowflake(1), &a).unwrap();
        c.save_messages(Snowflake(2), &b).unwrap();
        let all = c.search_page(&q("hello")).unwrap();
        assert_eq!((all.hits.len(), all.total, all.capped), (25, 65, false));
        let mut one = q("hello");
        one.channels = Some(vec![Snowflake(2)]);
        assert_eq!(c.search_page(&one).unwrap().total, 5);
        let mut page2 = q("hello");
        page2.offset = 25;
        let p2 = c.search_page(&page2).unwrap();
        assert_eq!(p2.hits.len(), 25);
        assert!(p2.hits[0].message.0 < all.hits[24].message.0, "pages continue where the last ended");
        let mut ranged = q("hello");
        ranged.min_id = Some(100);
        ranged.max_id = Some(200);
        // Channel 1 holds 100, 110 .. 200 (11 messages); channel 2's ids (15 .. 55) are all older.
        assert_eq!(c.search_page(&ranged).unwrap().total, 11);
        let mut by = q("hello");
        by.authors = vec!["AN".into()];
        assert_eq!(c.search_page(&by).unwrap().total, 5);
        // A filter alone (no words) lists that author's messages.
        let mut only = q("");
        only.authors = vec!["ann".into()];
        assert_eq!(c.search_page(&only).unwrap().total, 5);
        assert!(c.search_page(&LocalQuery { channels: Some(vec![]), ..q("hello") }).unwrap().hits.is_empty());
    }

    #[test]
    fn totals_cap_and_resave_replaces() {
        let c = Cache::in_memory().unwrap();
        let big: Vec<Message> = (1..=1200).map(|i| msg_in(1, i, "bob", "spam spam")).collect();
        c.save_messages(Snowflake(1), &big[..600]).unwrap();
        c.save_messages(Snowflake(2), &big[600..].iter().map(|m| Message { channel_id: Snowflake(2), ..m.clone() }).collect::<Vec<_>>()).unwrap();
        let p = c.search_page(&q("spam")).unwrap();
        assert_eq!((p.total, p.capped), (1000, true));
        // Saving a message again replaces its index entry, never doubling it.
        c.save_messages(Snowflake(2), &[msg_in(2, 1200, "bob", "edited")]).unwrap();
        assert_eq!(c.search_page(&q("edited")).unwrap().total, 1);
        assert!(c.search_page(&q("spam")).unwrap().hits.iter().all(|h| h.message.0 != 1200));
    }

    #[test]
    fn an_old_index_is_rebuilt_from_stored_messages() {
        let c = Cache::in_memory().unwrap();
        c.save_messages(Snowflake(1), &[msg_in(1, 10, "bob", "needle in a haystack")]).unwrap();
        {
            let conn = c.conn.lock();
            conn.execute("DELETE FROM kv WHERE k = 'fts_version'", []).unwrap();
            conn.execute("DELETE FROM messages_fts", []).unwrap();
            Cache::migrate_fts(&conn).unwrap();
        }
        assert_eq!(c.search_page(&q("needl")).unwrap().total, 1);
    }

    /// `cargo test -p unison-core --release cache::tests::bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_search_on_a_large_cache() {
        use std::time::Instant;
        let c = Cache::in_memory().unwrap();
        let words = ["battery", "garden", "release", "voice", "channel", "thread", "build", "server", "update", "crash", "design", "latency", "avatar", "reply", "sticker"];
        let mut seed = 7u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };
        let (channels, per) = (400u64, 500u64);
        for ch in 1..=channels {
            let batch: Vec<Message> = (1..=per)
                .map(|i| {
                    let text = (0..8).map(|_| words[next() % words.len()]).collect::<Vec<_>>().join(" ");
                    msg_in(ch, ch * 100_000 + i, "bob", &text)
                })
                .collect();
            c.save_messages(Snowflake(ch), &batch).unwrap();
        }
        println!("indexed {} messages", channels * per);
        for query in ["b", "ba", "bat", "batt", "battery", "battery garden", "zzzq", "thr rel"] {
            let mut times: Vec<u128> = (0..30)
                .map(|_| {
                    let mut x = q(query);
                    x.want_total = false;
                    let t = Instant::now();
                    c.search_page(&x).unwrap();
                    t.elapsed().as_micros()
                })
                .collect();
            times.sort();
            let t = Instant::now();
            c.search_page(&q(query)).unwrap();
            println!("{query:>16}: p50 {:>6} us  p95 {:>6} us  with total {:>6} us", times[15], times[28], t.elapsed().as_micros());
        }
        let mut scoped = q("battery");
        scoped.channels = Some((1..=3).map(Snowflake).collect());
        let t = Instant::now();
        c.search_page(&scoped).unwrap();
        println!("3-channel scope: {} us", t.elapsed().as_micros());
    }

    #[test]
    fn store_snapshot_round_trip() {
        let c = Cache::in_memory().unwrap();
        let mut s = Store::default();
        s.me = Some(User { id: Snowflake(5), username: "me".into(), ..Default::default() });
        s.private_channels = vec![Snowflake(3)];
        c.save_store(&s).unwrap();
        let back = c.load_store().unwrap().unwrap();
        assert_eq!(back.me.unwrap().id, Snowflake(5));
        assert_eq!(back.private_channels, vec![Snowflake(3)]);
    }
}
