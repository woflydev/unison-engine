use crate::Node;

/// Inline rules, tried in Discord's precedence order at each position.
pub(crate) fn parse(src: &str) -> Vec<Node> {
    let mut p = Parser { out: Vec::new(), text: String::new() };
    p.run(src);
    p.finish()
}

struct Parser {
    out: Vec<Node>,
    text: String,
}

impl Parser {
    fn finish(mut self) -> Vec<Node> {
        self.flush();
        self.out
    }

    fn flush(&mut self) {
        if !self.text.is_empty() {
            self.out.push(Node::Text(std::mem::take(&mut self.text)));
        }
    }

    fn emit(&mut self, n: Node) {
        self.flush();
        self.out.push(n);
    }

    fn run(&mut self, src: &str) {
        let mut i = 0;
        let bytes = src.as_bytes();
        while i < src.len() {
            let rest = &src[i..];
            if let Some((node, used)) = rule(rest, i == 0 || !is_word(bytes[i - 1])) {
                self.emit(node);
                i += used;
                continue;
            }
            // Escapes: a backslash before punctuation yields the literal character.
            if bytes[i] == b'\\' {
                if let Some(c) = rest[1..].chars().next().filter(|c| c.is_ascii_punctuation()) {
                    self.text.push(c);
                    i += 1 + c.len_utf8();
                    continue;
                }
            }
            let c = rest.chars().next().unwrap();
            self.text.push(c);
            i += c.len_utf8();
        }
    }
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

fn rule(s: &str, word_start: bool) -> Option<(Node, usize)> {
    let b = s.as_bytes();
    match b[0] {
        b'`' => code(s),
        b'|' if s.starts_with("||") => delimited(s, "||", Node::Spoiler),
        b'*' if s.starts_with("***") => delimited(s, "***", |c| Node::Bold(vec![Node::Italic(c)])),
        b'*' if s.starts_with("**") => delimited(s, "**", Node::Bold),
        b'*' if b.get(1).is_some_and(|c| !c.is_ascii_whitespace()) => delimited(s, "*", Node::Italic),
        b'_' if s.starts_with("__") => delimited(s, "__", Node::Underline),
        b'_' if word_start => {
            // `_x_` italics must not end inside a word (snake_case stays literal).
            let (node, used) = delimited(s, "_", Node::Italic)?;
            if s.as_bytes().get(used).is_some_and(|&c| is_word(c)) { None } else { Some((node, used)) }
        }
        b'~' if s.starts_with("~~") => delimited(s, "~~", Node::Strike),
        b'[' => masked_link(s),
        b'<' => angle(s),
        b'@' if s.starts_with("@everyone") => Some((Node::Everyone, 9)),
        b'@' if s.starts_with("@here") => Some((Node::Here, 5)),
        b'h' if word_start && (s.starts_with("https://") || s.starts_with("http://")) => autolink(s),
        _ => None,
    }
}

fn delimited(s: &str, delim: &str, make: impl Fn(Vec<Node>) -> Node) -> Option<(Node, usize)> {
    let inner_start = delim.len();
    let body = &s[inner_start..];
    // Find the closing delimiter, skipping escaped characters and code spans.
    let mut i = 0;
    let bb = body.as_bytes();
    while i < body.len() {
        if bb[i] == b'\\' {
            i += 2;
            continue;
        }
        if bb[i] == b'`' {
            if let Some((_, used)) = code(&body[i..]) {
                i += used;
                continue;
            }
        }
        if body[i..].starts_with(delim) && i > 0 {
            // For single `*`, do not close on `**`.
            if delim == "*" && body[i..].starts_with("**") {
                i += 2;
                continue;
            }
            let inner = &body[..i];
            if delim.len() == 1 && inner.ends_with(char::is_whitespace) {
                return None;
            }
            return Some((make(parse(inner)), inner_start + i + delim.len()));
        }
        i += body[i..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

fn code(s: &str) -> Option<(Node, usize)> {
    if s.starts_with("```") {
        return None; // block-level, handled by the block parser
    }
    let ticks = if s.starts_with("``") { 2 } else { 1 };
    let body = &s[ticks..];
    let end = body.find(&s[..ticks])?;
    if end == 0 {
        return None;
    }
    let inner = &body[..end];
    let inner = if ticks == 2 { inner.trim() } else { inner };
    Some((Node::InlineCode(inner.to_string()), ticks + end + ticks))
}

fn masked_link(s: &str) -> Option<(Node, usize)> {
    let close = s.find("](")?;
    let label = &s[1..close];
    if label.is_empty() || label.contains('\n') {
        return None;
    }
    let after = &s[close + 2..];
    let end = after.find(')')?;
    let url = after[..end].trim().trim_start_matches('<').trim_end_matches('>');
    if !(url.starts_with("https://") || url.starts_with("http://")) || url.contains(' ') {
        return None;
    }
    Some((Node::Link { url: url.to_string(), children: parse(label) }, close + 2 + end + 1))
}

fn autolink(s: &str) -> Option<(Node, usize)> {
    let mut end = s.find(|c: char| c.is_whitespace() || c == '<').unwrap_or(s.len());
    // Trailing punctuation is not part of the URL; balance parentheses like Discord.
    while end > 0 {
        let last = s[..end].chars().last().unwrap();
        let unbalanced = last == ')' && s[..end].matches('(').count() < s[..end].matches(')').count();
        if matches!(last, '.' | ',' | ':' | ';' | '"' | '\'' | '!' | '?' | ']') || unbalanced {
            end -= last.len_utf8();
        } else {
            break;
        }
    }
    let url = &s[..end];
    if url.len() <= "https://".len() {
        return None;
    }
    Some((Node::AutoLink(url.to_string()), end))
}

fn angle(s: &str) -> Option<(Node, usize)> {
    let end = s.find('>')?;
    let inner = &s[1..end];
    let used = end + 1;
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());

    // `<https://…>` suppresses the embed but is still a link; the brackets aren't shown.
    if (inner.starts_with("https://") || inner.starts_with("http://")) && !inner.contains(char::is_whitespace) {
        return Some((Node::AutoLink(inner.to_string()), used));
    }
    if let Some(id) = inner.strip_prefix("@&") {
        return digits(id).then(|| (Node::RoleMention(id.into()), used));
    }
    if let Some(id) = inner.strip_prefix("@!").or_else(|| inner.strip_prefix('@')) {
        return digits(id).then(|| (Node::UserMention(id.into()), used));
    }
    if let Some(id) = inner.strip_prefix('#') {
        return digits(id).then(|| (Node::ChannelMention(id.into()), used));
    }
    if let Some(which) = inner.strip_prefix("id:") {
        return matches!(which, "home" | "browse" | "customize" | "guide").then(|| (Node::GuildNav(which.into()), used));
    }
    if let Some(rest) = inner.strip_prefix("t:") {
        let (ts, style) = match rest.split_once(':') {
            Some((t, st)) if st.len() == 1 => (t, st.chars().next().unwrap()),
            None => (rest, 'f'),
            _ => return None,
        };
        let unix: i64 = ts.parse().ok()?;
        return Some((Node::Timestamp { unix, style }, used));
    }
    if let Some(rest) = inner.strip_prefix('/') {
        let (name, id) = rest.rsplit_once(':')?;
        return digits(id).then(|| (Node::SlashCommand { name: name.into(), id: id.into() }, used));
    }
    let (animated, rest) = match inner.strip_prefix("a:") {
        Some(r) => (true, r),
        None => (false, inner.strip_prefix(':')?),
    };
    let (name, id) = rest.split_once(':')?;
    if name.is_empty() || !digits(id) {
        return None;
    }
    Some((Node::Emoji { name: name.into(), id: id.into(), animated }, used))
}
