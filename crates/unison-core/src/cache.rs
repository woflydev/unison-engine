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
             CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                content, author UNINDEXED, channel UNINDEXED, id UNINDEXED, tokenize='unicode61 remove_diacritics 2');",
        )?;
        Ok(Self { conn: Mutex::new(conn), written: Mutex::new(Default::default()) })
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
            let mut del_fts = tx.prepare_cached("DELETE FROM messages_fts WHERE id = ?1")?;
            let mut fts = tx.prepare_cached("INSERT INTO messages_fts (content, author, channel, id) VALUES (?1, ?2, ?3, ?4)")?;
            for m in messages.iter().filter(|m| m.id.0 != 0) {
                put.execute(params![channel.0 as i64, m.id.0 as i64, serde_json::to_vec(m)?])?;
                del_fts.execute([m.id.0 as i64])?;
                if !m.content.is_empty() {
                    fts.execute(params![m.content, m.author.display_name(), channel.0 as i64, m.id.0 as i64])?;
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
        conn.execute("DELETE FROM messages_fts WHERE id = ?1", [id.0 as i64])?;
        Ok(())
    }

    /// Newest first, like the REST endpoint.
    pub fn load_messages(&self, channel: Id, limit: u32) -> Result<Vec<Message>> {
        let conn = self.conn.lock();
        let mut q = conn.prepare_cached("SELECT data FROM messages WHERE channel = ?1 ORDER BY id DESC LIMIT ?2")?;
        let rows = q.query_map(params![channel.0 as i64, limit], |r| r.get::<_, Vec<u8>>(0))?;
        Ok(rows.filter_map(|r| r.ok()).filter_map(|b| serde_json::from_slice(&b).ok()).collect())
    }

    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>> {
        // Quote each term so user input can't inject FTS syntax.
        let q: String = query
            .split_whitespace()
            .map(|t| format!("\"{}\"*", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" ");
        if q.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.conn.lock();
        let mut st = conn.prepare_cached(
            "SELECT channel, id, author, snippet(messages_fts, 0, '', '', '…', 12)
             FROM messages_fts WHERE messages_fts MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let rows = st.query_map(params![q, limit], |r| {
            Ok(SearchHit {
                channel: Snowflake(r.get::<_, i64>(0)? as u64),
                message: Snowflake(r.get::<_, i64>(1)? as u64),
                author: r.get(2)?,
                snippet: r.get(3)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
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
