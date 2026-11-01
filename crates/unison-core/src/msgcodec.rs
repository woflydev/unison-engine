//! How stored messages are written to the on-device cache: zstd frames, with a dictionary trained
//! on the account's own messages once there are enough of them. Messages repeat the same keys,
//! author fields and URL shapes, so a small dictionary shrinks a row about 9x (plain per-row zstd
//! only 2x) and a row still decodes alone in microseconds. Rows written before this (plain JSON)
//! stay readable: a stored blob that starts with `{` is JSON.

use zstd_safe::{CCtx, CDict, DCtx, DDict};

const LEVEL: i32 = 3;
const DICT_BYTES: usize = 48 << 10;

struct Dict {
    id: u32,
    c: CDict<'static>,
    d: DDict<'static>,
}

pub struct Codec {
    cctx: CCtx<'static>,
    dctx: DCtx<'static>,
    dict: Option<Dict>,
}

impl Default for Codec {
    fn default() -> Self {
        Self { cctx: CCtx::create(), dctx: DCtx::create(), dict: None }
    }
}

impl Codec {
    pub fn has_dict(&self) -> bool {
        self.dict.is_some()
    }

    pub fn set_dict(&mut self, bytes: &[u8]) {
        let id = zstd_safe::get_dict_id_from_dict(bytes).map_or(0, |n| n.get());
        self.dict = Some(Dict { id, c: CDict::create(bytes, LEVEL), d: DDict::create(bytes) });
    }

    pub fn encode(&mut self, json: &[u8]) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(zstd_safe::compress_bound(json.len()));
        match &self.dict {
            Some(d) => self.cctx.compress_using_cdict(&mut out, json, &d.c).ok()?,
            None => self.cctx.compress(&mut out, json, LEVEL).ok()?,
        };
        Some(out)
    }

    /// The JSON of a stored row (plain JSON passes through).
    pub fn decode(&mut self, blob: &[u8]) -> Option<Vec<u8>> {
        if blob.first() == Some(&b'{') {
            return Some(blob.to_vec());
        }
        let size = zstd_safe::get_frame_content_size(blob).ok().flatten()? as usize;
        if size > (16 << 20) {
            return None;
        }
        let mut out = Vec::with_capacity(size);
        let want = zstd_safe::get_dict_id_from_frame(blob).map(|n| n.get());
        match (&self.dict, want) {
            (Some(d), Some(id)) if d.id == id => self.dctx.decompress_using_ddict(&mut out, blob, &d.d).ok()?,
            (_, None) => self.dctx.decompress(&mut out, blob).ok()?,
            _ => return None,
        };
        Some(out)
    }
}

/// A dictionary from sample rows (their JSON), or `None` when there is too little to learn from.
pub fn train(samples: &[Vec<u8>]) -> Option<Vec<u8>> {
    if samples.len() < 100 {
        return None;
    }
    let sizes: Vec<usize> = samples.iter().map(Vec::len).collect();
    let flat: Vec<u8> = samples.concat();
    let mut dict = vec![0u8; DICT_BYTES];
    let n = zstd_safe::train_from_buffer(&mut dict[..], &flat, &sizes).ok()?;
    dict.truncate(n);
    Some(dict)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(i: usize) -> Vec<u8> {
        format!(
            r#"{{"id":"{i}","channel_id":"7","author":{{"id":"{u}","username":"user{u}","global_name":"User {u}","discriminator":"0","avatar":"61d091fefd08c1707485084f88fdfbf0","bot":false,"public_flags":0}},"content":"message number {i} about nothing much","timestamp":"2024-06-06T08:11:03.262000+00:00","mentions":[],"attachments":[],"embeds":[],"reactions":[],"pinned":false,"type":0,"flags":0}}"#,
            u = i % 9
        )
        .into_bytes()
    }

    #[test]
    fn round_trips_plain_json_plain_zstd_and_dictionary() {
        let mut c = Codec::default();
        assert_eq!(c.decode(b"{\"a\":1}").unwrap(), b"{\"a\":1}");
        let r = row(1);
        let plain = c.encode(&r).unwrap();
        assert_eq!(c.decode(&plain).unwrap(), r);
        let samples: Vec<Vec<u8>> = (0..400).map(row).collect();
        let dict = train(&samples).expect("trains");
        c.set_dict(&dict);
        let packed = c.encode(&r).unwrap();
        assert!(packed.len() * 2 < plain.len(), "{} vs {}", packed.len(), plain.len());
        assert_eq!(c.decode(&packed).unwrap(), r);
        assert_eq!(c.decode(&plain).unwrap(), r, "older rows stay readable after the dictionary arrives");
        let mut other = Codec::default();
        assert!(other.decode(&packed).is_none(), "a dictionary row is not read without its dictionary");
        assert!(train(&samples[..10]).is_none());
    }
}
