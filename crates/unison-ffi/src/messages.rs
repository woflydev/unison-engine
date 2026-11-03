use super::*;

#[derive(uniffi::Enum, Clone, Debug, PartialEq, Hash)]
pub enum SpanStyle {
    Bold,
    Italic,
    Underline,
    Strike,
    Spoiler,
    Code,
    CodeBlock { lang: Option<String> },
    CodeSection,
    CodeMeta,
    Quote,
    Heading { level: u8 },
    Subtext,
    ListItem { depth: u8 },
    Link { url: String },
    UserMention { id: String, is_me: bool },
    RoleMention { id: String, color: Option<u32>, is_me: bool },
    ChannelMention { id: String },
    Everyone,
    Emoji { url: String },
    Timestamp { unix: i64, style: String },
    SlashCommand { id: String },
    /// The note ending a long code block that was cut short; clicking it opens the whole block.
    CodeCollapse { hidden: u32 },
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Hash)]
pub struct Span {
    pub start: u32,
    pub len: u32,
    pub style: SpanStyle,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Hash)]
pub struct RichText {
    pub text: String,
    pub spans: Vec<Span>,
    pub jumbo_emoji: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct AttachmentItem {
    /// Flagged explicit by Discord's scan (the attachment's or its message's `CONTAINS_EXPLICIT_MEDIA`):
    /// shown behind a click-to-reveal cover, like a spoiler.
    pub sensitive: bool,
    pub id: String,
    pub filename: String,
    pub url: String,
    pub proxy_url: String,
    pub content_type: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size: u64,
    pub spoiler: bool,
    pub description: Option<String>,
    /// Voice messages and audio: length in seconds and Discord's base64 waveform (up to 256 samples).
    pub duration_secs: Option<f64>,
    pub waveform: Option<String>,
    pub is_voice: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct EmbedFieldItem {
    pub name: String,
    pub value: RichText,
    pub inline: bool,
}

/// A sticker on a message. `lottie` stickers are JSON animations; the rest are images.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Hash)]
pub struct StickerItem {
    pub url: String,
    pub lottie: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct EmbedItem {
    pub kind: String,
    pub title: Option<String>,
    pub url: Option<String>,
    pub description: Option<RichText>,
    pub color: Option<u32>,
    pub author_name: Option<String>,
    pub author_icon_url: Option<String>,
    pub provider_name: Option<String>,
    pub footer_text: Option<String>,
    pub image_url: Option<String>,
    pub image_width: Option<u32>,
    pub image_height: Option<u32>,
    pub thumbnail_url: Option<String>,
    pub thumbnail_width: Option<u32>,
    pub thumbnail_height: Option<u32>,
    pub video_url: Option<String>,
    pub video_width: Option<u32>,
    pub video_height: Option<u32>,
    pub fields: Vec<EmbedFieldItem>,
    /// Flagged by Discord's content scan: the shell covers its media until clicked.
    pub sensitive: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ReactionItem {
    pub emoji_name: String,
    pub emoji_id: Option<String>,
    pub emoji_url: Option<String>,
    pub count: u32,
    pub me: bool,
}

/// One emoji the reaction picker can offer.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct PickerEmoji {
    /// What `react` takes: the emoji itself, or `name:id` for a custom emoji.
    pub key: String,
    pub name: String,
    /// Set for unicode emoji.
    pub unicode: Option<String>,
    /// Set for custom emoji.
    pub url: Option<String>,
}

/// The user's favourite emoji and their most-used reactions, as Discord stores them.
#[derive(uniffi::Record, Clone, Debug, Default, PartialEq)]
pub struct ReactionEmojis {
    pub favorites: Vec<PickerEmoji>,
    pub frequent: Vec<PickerEmoji>,
    /// False until the settings have been read once this session.
    pub loaded: bool,
}

/// What a text-less original holds (picture, sticker, ...); the shell picks the glyph.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum ReplyMedia {
    None,
    Image,
    Video,
    File,
    Sticker,
    Embed,
}

impl From<unison_core::reply::ReplyMedia> for ReplyMedia {
    fn from(m: unison_core::reply::ReplyMedia) -> Self {
        use unison_core::reply::ReplyMedia as C;
        match m {
            C::None => Self::None,
            C::Image => Self::Image,
            C::Video => Self::Video,
            C::File => Self::File,
            C::Sticker => Self::Sticker,
            C::Embed => Self::Embed,
        }
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ReplyPreview {
    pub message_id: String,
    pub author_name: String,
    pub author_color: Option<u32>,
    /// The original's text, or "Click to see image" and the like when it has none.
    pub snippet: String,
    pub media: ReplyMedia,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct MessageItem {
    pub id: String,
    pub channel_id: String,
    pub author_id: String,
    pub author_name: String,
    pub author_color: Option<u32>,
    pub avatar_url: String,
    pub is_bot: bool,
    pub timestamp_ms: u64,
    pub edited: bool,
    pub pending: bool,
    pub content: RichText,
    /// Set for join/pin/boost and other system messages; rendered as a single line.
    pub system_text: Option<String>,
    /// False for message types we don't render natively (shown as an "open in web" card).
    pub supported: bool,
    pub attachments: Vec<AttachmentItem>,
    pub embeds: Vec<EmbedItem>,
    pub reactions: Vec<ReactionItem>,
    pub reply: Option<ReplyPreview>,
    /// This message as a one-line quote (the composer's "Replying to" bar).
    pub quote: String,
    pub quote_media: ReplyMedia,
    pub stickers: Vec<StickerItem>,
    pub has_components: bool,
    pub components: Vec<ComponentBlockItem>,
    pub poll: Option<PollItem>,
    pub forwarded: Option<ForwardItem>,
    pub thread: Option<ThreadChip>,
    /// Raw Discord message type, shown in the unsupported-type card.
    pub kind: u8,
    pub mentions_me: bool,
    pub pinned: bool,
    /// Continues the previous message's author block (same author, within 7 minutes, no reply or
    /// system row between): the shell draws it without avatar and name.
    pub grouped: bool,
    /// The user may add reactions here (drives the "add reaction" chip).
    pub can_react: bool,
    pub author_cosmetics: Option<crate::cosmetics::CosmeticsItem>,
    /// Icon of the author's highest role that has one, drawn beside the name.
    pub role_icon_url: Option<String>,
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum MemberRow {
    Header {
        title: String,
        count: u32,
    },
    Member {
        user_id: String,
        name: String,
        color: Option<u32>,
        avatar_url: String,
        status: String,
        is_bot: bool,
        /// Custom status or main activity, drawn under the name.
        activity: Option<presence::ActivityItem>,
        cosmetics: Option<crate::cosmetics::CosmeticsItem>,
    },
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct FileUpload {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    /// Hide the picture behind a spoiler cover (`is_spoiler` in the attachment entry).
    #[uniffi(default = false)]
    pub spoiler: bool,
    /// Alt text.
    #[uniffi(default = None)]
    pub description: Option<String>,
    /// A voice message: its length in seconds and base64 waveform (from `VoiceNoteRecorder`).
    #[uniffi(default = None)]
    pub voice_secs: Option<f64>,
    #[uniffi(default = None)]
    pub voice_waveform: Option<String>,
    /// Set when the shell converted or shrank the file: its type before, and the MD5 of its bytes before (`content_md5`).
    #[uniffi(default = None)]
    pub original_content_type: Option<String>,
    #[uniffi(default = None)]
    pub original_md5: Option<String>,
}

impl FileUpload {
    pub(crate) fn into_local(self) -> LocalFile {
        LocalFile {
            filename: self.filename,
            content_type: self.content_type,
            bytes: unison_core::Bytes::from(self.bytes),
            description: self.description,
            spoiler: self.spoiler,
            voice: self.voice_secs.zip(self.voice_waveform).map(|(duration_secs, waveform)| unison_core::rest::VoiceMeta { duration_secs, waveform }),
            original: self.original_content_type.zip(self.original_md5).map(|(content_type, md5)| unison_core::rest::OriginalFile { content_type, md5 }),
        }
    }
}

pub(crate) fn location(s: &Store, channel: Id) -> String {
    let Some(c) = s.channels.get(&channel) else {
        return String::new();
    };
    match c.guild_id.and_then(|g| s.guilds.get(&g)) {
        Some(g) => format!("{} · #{}", g.name(), c.name.as_deref().unwrap_or("")),
        None => dm_item(s, c).name,
    }
}

/// The server a channel's messages belong to, for naming people and roles in a message that came
/// without one (REST pages carry no server id). `hint` is what the surface itself knew.
pub(crate) fn guild_of_channel(s: &Store, channel: Id, hint: Option<Id>) -> Option<Id> {
    hint.or_else(|| s.channels.get(&channel).and_then(|c| unison_core::open_target::guild_of(s, c)))
}

pub(crate) struct StoreResolver<'a> {
    s: &'a Store,
    guild: Option<Id>,
}

impl Resolve for StoreResolver<'_> {
    fn user(&self, uid: &str) -> Option<String> {
        let u = id(uid);
        (self.s.users.contains_key(&u)
            || self
                .guild
                .is_some_and(|g| self.s.members.contains_key(&(g, u))))
        .then(|| self.s.display_name(self.guild, u))
    }
    fn role(&self, rid: &str) -> Option<String> {
        let r = id(rid);
        // A message fetched over REST (Inbox, search, saved) carries no server id, so the server
        // may be unknown; role ids are unique across servers, so look in all of them.
        self.guild
            .and_then(|g| self.s.roles.get(&g)?.get(&r))
            .or_else(|| self.s.roles.values().find_map(|m| m.get(&r)))
            .map(|r| r.name.clone())
    }
    fn channel(&self, cid: &str) -> Option<String> {
        self.s.channels.get(&id(cid)).and_then(|c| c.name.clone())
    }
    fn shortcode(&self, name: &str) -> Option<String> {
        unison_core::emoji_catalog::by_name(name).map(|e| e.emoji.to_string())
    }
}

/// Long code blocks cut down to their first lines (rules in `unison_core::comfort`).
fn collapse_code(mut r: RichText) -> RichText {
    if !r.text.contains("\n") {
        return r;
    }
    let blocks: Vec<(u32, u32)> = r.spans.iter().filter(|s| matches!(s.style, SpanStyle::CodeBlock { .. })).map(|s| (s.start, s.len)).collect();
    if blocks.is_empty() {
        return r;
    }
    let spans: Vec<(u32, u32)> = r.spans.iter().map(|s| (s.start, s.len)).collect();
    let Some(c) = unison_core::comfort::collapse_code(&r.text, &blocks, &spans) else { return r };
    let mut out: Vec<Span> = r.spans.drain(..).zip(c.spans).filter_map(|(s, m)| m.map(|(start, len)| Span { start, len, ..s })).collect();
    out.extend(c.notes.into_iter().map(|(start, len, hidden)| Span { start, len, style: SpanStyle::CodeCollapse { hidden } }));
    RichText { text: c.text, spans: out, jumbo_emoji: r.jumbo_emoji }
}

/// Message text for a one-line preview (replies, saved, search, inbox, notifications, quick
/// switcher): mentions shown as names, markdown flattened, whitespace collapsed, shortened to `max`
/// characters. Every popout and list that shows message text without its styling goes through this;
/// a raw `<@123>` or `<@&123>` reaching a shell is a bug.
pub(crate) fn preview_text(s: &Store, guild: Option<Id>, src: &str, max: usize) -> String {
    plain(s, guild, src).split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

/// `src` with mentions resolved and markdown flattened, otherwise unshortened. Spoilers stay
/// hidden (a block per run): a one-line preview must not give away what the author covered.
fn plain(s: &Store, guild: Option<Id>, src: &str) -> String {
    unison_markdown::render_with(src, &StoreResolver { s, guild }).masked_text()
}

pub(crate) fn rich(s: &Store, guild: Option<Id>, src: &str) -> RichText {
    let r = unison_markdown::render_with(src, &StoreResolver { s, guild });
    let me = s.my_id().map(|m| m.to_string());
    let spans = r
        .spans
        .into_iter()
        .map(|sp| Span {
            start: sp.start,
            len: sp.len,
            style: match sp.kind {
                SpanKind::Bold => SpanStyle::Bold,
                SpanKind::Italic => SpanStyle::Italic,
                SpanKind::Underline => SpanStyle::Underline,
                SpanKind::Strike => SpanStyle::Strike,
                SpanKind::Spoiler => SpanStyle::Spoiler,
                SpanKind::Code => SpanStyle::Code,
                SpanKind::CodeBlock { lang } => SpanStyle::CodeBlock { lang },
                SpanKind::CodeSection => SpanStyle::CodeSection,
                SpanKind::CodeMeta => SpanStyle::CodeMeta,
                SpanKind::Quote => SpanStyle::Quote,
                SpanKind::Heading(level) => SpanStyle::Heading { level },
                SpanKind::Subtext => SpanStyle::Subtext,
                SpanKind::ListItem { depth } => SpanStyle::ListItem { depth },
                SpanKind::Link(url) => SpanStyle::Link {
                    url: privacy::clean_outbound_url(&url),
                },
                SpanKind::UserMention(uid) => SpanStyle::UserMention {
                    is_me: me.as_deref() == Some(uid.as_str()),
                    id: uid,
                },
                SpanKind::RoleMention(rid) => SpanStyle::RoleMention {
                    is_me: guild.is_some_and(|g| s.has_role(g, id(&rid))),
                    color: guild
                        .and_then(|g| s.roles.get(&g))
                        .and_then(|r| r.get(&id(&rid)))
                        .map(|r| r.color)
                        .filter(|c| *c != 0),
                    id: rid,
                },
                SpanKind::ChannelMention(cid) => SpanStyle::ChannelMention { id: cid },
                SpanKind::Everyone => SpanStyle::Everyone,
                SpanKind::Emoji { id: eid, animated } => SpanStyle::Emoji {
                    url: emoji_url(&eid, animated, r.jumbo_emoji),
                },
                SpanKind::Timestamp { unix, style } => SpanStyle::Timestamp {
                    unix,
                    style: style.to_string(),
                },
                SpanKind::SlashCommand { id: cid } => SpanStyle::SlashCommand { id: cid },
            },
        })
        .collect();
    RichText {
        text: r.text,
        spans,
        jumbo_emoji: r.jumbo_emoji,
    }
}

/// "A, B, C and 9 others" for a card that shows the emoji itself (the same rule as the hover sentence).
#[uniffi::export]
pub fn reaction_names_line(names: Vec<String>, total: u32) -> String {
    unison_core::reaction_info::names_line(&names, total)
}

/// The `:name:` Discord writes a unicode emoji as (`skull` for 💀), if it has one.
#[uniffi::export]
pub fn emoji_shortcode(emoji: String) -> Option<String> {
    unison_core::emoji::shortcode(&emoji).map(str::to_string)
}

pub(crate) fn reaction_emojis(s: &Store) -> ReactionEmojis {
    let Some(prefs) = &s.emoji_prefs else {
        return ReactionEmojis::default();
    };
    // Custom emoji are stored by id: resolve them against the servers we're in (an emoji from a
    // server we've left can't be used, and the official picker hides it too).
    let custom = |key: &str| -> Option<PickerEmoji> {
        let eid: u64 = key.parse().ok()?;
        let e = s
            .guilds
            .values()
            .flat_map(|g| g.emojis.iter())
            .find(|e| e.id.map(|i| i.0) == Some(eid))?;
        let name = e.name.clone()?;
        Some(PickerEmoji {
            key: format!("{name}:{eid}"),
            name,
            unicode: None,
            url: Some(emoji_url(&eid.to_string(), e.animated, false)),
        })
    };
    let resolve = |key: &String| -> Option<PickerEmoji> {
        if let Some(u) = unison_core::emoji::by_name(key) {
            return Some(PickerEmoji {
                key: u.to_string(),
                name: key.clone(),
                unicode: Some(u.to_string()),
                url: None,
            });
        }
        custom(key)
    };
    ReactionEmojis {
        favorites: prefs.favorites.iter().filter_map(resolve).collect(),
        frequent: prefs.frequent.iter().filter_map(resolve).take(32).collect(),
        loaded: true,
    }
}

pub(crate) fn emoji_url(eid: &str, animated: bool, jumbo: bool) -> String {
    let size = if jumbo { 96 } else { 48 };
    let ext = if animated { "gif" } else { "webp" };
    format!("https://cdn.discordapp.com/emojis/{eid}.{ext}?size={size}&quality=lossless")
}

pub(crate) fn system_text(s: &Store, guild: Option<Id>, m: &Message) -> Option<String> {
    let who = s.display_name(guild, m.author.id);
    let target = m
        .mentions
        .first()
        .map(|u| s.display_name(guild, u.id))
        .unwrap_or_default();
    Some(match m.kind {
        message_type::RECIPIENT_ADD => format!("{who} added {target} to the group."),
        message_type::RECIPIENT_REMOVE if target.is_empty() || target == who => {
            format!("{who} left the group.")
        }
        message_type::RECIPIENT_REMOVE => format!("{who} removed {target} from the group."),
        message_type::CALL => format!("{who} started a call."),
        message_type::CHANNEL_NAME_CHANGE => {
            format!("{who} changed the channel name: {}", m.content)
        }
        message_type::CHANNEL_ICON_CHANGE => format!("{who} changed the channel icon."),
        message_type::CHANNEL_PINNED_MESSAGE => format!("{who} pinned a message to this channel."),
        message_type::USER_JOIN => format!("{who} joined the server."),
        message_type::GUILD_BOOST => format!("{who} boosted the server."),
        message_type::THREAD_CREATED => format!("{who} started a thread: {}", m.content),
        _ => return None,
    })
}

/// Author grouping: the rule lives here so every shell draws the same blocks. A message that started a
/// thread leads from its author's picture to its thread's card, so two such messages in a row are two
/// blocks, each with its own picture (as Discord draws them): a line cannot start from a picture that is
/// not there.
fn mark_grouped(items: &mut [MessageItem]) {
    for i in 1..items.len() {
        let (p, m) = (&items[i - 1], &items[i]);
        items[i].grouped = p.author_id == m.author_id
            && m.reply.is_none()
            && m.system_text.is_none()
            && p.system_text.is_none()
            && !(p.thread.is_some() && m.thread.is_some())
            && m.timestamp_ms.saturating_sub(p.timestamp_ms) < 7 * 60 * 1000;
    }
}

/// A transcript never points at where it already is: inside a thread, the message it started (shown as the
/// thread's first row) carries no entrance to the thread you are reading.
fn hide_entrances_to(items: &mut [MessageItem], channel: &str) {
    for it in items.iter_mut() {
        if it.thread.as_ref().is_some_and(|t| t.id == channel) {
            it.thread = None;
        }
    }
}

/// The first row of a thread made from a message: Discord sends a type-21 message with no content
/// whose `referenced_message` is the original. The official client draws the original there (author,
/// text, pictures), not a reply line to it. The row keeps the starter's id and channel, so anything done
/// to it asks the thread, and drops the original's reactions, which live on the message in the parent.
pub(crate) fn thread_starter_as_message(m: &Message) -> Option<Message> {
    if m.kind != message_type::THREAD_STARTER_MESSAGE {
        return None;
    }
    let orig = m.referenced_message.as_deref()?;
    let mut shown = orig.clone();
    shown.id = m.id;
    shown.channel_id = m.channel_id;
    shown.guild_id = m.guild_id.or(orig.guild_id);
    shown.kind = message_type::DEFAULT;
    shown.message_reference = None;
    shown.referenced_message = None;
    shown.reactions.clear();
    Some(shown)
}

pub(crate) fn message_item(s: &Store, guild: Option<Id>, m: &Message) -> MessageItem {
    if let Some(shown) = thread_starter_as_message(m) {
        return message_item(s, guild, &shown);
    }
    // A reply whose original is gone: the web client says so instead of hiding the line.
    let reply_deleted = m.referenced_message.is_none()
        && m.kind == message_type::REPLY
        && m.message_reference.as_ref().is_some_and(|r| r.message_id.is_some());
    let reply = m.referenced_message.as_ref().map(|r| {
        let (snippet, media) = unison_core::reply::summary_of(r, &plain(s, guild, &r.content));
        ReplyPreview {
            message_id: r.id.to_string(),
            author_name: s.display_name(guild, r.author.id),
            author_color: s.name_color(guild, r.author.id),
            snippet,
            media: media.into(),
        }
    });
    let reply = reply.or_else(|| {
        reply_deleted.then(|| ReplyPreview {
            message_id: String::new(),
            author_name: String::new(),
            author_color: None,
            snippet: "Original message was deleted".into(),
            media: ReplyMedia::None,
        })
    });
    let (quote, quote_media) = unison_core::reply::summary_of(m, &plain(s, guild, &m.content));
    let mentions_me = s.pings_me(m);
    let avatar = match (guild, &m.member) {
        (Some(g), Some(mem)) if mem.avatar.is_some() => format!(
            "https://cdn.discordapp.com/guilds/{g}/users/{}/avatars/{}.webp?size=80",
            m.author.id,
            mem.avatar.as_deref().unwrap_or("")
        ),
        _ => m.author.avatar_url(80),
    };
    MessageItem {
        id: m.id.to_string(),
        channel_id: m.channel_id.to_string(),
        author_id: m.author.id.to_string(),
        author_name: s.display_name(guild, m.author.id),
        author_color: s.name_color(guild, m.author.id),
        author_cosmetics: crate::cosmetics::item_of(&m.author),
        role_icon_url: unison_core::cosmetics::role_icon_url(s, guild, m.author.id),
        avatar_url: avatar,
        is_bot: m.author.bot,
        timestamp_ms: if m.id.0 == 0 {
            now_ms()
        } else {
            m.id.timestamp_ms()
        },
        edited: m.edited_timestamp.is_some(),
        pending: m.id.0 == 0,
        content: if s.expanded_code.contains(&m.id) { rich(s, guild, &m.content) } else { collapse_code(rich(s, guild, &m.content)) },
        system_text: system_text(s, guild, m),
        supported: message_type::is_supported(m.kind),
        attachments: m
            .attachments
            .iter()
            .map(|a| {
                let mut i = attachment_item(a);
                // Message-level flag (1 << 9): the scan flagged something in this message.
                if m.flags & (1 << 9) != 0 && a.is_image() | a.is_video() {
                    i.sensitive = true;
                    i.spoiler = true;
                }
                i
            })
            .collect(),
        embeds: m
            .embeds
            .iter()
            .map(|e| {
                let mut i = embed_item(s, guild, e);
                i.sensitive |= m.flags & (1 << 9) != 0 && (i.image_url.is_some() || i.video_url.is_some());
                i
            })
            .collect(),
        reactions: m
            .reactions
            .iter()
            .map(|r| ReactionItem {
                emoji_name: r.emoji.name.clone().unwrap_or_default(),
                emoji_id: r.emoji.id.map(|i| i.to_string()),
                emoji_url: r
                    .emoji
                    .id
                    .map(|i| emoji_url(&i.to_string(), r.emoji.animated, false)),
                count: r.count,
                me: r.me,
            })
            .collect(),
        reply,
        quote,
        quote_media: quote_media.into(),
        stickers: m
            .sticker_items
            .iter()
            .map(|st| match st.format_type {
                // Lottie: the animation is JSON, drawn by the shell's Lottie player.
                3 => StickerItem { url: format!("https://cdn.discordapp.com/stickers/{}.json", st.id), lottie: true },
                // APNG (2) and PNG (1) share the `.png` URL; the animation is in the file.
                4 => StickerItem { url: format!("https://media.discordapp.net/stickers/{}.gif?size=160", st.id), lottie: false },
                _ => StickerItem { url: format!("https://media.discordapp.net/stickers/{}.png?size=160", st.id), lottie: false },
            })
            .collect(),
        has_components: !m.components.is_empty(),
        components: component_items(s, guild, m),
        poll: poll_item(m),
        forwarded: forward_item(s, guild, m),
        thread: thread_chip(s, m),
        kind: m.kind,
        mentions_me,
        pinned: m.pinned,
        grouped: false,
        can_react: s.channels.get(&m.channel_id).is_some_and(|c| s.permissions(c) & unison_core::store::perm::ADD_REACTIONS != 0),
    }
}

pub(crate) fn attachment_item(a: &model::Attachment) -> AttachmentItem {
    AttachmentItem {
        sensitive: a.is_explicit(),
        id: a.id.to_string(),
        filename: a.filename.clone(),
        url: a.url.clone(),
        proxy_url: a.proxy_url.clone(),
        content_type: a.content_type.clone(),
        width: a.width,
        height: a.height,
        size: a.size,
        spoiler: a.is_spoiler() || a.is_explicit(),
        description: a.description.clone(),
        duration_secs: a.duration_secs,
        waveform: a.waveform.clone(),
        is_voice: a.waveform.is_some() || a.flags & (1 << 13) != 0,
    }
}

pub(crate) fn embed_item(s: &Store, guild: Option<Id>, e: &model::Embed) -> EmbedItem {
    let media = |m: &Option<model::EmbedMedia>| {
        m.as_ref()
            .and_then(|x| x.proxy_url.clone().or_else(|| x.url.clone()))
    };
    EmbedItem {
        kind: e.kind.clone().unwrap_or_else(|| "rich".into()),
        title: e.title.clone(),
        url: e.url.as_deref().map(privacy::clean_outbound_url),
        description: e.description.as_deref().map(|d| rich(s, guild, d)),
        color: e.color,
        author_name: e.author.as_ref().and_then(|a| a.name.clone()),
        author_icon_url: e
            .author
            .as_ref()
            .and_then(|a| a.proxy_icon_url.clone().or_else(|| a.icon_url.clone())),
        provider_name: e.provider.as_ref().and_then(|p| p.name.clone()),
        footer_text: e.footer.as_ref().and_then(|f| f.text.clone()),
        image_url: media(&e.image),
        image_width: e.image.as_ref().and_then(|i| i.width),
        image_height: e.image.as_ref().and_then(|i| i.height),
        thumbnail_url: media(&e.thumbnail),
        thumbnail_width: e.thumbnail.as_ref().and_then(|i| i.width),
        thumbnail_height: e.thumbnail.as_ref().and_then(|i| i.height),
        // GIFV and hosted video play from the media proxy, as the web client's <video> does.
        video_url: e.video.as_ref().and_then(|v| v.proxy_url.clone().or_else(|| v.url.clone())),
        video_width: e.video.as_ref().and_then(|i| i.width),
        video_height: e.video.as_ref().and_then(|i| i.height),
        fields: e
            .fields
            .iter()
            .map(|f| EmbedFieldItem {
                name: f.name.clone(),
                value: rich(s, guild, &f.value),
                inline: f.inline,
            })
            .collect(),
        sensitive: e.is_explicit(),
    }
}

#[uniffi::export]
impl UnisonEngine {
    pub fn messages(&self, account_id: String, channel_id: String) -> Vec<MessageItem> {
        if let Some(sess) = self.inner.session(id(&account_id)) {
            sess.prime_history(id(&channel_id));
        }
        self.with_store(&account_id, |s| {
            let c = id(&channel_id);
            let guild = s.channels.get(&c).and_then(|c| c.guild_id);
            s.history
                .get(&c)
                .map(|h| {
                    let mut items: Vec<MessageItem> =
                        h.messages.iter().map(|m| message_item(s, guild, m)).collect();
                    hide_entrances_to(&mut items, &channel_id);
                    mark_grouped(&mut items);
                    items
                })
                .unwrap_or_default()
        })
        .unwrap_or_default()
    }

    pub fn message(
        &self,
        account_id: String,
        channel_id: String,
        message_id: String,
    ) -> Option<MessageItem> {
        self.with_store(&account_id, |s| {
            let c = id(&channel_id);
            let guild = s.channels.get(&c).and_then(|c| c.guild_id);
            s.find_message(c, id(&message_id))
                .map(|m| message_item(s, guild, m))
        })
        .flatten()
    }

    pub fn has_older_messages(&self, account_id: String, channel_id: String) -> bool {
        self.with_store(&account_id, |s| {
            s.history
                .get(&id(&channel_id))
                .is_some_and(|h| h.has_more_before)
        })
        .unwrap_or(false)
    }

    /// Has the core read this channel's messages (from the network or the disk cache)? False for a
    /// channel never opened before: its first page is still on its way, which is when the shell
    /// shows a spinner rather than an empty transcript.
    pub fn is_channel_loaded(&self, account_id: String, channel_id: String) -> bool {
        self.with_store(&account_id, |s| s.history.contains_key(&id(&channel_id))).unwrap_or(false)
    }

    /// False after a jump-to: the window is detached from the newest messages.
    pub fn is_at_latest(&self, account_id: String, channel_id: String) -> bool {
        self.with_store(&account_id, |s| {
            s.history.get(&id(&channel_id)).is_none_or(|h| h.at_latest)
        })
        .unwrap_or(true)
    }

    /// Should "Jump to present" be on screen? The one rule (`unison_core::jump_present`): the
    /// shell reports how much loaded transcript lies below the viewport and whether the control
    /// shows now (hysteresis); the core adds whether the window is the newest page and how far
    /// in time its end is from the channel's latest message.
    pub fn jump_to_present_visible(&self, account_id: String, channel_id: String, screens_below: f64, messages_below: u32, shown: bool) -> bool {
        self.with_store(&account_id, |s| {
            let c = id(&channel_id);
            let at_latest = s.history.get(&c).is_none_or(|h| h.at_latest);
            unison_core::jump_present::should_show(&unison_core::jump_present::Reading {
                at_latest,
                screens_below: screens_below as f32,
                messages_below,
                tail_ms: s.window_tail_ms(c),
                shown,
            })
        })
        .unwrap_or(false)
    }

    /// DevTools only (refuses unless automation is on, i.e. an offline scripted run): make the
    /// loaded window of a channel look like a page around an old message, `tail_minutes` before
    /// the channel's newest message. Lets the shell's jump-to-present be checked without REST.
    pub fn dev_detach_window(&self, account_id: String, channel_id: String, tail_minutes: u32) -> bool {
        if !unison_core::rest::AUTOMATION.load(std::sync::atomic::Ordering::Relaxed) {
            return false;
        }
        let Some(session) = self.inner.session(id(&account_id)) else { return false };
        let mut guard = session.store.write();
        let s = &mut *guard;
        {
            let c = id(&channel_id);
            let Some(newest) = s.history.get(&c).and_then(|h| h.messages.back().map(|m| m.id)) else { return false };
            let later = newest.0 + (u64::from(tail_minutes) * 60_000 << 22);
            if let Some(ch) = s.channels.get_mut(&c) {
                ch.last_message_id = Some(unison_core::snowflake::Snowflake(later));
            }
            if let Some(h) = s.history.get_mut(&c) {
                h.at_latest = false;
            }
            true
        }
    }

    /// A detached window (opened around a message) grows by one page towards the present.
    pub fn load_newer(&self, account_id: String, channel_id: String) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let listener = self.listener.clone();
        self.inner.rt.spawn(async move {
            if let Err(e) = s.load_newer(id(&channel_id)).await {
                listener.on_error(account_id, e.user_message());
            }
        });
    }

    pub fn typing_names(&self, account_id: String, channel_id: String) -> Vec<String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.with_store(&account_id, |s| {
            let c = id(&channel_id);
            let guild = s.channels.get(&c).and_then(|c| c.guild_id);
            s.typing_users(c, now)
                .into_iter()
                .map(|u| s.display_name(guild, u))
                .collect()
        })
        .unwrap_or_default()
    }

    pub fn member_list(&self, account_id: String, guild_id: String) -> Vec<MemberRow> {
        self.with_store(&account_id, |s| {
            let g = id(&guild_id);
            let Some(list) = s.member_lists.get(&g) else {
                return vec![];
            };
            let roles = s.roles.get(&g);
            list.items
                .iter()
                .filter_map(|item| {
                    if let Some(group) = &item.group {
                        let title = match group.id.as_str() {
                            "online" => "Online".to_string(),
                            "offline" => "Offline".to_string(),
                            rid => roles
                                .and_then(|r| r.get(&id(rid)))
                                .map(|r| r.name.clone())
                                .unwrap_or_default(),
                        };
                        let count = if group.count > 0 {
                            group.count
                        } else {
                            list.group_counts.get(&group.id).copied().unwrap_or(0)
                        };
                        return Some(MemberRow::Header { title, count });
                    }
                    let m = item.member.as_ref()?;
                    let uid = m.member.id()?;
                    let user = s.users.get(&uid).cloned().unwrap_or_else(|| model::User {
                        id: uid,
                        ..Default::default()
                    });
                    Some(MemberRow::Member {
                        cosmetics: crate::cosmetics::item_of(m.member.user.as_ref().unwrap_or(&user)),
                        user_id: uid.to_string(),
                        name: m
                            .member
                            .nick
                            .clone()
                            .unwrap_or_else(|| user.display_name().to_string()),
                        color: s.name_color(Some(g), uid),
                        avatar_url: user.avatar_url(64),
                        status: m
                            .presence
                            .as_ref()
                            .map(|p| p.status.clone())
                            .unwrap_or_else(|| "offline".into()),
                        is_bot: user.bot,
                        activity: presence::row_activity(&match s.activities.get(&uid) {
                            Some(l) => l.clone(),
                            None => m
                                .presence
                                .as_ref()
                                .map(|p| unison_core::activity::Activity::parse_list(&serde_json::Value::Array(p.activities.clone())))
                                .unwrap_or_default(),
                        }),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
    }

    pub fn load_older(&self, account_id: String, channel_id: String) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let listener = self.listener.clone();
        self.inner.rt.spawn(async move {
            if let Err(e) = s.load_older(id(&channel_id)).await {
                listener.on_error(account_id, e.user_message());
            }
        });
    }

    /// Start sending a message. Returns its id (`nonce`): `sends` reports its progress under that
    /// id, `cancel_send` stops it while it uploads, and a failed one stays listed until
    /// `dismiss_send` so the shell can give the text and files back.
    pub fn send_message(
        &self,
        account_id: String,
        channel_id: String,
        content: String,
        reply_to: Option<String>,
        mention_reply: bool,
        files: Vec<FileUpload>,
        gesture: UserGesture,
    ) -> String {
        let nonce = unison_core::snowflake::Snowflake::nonce_now();
        let Some(s) = self.inner.session(id(&account_id)) else {
            return nonce.to_string();
        };
        let intent = gesture.intent();
        let listener = self.listener.clone();
        let files: Vec<LocalFile> = files.into_iter().map(FileUpload::into_local).collect();
        // Listed before this returns: from then on, not being listed means it went through.
        let reply = reply_to.as_deref().map(id);
        s.begin_send(id(&channel_id), nonce, &content, reply, mention_reply, &files);
        self.inner.rt.spawn(async move {
            match s
                .run_send(intent, id(&channel_id), nonce, content, reply, mention_reply, files)
                .await
            {
                Ok(()) | Err(unison_core::Error::Cancelled) => {}
                Err(e) => listener.on_error(account_id, e.user_message()),
            }
        });
        nonce.to_string()
    }

    /// Forward to each destination (one request apiece, sent in turn and capped, all from the one
    /// gesture), with an optional comment posted after each.
    pub fn forward_message(
        &self,
        account_id: String,
        channel_id: String,
        message_id: String,
        destination_ids: Vec<String>,
        comment: String,
        gesture: UserGesture,
    ) {
        use unison_core::send_rules::MAX_FORWARD_TARGETS;
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let targets: Vec<(_, UserIntent)> = destination_ids
            .iter()
            .take(MAX_FORWARD_TARGETS)
            .map(|d| (id(d), gesture.intent()))
            .collect();
        let listener = self.listener.clone();
        self.inner.rt.spawn(async move {
            match s.forward_all(targets, id(&channel_id), id(&message_id), comment).await {
                Ok(()) | Err(unison_core::Error::Cancelled) => {}
                Err(e) => listener.on_error(account_id, e.user_message()),
            }
        });
    }

    pub fn edit_message(
        &self,
        account_id: String,
        channel_id: String,
        message_id: String,
        content: String,
        gesture: UserGesture,
    ) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.edit(i, id(&channel_id), id(&message_id), content).await
        });
    }

    pub fn delete_message(
        &self,
        account_id: String,
        channel_id: String,
        message_id: String,
        gesture: UserGesture,
    ) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.delete(i, id(&channel_id), id(&message_id)).await
        });
    }

    /// `emoji` is a unicode emoji or `name:id` for custom emoji.
    pub fn react(
        &self,
        account_id: String,
        channel_id: String,
        message_id: String,
        emoji: String,
        add: bool,
        gesture: UserGesture,
    ) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.react(i, id(&channel_id), id(&message_id), emoji, add)
                .await
        });
    }

    pub fn pin(
        &self,
        account_id: String,
        channel_id: String,
        message_id: String,
        pinned: bool,
        gesture: UserGesture,
    ) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.pin(i, id(&channel_id), id(&message_id), pinned).await
        });
    }

    /// Mark the open channel read. The shell calls this on the user's next interaction with the
    /// window after new messages arrive, never from a timer.
    pub fn ack_channel(&self, account_id: String, channel_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.ack_latest(i, id(&channel_id)).await
        });
    }

    /// Call at most every ~8 s while the user is typing (the composer throttles).
    pub fn send_typing(&self, account_id: String, channel_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.typing(i, id(&channel_id)).await
        });
    }

    pub fn fetch_pins(&self, account_id: String, channel_id: String) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let listener = self.listener.clone();
        self.inner.rt.spawn(async move {
            match s.pins(id(&channel_id)).await {
                Ok(msgs) => {
                    let items = {
                        let st = s.store.read();
                        let guild = st.channels.get(&id(&channel_id)).and_then(|c| c.guild_id);
                        msgs.iter().map(|m| message_item(&st, guild, m)).collect()
                    };
                    listener.on_pins(account_id, channel_id, items);
                }
                Err(e) => listener.on_error(account_id, e.user_message()),
            }
        });
    }

    /// Favourites and frequent reactions from what's loaded (empty until `fetch_reaction_emoji`).
    pub fn reaction_emoji(&self, account_id: String) -> ReactionEmojis {
        self.with_store(&account_id, reaction_emojis)
            .unwrap_or_default()
    }

    /// Read the frecency settings if this session hasn't yet; answers with `on_reaction_emoji`.
    pub fn fetch_reaction_emoji(&self, account_id: String) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let listener = self.listener.clone();
        self.inner.rt.spawn(async move {
            match s.load_emoji_prefs().await {
                Ok(()) => {
                    let emoji = reaction_emojis(&s.store.read());
                    listener.on_reaction_emoji(account_id, emoji);
                }
                Err(e) => listener.on_error(account_id, e.user_message()),
            }
        });
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ReactionUserItem {
    pub user_id: String,
    pub name: String,
    pub avatar_url: String,
}

/// Who reacted with one emoji: Discord's hover sentence and the people themselves.
#[derive(uniffi::Record, Clone, Debug)]
pub struct ReactionWho {
    pub summary: String,
    /// Just the people ("A, B, C and 9 others"), for a card that shows the emoji itself.
    pub names_line: String,
    pub users: Vec<ReactionUserItem>,
}

#[uniffi::export]
impl UnisonEngine {
    /// `emoji` as `react` takes it; `label` is how the chip names it (`:name:` or the emoji itself).
    pub async fn reaction_who(&self, account_id: String, channel_id: String, message_id: String, emoji: String, label: String, total: u32) -> Result<ReactionWho, FfiError> {
        let s = self.inner.session(id(&account_id)).ok_or_else(|| FfiError::Failed { message: "no such account".into() })?;
        let (c, m) = (id(&channel_id), id(&message_id));
        let s2 = s.clone();
        let jh = self.inner.rt.spawn(async move { s2.reaction_users(c, m, &emoji).await });
        let people = jh.await.map_err(|e| FfiError::Failed { message: e.to_string() })??;
        let names: Vec<String> = people.iter().map(|(_, n)| n.clone()).collect();
        let store = s.store.read();
        Ok(ReactionWho {
            summary: unison_core::reaction_info::summary(&names, total, &label),
            names_line: unison_core::reaction_info::names_line(&names, total),
            users: people
                .into_iter()
                .map(|(uid, name)| ReactionUserItem { user_id: uid.to_string(), name, avatar_url: store.users.get(&uid).map(|u| u.avatar_url(64)).unwrap_or_default() })
                .collect(),
        })
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// Manual: `UNISON_BENCH_DB=<account sqlite copy> cargo test -p unison-ffi --release bench_message_items -- --ignored --nocapture`.
    /// Times the core's share of a channel hop (cache read + MessageItem build) on real cached data.
    #[test]
    #[ignore]
    fn bench_message_items() {
        let Ok(path) = std::env::var("UNISON_BENCH_DB") else { return };
        let cache = unison_core::cache::Cache::open(std::path::Path::new(&path)).unwrap();
        let store = cache.load_store().unwrap().expect("store");
        let mut ids: Vec<Id> = store.channels.keys().copied().collect();
        ids.sort();
        let (mut n, mut total, mut worst) = (0u32, std::time::Duration::ZERO, std::time::Duration::ZERO);
        for c in ids {
            let msgs = cache.load_messages(c, 50).unwrap();
            if msgs.len() < 20 { continue }
            let guild = store.channels.get(&c).and_then(|c| c.guild_id);
            let t = std::time::Instant::now();
            let mut items: Vec<MessageItem> = msgs.iter().map(|m| message_item(&store, guild, m)).collect();
            mark_grouped(&mut items);
            let d = t.elapsed();
            total += d; worst = worst.max(d); n += 1;
        }
        eprintln!("{n} channels: mean {:?}, worst {:?} per 50-message build", total / n.max(1), worst);
    }
}

#[cfg(test)]
mod grouping_tests {
    use super::*;
    use unison_core::model::Channel;
    use unison_core::snowflake::Snowflake;

    fn msg(id: u64, thread: Option<u64>) -> Message {
        let mut m = Message { id: Snowflake(id), channel_id: Snowflake(1), content: "hi".into(), ..Default::default() };
        m.author.id = Snowflake(7);
        m.thread = thread.map(|t| Channel { id: Snowflake(t), name: Some("t".into()), ..Default::default() });
        m
    }

    fn items(ms: &[Message]) -> Vec<MessageItem> {
        let s = Store::default();
        let mut v: Vec<MessageItem> = ms.iter().map(|m| message_item(&s, None, m)).collect();
        mark_grouped(&mut v);
        v
    }

    #[test]
    fn two_threads_on_consecutive_messages_are_two_blocks() {
        let one = items(&[msg(1 << 22, Some(100)), msg(2 << 22, Some(101))]);
        assert!(!one[1].grouped, "the second message needs its own picture for its line to start from");
        // Only one of them has a thread: still one block.
        assert!(items(&[msg(1 << 22, None), msg(2 << 22, Some(101))])[1].grouped);
        assert!(items(&[msg(1 << 22, Some(100)), msg(2 << 22, None)])[1].grouped);
    }

    #[test]
    fn a_thread_never_has_an_entrance_to_itself() {
        let mut v = items(&[msg(1 << 22, Some(100)), msg(2 << 22, Some(101))]);
        hide_entrances_to(&mut v, "100");
        assert!(v[0].thread.is_none(), "reading thread 100: no door to thread 100");
        assert!(v[1].thread.is_some(), "other threads keep theirs");
    }
}

#[cfg(test)]
mod starter_tests {
    use super::*;
    use unison_core::snowflake::Snowflake;

    #[test]
    fn a_thread_starter_shows_the_original_message_in_the_thread() {
        let orig = Message { id: Snowflake(5), channel_id: Snowflake(1), content: "the first post".into(), ..Default::default() };
        let starter = Message {
            id: Snowflake(9),
            channel_id: Snowflake(2),
            kind: message_type::THREAD_STARTER_MESSAGE,
            referenced_message: Some(Box::new(orig)),
            ..Default::default()
        };
        let s = Store::default();
        let item = message_item(&s, None, &starter);
        assert_eq!((item.id.as_str(), item.channel_id.as_str()), ("9", "2"), "the row belongs to the thread");
        assert!(item.reply.is_none(), "not a reply line");
        assert!(item.supported && item.system_text.is_none());
        assert!(item.content.text.contains("the first post"));
    }

    #[test]
    fn a_starter_whose_original_is_gone_is_left_alone() {
        let starter = Message { id: Snowflake(9), kind: message_type::THREAD_STARTER_MESSAGE, ..Default::default() };
        assert!(thread_starter_as_message(&starter).is_none());
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;
    use unison_core::model::Role;
    use unison_core::snowflake::Snowflake;

    #[test]
    fn a_role_is_named_even_when_the_message_has_no_server() {
        let mut s = Store::default();
        s.roles.entry(Snowflake(100)).or_default().insert(Snowflake(5), Role { id: Snowflake(5), name: "Mods".into(), ..Default::default() });
        assert_eq!(preview_text(&s, None, "ping <@&5> now", 80), "ping @Mods now", "Inbox, search and saved messages carry no server id");
        assert_eq!(preview_text(&s, Some(Snowflake(100)), "ping <@&5>", 80), "ping @Mods");
        assert_eq!(preview_text(&s, None, "<@&9>", 80), "@deleted-role", "a role that really is gone");
    }
}

/// One message a drag-selection across the transcript touches (`unison_core::selection_copy`).
#[derive(uniffi::Record, Clone, Debug)]
pub struct SelectionPart {
    pub author: String,
    pub time_label: String,
    pub text: String,
    pub grouped: bool,
    pub system: bool,
}

/// The text a selection across messages puts on the clipboard: name lines for each block, then the lines.
#[uniffi::export]
pub fn selection_copy_text(parts: Vec<SelectionPart>) -> String {
    let parts: Vec<unison_core::selection_copy::Part> = parts
        .into_iter()
        .map(|p| unison_core::selection_copy::Part { author: p.author, time_label: p.time_label, text: p.text, grouped: p.grouped, system: p.system })
        .collect();
    unison_core::selection_copy::copy_text(&parts)
}

/// Where to scroll for a jump to a loaded message (`unison_core::jump_scroll`): `None` when the
/// message is already comfortably in view (only highlight it), otherwise the offset that centres it,
/// kept inside what the transcript can scroll to. Document points, y down; the insets are what
/// floats over the viewport's top and bottom.
#[uniffi::export]
pub fn jump_scroll_target(row_top: f64, row_bottom: f64, offset: f64, viewport: f64, inset_top: f64, inset_bottom: f64, content: f64) -> Option<f64> {
    unison_core::jump_scroll::target_offset(&unison_core::jump_scroll::Geometry { row_top, row_bottom, offset, viewport, inset_top, inset_bottom, content })
}

/// Whether `rows` new rows (`screens` viewport heights tall together) arrive with motion or swap in
/// (`unison_core::arrival`): live messages and the unread ones fetched after a channel opens.
#[uniffi::export]
pub fn arrival_animates(rows: u32, screens: f64) -> bool {
    unison_core::arrival::animates(rows, screens)
}
