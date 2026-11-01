//! Composer autocomplete, emoji/sticker catalogs and the GIF picker.

use unison_core::emoji_catalog;
use unison_core::model::channel_type;
use unison_core::suggest::{self, EntryKind, Kind};

use super::*;

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum SuggestKind {
    Mention,
    Channel,
    Emoji,
}

impl From<SuggestKind> for Kind {
    fn from(k: SuggestKind) -> Kind {
        match k {
            SuggestKind::Mention => Kind::Mention,
            SuggestKind::Channel => Kind::Channel,
            SuggestKind::Emoji => Kind::Emoji,
        }
    }
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum SuggestItemKind {
    User,
    Role,
    Everyone,
    Here,
    Channel,
    Unicode,
    Custom,
}

/// What the caret is typing (UTF-16 offsets into the text given).
#[derive(uniffi::Record, Clone, Debug)]
pub struct SuggestTrigger {
    pub kind: SuggestKind,
    pub query: String,
    pub start: u32,
    pub end: u32,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SuggestItem {
    pub kind: SuggestItemKind,
    pub id: Option<String>,
    pub label: String,
    pub detail: String,
    /// What goes into the message: `<@id>`, `<#id>`, `<:name:id>` or the emoji.
    pub insert: String,
    /// What the composer shows for it.
    pub display: String,
    pub glyph: Option<String>,
    pub image_url: Option<String>,
    pub animated: bool,
    pub color: u32,
    pub channel_kind: u8,
    pub usable: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct CatalogEmoji {
    pub name: String,
    pub glyph: String,
    pub tones: Vec<String>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct CatalogCategory {
    pub id: String,
    pub title: String,
    pub emoji: Vec<CatalogEmoji>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ServerEmoji {
    pub id: String,
    pub name: String,
    pub url: String,
    pub animated: bool,
    pub usable: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ServerEmojiGroup {
    pub guild_id: String,
    pub guild_name: String,
    pub icon_url: Option<String>,
    pub emoji: Vec<ServerEmoji>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct StickerPick {
    pub id: String,
    pub name: String,
    pub url: String,
    pub usable: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct StickerGroup {
    pub guild_id: String,
    pub guild_name: String,
    pub icon_url: Option<String>,
    pub stickers: Vec<StickerPick>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct GifItem {
    pub id: String,
    /// The page URL: what gets sent as the message.
    pub url: String,
    /// Video preview (`media_format`).
    pub src: String,
    pub gif_src: String,
    pub width: u32,
    pub height: u32,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct GifCategory {
    pub name: String,
    pub src: String,
}

fn sg_item(s: suggest::Suggestion) -> SuggestItem {
    SuggestItem {
        kind: match s.kind {
            EntryKind::User => SuggestItemKind::User,
            EntryKind::Role => SuggestItemKind::Role,
            EntryKind::Everyone => SuggestItemKind::Everyone,
            EntryKind::Here => SuggestItemKind::Here,
            EntryKind::Channel => SuggestItemKind::Channel,
            EntryKind::Unicode => SuggestItemKind::Unicode,
            EntryKind::Custom => SuggestItemKind::Custom,
        },
        image_url: if s.kind == EntryKind::Custom { Some(emoji_url(&s.id.map(|i| i.to_string()).unwrap_or_default(), s.animated, false)) } else { s.image_url },
        id: s.id.map(|i| i.to_string()),
        label: s.label,
        detail: s.detail,
        insert: s.insert,
        display: s.display,
        glyph: s.glyph,
        animated: s.animated,
        color: s.color,
        channel_kind: s.channel_kind,
        usable: s.usable,
    }
}

/// Which popup (if any) the text before the caret asks for.
#[uniffi::export]
pub fn parse_suggest_trigger(before_caret: String) -> Option<SuggestTrigger> {
    suggest::parse_trigger(&before_caret).map(|t| SuggestTrigger {
        kind: match t.kind {
            Kind::Mention => SuggestKind::Mention,
            Kind::Channel => SuggestKind::Channel,
            Kind::Emoji => SuggestKind::Emoji,
        },
        query: t.query,
        start: t.start as u32,
        end: t.end as u32,
    })
}

/// Discord's unicode emoji by category, with skin tones. Static: ask once.
#[uniffi::export]
pub fn emoji_catalog() -> Vec<CatalogCategory> {
    emoji_catalog::CATEGORIES
        .iter()
        .map(|(cid, title)| CatalogCategory {
            id: cid.to_string(),
            title: title.to_string(),
            emoji: emoji_catalog::all()
                .iter()
                .filter(|e| e.category == *cid)
                .map(|e| CatalogEmoji {
                    name: e.names[0].to_string(),
                    glyph: e.emoji.to_string(),
                    tones: e.tones.iter().map(|t| t.to_string()).collect(),
                })
                .collect(),
        })
        .collect()
}

fn sticker_url(id: u64, format: u8) -> Option<String> {
    // Lottie (3) needs a renderer; APNG (2) and PNG (1) are served as PNG, GIF (4) as GIF.
    match format {
        3 => None,
        4 => Some(format!("https://media.discordapp.net/stickers/{id}.gif?size=160")),
        _ => Some(format!("https://media.discordapp.net/stickers/{id}.png?size=160")),
    }
}

fn gif_str(v: &serde_json::Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

fn gif_items(v: &serde_json::Value) -> Vec<GifItem> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|g| GifItem {
                    id: gif_str(g, "id"),
                    url: gif_str(g, "url"),
                    src: gif_str(g, "src"),
                    gif_src: gif_str(g, "gif_src"),
                    width: g["width"].as_u64().unwrap_or(0) as u32,
                    height: g["height"].as_u64().unwrap_or(0) as u32,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A token in raw composer text to show as a pill (UTF-16 offsets into the raw text).
#[derive(uniffi::Record, Clone, Debug)]
pub struct TokenSpan {
    pub start: u32,
    pub len: u32,
    pub raw: String,
    pub display: String,
}

#[uniffi::export]
impl UnisonEngine {
    /// Mentions, channels and custom emoji in `text` the composer can show as pills.
    pub fn composer_tokens(&self, account_id: String, channel_id: String, text: String) -> Vec<TokenSpan> {
        self.with_store(&account_id, |s| suggest::tokens(s, id(&channel_id), &text))
            .unwrap_or_default()
            .into_iter()
            .map(|t| TokenSpan { start: t.start as u32, len: t.len as u32, raw: t.raw, display: t.display })
            .collect()
    }

    /// Candidates for the popup the caret is in. Reads the store only.
    pub fn suggest(&self, account_id: String, channel_id: String, kind: SuggestKind, query: String, limit: u32) -> Vec<SuggestItem> {
        self.with_store(&account_id, |s| suggest::suggest(s, id(&channel_id), kind.into(), &query, limit as usize))
            .unwrap_or_default()
            .into_iter()
            .map(sg_item)
            .collect()
    }

    /// Ask the gateway for matching members when the guild is bigger than what we hold (op 8).
    /// Results arrive as a `MembersFound` delta.
    pub fn search_members(&self, account_id: String, channel_id: String, query: String) {
        if let Some(s) = self.inner.session(id(&account_id)) {
            s.search_members(id(&channel_id), &query);
        }
    }

    /// `:name:` → emoji (and `<:name:id>` for usable custom emoji), as the client does on send.
    pub fn expand_shortcodes(&self, account_id: String, channel_id: String, text: String) -> String {
        self.with_store(&account_id, |s| suggest::expand_shortcodes(s, id(&channel_id), &text)).unwrap_or(text)
    }

    /// Emoji picker search over unicode names and the custom emoji the user can see (locked ones
    /// included, flagged `usable: false`).
    pub fn search_emoji(&self, account_id: String, channel_id: String, query: String, limit: u32) -> Vec<SuggestItem> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return vec![];
        }
        self.with_store(&account_id, |s| {
            let frequent = s.emoji_prefs.as_ref().map(|p| p.frequent.clone()).unwrap_or_default();
            let rank = |n: &str| frequent.iter().position(|f| f == n).unwrap_or(usize::MAX);
            let mut out: Vec<(u32, SuggestItem)> = vec![];
            for (_g, gname, eid, name, animated, usable) in suggest::custom_emoji(s, Some(id(&channel_id))) {
                if let Some(score) = emoji_catalog::match_score(&q, &name.to_lowercase()) {
                    out.push((
                        score,
                        SuggestItem {
                            kind: SuggestItemKind::Custom,
                            id: Some(eid.to_string()),
                            label: name.clone(),
                            detail: gname,
                            insert: format!("<{}:{}:{}>", if animated { "a" } else { "" }, name, eid),
                            display: format!(":{name}:"),
                            glyph: None,
                            image_url: Some(emoji_url(&eid.to_string(), animated, false)),
                            animated,
                            color: 0,
                            channel_kind: 0,
                            usable,
                        },
                    ));
                }
            }
            for e in emoji_catalog::search(&q, limit as usize, &rank) {
                let score = e.names.iter().filter_map(|n| emoji_catalog::match_score(&q, n)).min().unwrap_or(3);
                out.push((
                    score,
                    SuggestItem {
                        kind: SuggestItemKind::Unicode,
                        id: None,
                        label: e.names[0].to_string(),
                        detail: String::new(),
                        insert: e.emoji.to_string(),
                        display: e.emoji.to_string(),
                        glyph: Some(e.emoji.to_string()),
                        image_url: None,
                        animated: false,
                        color: 0,
                        channel_kind: 0,
                        usable: true,
                    },
                ));
            }
            out.sort_by_key(|(sc, _)| *sc);
            out.into_iter().take(limit as usize).map(|(_, i)| i).collect()
        })
        .unwrap_or_default()
    }

    /// Custom emoji by server, the open channel's server first.
    pub fn server_emoji(&self, account_id: String, channel_id: String) -> Vec<ServerEmojiGroup> {
        self.with_store(&account_id, |s| {
            let here = s.channels.get(&id(&channel_id)).and_then(|c| c.guild_id);
            let mut groups: Vec<ServerEmojiGroup> = vec![];
            for (g, gname, eid, name, animated, usable) in suggest::custom_emoji(s, Some(id(&channel_id))) {
                let item = ServerEmoji { id: eid.to_string(), name, url: emoji_url(&eid.to_string(), animated, false), animated, usable };
                match groups.iter_mut().find(|x| x.guild_id == g.to_string()) {
                    Some(x) => x.emoji.push(item),
                    None => groups.push(ServerEmojiGroup {
                        guild_id: g.to_string(),
                        guild_name: gname,
                        icon_url: s.guilds.get(&g).and_then(|gg| gg.icon_url(64)),
                        emoji: vec![item],
                    }),
                }
            }
            groups.sort_by_key(|x| Some(x.guild_id.clone()) != here.map(|h| h.to_string()));
            groups
        })
        .unwrap_or_default()
    }

    /// The servers' custom stickers the user can send here (other servers' need Nitro).
    pub fn server_stickers(&self, account_id: String, channel_id: String) -> Vec<StickerGroup> {
        self.with_store(&account_id, |s| {
            let here = s.channels.get(&id(&channel_id)).and_then(|c| c.guild_id);
            let nitro = suggest::has_nitro(s);
            let mut groups: Vec<StickerGroup> = s
                .guilds
                .values()
                .filter_map(|g| {
                    let stickers: Vec<StickerPick> = g
                        .stickers
                        .iter()
                        .filter(|st| st.available != Some(false))
                        .filter_map(|st| {
                            Some(StickerPick {
                                id: st.id.to_string(),
                                name: st.name.clone(),
                                url: sticker_url(st.id.0, st.format_type)?,
                                usable: here == Some(g.id) || nitro,
                            })
                        })
                        .collect();
                    (!stickers.is_empty()).then(|| StickerGroup {
                        guild_id: g.id.to_string(),
                        guild_name: g.name().to_string(),
                        icon_url: g.icon_url(64),
                        stickers,
                    })
                })
                .collect();
            groups.sort_by_key(|x| (Some(x.guild_id.clone()) != here.map(|h| h.to_string()), x.guild_name.clone()));
            groups
        })
        .unwrap_or_default()
    }

    /// Whether the channel takes stickers, GIFs and external emoji (a text-like channel we can send in).
    pub fn can_use_pickers(&self, account_id: String, channel_id: String) -> bool {
        self.with_store(&account_id, |s| {
            s.channels
                .get(&id(&channel_id))
                .is_some_and(|c| c.kind != channel_type::GUILD_CATEGORY && c.has_text_chat())
        })
        .unwrap_or(false)
    }

    pub fn send_sticker(&self, account_id: String, channel_id: String, sticker_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, intent| async move { s.send_sticker(intent, id(&channel_id), id(&sticker_id)).await });
    }

    /// Trending (empty `query`: categories plus the trending grid) or search results, answered
    /// through `on_gifs`.
    pub fn fetch_gifs(&self, account_id: String, query: String) {
        let Some(s) = self.inner.session(id(&account_id)) else { return };
        let listener = self.listener.clone();
        self.inner.rt.spawn(async move {
            let q = query.trim().to_string();
            let result = if q.is_empty() {
                let cats = s.rest.gifs_trending().await;
                let items = s.rest.gifs_trending_gifs(50).await;
                match (cats, items) {
                    (Ok(cats), Ok(items)) => {
                        let categories = cats["categories"]
                            .as_array()
                            .map(|a| a.iter().map(|c| GifCategory { name: gif_str(c, "name"), src: gif_str(c, "src") }).collect())
                            .unwrap_or_default();
                        Ok((categories, gif_items(&items)))
                    }
                    (Err(e), _) | (_, Err(e)) => Err(e),
                }
            } else {
                s.rest.gifs_search(&q, 50).await.map(|v| (vec![], gif_items(&v)))
            };
            match result {
                Ok((categories, gifs)) => listener.on_gifs(account_id, query, categories, gifs),
                Err(e) => listener.on_error(account_id, e.user_message()),
            }
        });
    }

    /// Discord's own sticker packs (usable anywhere), shaped like the server groups.
    pub async fn sticker_packs(&self, account_id: String) -> Result<Vec<StickerGroup>, FfiError> {
        let s = self
            .inner
            .session(id(&account_id))
            .ok_or_else(|| FfiError::Failed { message: "no such account".into() })?;
        let jh = self.inner.rt.spawn(async move { s.rest.sticker_packs().await });
        let v = jh.await.map_err(|e| FfiError::Failed { message: e.to_string() })??;
        Ok(v["sticker_packs"]
            .as_array()
            .map(|packs| {
                packs
                    .iter()
                    .filter_map(|p| {
                        let stickers: Vec<StickerPick> = p["stickers"]
                            .as_array()?
                            .iter()
                            .filter_map(|st| {
                                let sid: u64 = st["id"].as_str()?.parse().ok()?;
                                Some(StickerPick {
                                    id: sid.to_string(),
                                    name: gif_str(st, "name"),
                                    url: sticker_url(sid, st["format_type"].as_u64().unwrap_or(1) as u8)?,
                                    usable: true,
                                })
                            })
                            .collect();
                        (!stickers.is_empty()).then(|| StickerGroup {
                            guild_id: gif_str(p, "id"),
                            guild_name: gif_str(p, "name"),
                            icon_url: None,
                            stickers,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The user's favourite GIFs, in their order (`id` and `url` are the GIF page URL).
    pub fn favorite_gifs(&self, account_id: String) -> Vec<GifItem> {
        self.with_store(&account_id, |s| {
            s.emoji_prefs
                .as_ref()
                .map(|p| {
                    p.gifs
                        .iter()
                        .map(|g| GifItem { id: g.url.clone(), url: g.url.clone(), src: g.src.clone(), gif_src: g.src.clone(), width: g.width, height: g.height })
                        .collect()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default()
    }

    /// Star or unstar a GIF. Stored as an image-format favourite pointing at the GIF file, so the
    /// tile can show it without a video decoder.
    pub fn set_favorite_gif(&self, account_id: String, gif: GifItem, favorite: bool, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, intent| async move {
            let entry = favorite.then(|| {
                let order = s.store.read().emoji_prefs.as_ref().and_then(|p| p.gifs.iter().map(|g| g.order).max()).map_or(1, |m| m + 1);
                unison_core::model::settings_proto::FavoriteGif { url: gif.url.clone(), format: 1, src: gif.gif_src.clone(), width: gif.width, height: gif.height, order }
            });
            s.set_favorite_gif(intent, gif.url, entry).await
        });
    }

    /// Tell Discord which GIF answered a search (`POST /gifs/select`), as picking one does.
    pub fn gif_selected(&self, account_id: String, gif_id: String, query: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, intent| async move { s.rest.gifs_select(intent, &gif_id, &query).await });
    }
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq)]
pub enum ComposerStyleKind {
    Marker,
    Bold,
    Italic,
    Underline,
    Strike,
    Spoiler,
    Code,
    CodeBlock,
    Quote,
    Heading { level: u8 },
    Subtext,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ComposerStyleSpan {
    pub start: u32,
    pub len: u32,
    pub kind: ComposerStyleKind,
}

/// How the composer should style what is typed: markdown shows as formatting, markers dimmed.
#[uniffi::export]
pub fn composer_styles(text: String) -> Vec<ComposerStyleSpan> {
    use unison_markdown::SourceKind as K;
    unison_markdown::style_source(&text)
        .into_iter()
        .map(|s| ComposerStyleSpan {
            start: s.start,
            len: s.len,
            kind: match s.kind {
                K::Marker => ComposerStyleKind::Marker,
                K::Bold => ComposerStyleKind::Bold,
                K::Italic => ComposerStyleKind::Italic,
                K::Underline => ComposerStyleKind::Underline,
                K::Strike => ComposerStyleKind::Strike,
                K::Spoiler => ComposerStyleKind::Spoiler,
                K::Code => ComposerStyleKind::Code,
                K::CodeBlock => ComposerStyleKind::CodeBlock,
                K::Quote => ComposerStyleKind::Quote,
                K::Heading(level) => ComposerStyleKind::Heading { level },
                K::Subtext => ComposerStyleKind::Subtext,
            },
        })
        .collect()
}
