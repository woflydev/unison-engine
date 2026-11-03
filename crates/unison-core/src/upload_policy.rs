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

/// A conversion that keeps the picture as it is: full size, high quality.
const CONVERT_STEP: ImageStep = ImageStep { max_edge: 16384, quality: 0.92 };

const VIDEO_HEIGHTS: [u32; 4] = [1080, 720, 540, 360];

/// What to do with a file of `size` bytes and `content_type` against `limit`. Animated and
/// vector formats (GIF, SVG) are left alone: re-encoding would flatten them.
pub fn plan(enabled: bool, convert_heic: bool, limit: u64, size: u64, content_type: &str) -> Plan {
    // The web client turns every HEIC (and JXR) picture into a JPEG before it uploads, whatever its size. Only the
    // first step is wanted when it fits; the ladder follows for one that is still over the limit.
    if convert_heic && matches!(content_type, "image/heic" | "image/heif") {
        let mut steps = vec![CONVERT_STEP];
        if enabled && size > limit {
            steps.extend(IMAGE_STEPS);
        }
        return Plan::Image(steps);
    }
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

/// Bytes per second the audio track takes in a shrunk movie (AAC at 96 kbit/s).
const AUDIO_BYTES_PER_SEC: f64 = 12_000.0;
/// The least bits per pixel (at 30 frames a second) H.264 holds up at: below it blocks and smearing show.
const MIN_BITS_PER_PIXEL: f64 = 0.055;
const VIDEO_LADDER: [u32; 5] = [1080, 720, 540, 360, 240];

#[derive(Clone, Debug, PartialEq)]
pub struct VideoAttempts {
    /// Heights to export at, best first. Never above the source; empty when nothing can fit.
    pub heights: Vec<u32>,
    /// The video bitrate the budget allows, for the shell's encoder.
    pub video_bps: u64,
    /// False when even the smallest size would be a smear: the movie is too long for the limit.
    pub feasible: bool,
}

/// Which sizes of a movie `duration_secs` long and `source_height` tall are worth exporting to land
/// under `target`. One export at the right size usually fits, so nothing is encoded at 1080 only to be
/// thrown away: the bitrate the budget allows decides where to start.
pub fn video_attempts(duration_secs: f64, source_height: u32, target: u64) -> VideoAttempts {
    let duration = duration_secs.max(0.5);
    let budget = (target as f64 * 0.97 - duration * AUDIO_BYTES_PER_SEC).max(0.0);
    let bps = (budget * 8.0 / duration) as u64;
    let bpp = |h: u32| bps as f64 / (f64::from(h) * f64::from(h) * 16.0 / 9.0 * 30.0);
    let source = if source_height == 0 { 1080 } else { source_height };
    // Sizes the source has, then the source's own size when it is below the ladder.
    let mut sizes: Vec<u32> = VIDEO_LADDER.iter().copied().filter(|&h| h <= source).collect();
    if sizes.is_empty() {
        sizes.push(source);
    }
    let heights: Vec<u32> = sizes.iter().copied().skip_while(|&h| bpp(h) < MIN_BITS_PER_PIXEL && h != *sizes.last().unwrap()).collect();
    let feasible = heights.last().is_some_and(|&h| bpp(h) >= MIN_BITS_PER_PIXEL * 0.7);
    VideoAttempts { heights: if feasible { heights } else { vec![] }, video_bps: bps, feasible }
}

/// Image steps worth trying for a `width`×`height` picture of `size` bytes against `target`: steps that
/// would still be far too big (a huge photo at the first step) are left out, and steps that come to the same
/// pixel size as the one before are folded into the one with the better quality.
pub fn image_steps(steps: &[ImageStep], width: u32, height: u32, target: u64) -> Vec<ImageStep> {
    let longest = width.max(height).max(1);
    // Rough JPEG size: bits per pixel for each quality, from photographs.
    let bits = |q: f32| 0.5 + 2.0 * f64::from((q - 0.5).max(0.0)) / 0.38 * 0.9;
    let mut out: Vec<ImageStep> = Vec::new();
    for s in steps {
        let edge = s.max_edge.min(longest);
        let scale = f64::from(edge) / f64::from(longest);
        let est = f64::from(width) * scale * f64::from(height) * scale * bits(s.quality) / 8.0;
        if est > target as f64 * 3.0 && edge != steps.last().map_or(0, |l| l.max_edge.min(longest)) {
            continue;
        }
        if out.last().is_some_and(|l| l.max_edge.min(longest) == edge) {
            continue;
        }
        out.push(*s);
    }
    if out.is_empty() {
        out.extend(steps.last());
    }
    out
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
        assert_eq!(plan(true, false, l, l, "image/png"), Plan::Keep);
        assert!(matches!(plan(true, false, l, l + 1, "image/png"), Plan::Image(s) if s.len() == 5));
        assert!(matches!(plan(true, false, l, 50 * MIB, "video/quicktime"), Plan::Video(h) if h[0] == 1080));
        assert_eq!(plan(true, false, l, 50 * MIB, "image/gif"), Plan::Keep);
        assert_eq!(plan(true, false, l, 50 * MIB, "application/zip"), Plan::Keep);
        assert_eq!(plan(false, false, l, 50 * MIB, "image/png"), Plan::Keep);
    }

    #[test]
    fn heic_becomes_jpeg_at_any_size_when_asked() {
        let l = 10 * MIB;
        // Small: only the conversion step.
        assert!(matches!(plan(true, true, l, 1000, "image/heic"), Plan::Image(s) if s.len() == 1 && s[0].quality > 0.9));
        // Big: the conversion, then the ladder.
        assert!(matches!(plan(true, true, l, 20 * MIB, "image/heif"), Plan::Image(s) if s.len() == 6));
        // Compression off still converts, and does not shrink.
        assert!(matches!(plan(false, true, l, 20 * MIB, "image/heic"), Plan::Image(s) if s.len() == 1));
        // Off: as before.
        assert_eq!(plan(true, false, l, 1000, "image/heic"), Plan::Keep);
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
    fn a_short_clip_starts_big_and_a_long_one_starts_small() {
        let t = target_bytes(10 * MIB);
        let short = video_attempts(20.0, 2160, t);
        assert!(short.feasible && short.heights[0] == 1080, "{short:?}");
        let long = video_attempts(300.0, 2160, t);
        assert!(long.feasible && long.heights[0] <= 360, "{long:?}");
        // Never above the source.
        assert!(video_attempts(20.0, 480, t).heights.iter().all(|&h| h <= 480));
        // Two hours cannot fit in 10 MiB as anything watchable.
        let huge = video_attempts(7200.0, 1080, t);
        assert!(!huge.feasible && huge.heights.is_empty());
    }

    #[test]
    fn image_steps_skip_what_cannot_fit_and_what_repeats() {
        let t = target_bytes(10 * MIB);
        let all = IMAGE_STEPS.to_vec();
        // A phone photo: nothing skipped at the top that could fit.
        let phone = image_steps(&all, 4032, 3024, t);
        assert!(!phone.is_empty() && phone[0].max_edge >= 2560);
        // A small picture that is only big in bytes: one size, the best quality.
        let small = image_steps(&all, 1000, 800, t);
        assert_eq!(small.len(), 1);
        assert_eq!(small[0].quality, 0.88);
        // Always something to try.
        assert!(!image_steps(&all, 30000, 30000, 1000).is_empty());
    }

    #[test]
    fn renames_keep_the_stem() {
        assert_eq!(renamed("IMG 1.heic", "jpg"), "IMG 1.jpg");
        assert_eq!(renamed("clip", "mp4"), "clip.mp4");
        assert!(target_bytes(10 * MIB) < 10 * MIB);
    }
}
