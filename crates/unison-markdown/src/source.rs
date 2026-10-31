//! Styling of the *source* text, for the composer: which ranges of what the user typed are bold,
//! italic, code and so on, and which are the markers themselves (`**`, `` ` ``, `# `). Offsets are
//! UTF-16 units of the input, so a shell can apply them to its text view directly. Unlike
//! `render`, nothing is removed or replaced.

#[derive(Debug, Clone, PartialEq)]
pub enum SourceKind {
    /// Formatting characters: shown dimmed, never hidden (the text stays exactly what is sent).
    Marker,
    Bold,
    Italic,
    Underline,
    Strike,
    Spoiler,
    Code,
    CodeBlock,
    Quote,
    Heading(u8),
    Subtext,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceSpan {
    pub start: u32,
    pub len: u32,
    pub kind: SourceKind,
}

pub fn style_source(src: &str) -> Vec<SourceSpan> {
    // UTF-16 offset of every byte position.
    let mut off = vec![0u32; src.len() + 1];
    let mut n = 0u32;
    for (i, ch) in src.char_indices() {
        for k in 0..ch.len_utf8() {
            off[i + k] = n;
        }
        n += ch.len_utf16() as u32;
    }
    off[src.len()] = n;
    let mut out = Vec::new();
    let mut push = |a: usize, b: usize, kind: SourceKind, out: &mut Vec<SourceSpan>| {
        if b > a {
            out.push(SourceSpan { start: off[a], len: off[b] - off[a], kind });
        }
    };

    let mut pos = 0;
    let mut in_fence = false;
    for line in src.split_inclusive('\n') {
        let start = pos;
        pos += line.len();
        let text = line.trim_end_matches('\n');
        let end = start + text.len();
        if text.trim_start().starts_with("```") {
            in_fence = !in_fence || text.trim_start()[3..].contains("```");
            let fence_open = in_fence;
            push(start, end, SourceKind::CodeBlock, &mut out);
            push(start, start + text.len().min(text.find("```").unwrap_or(0) + 3), SourceKind::Marker, &mut out);
            if !fence_open {
                in_fence = false;
            }
            continue;
        }
        if in_fence {
            push(start, end, SourceKind::CodeBlock, &mut out);
            continue;
        }
        let mut body = start;
        for (p, kind) in [("### ", SourceKind::Heading(3)), ("## ", SourceKind::Heading(2)), ("# ", SourceKind::Heading(1)), ("-# ", SourceKind::Subtext)] {
            if text.starts_with(p) {
                push(start, start + p.len(), SourceKind::Marker, &mut out);
                push(start + p.len(), end, kind, &mut out);
                body = start + p.len();
                break;
            }
        }
        if body == start {
            if text.starts_with("> ") || text.starts_with(">>> ") {
                let m = if text.starts_with(">>> ") { 4 } else { 2 };
                push(start, start + m, SourceKind::Marker, &mut out);
                push(start + m, end, SourceKind::Quote, &mut out);
                body = start + m;
            } else {
                let indent = text.len() - text.trim_start_matches(' ').len();
                let t = &text[indent..];
                if t.starts_with("- ") || t.starts_with("* ") {
                    push(start + indent, start + indent + 2, SourceKind::Marker, &mut out);
                    body = start + indent + 2;
                }
            }
        }
        inline(src, body, end, &mut out, &mut push);
    }
    out
}

fn inline(src: &str, from: usize, to: usize, out: &mut Vec<SourceSpan>, push: &mut impl FnMut(usize, usize, SourceKind, &mut Vec<SourceSpan>)) {
    let b = src.as_bytes();
    let mut i = from;
    while i < to {
        match b[i] {
            b'\\' => i += 2,
            b'`' => {
                let ticks = b[i..to].iter().take_while(|c| **c == b'`').count();
                let delim = &src[i..i + ticks];
                match src[i + ticks..to].find(delim) {
                    Some(rel) if rel > 0 => {
                        let close = i + ticks + rel;
                        push(i, i + ticks, SourceKind::Marker, out);
                        push(i + ticks, close, SourceKind::Code, out);
                        push(close, close + ticks, SourceKind::Marker, out);
                        i = close + ticks;
                    }
                    _ => i += ticks,
                }
            }
            c @ (b'*' | b'_' | b'~' | b'|') => {
                let run = b[i..to].iter().take_while(|x| **x == c).count();
                let (len, kind) = match (c, run) {
                    (b'*', 3..) => (3, None),
                    (b'*', 2) => (2, Some(SourceKind::Bold)),
                    (b'_', 2..) => (2, Some(SourceKind::Underline)),
                    (b'~', 2..) => (2, Some(SourceKind::Strike)),
                    (b'|', 2..) => (2, Some(SourceKind::Spoiler)),
                    (b'*' | b'_', _) => (1, Some(SourceKind::Italic)),
                    _ => {
                        i += run;
                        continue;
                    }
                };
                let delim = &src[i..i + len];
                let inner_start = i + len;
                // The closer: the same delimiter later in the line, not directly after whitespace,
                // and for a single `*`/`_` not part of a longer run.
                let mut search = inner_start;
                let mut found = None;
                while let Some(rel) = src[search..to].find(delim) {
                    let at = search + rel;
                    let after_ws = at > inner_start && b[at - 1].is_ascii_whitespace();
                    let longer = len == 1 && (at + 1 < to && b[at + 1] == c);
                    if at > inner_start && !after_ws && !longer {
                        found = Some(at);
                        break;
                    }
                    search = at + len.max(1);
                }
                match (found, kind) {
                    (Some(close), Some(kind)) => {
                        push(i, inner_start, SourceKind::Marker, out);
                        push(inner_start, close, kind, out);
                        inline(src, inner_start, close, out, push);
                        push(close, close + len, SourceKind::Marker, out);
                        i = close + len;
                    }
                    _ => i += len.max(1),
                }
            }
            _ => i += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> Vec<(String, SourceKind)> {
        let u: Vec<u16> = s.encode_utf16().collect();
        style_source(s).into_iter().map(|x| (String::from_utf16(&u[x.start as usize..(x.start + x.len) as usize]).unwrap(), x.kind)).collect()
    }

    #[test]
    fn marks_markers_and_content() {
        let k = kinds("hi **bold** and *it*");
        assert!(k.contains(&("bold".into(), SourceKind::Bold)));
        assert!(k.contains(&("it".into(), SourceKind::Italic)));
        assert_eq!(k.iter().filter(|(_, x)| *x == SourceKind::Marker).count(), 4);
    }

    #[test]
    fn nesting_blocks_and_unicode() {
        let k = kinds("# Title\n> **q** 😀 `c`");
        assert!(k.contains(&("Title".into(), SourceKind::Heading(1))));
        assert!(k.contains(&("q".into(), SourceKind::Bold)));
        assert!(k.contains(&("c".into(), SourceKind::Code)));
        // Offsets count UTF-16 units: the emoji is two.
        let s = style_source("😀 **b**");
        assert_eq!(s.iter().find(|x| x.kind == SourceKind::Bold).unwrap().start, 5);
    }

    #[test]
    fn unclosed_and_spaced_delimiters_stay_plain() {
        assert!(kinds("2 * 3 * 4 and **open").iter().all(|(_, k)| *k != SourceKind::Bold));
        assert!(style_source("a * b").is_empty());
    }

    #[test]
    fn code_blocks_and_lists() {
        let k = kinds("```\nlet x = **1**\n```\n- item");
        assert!(k.iter().any(|(t, x)| t.contains("let x") && *x == SourceKind::CodeBlock));
        assert!(!k.iter().any(|(_, x)| *x == SourceKind::Bold));
        assert!(k.contains(&("- ".into(), SourceKind::Marker)));
    }
}
