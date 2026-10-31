//! The message-search grammar (`from:`, `in:`, `has:`, `before:` ...): which filters exist, how a
//! query string becomes filters, and how a calendar day becomes a snowflake bound. Pure, so every
//! shell parses a query the same way; name lookups are passed in by the caller.

pub const FILTERS: [(&str, &str); 8] = [
    ("from:", "user"),
    ("mentions:", "user"),
    ("in:", "channel"),
    ("has:", "link, embed, file, image, video, sound, sticker"),
    ("before:", "specific date"),
    ("during:", "specific date"),
    ("after:", "specific date"),
    ("pinned:", "true or false"),
];

/// Where a search looks. `Server` and `Channel` can also ask Discord; `Everywhere` is the local
/// index of every chat the account has loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Server,
    Channel,
    Everywhere,
}

impl Scope {
    /// The scopes on offer, default first: a server search starts server-wide, a DM starts in
    /// the conversation.
    pub fn options(in_guild: bool) -> &'static [Scope] {
        if in_guild {
            &[Scope::Server, Scope::Channel, Scope::Everywhere]
        } else {
            &[Scope::Channel, Scope::Everywhere]
        }
    }

    /// The scope chips, broadest first (⌘1 is the first): Everywhere, This server, This channel.
    pub fn tabs(in_guild: bool) -> &'static [Scope] {
        if in_guild {
            &[Scope::Everywhere, Scope::Server, Scope::Channel]
        } else {
            &[Scope::Everywhere, Scope::Channel]
        }
    }

    pub fn asks_discord(self) -> bool {
        self != Scope::Everywhere
    }
}

pub const HAS_VALUES: [&str; 7] = ["link", "embed", "file", "image", "video", "sound", "sticker"];

const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub content: String,
    pub author_ids: Vec<String>,
    pub channel_ids: Vec<String>,
    pub mentions: Vec<String>,
    pub has: Vec<String>,
    pub pinned: Option<bool>,
    pub min_id: Option<String>,
    pub max_id: Option<String>,
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Snowflake at the start (or, with `end_of_day`, the end) of a `YYYY-MM-DD` day in a time zone
/// `tz_offset_secs` east of UTC, as the web client builds `min_id` / `max_id`.
pub fn day_snowflake(day: &str, end_of_day: bool, tz_offset_secs: i32) -> Option<String> {
    let mut it = day.split('-');
    let (y, m, d): (i64, i64, i64) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) || y < 2015 {
        return None;
    }
    let secs = days_from_civil(y, m, d) * 86_400 - tz_offset_secs as i64 + if end_of_day { 86_400 } else { 0 };
    let ms = (secs * 1000 - DISCORD_EPOCH_MS - 1).max(0) as u64;
    Some((ms << 22).to_string())
}

/// The token being typed (after the last space) and the query before it (kept with its space).
pub fn split(q: &str) -> (&str, &str) {
    match q.rfind(' ') {
        Some(i) => (&q[..=i], &q[i + 1..]),
        None => ("", q),
    }
}

/// Turn a query into filters. `user` and `channel` resolve a typed name to an id; an unknown name
/// stays in the text search, as the web client does.
pub fn parse(q: &str, tz_offset_secs: i32, user: impl Fn(&str) -> Option<String>, channel: impl Fn(&str) -> Option<String>) -> Parsed {
    let mut p = Parsed::default();
    let mut text: Vec<&str> = Vec::new();
    for tok in q.split(' ').filter(|t| !t.is_empty()) {
        let Some((key, val)) = tok.split_once(':').filter(|(k, _)| !k.is_empty()) else {
            text.push(tok);
            continue;
        };
        match key.to_lowercase().as_str() {
            k @ ("from" | "mentions") => match user(val) {
                Some(id) if k == "from" => p.author_ids.push(id),
                Some(id) => p.mentions.push(id),
                None => text.push(tok),
            },
            "in" => match channel(val.trim_start_matches('#')) {
                Some(id) => p.channel_ids.push(id),
                None => text.push(tok),
            },
            "has" => p.has.push(val.to_lowercase()),
            "pinned" => p.pinned = Some(val.eq_ignore_ascii_case("true")),
            "before" => p.max_id = day_snowflake(val, false, tz_offset_secs),
            "after" => p.min_id = day_snowflake(val, true, tz_offset_secs),
            "during" => {
                p.min_id = day_snowflake(val, false, tz_offset_secs);
                p.max_id = day_snowflake(val, true, tz_offset_secs);
            }
            _ => text.push(tok),
        }
    }
    p.content = text.join(" ");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_bounds_match_known_snowflake() {
        // 2020-01-01T00:00:00Z = 1577836800000 ms; (ms - epoch - 1) << 22.
        let expected = (((1_577_836_800_000i64 - DISCORD_EPOCH_MS) - 1) as u64) << 22;
        assert_eq!(day_snowflake("2020-01-01", false, 0), Some(expected.to_string()));
        // A zone 10 hours east starts the day 10 hours earlier in UTC.
        let shifted = (((1_577_836_800_000i64 - 36_000_000 - DISCORD_EPOCH_MS) - 1) as u64) << 22;
        assert_eq!(day_snowflake("2020-01-01", false, 36_000), Some(shifted.to_string()));
        assert!(day_snowflake("nope", false, 0).is_none());
        assert!(day_snowflake("2020-13-01", false, 0).is_none());
    }

    #[test]
    fn parses_filters_and_keeps_unknown_names_in_the_text() {
        let user = |n: &str| (n == "alice").then(|| "42".to_string());
        let chan = |n: &str| (n == "general").then(|| "7".to_string());
        let p = parse("hello from:alice in:#general has:Image from:bob pinned:true world", 0, user, chan);
        assert_eq!(p.content, "hello from:bob world");
        assert_eq!(p.author_ids, ["42"]);
        assert_eq!(p.channel_ids, ["7"]);
        assert_eq!(p.has, ["image"]);
        assert_eq!(p.pinned, Some(true));
    }

    #[test]
    fn scopes_start_where_the_user_is() {
        assert_eq!(Scope::options(true)[0], Scope::Server);
        assert_eq!(Scope::options(false), [Scope::Channel, Scope::Everywhere]);
        assert!(!Scope::Everywhere.asks_discord());
    }

    #[test]
    fn chips_run_broad_to_narrow() {
        assert_eq!(Scope::tabs(true), [Scope::Everywhere, Scope::Server, Scope::Channel]);
        assert_eq!(Scope::tabs(false), [Scope::Everywhere, Scope::Channel]);
    }

    #[test]
    fn splits_the_token_being_typed() {
        assert_eq!(split("from:al"), ("", "from:al"));
        assert_eq!(split("hi from:al"), ("hi ", "from:al"));
    }
}
