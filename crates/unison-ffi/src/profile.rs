use super::*;

#[derive(uniffi::Record, Clone)]
pub struct ProfileRole {
    pub name: String,
    pub color: Option<u32>,
}

#[derive(uniffi::Record, Clone)]
pub struct ProfileBadge {
    pub description: String,
    pub icon_url: String,
}

#[derive(uniffi::Record, Clone)]
pub struct ProfileConnection {
    pub kind: String,
    pub name: String,
    pub verified: bool,
}

#[derive(uniffi::Record, Clone)]
pub struct MutualGuild {
    pub guild_id: String,
    pub name: String,
    pub icon_url: Option<String>,
    pub nick: Option<String>,
}

#[derive(uniffi::Record, Clone)]
pub struct MutualFriend {
    pub user_id: String,
    pub name: String,
    pub avatar_url: String,
}

#[derive(uniffi::Record, Clone)]
pub struct ProfileItem {
    pub user_id: String,
    pub display_name: String,
    pub username: String,
    pub avatar_url: String,
    pub banner_url: Option<String>,
    pub accent_color: Option<u32>,
    pub status: String,
    pub is_bot: bool,
    pub pronouns: Option<String>,
    pub bio: Option<RichText>,
    pub roles: Vec<ProfileRole>,
    pub member_since_ms: Option<i64>,
    pub mutual_guilds: u32,
    pub mutual_friends: u32,
    pub is_friend: bool,
    /// Account creation time, from the user ID.
    pub created_ms: i64,
    pub badges: Vec<ProfileBadge>,
    pub connections: Vec<ProfileConnection>,
    pub mutual_guild_list: Vec<MutualGuild>,
    pub mutual_friend_list: Vec<MutualFriend>,
    /// True once the full profile has been fetched.
    pub complete: bool,
    /// Your private note on this person.
    pub note: String,
}

impl ProfileItem {
    pub(crate) fn empty(user_id: &str) -> Self {
        ProfileItem {
            user_id: user_id.into(),
            display_name: user_id.into(),
            username: String::new(),
            avatar_url: String::new(),
            banner_url: None,
            accent_color: None,
            status: "offline".into(),
            is_bot: false,
            pronouns: None,
            bio: None,
            roles: vec![],
            member_since_ms: None,
            mutual_guilds: 0,
            mutual_friends: 0,
            is_friend: false,
            created_ms: 0,
            badges: vec![],
            connections: vec![],
            mutual_guild_list: vec![],
            mutual_friend_list: vec![],
            complete: false,
            note: String::new(),
        }
    }
}

pub(crate) fn local_profile(s: &Store, user: Id, guild: Option<Id>) -> Option<ProfileItem> {
    let u = s.users.get(&user)?;
    let status = guild
        .and_then(|g| s.member_lists.get(&g))
        .and_then(|l| {
            l.items
                .iter()
                .filter_map(|i| i.member.as_ref())
                .find(|m| m.member.user.as_ref().map(|x| x.id) == Some(user))
        })
        .and_then(|m| m.presence.as_ref().map(|p| p.status.clone()))
        .unwrap_or_else(|| {
            if Some(user) == s.my_id() {
                s.my_status.clone()
            } else {
                "offline".into()
            }
        });
    let roles = match (guild, guild.and_then(|g| s.members.get(&(g, user)))) {
        (Some(g), Some(m)) => {
            let all = s.roles.get(&g);
            let mut rs: Vec<_> = m
                .roles
                .iter()
                .filter_map(|r| all.and_then(|a| a.get(r)))
                .collect();
            rs.sort_by_key(|r| std::cmp::Reverse(r.position));
            rs.iter()
                .map(|r| ProfileRole {
                    name: r.name.clone(),
                    color: (r.color != 0).then_some(r.color),
                })
                .collect()
        }
        _ => vec![],
    };
    Some(ProfileItem {
        user_id: user.to_string(),
        display_name: s.display_name(guild, user),
        username: u.username.clone(),
        avatar_url: u.avatar_url(160),
        banner_url: None,
        accent_color: None,
        status,
        is_bot: u.bot,
        pronouns: None,
        bio: None,
        roles,
        member_since_ms: None,
        mutual_guilds: 0,
        mutual_friends: 0,
        is_friend: s.relationships.iter().any(|r| r.id == user && r.kind == 1),
        created_ms: (user.0 >> 22) as i64 + 1_420_070_400_000,
        badges: vec![],
        connections: vec![],
        mutual_guild_list: vec![],
        mutual_friend_list: vec![],
        complete: false,
        note: s.notes.get(&user).cloned().unwrap_or_default(),
    })
}

pub(crate) fn merge_profile(
    s: &Store,
    guild: Option<Id>,
    p: &mut ProfileItem,
    v: &serde_json::Value,
) {
    let str_of = |x: &serde_json::Value| x.as_str().filter(|t| !t.is_empty()).map(str::to_string);
    let user = &v["user"];
    if let (Some(uid), Some(hash)) = (str_of(&user["id"]), str_of(&user["banner"])) {
        let ext = if hash.starts_with("a_") {
            "gif"
        } else {
            "webp"
        };
        p.banner_url = Some(format!(
            "https://cdn.discordapp.com/banners/{uid}/{hash}.{ext}?size=600"
        ));
    }
    p.accent_color = user["accent_color"]
        .as_u64()
        .map(|c| c as u32)
        .or_else(|| v["user_profile"]["accent_color"].as_u64().map(|c| c as u32));
    let gp = &v["guild_member_profile"];
    p.pronouns = str_of(&gp["pronouns"]).or_else(|| str_of(&v["user_profile"]["pronouns"]));
    let bio = str_of(&gp["bio"])
        .or_else(|| str_of(&v["user_profile"]["bio"]))
        .or_else(|| str_of(&user["bio"]));
    p.bio = bio.map(|b| rich(s, guild, &b));
    p.member_since_ms = str_of(&v["guild_member"]["joined_at"]).and_then(|t| iso_ms(&t));
    p.mutual_guilds = v["mutual_guilds"].as_array().map_or(0, |a| a.len() as u32);
    p.mutual_friends = v["mutual_friends"].as_array().map_or(0, |a| a.len() as u32);
    p.badges = v["badges"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|b| {
                    Some(ProfileBadge {
                        description: str_of(&b["description"])?,
                        icon_url: format!(
                            "https://cdn.discordapp.com/badge-icons/{}.png",
                            str_of(&b["icon"])?
                        ),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    p.connections = v["connected_accounts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some(ProfileConnection {
                        kind: str_of(&c["type"])?,
                        name: str_of(&c["name"])?,
                        verified: c["verified"].as_bool().unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    p.mutual_guild_list = v["mutual_guilds"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|g| {
                    let id: Id = str_of(&g["id"])?.parse().ok()?;
                    let guild = s.guilds.get(&id)?;
                    Some(MutualGuild {
                        guild_id: id.to_string(),
                        name: guild.name().to_string(),
                        icon_url: guild.icon_url(64),
                        nick: str_of(&g["nick"]),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    p.mutual_friend_list = v["mutual_friends"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    let id: Id = str_of(&f["id"])?.parse().ok()?;
                    let name = str_of(&f["global_name"]).or_else(|| str_of(&f["username"]))?;
                    let avatar_url = match str_of(&f["avatar"]) {
                        Some(h) => {
                            format!("https://cdn.discordapp.com/avatars/{id}/{h}.webp?size=64")
                        }
                        None => format!(
                            "https://cdn.discordapp.com/embed/avatars/{}.png",
                            (id.0 >> 22) % 6
                        ),
                    };
                    Some(MutualFriend {
                        user_id: id.to_string(),
                        name,
                        avatar_url,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    p.complete = true;
}

/// `2021-05-01T12:34:56.123+00:00` → Unix ms (UTC offsets are always +00:00 from Discord).
pub(crate) fn iso_ms(t: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| t.get(r)?.parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (
        n(0..4)?,
        n(5..7)?,
        n(8..10)?,
        n(11..13)?,
        n(14..16)?,
        n(17..19)?,
    );
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(((days * 24 + hh) * 60 + mm) * 60_000 + ss * 1000)
}

#[uniffi::export]
impl UnisonEngine {
    /// A user's profile card. What's known locally is returned at once; the full profile
    /// (bio, pronouns, mutuals) follows through `on_profile`, fetched like the official popout.
    pub fn profile(
        &self,
        account_id: String,
        user_id: String,
        guild_id: Option<String>,
    ) -> Option<ProfileItem> {
        let guild = guild_id.as_deref().map(id);
        self.with_store(&account_id, |s| local_profile(s, id(&user_id), guild))
            .flatten()
    }

    /// Save your private note on someone (up to 256 characters; empty clears it).
    pub fn set_user_note(&self, account_id: String, user_id: String, note: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.set_note(i, id(&user_id), &note).await });
    }

    pub fn fetch_profile(&self, account_id: String, user_id: String, guild_id: Option<String>) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let listener = self.listener.clone();
        let guild = guild_id.as_deref().map(id);
        self.inner.rt.spawn(async move {
            let uid = id(&user_id);
            // The card and the full profile both ask when they open: one request serves both.
            let key = format!("profile:{uid}:{}", guild.map(|g| g.to_string()).unwrap_or_default());
            if !s.reads.begin(&key, std::time::Duration::ZERO) {
                return;
            }
            if Some(uid) != s.store.read().my_id() {
                s.load_note(uid).await;
            }
            let result = s.profile(uid, guild).await;
            s.reads.finish(&key, false);
            match result {
                Ok(v) => {
                    let item = {
                        let st = s.store.read();
                        let mut p = local_profile(&st, uid, guild)
                            .unwrap_or_else(|| ProfileItem::empty(&user_id));
                        merge_profile(&st, guild, &mut p, &v);
                        p
                    };
                    listener.on_profile(account_id, item);
                }
                Err(e) => tracing::debug!("profile fetch failed: {e}"),
            }
        });
    }
}
