use super::*;

#[derive(Debug, Clone)]
pub struct OutgoingMessage {
    pub content: String,
    pub nonce: Snowflake,
    pub guild_id: Option<Id>,
    pub reply_to: Option<Id>,
    pub mention_reply: bool,
    pub silent: bool,
    pub attachments: Vec<UploadedAttachment>,
}

/// The message being forwarded.
#[derive(Debug, Clone, Copy)]
pub struct ForwardSource {
    pub guild_id: Option<Id>,
    pub channel_id: Id,
    pub message_id: Id,
}

/// What makes an attachment a voice message: its length and the waveform the player draws.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceMeta {
    pub duration_secs: f64,
    /// Base64, one byte per sample.
    pub waveform: String,
}

#[derive(Debug, Clone)]
pub struct LocalFile {
    pub filename: String,
    pub content_type: String,
    /// Reference-counted, so the session can keep a copy of a failed send without duplicating the file.
    pub bytes: bytes::Bytes,
    /// Alt text.
    pub description: Option<String>,
    pub spoiler: bool,
    pub voice: Option<VoiceMeta>,
    /// Set when the shell changed the file before upload (converted or shrunk it): what it was.
    pub original: Option<OriginalFile>,
}

/// What a converted or shrunk file was before: the web client sends its content type with the slot request
/// and an `X-Discord-Original-MD5` header carrying the MD5 of the original bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct OriginalFile {
    pub content_type: String,
    /// 32 lowercase hex digits.
    pub md5: String,
}

/// MD5 of `bytes` as 32 lowercase hex digits (the web client's `original MD5`).
pub fn md5_hex(bytes: &[u8]) -> String {
    use md5::{Digest, Md5};
    Md5::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone)]
pub struct UploadedAttachment {
    pub filename: String,
    pub uploaded_filename: String,
    pub description: Option<String>,
    pub spoiler: bool,
    pub voice: Option<VoiceMeta>,
}

/// `POST /channels/{id}/messages` body, key for key what `MessageQueue.handleSend` posts:
/// `{mobile_network_type, content, nonce, tts, message_reference?, allowed_mentions?, flags}`.
/// A reply's reference is `getSendMessageOptionsForReply`: no `type`, and `guild_id` left out in
/// DMs (it's `undefined` there, which `JSON.stringify` drops). With the reply ping off it adds
/// `allowed_mentions: {parse: Object.values(AllowedMentionTypes), replied_user: false}`.
fn message_body(channel: Id, draft: &OutgoingMessage) -> Value {
    let mut body = json!({
        "mobile_network_type": "unknown",
        "content": draft.content,
        "nonce": draft.nonce.to_string(),
        "tts": false,
    });
    if let Some(reply) = draft.reply_to {
        let mut reference = serde_json::Map::new();
        if let Some(g) = draft.guild_id {
            reference.insert("guild_id".into(), g.to_string().into());
        }
        reference.insert("channel_id".into(), channel.to_string().into());
        reference.insert("message_id".into(), reply.to_string().into());
        body["message_reference"] = Value::Object(reference);
        if !draft.mention_reply {
            body["allowed_mentions"] =
                json!({ "parse": ["users", "roles", "everyone"], "replied_user": false });
        }
    }
    let voice = draft.attachments.iter().any(|a| a.voice.is_some());
    body["flags"] = ((if draft.silent { 1 << 12 } else { 0 }) | if voice { crate::voice_note::FLAG } else { 0 }).into();
    if !draft.attachments.is_empty() {
        body["attachments"] = Value::Array(
            draft
                .attachments
                .iter()
                .enumerate()
                .map(|(i, a)| attachment_json(i, a))
                .collect(),
        );
        body["channel_id"] = channel.to_string().into();
        body["type"] = 0.into();
        body["sticker_ids"] = json!([]);
    }
    body
}

/// One entry of the message's `attachments`, key for key what the web client builds (build
/// 655747): `id`, `description` when set, `filename`, `uploaded_filename`, `is_spoiler: true`.
pub(crate) fn attachment_json(i: usize, a: &UploadedAttachment) -> Value {
    let mut v = serde_json::Map::new();
    v.insert("id".into(), i.to_string().into());
    if let Some(d) = a.description.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        v.insert("description".into(), d.into());
    }
    v.insert("filename".into(), a.filename.clone().into());
    v.insert("uploaded_filename".into(), a.uploaded_filename.clone().into());
    if a.spoiler {
        v.insert("is_spoiler".into(), true.into());
    }
    if let Some(voice) = &a.voice {
        v.insert("duration_secs".into(), voice.duration_secs.into());
        v.insert("waveform".into(), voice.waveform.clone().into());
    }
    Value::Object(v)
}

/// One slot request, as the web client's `getUploadPayload` builds it: name, size, a counter id and `is_clip`.
/// (`original_content_type` is sent only for a file the client compressed or converted first.)
fn slot_request(file: &LocalFile) -> Value {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut v = json!({ "filename": file.filename, "file_size": file.bytes.len(), "id": id.to_string(), "is_clip": false });
    if let Some(o) = &file.original {
        v["original_content_type"] = o.content_type.clone().into();
    }
    v
}

impl Rest {
    pub async fn messages(
        &self,
        channel: Id,
        before: Option<Id>,
        limit: u8,
        preload: bool,
    ) -> Result<Vec<Message>> {
        // `preload=true` marks the read the official client makes on mouse-down (FIDELITY "Channel open").
        let path = match (before, preload) {
            (Some(b), _) => format!("/channels/{channel}/messages?before={b}&limit={limit}"),
            (None, true) => format!("/channels/{channel}/messages?limit={limit}&preload=true"),
            (None, false) => format!("/channels/{channel}/messages?limit={limit}"),
        };
        self.get(&path).await
    }

    /// The page of messages after `after` (a detached window growing towards the present).
    pub async fn messages_after(&self, channel: Id, after: Id, limit: u8) -> Result<Vec<Message>> {
        self.get(&format!("/channels/{channel}/messages?after={after}&limit={limit}")).await
    }

    pub async fn messages_around(&self, channel: Id, around: Id) -> Result<Vec<Message>> {
        self.get(&format!(
            "/channels/{channel}/messages?around={around}&limit=50"
        ))
        .await
    }

    pub async fn pins(&self, channel: Id) -> Result<Value> {
        self.get(&format!("/channels/{channel}/messages/pins?limit=50"))
            .await
    }

    pub async fn send_message(
        &self,
        intent: UserIntent,
        channel: Id,
        draft: &OutgoingMessage,
    ) -> Result<Message> {
        let body = message_body(channel, draft);
        self.write(
            intent,
            Method::POST,
            &format!("/channels/{channel}/messages"),
            Some(body),
        )
        .await
    }

    /// Forward: a message whose `message_reference` has `type: 1` and no content of its own
    /// (`capture`: key order and `flags` from the official client's forward modal, optional
    /// comment is sent as a second plain message after the forward).
    pub async fn forward_message(
        &self,
        intent: UserIntent,
        dest: Id,
        source: ForwardSource,
        nonce: Snowflake,
    ) -> Result<Message> {
        let mut reference = serde_json::Map::new();
        reference.insert("type".into(), 1.into());
        if let Some(g) = source.guild_id {
            reference.insert("guild_id".into(), g.to_string().into());
        }
        reference.insert("channel_id".into(), source.channel_id.to_string().into());
        reference.insert("message_id".into(), source.message_id.to_string().into());
        let body = json!({
            "mobile_network_type": "unknown",
            "content": "",
            "nonce": nonce.to_string(),
            "tts": false,
            "message_reference": Value::Object(reference),
            "flags": 0,
        });
        self.write(intent, Method::POST, &format!("/channels/{dest}/messages"), Some(body)).await
    }

    pub async fn edit_message(
        &self,
        intent: UserIntent,
        channel: Id,
        message: Id,
        content: &str,
    ) -> Result<Message> {
        self.write(
            intent,
            Method::PATCH,
            &format!("/channels/{channel}/messages/{message}"),
            Some(json!({ "content": content })),
        )
        .await
    }

    pub async fn delete_message(&self, intent: UserIntent, channel: Id, message: Id) -> Result<()> {
        let _: Value = self
            .write(
                intent,
                Method::DELETE,
                &format!("/channels/{channel}/messages/{message}"),
                None,
            )
            .await?;
        Ok(())
    }

    pub async fn typing(&self, intent: UserIntent, channel: Id) -> Result<()> {
        let _: Value = self
            .write(
                intent,
                Method::POST,
                &format!("/channels/{channel}/typing"),
                None,
            )
            .await?;
        Ok(())
    }

    /// Mark read. Opening a channel is the gesture; body matches `ReadState._ack`.
    pub async fn ack(&self, intent: UserIntent, channel: Id, message: Id) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let token = self.ack_token.lock().clone();
        let body = json!({ "token": token, "last_viewed": last_viewed_days(now) });
        let v: Value = self
            .write(
                intent,
                Method::POST,
                &format!("/channels/{channel}/messages/{message}/ack"),
                Some(body),
            )
            .await?;
        if let Some(t) = v["token"].as_str() {
            *self.ack_token.lock() = Some(t.to_string());
        }
        Ok(())
    }

    /// "Mark as Read" from a context menu (a manual ack, unlike the automatic one on opening a channel).
    pub async fn ack_manual(&self, intent: UserIntent, channel: Id, message: Id) -> Result<()> {
        let body = json!({ "manual": true, "mention_count": 0 });
        let v: Value = self
            .write(
                intent,
                Method::POST,
                &format!("/channels/{channel}/messages/{message}/ack"),
                Some(body),
            )
            .await?;
        if let Some(t) = v["token"].as_str() {
            *self.ack_token.lock() = Some(t.to_string());
        }
        Ok(())
    }

    pub async fn pin(
        &self,
        intent: UserIntent,
        channel: Id,
        message: Id,
        pinned: bool,
    ) -> Result<()> {
        let method = if pinned { Method::PUT } else { Method::DELETE };
        let _: Value = self
            .write(
                intent,
                method,
                &format!("/channels/{channel}/messages/pins/{message}"),
                None,
            )
            .await?;
        Ok(())
    }

    /// Two-step upload as the web client does it: every file asks for its own slot (`POST /channels/{id}/attachments`
    /// with one file in `files`) and then `PUT`s its bytes, all files at once (`Promise.all` over the uploads in the
    /// bundle). Returns refs for `send_message`. `progress(file_index, bytes)` hears about every piece sent; setting
    /// `cancel` stops the upload with [`Error::Cancelled`].
    pub async fn upload_attachments(
        &self,
        intent: UserIntent,
        channel: Id,
        files: Vec<LocalFile>,
        progress: std::sync::Arc<dyn Fn(usize, u64) + Send + Sync>,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Vec<UploadedAttachment>> {
        // One gesture, one slot request per file: each request redeems an intent of its own, all with the gesture's time.
        let mut intents = Vec::with_capacity(files.len());
        let mut rest = intent;
        for _ in 0..files.len() {
            let (one, more) = rest.split();
            intents.push(one);
            rest = more;
        }
        let puts = files.into_iter().zip(intents).enumerate().map(|(index, (file, intent))| {
            let (progress, cancel) = (progress.clone(), cancel.clone());
            async move {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                // A file the client changed first carries the MD5 of what it was (`buildHeadersForMd5`).
                let more: Vec<(&'static str, String)> = file.original.iter().map(|o| ("x-discord-original-md5", o.md5.clone())).collect();
                let v: Value = self
                    .write_with_headers(
                        intent,
                        Method::POST,
                        &format!("/channels/{channel}/attachments"),
                        Some(json!({ "files": [slot_request(&file)] })),
                        more,
                    )
                    .await?;
                let slot = v["attachments"].get(0).cloned().ok_or_else(|| Error::Other("Discord didn't accept the files. Try again.".into()))?;
                let url = slot["upload_url"].as_str().ok_or_else(|| Error::Decode("no upload_url".into()))?;
                let on_bytes: std::sync::Arc<dyn Fn(u64) + Send + Sync> = std::sync::Arc::new(move |n| progress(index, n));
                // The web client sends every file as `application/octet-stream`, whatever it is.
                let headers = self.identity.upload_put_headers(file.bytes.len());
                let status = self.transport.put_progress(url, headers, file.bytes, on_bytes, cancel).await?;
                if !(200..300).contains(&status) {
                    return Err(if status == 413 {
                        Error::Http { status, code: None, message: String::new() }
                    } else {
                        Error::Other(format!("The upload to Discord failed (HTTP {status}). Try again."))
                    });
                }
                Ok(UploadedAttachment {
                    description: file.description,
                    spoiler: file.spoiler,
                    voice: file.voice,
                    filename: file.filename,
                    uploaded_filename: slot["upload_filename"].as_str().unwrap_or_default().to_string(),
                })
            }
        });
        let out: Vec<UploadedAttachment> = futures_util::future::try_join_all(puts).await?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(guild: Option<Id>, reply: Option<Id>, mention: bool) -> OutgoingMessage {
        OutgoingMessage {
            content: "hi".into(),
            nonce: Snowflake(1300000000000000000),
            guild_id: guild,
            reply_to: reply,
            mention_reply: mention,
            silent: false,
            attachments: vec![],
        }
    }

    #[test]
    fn reply_body_matches_official_client() {
        let ch = Snowflake(20);
        // Ping on: no allowed_mentions at all.
        let on = serde_json::to_string(&message_body(
            ch,
            &draft(Some(Snowflake(10)), Some(Snowflake(30)), true),
        ))
        .unwrap();
        assert_eq!(
            on,
            r#"{"mobile_network_type":"unknown","content":"hi","nonce":"1300000000000000000","tts":false,"message_reference":{"guild_id":"10","channel_id":"20","message_id":"30"},"flags":0}"#
        );
        // Ping off: replied_user false, every parse type kept.
        let off = serde_json::to_string(&message_body(
            ch,
            &draft(Some(Snowflake(10)), Some(Snowflake(30)), false),
        ))
        .unwrap();
        assert_eq!(
            off,
            r#"{"mobile_network_type":"unknown","content":"hi","nonce":"1300000000000000000","tts":false,"message_reference":{"guild_id":"10","channel_id":"20","message_id":"30"},"allowed_mentions":{"parse":["users","roles","everyone"],"replied_user":false},"flags":0}"#
        );
        // DMs have no guild_id key, not a null one.
        let dm = message_body(ch, &draft(None, Some(Snowflake(30)), true));
        assert!(dm["message_reference"].get("guild_id").is_none());
        // Not a reply: no reference, and the ping setting is irrelevant.
        let plain = message_body(ch, &draft(None, None, false));
        assert!(
            plain.get("message_reference").is_none() && plain.get("allowed_mentions").is_none()
        );
    }

    #[test]
    fn a_changed_file_says_what_it_was() {
        let plain = LocalFile { filename: "a.jpg".into(), content_type: "image/jpeg".into(), bytes: bytes::Bytes::from_static(b"x"), description: None, spoiler: false, voice: None, original: None };
        let v = slot_request(&plain);
        assert!(v.get("original_content_type").is_none());
        assert_eq!(v["is_clip"], false);
        assert_eq!(v["file_size"], 1);
        let changed = LocalFile { original: Some(OriginalFile { content_type: "image/heic".into(), md5: md5_hex(b"x") }), ..plain };
        assert_eq!(slot_request(&changed)["original_content_type"], "image/heic");
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn attachment_entry_matches_web_client() {
        let with = UploadedAttachment { filename: "a.png".into(), uploaded_filename: "u/a.png".into(), description: Some("  a cat ".into()), spoiler: true, voice: None };
        assert_eq!(
            serde_json::to_string(&attachment_json(0, &with)).unwrap(),
            r#"{"id":"0","description":"a cat","filename":"a.png","uploaded_filename":"u/a.png","is_spoiler":true}"#
        );
        let plain = UploadedAttachment { description: Some("  ".into()), spoiler: false, ..with };
        assert_eq!(serde_json::to_string(&attachment_json(1, &plain)).unwrap(), r#"{"id":"1","filename":"a.png","uploaded_filename":"u/a.png"}"#);
    }
}
