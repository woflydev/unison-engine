//! Discord-flavoured markdown.
//!
//! [`parse`] builds a small AST; [`render`] flattens it into display text plus
//! styled spans with UTF-16 offsets, ready to become an `NSAttributedString`
//! without any further parsing on the UI side.

mod inline;

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Text(String),
    Bold(Vec<Node>),
    Italic(Vec<Node>),
    Underline(Vec<Node>),
    Strike(Vec<Node>),
    Spoiler(Vec<Node>),
    InlineCode(String),
    CodeBlock { lang: Option<String>, code: String },
    Quote(Vec<Node>),
    Heading { level: u8, children: Vec<Node> },
    Subtext(Vec<Node>),
    ListItem { marker: ListMarker, depth: u8, children: Vec<Node> },
    Link { url: String, children: Vec<Node> },
    AutoLink(String),
    UserMention(String),
    RoleMention(String),
    ChannelMention(String),
    Everyone,
    Here,
    Emoji { name: String, id: String, animated: bool },
    Timestamp { unix: i64, style: char },
    SlashCommand { name: String, id: String },
    /// `<id:customize>` and friends: Discord's built-in navigation links.
    GuildNav(String),
    LineBreak,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ListMarker {
    Bullet,
    Ordered(u32),
}

/// Parse a full message body.
pub fn parse(src: &str) -> Vec<Node> {
    let mut out = Vec::new();
    let mut rest = src;
    let mut paragraph = String::new();
    let mut quote_all = false;

    while !rest.is_empty() {
        // Fenced code blocks may span lines and take priority over everything.
        if let Some(after) = rest.strip_prefix("```") {
            if let Some(end) = after.find("```") {
                flush(&mut out, &mut paragraph, quote_all);
                let body = &after[..end];
                // Discord: /^```(?:([a-z0-9_+\-.#]+?)\n)?\n*([^\n][^]*?)\n*```/i
                let (lang, code) = match body.split_once('\n') {
                    Some((l, c)) if !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b"_+-.#".contains(&b)) => (Some(l.to_string()), c),
                    _ => (None, body),
                };
                let code = code.trim_start_matches('\n').trim_end_matches('\n').to_string();
                out.push(wrap_quote(Node::CodeBlock { lang, code }, quote_all));
                rest = &after[end + 3..];
                rest = rest.strip_prefix('\n').unwrap_or(rest);
                continue;
            }
        }
        let (line, next) = match rest.find('\n') {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, ""),
        };
        let has_newline = next.len() < rest.len() - line.len();

        // Within a line, a fence opening mid-line ends the current paragraph text there.
        if let Some(i) = line.find("```").filter(|&i| i > 0 && rest[i + 3..].contains("```")) {
            paragraph.push_str(&line[..i]);
            rest = &rest[i..];
            continue;
        }

        if let Some(body) = line.strip_prefix(">>> ") {
            flush(&mut out, &mut paragraph, quote_all);
            quote_all = true;
            paragraph.push_str(body);
            if has_newline {
                paragraph.push('\n');
            }
        } else if let Some(node) = block_line(line, quote_all) {
            flush(&mut out, &mut paragraph, quote_all);
            out.push(node);
        } else {
            paragraph.push_str(line);
            if has_newline {
                paragraph.push('\n');
            }
        }
        rest = next;
    }
    flush(&mut out, &mut paragraph, quote_all);
    out
}

fn wrap_quote(node: Node, quote: bool) -> Node {
    if quote { Node::Quote(vec![node]) } else { node }
}

fn flush(out: &mut Vec<Node>, paragraph: &mut String, quote_all: bool) {
    if paragraph.is_empty() {
        return;
    }
    let nodes = inline::parse(paragraph);
    paragraph.clear();
    if quote_all {
        out.push(Node::Quote(nodes));
    } else {
        out.extend(nodes);
    }
}

fn block_line(line: &str, in_quote: bool) -> Option<Node> {
    if !in_quote {
        if let Some(body) = line.strip_prefix("> ") {
            let mut children = inline::parse(body);
            children.push(Node::LineBreak);
            return Some(Node::Quote(children));
        }
    }
    // Discord tolerates a little indentation before a heading marker.
    let unindented = line.strip_prefix("   ").or_else(|| line.strip_prefix("  ")).or_else(|| line.strip_prefix(' ')).unwrap_or(line);
    for (prefix, level) in [("### ", 3u8), ("## ", 2), ("# ", 1)] {
        if let Some(body) = unindented.strip_prefix(prefix) {
            return Some(Node::Heading { level, children: line_children(body) });
        }
    }
    if let Some(body) = line.strip_prefix("-# ") {
        return Some(Node::Subtext(line_children(body)));
    }
    let indent = line.len() - line.trim_start_matches(' ').len();
    let trimmed = &line[indent..];
    let depth = (indent / 2).min(255) as u8;
    if let Some(body) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
        return Some(Node::ListItem { marker: ListMarker::Bullet, depth, children: line_children(body) });
    }
    if let Some(dot) = trimmed.find(". ") {
        if dot > 0 && dot <= 9 && trimmed[..dot].bytes().all(|b| b.is_ascii_digit()) {
            let n = trimmed[..dot].parse().unwrap_or(1);
            return Some(Node::ListItem {
                marker: ListMarker::Ordered(n),
                depth,
                children: line_children(&trimmed[dot + 2..]),
            });
        }
    }
    None
}

fn line_children(body: &str) -> Vec<Node> {
    let mut c = inline::parse(body);
    c.push(Node::LineBreak);
    c
}

/// Style attached to a run of rendered text. Offsets are UTF-16 code units.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub start: u32,
    pub len: u32,
    pub kind: SpanKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpanKind {
    Bold,
    Italic,
    Underline,
    Strike,
    Spoiler,
    Code,
    CodeBlock { lang: Option<String> },
    /// A heading line inside an `md`/`markdown` code block (highlight.js `hljs-section`).
    CodeSection,
    /// A preprocessor line in a C-family code block (highlight.js `hljs-meta`).
    CodeMeta,
    Quote,
    Heading(u8),
    Subtext,
    ListItem { depth: u8 },
    Link(String),
    UserMention(String),
    RoleMention(String),
    ChannelMention(String),
    Everyone,
    Emoji { id: String, animated: bool },
    Timestamp { unix: i64, style: char },
    SlashCommand { id: String },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Rendered {
    pub text: String,
    pub spans: Vec<Span>,
    /// True if the message consists only of 1–30 custom/unicode emoji (rendered jumbo).
    pub jumbo_emoji: bool,
}

/// Supplies display names for mentions while rendering.
pub trait Resolve {
    fn user(&self, id: &str) -> Option<String>;
    fn role(&self, id: &str) -> Option<String>;
    fn channel(&self, id: &str) -> Option<String>;
    /// The emoji for a `:name:` shortcode (`:bulb:` becomes the light bulb), as Discord's renderer does.
    fn shortcode(&self, _name: &str) -> Option<String> {
        None
    }
}

struct NoResolve;
impl Resolve for NoResolve {
    fn user(&self, _: &str) -> Option<String> {
        None
    }
    fn role(&self, _: &str) -> Option<String> {
        None
    }
    fn channel(&self, _: &str) -> Option<String> {
        None
    }
}

/// Parse and flatten in one step, with mentions shown as raw IDs.
pub fn render(src: &str) -> Rendered {
    render_with(src, &NoResolve)
}

/// Parse and flatten, resolving mention names through `resolve`.
pub fn render_with(src: &str, resolve: &dyn Resolve) -> Rendered {
    let nodes = parse(src);
    let mut r = Renderer { resolve: Some(resolve), ..Default::default() };
    r.nodes(&nodes);
    let text = r.text.trim_end_matches('\n').to_string();
    let len16 = text.encode_utf16().count() as u32;
    let mut spans = r.spans;
    for s in &mut spans {
        if s.start + s.len > len16 {
            s.len = len16.saturating_sub(s.start);
        }
    }
    spans.retain(|s| s.len > 0);
    let jumbo_emoji = is_jumbo(&nodes);
    Rendered { text, spans, jumbo_emoji }
}

impl Rendered {
    /// The text with every spoiler run replaced by a block of `\u{2592}`, one per character (at
    /// most `SPOILER_MASK_MAX`), so a one-line preview (reply line, composer reply bar, Inbox,
    /// Saved, search, notifications) never shows what the author hid. The full transcript keeps the
    /// text and draws the cover itself; this is for surfaces that show plain text.
    pub fn masked_text(&self) -> String {
        let mut runs: Vec<(usize, usize)> = self.spans.iter().filter(|s| s.kind == SpanKind::Spoiler).map(|s| (s.start as usize, (s.start + s.len) as usize)).collect();
        if runs.is_empty() {
            return self.text.clone();
        }
        runs.sort_unstable();
        let mut out = String::with_capacity(self.text.len());
        let mut pos16 = 0usize;
        let mut masked: Vec<(usize, usize)> = Vec::new();
        for ch in self.text.chars() {
            let a = pos16;
            pos16 += ch.len_utf16();
            match runs.iter().find(|(s, e)| a >= *s && a < *e) {
                Some(&(s, e)) => {
                    if !masked.iter().any(|&(ms, me)| s >= ms && e <= me) {
                        out.extend(std::iter::repeat('\u{2592}').take((e - s).clamp(1, SPOILER_MASK_MAX)));
                        masked.push((s, e));
                    }
                }
                None => out.push(ch),
            }
        }
        out
    }
}

/// Longest block drawn for one hidden run in a plain-text preview.
pub const SPOILER_MASK_MAX: usize = 12;

fn is_jumbo(nodes: &[Node]) -> bool {
    let mut count = 0;
    for n in nodes {
        match n {
            Node::Emoji { .. } => count += 1,
            Node::Text(t) if t.trim().is_empty() => {}
            Node::Text(t) => {
                for ch in t.chars().filter(|c| !c.is_whitespace()) {
                    if !is_emoji_char(ch) {
                        return false;
                    }
                    // Joiners and variation selectors belong to the emoji before them.
                    if !matches!(ch as u32, 0xFE0F | 0x200D | 0x20E3 | 0xE0020..=0xE007F) {
                        count += 1;
                    }
                }
            }
            _ => return false,
        }
    }
    (1..=30).contains(&count)
}

fn is_emoji_char(c: char) -> bool {
    matches!(c as u32,
        0x1F300..=0x1FAFF | 0x2600..=0x27BF | 0x1F000..=0x1F2FF | 0xFE0F | 0x200D | 0x20E3 | 0xE0020..=0xE007F)
}

#[derive(Default)]
struct Renderer<'r> {
    text: String,
    pos16: u32,
    spans: Vec<Span>,
    resolve: Option<&'r dyn Resolve>,
}

impl Renderer<'_> {
    fn push(&mut self, s: &str) {
        self.text.push_str(s);
        self.pos16 += s.encode_utf16().count() as u32;
    }

    fn styled(&mut self, kind: SpanKind, children: &[Node]) {
        let start = self.pos16;
        self.nodes(children);
        self.spans.push(Span { start, len: self.pos16 - start, kind });
    }

    fn leaf(&mut self, kind: SpanKind, text: &str) {
        let start = self.pos16;
        self.push(text);
        self.spans.push(Span { start, len: self.pos16 - start, kind });
    }

    /// One span of `kind` per line of `code` (starting at `start`) that `matches`.
    fn code_lines(&mut self, start: u32, code: &str, kind: SpanKind, matches: impl Fn(&str) -> bool) {
        let mut at = start;
        for line in code.split('\n') {
            let len = line.encode_utf16().count() as u32;
            if matches(line) {
                self.spans.push(Span { start: at, len, kind: kind.clone() });
            }
            at += len + 1;
        }
    }

    fn block_break(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.push("\n");
        }
    }

    fn nodes(&mut self, nodes: &[Node]) {
        for n in nodes {
            self.node(n);
        }
    }

    /// Plain text, with known `:shortcode:` runs replaced by their emoji.
    fn text(&mut self, t: &str) {
        let Some(r) = self.resolve else { return self.push(t) };
        let mut rest = t;
        while let Some(open) = rest.find(':') {
            let after = &rest[open + 1..];
            let Some(close) = after.find(':') else { break };
            let name = &after[..close];
            let valid = !name.is_empty() && name.len() <= 32 && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-'));
            match valid.then(|| r.shortcode(name)).flatten() {
                Some(e) => {
                    self.push(&rest[..open]);
                    self.push(&e);
                    rest = &after[close + 1..];
                }
                None => {
                    // Not a shortcode: keep the colon and look again from the next one.
                    self.push(&rest[..open + 1]);
                    rest = after;
                }
            }
        }
        self.push(rest);
    }

    fn node(&mut self, n: &Node) {
        match n {
            Node::Text(t) => self.text(t),
            Node::LineBreak => self.push("\n"),
            Node::Bold(c) => self.styled(SpanKind::Bold, c),
            Node::Italic(c) => self.styled(SpanKind::Italic, c),
            Node::Underline(c) => self.styled(SpanKind::Underline, c),
            Node::Strike(c) => self.styled(SpanKind::Strike, c),
            Node::Spoiler(c) => self.styled(SpanKind::Spoiler, c),
            Node::InlineCode(code) => self.leaf(SpanKind::Code, code),
            Node::CodeBlock { lang, code } => {
                self.block_break();
                let start = self.pos16;
                self.leaf(SpanKind::CodeBlock { lang: lang.clone() }, code);
                // The few highlight.js rules people use for coloured text in bots' embeds.
                match lang.as_deref().map(str::to_ascii_lowercase).as_deref() {
                    Some("md" | "markdown") => self.code_lines(start, code, SpanKind::CodeSection, |l| {
                        let hashes = l.bytes().take_while(|&b| b == b'#').count();
                        (1..=6).contains(&hashes) && l[hashes..].chars().next().is_none_or(char::is_whitespace)
                    }),
                    Some("c" | "h" | "cpp" | "cc" | "c++" | "hpp" | "cs" | "csharp" | "glsl" | "objc" | "objectivec") => {
                        self.code_lines(start, code, SpanKind::CodeMeta, |l| l.trim_start().starts_with('#'))
                    }
                    _ => {}
                }
                self.push("\n");
            }
            Node::Quote(c) => {
                self.block_break();
                self.styled(SpanKind::Quote, c);
                self.block_break();
            }
            Node::Heading { level, children } => {
                self.block_break();
                self.styled(SpanKind::Heading(*level), children);
            }
            Node::Subtext(c) => {
                self.block_break();
                self.styled(SpanKind::Subtext, c);
            }
            Node::ListItem { marker, depth, children } => {
                self.block_break();
                let start = self.pos16;
                match marker {
                    ListMarker::Bullet => self.push("• "),
                    ListMarker::Ordered(n) => self.push(&format!("{n}. ")),
                }
                self.nodes(children);
                self.spans.push(Span { start, len: self.pos16 - start, kind: SpanKind::ListItem { depth: *depth } });
            }
            Node::Link { url, children } => self.styled(SpanKind::Link(url.clone()), children),
            Node::AutoLink(url) => match channel_link(url).and_then(|(c, m)| Some((self.resolve?.channel(&c)?, c, m))) {
                // A link to a channel or message in a server reads as a chip, like the official client.
                Some((name, c, m)) => match m {
                    Some(m) => self.leaf(SpanKind::ChannelMention(format!("{c}/{m}")), &format!("#{name} \u{203A} \u{1F4AC}")),
                    None => self.leaf(SpanKind::ChannelMention(c), &format!("#{name}")),
                },
                None => self.leaf(SpanKind::Link(url.clone()), url),
            },
            Node::UserMention(id) => {
                let name = self.resolve.and_then(|r| r.user(id)).unwrap_or_else(|| id.clone());
                self.leaf(SpanKind::UserMention(id.clone()), &format!("@{name}"))
            }
            Node::RoleMention(id) => {
                let name = self.resolve.and_then(|r| r.role(id)).unwrap_or_else(|| "deleted-role".into());
                self.leaf(SpanKind::RoleMention(id.clone()), &format!("@{name}"))
            }
            Node::ChannelMention(id) => {
                let name = self.resolve.and_then(|r| r.channel(id)).unwrap_or_else(|| "unknown".into());
                self.leaf(SpanKind::ChannelMention(id.clone()), &format!("#{name}"))
            }
            Node::Everyone => self.leaf(SpanKind::Everyone, "@everyone"),
            Node::Here => self.leaf(SpanKind::Everyone, "@here"),
            // U+FFFC is the attachment placeholder the UI replaces with the emoji image.
            Node::Emoji { id, animated, .. } => {
                self.leaf(SpanKind::Emoji { id: id.clone(), animated: *animated }, "\u{FFFC}")
            }
            Node::Timestamp { unix, style } => {
                self.leaf(SpanKind::Timestamp { unix: *unix, style: *style }, &format!("<t:{unix}>"))
            }
            Node::SlashCommand { name, id } => self.leaf(SpanKind::SlashCommand { id: id.clone() }, &format!("/{name}")),
            // Drawn like a mention chip (the shell treats an empty command id as plain chip styling).
            Node::GuildNav(which) => {
                let label = match which.as_str() {
                    "customize" => "Channels & Roles",
                    "browse" => "Browse Channels",
                    "guide" => "Server Guide",
                    _ => "Home",
                };
                self.leaf(SpanKind::SlashCommand { id: String::new() }, label)
            }
        }
    }
}

/// `(channel, message)` of a `discord.com/channels/<guild>/<channel>[/<message>]` link.
fn channel_link(url: &str) -> Option<(String, Option<String>)> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    if !matches!(host, "discord.com" | "ptb.discord.com" | "canary.discord.com" | "discordapp.com") {
        return None;
    }
    let mut parts = path.split(['?', '#']).next()?.strip_prefix("channels/")?.split('/');
    let _guild = parts.next().filter(|g| *g == "@me" || g.bytes().all(|b| b.is_ascii_digit()))?;
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    let channel = parts.next().filter(|c| digits(c))?.to_string();
    let message = parts.next().filter(|m| digits(m)).map(String::from);
    Some((channel, message))
}

pub mod source;
pub use source::{SourceKind, SourceSpan, style_source};

#[cfg(test)]
mod tests;
