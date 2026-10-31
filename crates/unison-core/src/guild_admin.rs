//! Server administration rules: the permission catalogue, which roles I may edit and which
//! permission bits I may hand out, request bodies for roles, channels and images, and the limits on
//! emoji names. Pure, so every shell offers the same options and the same refusals.

use serde_json::{Value, json};

use crate::model::Id;
use crate::store::{Store, perm};

/// `(bit, label, group)` in the order the official role editor lists them.
pub const PERMISSIONS: [(u64, &str, &str); 40] = [
    (1 << 10, "View Channels", "General"),
    (1 << 4, "Manage Channels", "General"),
    (1 << 28, "Manage Roles", "General"),
    (1 << 30, "Manage Expressions", "General"),
    (1 << 43, "Create Expressions", "General"),
    (1 << 7, "View Audit Log", "General"),
    (1 << 29, "Manage Webhooks", "General"),
    (1 << 5, "Manage Server", "General"),
    (1 << 0, "Create Invite", "Membership"),
    (1 << 26, "Change Nickname", "Membership"),
    (1 << 27, "Manage Nicknames", "Membership"),
    (1 << 1, "Kick Members", "Membership"),
    (1 << 2, "Ban Members", "Membership"),
    (1 << 40, "Timeout Members", "Membership"),
    (1 << 11, "Send Messages", "Text"),
    (1 << 38, "Send Messages in Threads", "Text"),
    (1 << 35, "Create Public Threads", "Text"),
    (1 << 36, "Create Private Threads", "Text"),
    (1 << 14, "Embed Links", "Text"),
    (1 << 15, "Attach Files", "Text"),
    (1 << 6, "Add Reactions", "Text"),
    (1 << 18, "Use External Emoji", "Text"),
    (1 << 37, "Use External Stickers", "Text"),
    (1 << 17, "Mention @everyone, @here and All Roles", "Text"),
    (1 << 13, "Manage Messages", "Text"),
    (1 << 34, "Manage Threads", "Text"),
    (1 << 16, "Read Message History", "Text"),
    (1 << 31, "Use Application Commands", "Text"),
    (1 << 20, "Connect", "Voice"),
    (1 << 21, "Speak", "Voice"),
    (1 << 9, "Video", "Voice"),
    (1 << 25, "Use Voice Activity", "Voice"),
    (1 << 8, "Priority Speaker", "Voice"),
    (1 << 22, "Mute Members", "Voice"),
    (1 << 23, "Deafen Members", "Voice"),
    (1 << 24, "Move Members", "Voice"),
    (1 << 32, "Request to Speak", "Events"),
    (1 << 33, "Manage Events", "Events"),
    (1 << 44, "Create Events", "Events"),
    (1 << 3, "Administrator", "Advanced"),
];

/// The permission bits I may set on a role: all of them as the owner or an administrator, otherwise
/// only the ones I hold myself.
pub fn grantable(my_permissions: u64) -> u64 {
    if my_permissions & perm::ADMINISTRATOR != 0 || my_permissions == perm::ALL {
        u64::MAX
    } else {
        my_permissions
    }
}

/// The permissions to save: my changes where I'm allowed to make them, everything else as it was.
pub fn permissions_for_save(old: u64, wanted: u64, grantable: u64) -> u64 {
    (wanted & grantable) | (old & !grantable)
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoleDraft {
    pub name: String,
    /// 0xRRGGBB; 0 for no colour.
    pub color: u32,
    pub hoist: bool,
    pub mentionable: bool,
    pub permissions: u64,
}

impl RoleDraft {
    /// `POST`/`PATCH` body. The name is trimmed and limited to 100 characters.
    pub fn body(&self) -> Value {
        let name: String = self.name.trim().chars().take(100).collect();
        json!({
            "name": if name.is_empty() { "new role".to_string() } else { name },
            "color": self.color,
            "hoist": self.hoist,
            "mentionable": self.mentionable,
            "permissions": self.permissions.to_string(),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChannelDraft {
    pub name: String,
    pub topic: String,
    pub nsfw: bool,
    pub slowmode_secs: u32,
}

/// Slowmode lengths of the official client (seconds), 0 = off.
pub const SLOWMODE_CHOICES: [(&str, u32); 14] = [
    ("Off", 0),
    ("5s", 5),
    ("10s", 10),
    ("15s", 15),
    ("30s", 30),
    ("1m", 60),
    ("2m", 120),
    ("5m", 300),
    ("10m", 600),
    ("15m", 900),
    ("30m", 1800),
    ("1h", 3600),
    ("2h", 7200),
    ("6h", 21_600),
];

impl ChannelDraft {
    /// `PATCH /channels/{id}`. Names are lower-cased with spaces as dashes, as text channels require
    /// (the shell passes voice names through the same call, so only `-` and case are touched).
    pub fn body(&self, text_like: bool) -> Value {
        let mut name = self.name.trim().to_string();
        if text_like {
            name = name.to_lowercase().split_whitespace().collect::<Vec<_>>().join("-");
        }
        let mut b = json!({ "name": name.chars().take(100).collect::<String>() });
        if text_like {
            b["topic"] = self.topic.chars().take(1024).collect::<String>().into();
            b["nsfw"] = self.nsfw.into();
            b["rate_limit_per_user"] = self.slowmode_secs.min(21_600).into();
        }
        b
    }
}

// ----- New channels, ordering and permission overwrites -----

/// Channel kinds the "Create Channel" form offers: `(label, channel type)`.
pub const NEW_CHANNEL_KINDS: [(&str, u8); 6] = [
    ("Text", 0),
    ("Voice", 2),
    ("Announcement", 5),
    ("Stage", 13),
    ("Forum", 15),
    ("Category", 4),
];

#[derive(Clone, Debug, PartialEq)]
pub struct NewChannel {
    pub name: String,
    pub kind: u8,
    /// The category it goes in; categories have none.
    pub parent: Option<Id>,
    pub topic: String,
}

impl NewChannel {
    fn text_like(&self) -> bool {
        matches!(self.kind, 0 | 5 | 15)
    }

    /// Why the form can't be saved yet, if so.
    pub fn check(&self) -> Option<String> {
        if self.name.trim().is_empty() {
            return Some("Give the channel a name.".into());
        }
        if !NEW_CHANNEL_KINDS.iter().any(|(_, k)| *k == self.kind) {
            return Some("Pick a channel type.".into());
        }
        None
    }

    /// `POST /guilds/{g}/channels`. Text-like names become lower-case with dashes; categories and
    /// voice keep what was typed.
    pub fn body(&self) -> Value {
        let mut name = self.name.trim().to_string();
        if self.text_like() {
            name = name.to_lowercase().split_whitespace().collect::<Vec<_>>().join("-");
        }
        let mut b = json!({ "name": name.chars().take(100).collect::<String>(), "type": self.kind });
        if self.kind != 4 {
            if let Some(p) = self.parent {
                b["parent_id"] = p.to_string().into();
            }
        }
        if self.text_like() && !self.topic.trim().is_empty() {
            b["topic"] = self.topic.trim().chars().take(1024).collect::<String>().into();
        }
        b
    }
}

/// `order` with `id` moved one place toward the end (`later`) or the start. `None` at the edge or if
/// `id` is not in the list.
pub fn step(order: &[Id], id: Id, later: bool) -> Option<Vec<Id>> {
    let i = order.iter().position(|x| *x == id)?;
    let j = if later { i.checked_add(1).filter(|j| *j < order.len())? } else { i.checked_sub(1)? };
    let mut v = order.to_vec();
    v.swap(i, j);
    Some(v)
}

/// `[{id, position}]` for every entry whose index changed from `before` to `after`.
pub fn position_changes(before: &[Id], after: &[Id]) -> Vec<Value> {
    after
        .iter()
        .enumerate()
        .filter(|(i, id)| before.get(*i) != Some(id))
        .map(|(i, id)| json!({ "id": id.to_string(), "position": i }))
        .collect()
}

/// Where a dragged channel lands relative to the one under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropZone {
    Before,
    /// Into a category, at the end of its group.
    Into,
    After,
}

/// The PATCH entries for a drop: `after` is the new order of the list the channel now lives in,
/// `before` the order that list had (without the channel when it came from elsewhere). When the
/// channel changes category the entry carries `parent_id` (null for "no category").
pub fn drop_changes(before: &[Id], after: &[Id], dragged: Id, new_parent: Option<Option<Id>>) -> Vec<Value> {
    let mut v = position_changes(before, after);
    if let Some(parent) = new_parent {
        let parent = parent.map_or(Value::Null, |p| p.to_string().into());
        match v.iter_mut().find(|e| e["id"] == dragged.to_string()) {
            Some(e) => e["parent_id"] = parent,
            None => {
                let at = after.iter().position(|x| *x == dragged).unwrap_or(0);
                v.push(json!({ "id": dragged.to_string(), "position": at, "parent_id": parent }));
            }
        }
    }
    v
}

/// Permission bits a channel overwrite can set, with the group they appear under.
pub fn channel_permissions(text_like: bool, voice_like: bool) -> Vec<(u64, &'static str, &'static str)> {
    const GENERAL: [u64; 5] = [1 << 10, 1 << 4, 1 << 28, 1 << 29, 1 << 0];
    PERMISSIONS
        .iter()
        .filter(|(bit, _, group)| match *group {
            "General" => GENERAL.contains(bit),
            "Text" => text_like,
            "Voice" => voice_like,
            "Events" => voice_like && *bit != 1 << 33 && *bit != 1 << 44,
            _ => false,
        })
        .map(|(b, l, g)| {
            // In a channel the role-management bit is about who may change this channel's access.
            if *b == 1 << 28 { (*b, "Manage Permissions", *g) } else { (*b, *l, *g) }
        })
        .collect()
}

/// An overwrite after my edit: bits I may not change keep their old state, and a bit can't be both
/// allowed and denied (deny wins, as when a person flips a toggle to "deny").
pub fn overwrite_for_save(old: (u64, u64), wanted: (u64, u64), grantable: u64) -> (u64, u64) {
    let allow = (wanted.0 & grantable) | (old.0 & !grantable);
    let deny = (wanted.1 & grantable) | (old.1 & !grantable);
    (allow & !deny, deny)
}

/// `PUT /channels/{c}/permissions/{id}`; `type` 0 = role, 1 = member.
pub fn overwrite_body(target: Id, is_role: bool, allow: u64, deny: u64) -> Value {
    json!({ "id": target.to_string(), "type": if is_role { 0 } else { 1 }, "allow": allow.to_string(), "deny": deny.to_string() })
}

pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// `data:<type>;base64,...` for an icon, banner or emoji upload; `None` for anything Discord won't take.
pub fn data_uri(content_type: &str, bytes: &[u8]) -> Option<String> {
    use base64::Engine as _;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES || !matches!(content_type, "image/png" | "image/jpeg" | "image/gif" | "image/webp") {
        return None;
    }
    Some(format!("data:{content_type};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// Emoji upload limit: 256 KiB.
pub const MAX_EMOJI_BYTES: usize = 256 * 1024;

/// A valid emoji name: letters, digits and underscores, 2 to 32 characters. Anything else is turned
/// into underscores; too-short names are padded.
pub fn emoji_name(raw: &str) -> String {
    let stem = raw.rsplit_once('.').map_or(raw, |(s, _)| s);
    let mut n: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).take(32).collect();
    while n.len() < 2 {
        n.push('_');
    }
    n
}

impl Store {
    /// Whether I may edit this role's settings: Manage Roles, and the role is below my top role
    /// (the owner may edit any). `@everyone` can be edited, never deleted.
    pub fn can_edit_role(&self, guild: Id, role: Id) -> bool {
        let Some(me) = self.my_id() else { return false };
        if self.guild_permissions(guild) & perm::MANAGE_ROLES == 0 {
            return false;
        }
        let owner = self.guilds.get(&guild).is_some_and(|g| g.props().owner_id == Some(me));
        let Some(r) = self.roles.get(&guild).and_then(|m| m.get(&role)) else { return false };
        owner || role == guild || r.position < self.highest_role(guild, me)
    }

    /// Channels sharing a place with `channel` (same category, or the categories themselves), in the
    /// order the sidebar lists them: text-like before voice-like, then by position.
    pub fn channel_siblings(&self, channel: Id) -> Vec<Id> {
        use crate::model::channel_type as t;
        let Some(c) = self.channels.get(&channel) else { return vec![] };
        let Some(g) = c.guild_id else { return vec![] };
        let category = c.kind == t::GUILD_CATEGORY;
        let voice = |k: u8| matches!(k, t::GUILD_VOICE | t::GUILD_STAGE_VOICE);
        let mut list: Vec<&crate::model::Channel> = self
            .guild_channels
            .get(&g)
            .into_iter()
            .flatten()
            .filter_map(|id| self.channels.get(id))
            .filter(|x| !x.is_thread() && (x.kind == t::GUILD_CATEGORY) == category && (category || x.parent_id == c.parent_id))
            .collect();
        list.sort_by_key(|x| (!category && voice(x.kind), x.position, x.id));
        list.into_iter().map(|x| x.id).collect()
    }

    fn is_voice_like(kind: u8) -> bool {
        use crate::model::channel_type as t;
        matches!(kind, t::GUILD_VOICE | t::GUILD_STAGE_VOICE)
    }

    /// Where a dragged channel may land over `target`: before or after a channel of the same kind
    /// of list (text-like with text-like), into a category, or before or after another category when a
    /// category is dragged. Empty when I may not move it or it makes no sense.
    pub fn channel_drop_zones(&self, dragged: Id, target: Id) -> Vec<DropZone> {
        use crate::model::channel_type as t;
        let (Some(d), Some(x)) = (self.channels.get(&dragged), self.channels.get(&target)) else { return vec![] };
        if dragged == target || d.guild_id.is_none() || d.guild_id != x.guild_id || d.is_thread() || x.is_thread() {
            return vec![];
        }
        if self.permissions(d) & perm::MANAGE_CHANNELS == 0 {
            return vec![];
        }
        match (d.kind == t::GUILD_CATEGORY, x.kind == t::GUILD_CATEGORY) {
            (true, true) => vec![DropZone::Before, DropZone::After],
            (false, true) => vec![DropZone::Into],
            (false, false) if Self::is_voice_like(d.kind) == Self::is_voice_like(x.kind) => vec![DropZone::Before, DropZone::After],
            _ => vec![],
        }
    }

    /// The guild and the PATCH body for dropping `dragged` on `target`, or `None` when that is not a move.
    pub fn channel_drop_plan(&self, dragged: Id, target: Id, zone: DropZone) -> Option<(Id, Vec<Value>)> {
        use crate::model::channel_type as t;
        if !self.channel_drop_zones(dragged, target).contains(&zone) {
            return None;
        }
        let d = self.channels.get(&dragged)?;
        let x = self.channels.get(&target)?;
        let guild = d.guild_id?;
        let parent = if d.kind == t::GUILD_CATEGORY { None } else if x.kind == t::GUILD_CATEGORY { Some(x.id) } else { x.parent_id };
        let before_full = if d.kind == t::GUILD_CATEGORY || d.parent_id == parent {
            self.channel_siblings(dragged)
        } else {
            self.siblings_in(guild, parent)
        };
        let without: Vec<Id> = before_full.iter().copied().filter(|c| *c != dragged).collect();
        let at = match zone {
            DropZone::Before => without.iter().position(|c| *c == target)?,
            DropZone::After => without.iter().position(|c| *c == target)? + 1,
            // End of the group: after the last text-like channel, or after everything for voice.
            DropZone::Into if Self::is_voice_like(d.kind) => without.len(),
            DropZone::Into => without.iter().position(|c| self.channels.get(c).is_some_and(|c| Self::is_voice_like(c.kind))).unwrap_or(without.len()),
        };
        let mut after = without.clone();
        after.insert(at, dragged);
        let moved_parent = (d.kind != t::GUILD_CATEGORY && d.parent_id != parent).then_some(parent);
        let before = if moved_parent.is_some() { &without } else { &before_full };
        let changes = drop_changes(before, &after, dragged, moved_parent);
        (!changes.is_empty()).then_some((guild, changes))
    }

    /// Non-category channels under `parent` (none = uncategorized), in sidebar order.
    fn siblings_in(&self, guild: Id, parent: Option<Id>) -> Vec<Id> {
        use crate::model::channel_type as t;
        let mut list: Vec<&crate::model::Channel> = self
            .guild_channels
            .get(&guild)
            .into_iter()
            .flatten()
            .filter_map(|id| self.channels.get(id))
            .filter(|x| !x.is_thread() && x.kind != t::GUILD_CATEGORY && x.parent_id == parent)
            .collect();
        list.sort_by_key(|x| (Self::is_voice_like(x.kind), x.position, x.id));
        list.into_iter().map(|x| x.id).collect()
    }

    /// Roles from the lowest position up; `@everyone` first.
    pub fn roles_ascending(&self, guild: Id) -> Vec<Id> {
        let mut v: Vec<&crate::model::Role> = self.roles.get(&guild).into_iter().flat_map(|m| m.values()).collect();
        v.sort_by_key(|r| (r.id != guild, r.position, r.id));
        v.into_iter().map(|r| r.id).collect()
    }

    /// May I move this role one place (`later` = to a higher position)? Both it and the role it swaps
    /// with must be below my top role, and `@everyone` never moves.
    pub fn can_move_role(&self, guild: Id, role: Id, later: bool) -> bool {
        if role == guild || !self.can_edit_role(guild, role) {
            return false;
        }
        let before = self.roles_ascending(guild);
        let Some(after) = step(&before, role, later) else { return false };
        let other = after.iter().zip(&before).find(|(a, b)| a != b && **a != role).map(|(a, _)| *a);
        other.is_some_and(|o| o != guild && self.can_edit_role(guild, o))
    }

    pub fn can_manage_expressions(&self, guild: Id) -> bool {
        self.guild_permissions(guild) & (1 << 30) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_bits_are_unique() {
        let mut seen = 0u64;
        for (bit, _, _) in PERMISSIONS {
            assert_eq!(bit.count_ones(), 1);
            assert_eq!(seen & bit, 0, "duplicate bit {bit}");
            seen |= bit;
        }
    }

    #[test]
    fn you_cannot_hand_out_what_you_lack() {
        let mine = (1 << 11) | (1 << 28);
        assert_eq!(grantable(mine), mine);
        assert_eq!(grantable(perm::ADMINISTRATOR), u64::MAX);
        // Wanted: send messages + kick; old had ban. Kick is not mine to grant, ban is not mine to remove.
        let old = 1 << 2;
        let wanted = (1 << 11) | (1 << 1);
        assert_eq!(permissions_for_save(old, wanted, mine), (1 << 11) | (1 << 2));
    }

    #[test]
    fn a_drop_into_another_category_carries_the_new_parent() {
        let (a, b, c, cat) = { use crate::snowflake::Snowflake as S; (S(1), S(2), S(3), S(9)) };
        // `b` joins a list of [a, c] between them, coming from elsewhere.
        let v = drop_changes(&[a, c], &[a, b, c], b, Some(Some(cat)));
        let moved = v.iter().find(|e| e["id"] == "2").unwrap();
        assert_eq!((moved["position"].as_u64(), moved["parent_id"].as_str()), (Some(1), Some("9")));
        assert!(v.iter().any(|e| e["id"] == "3" && e["position"] == 2));
        assert!(!v.iter().any(|e| e["id"] == "1"));
        // Leaving every category sends an explicit null.
        let out = drop_changes(&[], &[b], b, Some(None));
        assert!(out[0]["parent_id"].is_null());
        // Staying put in the same category changes nothing.
        assert!(drop_changes(&[a, b], &[a, b], b, None).is_empty());
    }

    #[test]
    fn bodies() {
        let r = RoleDraft { name: "  Mods ".into(), color: 0xff0000, hoist: true, mentionable: false, permissions: 8 }.body();
        assert_eq!(r["name"], "Mods");
        assert_eq!(r["permissions"], "8");
        let c = ChannelDraft { name: "Off Topic Chat".into(), topic: "hi".into(), nsfw: true, slowmode_secs: 99_999 };
        let b = c.body(true);
        assert_eq!((b["name"].as_str(), b["rate_limit_per_user"].as_u64()), (Some("off-topic-chat"), Some(21_600)));
        assert!(c.body(false).get("topic").is_none());
    }

    #[test]
    fn uploads_and_names() {
        assert!(data_uri("image/png", &[1, 2, 3]).unwrap().starts_with("data:image/png;base64,"));
        assert!(data_uri("image/svg+xml", &[1]).is_none());
        assert!(data_uri("image/png", &[]).is_none());
        assert_eq!(emoji_name("party parrot.gif"), "party_parrot");
        assert_eq!(emoji_name("a.png"), "a_");
    }

    #[test]
    fn new_channel_forms() {
        let c = NewChannel { name: " Game Night ".into(), kind: 0, parent: Some(crate::snowflake::Snowflake(7)), topic: " hi ".into() };
        let b = c.body();
        assert_eq!((b["name"].as_str(), b["parent_id"].as_str(), b["topic"].as_str(), b["type"].as_u64()), (Some("game-night"), Some("7"), Some("hi"), Some(0)));
        let cat = NewChannel { name: "Info".into(), kind: 4, parent: Some(crate::snowflake::Snowflake(7)), topic: "x".into() }.body();
        assert!(cat.get("parent_id").is_none() && cat.get("topic").is_none());
        assert!(NewChannel { name: " ".into(), kind: 0, parent: None, topic: String::new() }.check().is_some());
        assert!(NewChannel { name: "a".into(), kind: 99, parent: None, topic: String::new() }.check().is_some());
    }

    #[test]
    fn stepping_and_position_changes() {
        let o: Vec<Id> = (1..=4u64).map(crate::snowflake::Snowflake).collect();
        assert_eq!(step(&o, crate::snowflake::Snowflake(1), false), None);
        assert_eq!(step(&o, crate::snowflake::Snowflake(4), true), None);
        let n = step(&o, crate::snowflake::Snowflake(2), true).unwrap();
        assert_eq!(n.iter().map(|i| i.0).collect::<Vec<_>>(), vec![1, 3, 2, 4]);
        let ch = position_changes(&o, &n);
        assert_eq!(ch.len(), 2);
        assert_eq!((ch[0]["id"].as_str(), ch[0]["position"].as_u64()), (Some("3"), Some(1)));
    }

    #[test]
    fn overwrites_keep_what_you_cannot_touch() {
        // Grantable: bits 0 and 1. Old: allow bit 2 (not mine), deny nothing. Wanted: allow 0 and 1, plus try 2.
        assert_eq!(overwrite_for_save((0b100, 0), (0b111, 0), 0b011), (0b111, 0));
        // I can't clear bit 2 either.
        assert_eq!(overwrite_for_save((0b100, 0), (0b001, 0), 0b011), (0b101, 0));
        // Allow and deny on one bit: deny wins.
        assert_eq!(overwrite_for_save((0, 0), (0b01, 0b01), 0b11), (0, 0b01));
        let b = overwrite_body(crate::snowflake::Snowflake(9), false, 3, 4);
        assert_eq!((b["type"].as_u64(), b["allow"].as_str(), b["deny"].as_str()), (Some(1), Some("3"), Some("4")));
        let text = channel_permissions(true, false);
        assert!(text.iter().any(|p| p.1 == "Send Messages") && !text.iter().any(|p| p.1 == "Connect"));
        assert!(text.iter().any(|p| p.1 == "Manage Permissions"));
    }
}
