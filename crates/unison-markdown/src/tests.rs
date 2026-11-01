use crate::{ListMarker, Node::*, SpanKind, parse, render};

fn t(s: &str) -> crate::Node {
    Text(s.into())
}

#[test]
fn emphasis() {
    assert_eq!(parse("**b** *i* __u__ ~~s~~ ||sp||"), vec![
        Bold(vec![t("b")]),
        t(" "),
        Italic(vec![t("i")]),
        t(" "),
        Underline(vec![t("u")]),
        t(" "),
        Strike(vec![t("s")]),
        t(" "),
        Spoiler(vec![t("sp")]),
    ]);
}

#[test]
fn nested_and_bold_italic() {
    assert_eq!(parse("***x***"), vec![Bold(vec![Italic(vec![t("x")])])]);
    assert_eq!(parse("**a __b__**"), vec![Bold(vec![t("a "), Underline(vec![t("b")])])]);
}

#[test]
fn snake_case_is_not_italic() {
    assert_eq!(parse("snake_case_name"), vec![t("snake_case_name")]);
    assert_eq!(parse("_it_"), vec![Italic(vec![t("it")])]);
}

#[test]
fn code_wins_over_emphasis() {
    assert_eq!(parse("`**not bold**`"), vec![InlineCode("**not bold**".into())]);
    assert_eq!(parse("``a ` b``"), vec![InlineCode("a ` b".into())]);
}

#[test]
fn fenced_code_block_with_lang() {
    let n = parse("hi\n```rs\nfn main() {}\n```\nbye");
    assert_eq!(n, vec![
        t("hi\n"),
        CodeBlock { lang: Some("rs".into()), code: "fn main() {}".into() },
        t("bye"),
    ]);
}

#[test]
fn mentions_emoji_timestamps() {
    assert_eq!(parse("<@123> <@!4> <@&5> <#6> <a:wave:7> <:ok:8> <t:1700000000:R> </ping:9>"), vec![
        UserMention("123".into()),
        t(" "),
        UserMention("4".into()),
        t(" "),
        RoleMention("5".into()),
        t(" "),
        ChannelMention("6".into()),
        t(" "),
        Emoji { name: "wave".into(), id: "7".into(), animated: true },
        t(" "),
        Emoji { name: "ok".into(), id: "8".into(), animated: false },
        t(" "),
        Timestamp { unix: 1_700_000_000, style: 'R' },
        t(" "),
        SlashCommand { name: "ping".into(), id: "9".into() },
    ]);
}

#[test]
fn links() {
    assert_eq!(parse("see https://a.com/x(1)."), vec![t("see "), AutoLink("https://a.com/x(1)".into()), t(".")]);
    assert_eq!(parse("[docs](https://d.io)"), vec![Link { url: "https://d.io".into(), children: vec![t("docs")] }]);
    assert_eq!(parse("[x](javascript:alert)"), vec![t("[x](javascript:alert)")]);
}

#[test]
fn blocks() {
    assert_eq!(parse("# Title\n-# small\n> quoted\n- item\n2. two"), vec![
        Heading { level: 1, children: vec![t("Title"), LineBreak] },
        Subtext(vec![t("small"), LineBreak]),
        Quote(vec![t("quoted"), LineBreak]),
        ListItem { marker: ListMarker::Bullet, depth: 0, children: vec![t("item"), LineBreak] },
        ListItem { marker: ListMarker::Ordered(2), depth: 0, children: vec![t("two"), LineBreak] },
    ]);
}

#[test]
fn multi_line_quote() {
    assert_eq!(parse(">>> a\nb"), vec![Quote(vec![t("a\nb")])]);
}

#[test]
fn escapes() {
    assert_eq!(parse(r"\*not\*"), vec![t("*not*")]);
}

#[test]
fn render_utf16_offsets() {
    let r = render("😀 **b**");
    assert_eq!(r.text, "😀 b");
    // 😀 is two UTF-16 units, plus a space.
    assert_eq!(r.spans, vec![crate::Span { start: 3, len: 1, kind: SpanKind::Bold }]);
}

#[test]
fn jumbo() {
    assert!(render("<:a:1> <:b:2>").jumbo_emoji);
    assert!(render("😀😀").jumbo_emoji);
    assert!(!render("hi 😀").jumbo_emoji);
    assert!(render("👨‍👩‍👧 😀").jumbo_emoji, "a joined family counts per emoji, not per joiner");
    assert!(!render(&"😀".repeat(31)).jumbo_emoji, "past 30 it is a wall of emoji, not a few");
}

#[test]
fn bracketed_links_hide_brackets() {
    let r = render("Link: <https://hoyo.link/x>!");
    assert_eq!(r.text, "Link: https://hoyo.link/x!");
}

#[test]
fn heading_with_timestamp() {
    let r = render("NA\n # <t:1761700000:t>\n- Daily reset <t:1761700000:R>");
    assert!(!r.text.contains('#'), "indented heading marker is consumed: {:?}", r.text);
    assert!(r.spans.iter().any(|s| matches!(s.kind, SpanKind::Heading(1))));
}

#[test]
fn markdown_code_block_sections() {
    // #server-status: a reset-time field rendered by Discord as a highlighted md code block.
    let r = render("```md\n# 08:45 PM\nplain\n#not\n## 2```• Daily");
    assert_eq!(r.text, "# 08:45 PM\nplain\n#not\n## 2\n• Daily");
    let sections: Vec<_> = r.spans.iter().filter(|s| s.kind == SpanKind::CodeSection).map(|s| (s.start, s.len)).collect();
    assert_eq!(sections, vec![(0, 10), (22, 4)]);
    assert!(render("```rs\n# x```").spans.iter().all(|s| s.kind != SpanKind::CodeSection));
}

#[test]
fn code_block_newlines_and_lang_like_discord() {
    // No language: leading and trailing blank lines are dropped, as Discord's regex does.
    assert_eq!(parse("```\n\n# 02:45 AM\n\n```"), vec![CodeBlock { lang: None, code: "# 02:45 AM".into() }]);
    // A first line with a space isn't a language.
    assert_eq!(parse("```a b\nc```"), vec![CodeBlock { lang: None, code: "a b\nc".into() }]);
    assert_eq!(parse("```c++\nx```"), vec![CodeBlock { lang: Some("c++".into()), code: "x".into() }]);
    let r = render("```glsl\n# 09:45 AM```");
    assert!(r.spans.iter().any(|s| s.kind == SpanKind::CodeMeta && (s.start, s.len) == (0, 10)));
}

#[test]
fn guild_nav_links_and_headings_in_one_message() {
    let r = render("# Welcome\n**bold** see <id:customize>\n- <:Steam:123> **Steam:** https://x.y");
    assert!(r.text.contains("Channels & Roles"), "{}", r.text);
    assert!(r.spans.iter().any(|s| matches!(s.kind, SpanKind::Heading(1))));
    assert!(r.spans.iter().any(|s| matches!(s.kind, SpanKind::ListItem { .. })));
    assert!(r.spans.iter().any(|s| matches!(s.kind, SpanKind::Emoji { .. })));
}

#[test]
fn channel_links_become_chips() {
    use crate::{Resolve, render_with};
    struct R;
    impl Resolve for R {
        fn user(&self, _: &str) -> Option<String> { None }
        fn role(&self, _: &str) -> Option<String> { None }
        fn channel(&self, id: &str) -> Option<String> { (id == "22").then(|| "rules".into()) }
    }
    let r = render_with("see https://discord.com/channels/1/22 and https://discord.com/channels/1/22/33 and https://discord.com/channels/1/99", &R);
    assert!(r.text.contains("#rules ") && r.text.contains("https://discord.com/channels/1/99"), "{}", r.text);
    assert!(r.spans.iter().any(|s| matches!(&s.kind, SpanKind::ChannelMention(c) if c == "22/33")));
    assert!(r.spans.iter().any(|s| matches!(&s.kind, SpanKind::ChannelMention(c) if c == "22")));
}

#[test]
fn shortcodes_become_emoji() {
    use crate::{Resolve, render_with};
    struct R;
    impl Resolve for R {
        fn user(&self, _: &str) -> Option<String> { None }
        fn role(&self, _: &str) -> Option<String> { None }
        fn channel(&self, _: &str) -> Option<String> { None }
        fn shortcode(&self, n: &str) -> Option<String> { (n == "bulb").then(|| "💡".into()) }
    }
    assert_eq!(render_with("a :bulb: b :nope: c: d:e", &R).text, "a 💡 b :nope: c: d:e");
}

#[test]
fn masked_text_hides_spoilers_only() {
    use crate::render;
    assert_eq!(render("plain **bold** text").masked_text(), "plain bold text");
    assert_eq!(render("the end is ||the butler did it|| ok").masked_text(), "the end is \u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592} ok");
    assert_eq!(render("||a|| and ||bc||").masked_text(), "\u{2592} and \u{2592}\u{2592}");
    let m = render("x ||secret https://example.com/|| y").masked_text();
    assert!(!m.contains("secret") && !m.contains("example"), "{m}");
    let m = render("||**bold** and *it*||").masked_text();
    assert!(!m.contains("bold") && !m.contains("it"), "{m}");
}
