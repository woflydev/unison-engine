//! What the line above a reply (and the composer's "Replying to" bar) says about the original
//! message. A message with no text still has something to point at: its picture, sticker or
//! attachment, so the line says so instead of staying empty.

use crate::model::Message;

/// The kind of thing a text-less original holds; the shell picks the glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyMedia {
    None,
    Image,
    Video,
    File,
    Sticker,
    Embed,
}

const SNIPPET_CHARS: usize = 120;

/// The text for a reply preview and the media it falls back to when the text is empty.
pub fn summary(m: &Message) -> (String, ReplyMedia) {
    summary_of(m, &m.content)
}

/// Like `summary`, for text the caller already resolved (mentions turned into names): a shell must
/// never show a raw `<@123>` or `<@&123>`.
pub fn summary_of(m: &Message, content: &str) -> (String, ReplyMedia) {
    let text: String = content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(SNIPPET_CHARS)
        .collect();
    let media = media_of(m);
    if !text.is_empty() {
        return (text, media);
    }
    let note = match media {
        ReplyMedia::Image => "Click to see image",
        ReplyMedia::Video => "Click to see video",
        ReplyMedia::File => "Click to see attachment",
        ReplyMedia::Sticker => "Click to see sticker",
        ReplyMedia::Embed => "Click to see embed",
        ReplyMedia::None => "",
    };
    (note.into(), media)
}

fn media_of(m: &Message) -> ReplyMedia {
    if let Some(a) = m.attachments.first() {
        let ct = a.content_type.as_deref().unwrap_or("");
        return if ct.starts_with("image/") {
            ReplyMedia::Image
        } else if ct.starts_with("video/") {
            ReplyMedia::Video
        } else {
            ReplyMedia::File
        };
    }
    if !m.sticker_items.is_empty() {
        return ReplyMedia::Sticker;
    }
    if !m.embeds.is_empty() {
        return ReplyMedia::Embed;
    }
    ReplyMedia::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Attachment, Embed};

    fn msg(content: &str) -> Message {
        Message { content: content.into(), ..Default::default() }
    }

    fn attach(ct: &str) -> Attachment {
        Attachment { content_type: Some(ct.into()), ..Default::default() }
    }

    #[test]
    fn text_wins_and_keeps_the_media_kind() {
        let mut m = msg("look at\nthis");
        m.attachments.push(attach("image/png"));
        assert_eq!(summary(&m), ("look at this".into(), ReplyMedia::Image));
    }

    #[test]
    fn image_only_says_so() {
        let mut m = msg("  ");
        m.attachments.push(attach("image/webp"));
        assert_eq!(summary(&m), ("Click to see image".into(), ReplyMedia::Image));
    }

    #[test]
    fn other_attachments_and_embeds() {
        let mut v = msg("");
        v.attachments.push(attach("video/mp4"));
        assert_eq!(summary(&v).1, ReplyMedia::Video);
        let mut f = msg("");
        f.attachments.push(attach("application/zip"));
        assert_eq!(summary(&f), ("Click to see attachment".into(), ReplyMedia::File));
        let mut e = msg("");
        e.embeds.push(Embed::default());
        assert_eq!(summary(&e), ("Click to see embed".into(), ReplyMedia::Embed));
    }

    #[test]
    fn nothing_to_show_stays_empty() {
        assert_eq!(summary(&msg("")), (String::new(), ReplyMedia::None));
    }
}
