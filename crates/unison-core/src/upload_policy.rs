//! How big an upload may be and how to shrink one that is too big. The core decides the limit,
//! whether to compress, and the ladder of attempts; the shell only runs an attempt (re-encode a
//! picture, export a movie) and reports the size back, because encoders are per-OS.

const MIB: u64 = 1024 * 1024;
/// Aim a little under the limit: containers add overhead the encoder can't predict.
const HEADROOM_PCT: u64 = 95;

/// Largest file one message may carry: the best of the account's Nitro tier and the server's boost
/// tier. `premium_type`: 1 Classic, 2 Nitro, 3 Basic. `capture`: figures from Discord's help
/// pages; the real limit is also visible in the 413 body.
pub fn limit_bytes(premium_type: Option<u8>, boost_tier: Option<u8>) -> u64 {
    let account = match premium_type {
        Some(2) => 500 * MIB,
        Some(1) | Some(3) => 50 * MIB,
        _ => 10 * MIB,
    };
    let server = match boost_tier {
        Some(2) => 50 * MIB,
        Some(3) => 100 * MIB,
        _ => 10 * MIB,
    };
    account.max(server)
}

pub fn target_bytes(limit: u64) -> u64 {
    limit / 100 * HEADROOM_PCT
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageStep {
    /// Longest edge in pixels; smaller pictures are not enlarged.
    pub max_edge: u32,
    pub quality: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// Send as is: it fits, compression is off, or the type can't be shrunk.
    Keep,
    /// Re-encode as JPEG, trying each step until one fits.
    Image(Vec<ImageStep>),
    /// Export as MP4 at each height in turn until one fits.
    Video(Vec<u32>),
}

const IMAGE_STEPS: [ImageStep; 5] = [
    ImageStep { max_edge: 3840, quality: 0.88 },
    ImageStep { max_edge: 2560, quality: 0.80 },
    ImageStep { max_edge: 2048, quality: 0.72 },
    ImageStep { max_edge: 1600, quality: 0.62 },
    ImageStep { max_edge: 1280, quality: 0.50 },
];

const VIDEO_HEIGHTS: [u32; 4] = [1080, 720, 540, 360];

/// What to do with a file of `size` bytes and `content_type` against `limit`. Animated and
/// vector formats (GIF, SVG) are left alone: re-encoding would flatten them.
pub fn plan(enabled: bool, limit: u64, size: u64, content_type: &str) -> Plan {
    if !enabled || size <= limit {
        return Plan::Keep;
    }
    match content_type {
        "image/jpeg" | "image/png" | "image/webp" | "image/heic" | "image/heif" | "image/tiff" | "image/bmp" => {
            Plan::Image(IMAGE_STEPS.to_vec())
        }
        "video/mp4" | "video/quicktime" | "video/x-m4v" => Plan::Video(VIDEO_HEIGHTS.to_vec()),
        _ => Plan::Keep,
    }
}

/// The name of a re-encoded file: same stem, the container the attempt produced.
pub fn renamed(filename: &str, ext: &str) -> String {
    let stem = filename.rsplit_once('.').map_or(filename, |(s, _)| s);
    format!("{stem}.{ext}")
}

/// Whether compression is on: the user's choice if they made one, otherwise on for free accounts.
pub fn effective(choice: Option<bool>, premium_type: Option<u8>) -> bool {
    choice.unwrap_or(!premium_type.is_some_and(|t| t > 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_follow_the_best_tier() {
        assert_eq!(limit_bytes(None, None), 10 * MIB);
        assert_eq!(limit_bytes(Some(0), Some(0)), 10 * MIB);
        assert_eq!(limit_bytes(Some(3), None), 50 * MIB);
        assert_eq!(limit_bytes(Some(2), Some(3)), 500 * MIB);
        assert_eq!(limit_bytes(None, Some(3)), 100 * MIB);
    }

    #[test]
    fn only_big_shrinkable_files_get_a_plan() {
        let l = 10 * MIB;
        assert_eq!(plan(true, l, l, "image/png"), Plan::Keep);
        assert!(matches!(plan(true, l, l + 1, "image/png"), Plan::Image(s) if s.len() == 5));
        assert!(matches!(plan(true, l, 50 * MIB, "video/quicktime"), Plan::Video(h) if h[0] == 1080));
        assert_eq!(plan(true, l, 50 * MIB, "image/gif"), Plan::Keep);
        assert_eq!(plan(true, l, 50 * MIB, "application/zip"), Plan::Keep);
        assert_eq!(plan(false, l, 50 * MIB, "image/png"), Plan::Keep);
    }

    #[test]
    fn default_follows_nitro_unless_chosen() {
        assert!(effective(None, None));
        assert!(effective(None, Some(0)));
        assert!(!effective(None, Some(2)));
        assert!(effective(Some(true), Some(2)));
        assert!(!effective(Some(false), None));
    }

    #[test]
    fn renames_keep_the_stem() {
        assert_eq!(renamed("IMG 1.heic", "jpg"), "IMG 1.jpg");
        assert_eq!(renamed("clip", "mp4"), "clip.mp4");
        assert!(target_bytes(10 * MIB) < 10 * MIB);
    }
}
