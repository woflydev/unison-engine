//! Per-account SQLite cache: the last session's state (for an instant cold start),
//! recent message history, a full-text index for local search, and cookies.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{Id, Message};
use crate::msgcodec::{self, Codec};
use crate::snowflake::Snowflake;
use crate::store::Store;
use crate::Result;

/// Messages kept on disk per channel unless the search-history level says otherwise.
const DISK_MESSAGES_PER_CHANNEL: i64 = 500;

/// Dictionary training needs this many stored messages to learn anything.
const DICT_MIN_ROWS: i64 = 300;
/// Channels most recently used are never evicted to meet the size budget.
const KEEP_CHANNELS: i64 = 5;

/// Bump when the text index layout changes; `migrate_fts` rebuilds it.
const FTS_VERSION: &[u8] = b"3";

pub struct Cache {
    conn: Mutex<Connection>,
    codec: Mutex<Codec>,
    /// Size the cache may use, in bytes (0: no limit), and messages kept per channel.
    budget: AtomicU64,
    per_channel: AtomicI64,
    dict_tried: AtomicBool,
    /// Hash of the last bytes written per snapshot key: an unchanged snapshot costs no compression,
    /// no SQLite write and no WAL growth.
    written: Mutex<std::collections::HashMap<&'static str, u64>>,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
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
                PRIMARY KEY (channel, id)) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS channel_seen (channel INTEGER PRIMARY KEY, at INTEGER NOT NULL);",
        )?;
        let mut codec = Codec::default();
        if let Some(d) = conn.query_row("SELECT v FROM kv WHERE k = 'msg_dict'", [], |r| r.get::<_, Vec<u8>>(0)).optional()? {
            codec.set_dict(&d);
        }
        Self::migrate_fts(&conn, &mut codec)?;
        Ok(Self {
            conn: Mutex::new(conn),
            codec: Mutex::new(codec),
            budget: AtomicU64::new(0),
            per_channel: AtomicI64::new(DISK_MESSAGES_PER_CHANNEL),
            dict_tried: AtomicBool::new(false),
            written: Mutex::new(Default::default()),
        })
    }

    /// The text index is keyed by message id (its rowid), so deleting and ordering newest-first
    /// are index lookups, and it carries 1-3 letter prefix tables so the first keystroke is a
    /// lookup, not a scan. Older caches are rebuilt once from the stored messages.
    fn migrate_fts(conn: &Connection, codec: &mut Codec) -> Result<()> {
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
                if let Some(m) = codec.decode(&data).and_then(|j| serde_json::from_slice::<Message>(&j).ok()) {
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

    /// A stored row as a message (rows are zstd frames, or plain JSON from older versions).
    fn unpack(codec: &mut Codec, blob: &[u8]) -> Option<Message> {
        serde_json::from_slice(&codec.decode(blob)?).ok()
    }

    /// A message as the bytes to store.
    fn pack(codec: &mut Codec, m: &Message) -> Result<Vec<u8>> {
        let json = serde_json::to_vec(m)?;
        Ok(codec.encode(&json).unwrap_or(json))
    }

    pub fn save_messages(&self, channel: Id, messages: &[Message]) -> Result<()> {
        {
            let mut conn = self.conn.lock();
            let tx = conn.transaction()?;
            {
                let mut codec = self.codec.lock();
                let mut put = tx.prepare_cached("INSERT OR REPLACE INTO messages (channel, id, data) VALUES (?1, ?2, ?3)")?;
                let mut del_fts = tx.prepare_cached("DELETE FROM messages_fts WHERE rowid = ?1")?;
                let mut fts = tx.prepare_cached("INSERT INTO messages_fts (rowid, content, author, channel) VALUES (?1, ?2, ?3, ?4)")?;
                for m in messages.iter().filter(|m| m.id.0 != 0) {
                    put.execute(params![channel.0 as i64, m.id.0 as i64, Self::pack(&mut codec, m)?])?;
                    del_fts.execute([m.id.0 as i64])?;
                    if !m.content.is_empty() {
                        fts.execute(params![m.id.0 as i64, m.content, m.author.display_name(), channel.0 as i64])?;
                    }
                }
            }
            tx.execute(
                "DELETE FROM messages WHERE channel = ?1 AND id NOT IN
                   (SELECT id FROM messages WHERE channel = ?1 ORDER BY id DESC LIMIT ?2)",
                params![channel.0 as i64, self.per_channel.load(Ordering::Relaxed)],
            )?;
            tx.execute("INSERT OR REPLACE INTO channel_seen (channel, at) VALUES (?1, ?2)", params![channel.0 as i64, now_secs()])?;
            tx.commit()?;
        }
        self.maybe_train()?;
        self.enforce_budget()
    }

    /// Messages Discord's own search returned: they are already in hand, so they join the index.
    /// Unlike `save_messages` this keeps older ones (a hit is usually far back) and never replaces a
    /// row already stored, which may hold more (member, guild). The size budget still applies.
    pub fn index_found(&self, channel: Id, messages: &[Message]) -> Result<()> {
        {
            let mut conn = self.conn.lock();
            let tx = conn.transaction()?;
            {
                let mut codec = self.codec.lock();
                let mut put = tx.prepare_cached("INSERT OR IGNORE INTO messages (channel, id, data) VALUES (?1, ?2, ?3)")?;
                let mut fts = tx.prepare_cached("INSERT INTO messages_fts (rowid, content, author, channel) VALUES (?1, ?2, ?3, ?4)")?;
                for m in messages.iter().filter(|m| m.id.0 != 0) {
                    let added = put.execute(params![channel.0 as i64, m.id.0 as i64, Self::pack(&mut codec, m)?])?;
                    if added > 0 && !m.content.is_empty() {
                        fts.execute(params![m.id.0 as i64, m.content, m.author.display_name(), channel.0 as i64])?;
                    }
                }
            }
            tx.execute("INSERT OR IGNORE INTO channel_seen (channel, at) VALUES (?1, ?2)", params![channel.0 as i64, now_secs()])?;
            tx.commit()?;
        }
        self.maybe_train()?;
        self.enforce_budget()
    }

    /// Note that the person looked at `channel` (it is evicted last).
    pub fn touch_channels(&self, channels: &[Id]) -> Result<()> {
        let conn = self.conn.lock();
        let mut st = conn.prepare_cached("INSERT OR REPLACE INTO channel_seen (channel, at) VALUES (?1, ?2)")?;
        for c in channels {
            st.execute(params![c.0 as i64, now_secs()])?;
        }
        Ok(())
    }

    /// Once there are enough stored messages, learn a dictionary from them and rewrite every row
    /// with it (about 9x smaller than the JSON), then give the freed pages back to the disk.
    fn maybe_train(&self) -> Result<()> {
        if self.codec.lock().has_dict() || self.dict_tried.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        let rows: i64 = conn.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?;
        if rows < DICT_MIN_ROWS {
            self.dict_tried.store(false, Ordering::Relaxed);
            return Ok(());
        }
        let mut codec = self.codec.lock();
        let samples: Vec<Vec<u8>> = {
            let mut q = conn.prepare("SELECT data FROM messages ORDER BY id DESC LIMIT 2000")?;
            let all = q.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
            all.filter_map(|r| r.ok()).filter_map(|b| codec.decode(&b)).collect()
        };
        let Some(dict) = msgcodec::train(&samples) else { return Ok(()) };
        let tx = conn.transaction()?;
        tx.execute("INSERT OR REPLACE INTO kv (k, v) VALUES ('msg_dict', ?1)", [&dict])?;
        let old = std::mem::take(&mut *codec);
        let mut next = Codec::default();
        next.set_dict(&dict);
        {
            let mut old = old;
            let all: Vec<(i64, i64, Vec<u8>)> = {
                let mut q = tx.prepare("SELECT channel, id, data FROM messages")?;
                let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                rows.filter_map(|r| r.ok()).collect()
            };
            let mut put = tx.prepare("UPDATE messages SET data = ?3 WHERE channel = ?1 AND id = ?2")?;
            for (c, i, blob) in all {
                if let Some(json) = old.decode(&blob) {
                    if let Some(packed) = next.encode(&json) {
                        put.execute(params![c, i, packed])?;
                    }
                }
            }
        }
        tx.commit()?;
        *codec = next;
        // Existing files were created without incremental vacuum; one full pass turns it on and
        // returns the space the rewrite freed.
        let _ = conn.execute_batch("PRAGMA auto_vacuum=INCREMENTAL; VACUUM;");
        Ok(())
    }

    /// Bytes the cache holds right now (live pages, not counting space waiting to be reused).
    pub fn used_bytes(&self) -> Result<u64> {
        let conn = self.conn.lock();
        let (pages, free, size): (i64, i64, i64) = conn.query_row(
            "SELECT (SELECT page_count FROM pragma_page_count), (SELECT freelist_count FROM pragma_freelist_count), (SELECT page_size FROM pragma_page_size)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(((pages - free).max(0) * size) as u64)
    }

    pub fn message_count(&self) -> Result<u64> {
        Ok(self.conn.lock().query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))? as u64)
    }

    /// `bytes` of room (0: no limit) and `per_channel` messages kept for each channel.
    pub fn set_limits(&self, bytes: u64, per_channel: i64) {
        self.budget.store(bytes, Ordering::Relaxed);
        self.per_channel.store(per_channel.max(50), Ordering::Relaxed);
    }

    /// Over budget: drop whole channels, least recently used first, until it fits with 10% room.
    /// The newest few channels stay whatever the budget says.
    pub fn enforce_budget(&self) -> Result<()> {
        let budget = self.budget.load(Ordering::Relaxed);
        if budget == 0 {
            return Ok(());
        }
        let mut used = self.used_bytes()?;
        if used <= budget {
            return Ok(());
        }
        let target = budget / 10 * 9;
        let conn = self.conn.lock();
        let victims: Vec<(i64, i64)> = {
            let mut q = conn.prepare(
                "SELECT s.channel, (SELECT coalesce(sum(length(data)), 0) FROM messages m WHERE m.channel = s.channel)
                   FROM channel_seen s ORDER BY s.at DESC LIMIT -1 OFFSET ?1",
            )?;
            let rows = q.query_map([KEEP_CHANNELS], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.filter_map(|r| r.ok()).collect::<Vec<_>>().into_iter().rev().collect()
        };
        for (channel, bytes) in victims {
            if used <= target {
                break;
            }
            conn.execute("DELETE FROM messages WHERE channel = ?1", [channel])?;
            conn.execute("DELETE FROM messages_fts WHERE channel = ?1", [channel.to_string()])?;
            conn.execute("DELETE FROM channel_seen WHERE channel = ?1", [channel])?;
            // The index costs about as much again as the rows it points at.
            used = used.saturating_sub(bytes as u64 * 2);
        }
        let _ = conn.execute_batch("PRAGMA incremental_vacuum;");
        Ok(())
    }

    /// Forget every stored message (the search history), keeping the dictionary and everything else.
    pub fn clear_messages(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch("DELETE FROM messages; DELETE FROM messages_fts; DELETE FROM channel_seen;")?;
        let _ = conn.execute_batch("PRAGMA incremental_vacuum;");
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
        let mut codec = self.codec.lock();
        Ok(rows.filter_map(|r| r.ok()).filter_map(|b| Self::unpack(&mut codec, &b)).collect())
    }

    /// The stored messages around `message` (up to `half` before it and `half` after), newest
    /// first, when `message` itself is stored; empty otherwise. A jump to a message we have seen
    /// before can paint this window at once instead of waiting for `?around=`.
    pub fn load_around(&self, channel: Id, message: Id, half: u32) -> Result<Vec<Message>> {
        let conn = self.conn.lock();
        let read = |sql: &str, limit: u32| -> Result<Vec<Message>> {
            let mut q = conn.prepare_cached(sql)?;
            let rows = q.query_map(params![channel.0 as i64, message.0 as i64, limit], |r| r.get::<_, Vec<u8>>(0))?;
            let mut codec = self.codec.lock();
            Ok(rows.filter_map(|r| r.ok()).filter_map(|b| Self::unpack(&mut codec, &b)).collect())
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
                .and_then(|b| Self::unpack(&mut self.codec.lock(), &b));
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
        self.conn.lock().execute_batch("DELETE FROM kv; DELETE FROM messages; DELETE FROM messages_fts; DELETE FROM channel_seen;")?;
        *self.codec.lock() = Codec::default();
        self.dict_tried.store(false, Ordering::Relaxed);
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
            Cache::migrate_fts(&conn, &mut Codec::default()).unwrap();
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

    #[test]
    fn rows_are_compressed_once_there_are_enough_and_still_read_back() {
        let dir = std::env::temp_dir().join(format!("unison-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.sqlite");
        let c = Cache::open(&path).unwrap();
        for ch in 1..=8u64 {
            let batch: Vec<Message> = (0..60).map(|i| msg_in(ch, ch * 1000 + i, "bob", &format!("message {i} in channel {ch}"))).collect();
            c.save_messages(Snowflake(ch), &batch).unwrap();
        }
        assert!(c.codec.lock().has_dict(), "480 messages are enough to train a dictionary");
        let back = c.load_messages(Snowflake(3), 100).unwrap();
        assert_eq!(back.len(), 60);
        assert_eq!(back[0].content, "message 59 in channel 3");
        assert_eq!(c.search_page(&q("message 59")).unwrap().total, 8, "the text index sees every channel's 59th");
        drop(c);
        let again = Cache::open(&path).unwrap();
        assert_eq!(again.load_messages(Snowflake(8), 5).unwrap().len(), 5, "a new process reads the dictionary rows");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn over_budget_drops_the_least_recently_used_channels_first() {
        let c = Cache::in_memory().unwrap();
        for ch in 1..=9u64 {
            let batch: Vec<Message> = (0..40).map(|i| msg_in(ch, ch * 1000 + i, "bob", &format!("hello {i} {}", "x".repeat(200)))).collect();
            c.save_messages(Snowflake(ch), &batch).unwrap();
            c.conn.lock().execute("UPDATE channel_seen SET at = ?1 WHERE channel = ?2", params![ch as i64, ch as i64]).unwrap();
        }
        c.set_limits(1, 500);
        c.enforce_budget().unwrap();
        let left: Vec<i64> = {
            let conn = c.conn.lock();
            let mut q = conn.prepare("SELECT DISTINCT channel FROM messages ORDER BY channel").unwrap();
            q.query_map([], |r| r.get(0)).unwrap().filter_map(|r| r.ok()).collect()
        };
        assert_eq!(left, vec![5, 6, 7, 8, 9], "the five newest channels survive any budget");
        assert!(c.search_page(&q("hello")).unwrap().hits.iter().all(|h| h.channel.0 >= 5));
    }

    #[test]
    fn found_messages_join_the_index_without_displacing_what_is_stored() {
        let c = Cache::in_memory().unwrap();
        c.set_limits(0, 50);
        let newest: Vec<Message> = (500..560).map(|i| msg_in(1, i, "bob", &format!("recent {i}"))).collect();
        c.save_messages(Snowflake(1), &newest).unwrap();
        // An old hit, far below the newest 50: save_messages would drop it at once.
        c.index_found(Snowflake(1), &[msg_in(1, 7, "ann", "needle from long ago")]).unwrap();
        assert_eq!(c.search_page(&q("needle")).unwrap().total, 1, "an old hit is kept and findable");
        c.index_found(Snowflake(1), &[msg_in(1, 530, "eve", "changed")]).unwrap();
        assert_eq!(c.search_page(&q("changed")).unwrap().total, 0, "a stored row is never replaced");
        assert_eq!(c.search_page(&q("recent 530")).unwrap().total, 1);
    }
}
