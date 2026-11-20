//! Message HTML: what Teams sends and what we send back.

use super::*;

/// The quote block Teams puts before a reply's text, which [`reply_content`] reads back.
pub(super) fn reply_html(m: &Msg) -> String {
    let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let mri = if m.sender_id.starts_with("28:") { m.sender_id.clone() } else { format!("8:orgid:{}", m.sender_id) };
    let ms = chrono::DateTime::parse_from_rfc3339(&m.time).map(|t| t.timestamp_millis().to_string()).unwrap_or_default();
    // Teams quotes a preview, not the whole message.
    let preview: String = m.text.chars().take(200).collect();
    format!(
        r#"<blockquote itemscope="" itemtype="http://schema.skype.com/Reply" itemid="{}"><strong itemprop="mri" itemid="{}">{}</strong><span itemprop="time" itemid="{ms}"></span><p itemprop="preview">{}</p></blockquote>"#,
        escape(&m.server_id), escape(&mri), escape(&m.sender), escape(&preview).replace('\n', " "),
    )
}

/// `raw`'s quoted reply, then `text`, then `raw`'s pictures and the new `images`.
pub(super) fn edited_html(raw: &str, text: &str, images: &[(String, (u32, u32))], mentions: &[Mention]) -> String {
    let quote = raw.find("<blockquote").and_then(|start| {
        let open_end = start + raw[start..].find('>')?;
        let end = open_end + raw[open_end..].find("</blockquote>")? + "</blockquote>".len();
        raw[start..open_end].contains("schema.skype.com/Reply").then(|| &raw[start..end])
    }).unwrap_or("");
    let pictures: String = raw.split("<img").skip(1).filter_map(|tag| tag.split_once('>').map(|(attrs, _)| attrs))
        .filter(|attrs| attrs.contains("schema.skype.com/AMSImage"))
        .map(|attrs| format!("<div><img{attrs}></div>"))
        .collect();
    let new = message_html(text, images, mentions);
    // Pictures go after the text and before the new ones, as `message_html` places pictures.
    let at = new.find("<div><img").unwrap_or(new.len());
    format!("{quote}{}{pictures}{}", &new[..at], &new[at..])
}

/// Plain text as the HTML a Teams message carries; what [`html_to_text`] undoes.
/// A message's HTML: the text, then each picture as Teams sends a pasted one.
pub(super) fn message_html(text: &str, images: &[(String, (u32, u32))], mentions: &[Mention]) -> String {
    images.iter().fold(mention_html(text, mentions), |mut html, (url, (w, h))| {
        html.push_str(&format!(r#"<div><img src="{url}" itemscope="" itemtype="http://schema.skype.com/AMSImage" style="width:{w}px; height:{h}px"></div>"#));
        html
    })
}

/// Each fenced code block goes as a Teams code block, as Teams sends one, so it shows as code there too.
pub fn text_to_html(text: &str) -> String {
    let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('\n', "<br>");
    #[cfg(feature = "markdown")]
    let blocks = crate::markdown::code_blocks(text);
    #[cfg(not(feature = "markdown"))]
    let blocks: Vec<(Range<usize>, String, String)> = vec![];
    let mut html = String::new();
    let mut at = 0;
    for (r, lang, code) in blocks {
        html.push_str(&linked_html(&text[at..r.start]));
        let lang: String = lang.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '+' | '#')).collect();
        let class = if lang.is_empty() { String::new() } else { format!(r#" class="language-{lang}""#) };
        html.push_str(&format!("<pre{class}><code>{}</code></pre>", escape(code.trim_end_matches('\n'))));
        at = r.end;
    }
    html.push_str(&linked_html(&text[at..]));
    html
}

/// Web links in outgoing plain text, also used by its optimistic local message.
pub fn text_links(text: &str) -> Vec<(Range<usize>, String)> {
    #[cfg(feature = "markdown")]
    let code: Vec<_> = pulldown_cmark::Parser::new(text).into_offset_iter().filter_map(|(event, range)| {
        matches!(event, pulldown_cmark::Event::Code(_) | pulldown_cmark::Event::Start(pulldown_cmark::Tag::CodeBlock(_))).then_some(range)
    }).collect();
    let mut links = Vec::new();
    let mut at = 0;
    for (start, _) in text.char_indices() {
        if start < at { continue; }
        let tail = &text[start..];
        let prefix = if tail.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://")) { 8 }
            else if tail.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("http://")) { 7 }
            else { continue };
        if text[..start].chars().next_back().is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '/' | ':' | '@' | '.' | '-')) { continue; }
        #[cfg(feature = "markdown")]
        if code.iter().any(|range| range.contains(&start)) { continue; }
        let end = tail.find(|c: char| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | '\'' | '`')).unwrap_or(tail.len());
        let mut url = tail[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        loop {
            let pair = match url.chars().next_back() {
                Some(')') => ('(', ')'), Some(']') => ('[', ']'), Some('}') => ('{', '}'), _ => break,
            };
            if url.matches(pair.1).count() <= url.matches(pair.0).count() { break; }
            url = url[..url.len() - 1].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        }
        let host = url.get(prefix..).unwrap_or("").split(['/', '?', '#']).next().unwrap_or("");
        if host.is_empty() { continue; }
        at = start + url.len();
        links.push((start..at, url.to_owned()));
    }
    links
}

fn linked_html(text: &str) -> String {
    let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\n', "<br>");
    let mut html = String::new();
    let mut at = 0;
    for (range, url) in text_links(text) {
        html.push_str(&escape(&text[at..range.start]));
        let label = escape(&text[range.clone()]);
        // Normalize the scheme so incoming link parsing accepts uppercase HTTP(S) too.
        let (scheme, rest) = url.split_once("://").unwrap();
        html.push_str(&format!("<a href=\"{}://{}\">{label}</a>", scheme.to_ascii_lowercase(), escape(rest)));
        at = range.end;
    }
    html.push_str(&escape(&text[at..]));
    html
}

/// Split Teams' embedded reply preview from the new message; malformed HTML stays visible.
pub(super) fn reply_content(raw: &str) -> (Option<Reply>, (String, Vec<(Range<usize>, String)>)) {
    let convert = |html: &str| rich(html, true);
    let parse = || {
        let start = raw.find("<blockquote")?;
        let open_end = start + raw[start..].find('>')?;
        if !raw[start..open_end].contains("schema.skype.com/Reply") { return None; }
        let end = open_end + raw[open_end..].find("</blockquote>")?;
        let id = raw[start..open_end].split_once("itemid=").and_then(|(_, tail)| {
            let quote = tail.chars().next()?;
            matches!(quote, '\'' | '"').then(|| tail[1..].split(quote).next().unwrap_or("").to_owned())
        }).unwrap_or_default();
        let quoted = &raw[open_end + 1..end];
        // shortcut: nested reply previews stay plain text; extend parsing if Teams sends them.
        if quoted.contains("<blockquote") { return None; }
        let element = |tag: &str| -> Option<(&str, &str)> {
            let start = quoted.find(&format!("<{tag}"))?;
            let open_end = start + quoted[start..].find('>')?;
            let end = open_end + quoted[open_end..].find(&format!("</{tag}>"))?;
            Some((&quoted[start..open_end], &quoted[open_end + 1..end]))
        };
        let sender = element("strong").map(|(_, body)| html_to_text(body)).unwrap_or_default();
        let sender_id = element("strong").and_then(|(tag, _)| {
            let tail = tag.split_once("itemid=")?.1;
            let quote = tail.chars().next()?;
            matches!(quote, '\'' | '"').then(|| super::parse::avatar_id(tail[1..].split(quote).next().unwrap_or("")))
        }).unwrap_or_default();
        let time = element("span").and_then(|(tag, _)| {
            let tail = tag.split_once("itemid=")?.1;
            let quote = tail.chars().next()?;
            if !matches!(quote, '\'' | '"') { return None; }
            tail[1..].split(quote).next()?.parse::<i64>().ok().filter(|ms| *ms >= 0).map(iso)
        }).unwrap_or_default();
        let text = html_to_text(&quoted[quoted.find("<p")?..]);
        let mut body = raw[..start].to_owned();
        body.push_str(&raw[end + "</blockquote>".len()..]);
        Some((Some(Reply { id, sender, sender_id, time, text }), convert(&body)))
    };
    parse().unwrap_or_else(|| (None, convert(raw)))
}

/// Teams message HTML as plain text: tags dropped, block ends as newlines, common entities decoded.
pub fn html_to_text(html: &str) -> String {
    rich(html, false).0
}

/// Emoji-only messages use Teams' artwork; mixed text keeps its inline Unicode.
#[cfg(any(feature = "image", test))]
pub fn animated_emojis(raw: &str) -> Vec<(String, String, u16)> {
    let mut emojis = Vec::new();
    for tag in raw.split('<').filter_map(|tag| tag.strip_prefix("img ")).filter_map(|tag| tag.split('>').next()) {
        if !tag.contains("schema.skype.com/Emoji") { continue; }
        let attr = |name: &str| {
            let tail = tag.split_once(&format!("{name}="))?.1;
            let quote = tail.chars().next()?;
            matches!(quote, '\'' | '"').then(|| tail[1..].split(quote).next().unwrap_or(""))
        };
        let Some(src) = attr("src") else { return vec![]; };
        let Some((url, alt)) = super::emoji_animation(src).zip(attr("alt")) else { return vec![]; };
        if alt.is_empty() { return vec![]; }
        emojis.push((url, html_to_text(alt), 58));
    }
    let text: String = html_to_text(raw).chars().filter(|c| !c.is_whitespace()).collect();
    let alts: String = emojis.iter().flat_map(|(_, alt, _)| alt.chars()).filter(|c| !c.is_whitespace()).collect();
    if text == alts { emojis } else { vec![] }
}

/// [`html_to_text`] plus its `http(s)` links (byte ranges of the text and where they point), with
/// code blocks fenced as typed (```lang), so Markdown reads them as code blocks again.
// shortcut: inline formatting stays plain; extend when the message view needs it.
pub fn html_to_rich(html: &str) -> (String, Vec<(Range<usize>, String)>) {
    let (text, mut links) = rich(html, true);
    links.retain(|(_, url)| !url.starts_with("mention:"));
    (text, links)
}

pub(super) fn rich(html: &str, fences: bool) -> (String, Vec<(Range<usize>, String)>) {
    let decode = |s: &str| s.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&");
    let mut out = String::new();
    let mut links = Vec::new();
    let mut open: Option<(usize, String)> = None;
    let mut mention = None;
    let mut span_depth = 0usize;
    // The open code block: where its code starts in `out`, and its language.
    let mut pre: Option<(usize, String)> = None;
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        // Newlines between tags are source formatting, not content; keeping them doubles every line break.
        if !(rest[..i].trim().is_empty() && rest[..i].contains('\n')) {
            out.push_str(&decode(&rest[..i]));
        }
        let Some(j) = rest[i..].find('>') else { break };
        let inner = &rest[i + 1..i + j];
        let tag = inner.trim_start_matches('/').split([' ', '/']).next().unwrap_or("").to_ascii_lowercase();
        if tag == "img" && inner.contains("schema.skype.com/Emoji")
            && let Some((_, tail)) = inner.split_once("alt=")
            && let Some(quote @ ('\'' | '"')) = tail.chars().next()
            && let Some(alt) = tail[1..].split(quote).next() {
            out.push_str(&decode(alt));
        }
        // In a code block every <br> is a line, so blank lines between sections stay.
        if matches!(tag.as_str(), "br" | "p" | "div" | "li" | "tr" | "pre") && (!out.ends_with('\n') || pre.is_some() && tag == "br") {
            out.push('\n');
        }
        if tag == "pre" && !inner.starts_with('/') {
            let lang = inner.split_once("language-").and_then(|(_, l)| l.split(['"', '\'', ' ']).next()).unwrap_or("");
            pre = Some((out.len(), lang.to_owned()));
        } else if tag == "pre" && let Some((start, lang)) = pre.take().filter(|_| fences) {
            // One backtick more than the longest run inside, so a fence in the code can't close it.
            let run = out[start..].split(|c| c != '`').map(str::len).max().unwrap_or(0);
            let fence = "`".repeat(run.max(2) + 1);
            let open = format!("{fence}{lang}\n");
            out.insert_str(start, &open);
            links.iter_mut().filter(|(r, _): &&mut (Range<usize>, String)| r.start >= start).for_each(|(r, _)| *r = r.start + open.len()..r.end + open.len());
            if !out.ends_with('\n') { out.push('\n'); }
            out.push_str(&fence);
            out.push('\n');
        } else if tag == "pre" {
            pre = None;
        }
        if tag == "span" {
            if inner.starts_with('/') {
                if mention.as_ref().is_some_and(|(_, _, depth)| *depth == span_depth)
                    && let Some((start, id, _)) = mention.take() {
                    links.push((start..out.len(), format!("mention:{id}")));
                }
                span_depth = span_depth.saturating_sub(1);
            } else {
                span_depth += 1;
                if inner.contains("schema.skype.com/Mention") {
                    mention = inner.split_once("itemid=").and_then(|(_, tail)| {
                        let quote = tail.chars().next()?;
                        matches!(quote, '\'' | '"').then(|| (out.len(), tail[1..].split(quote).next().unwrap_or("").to_owned(), span_depth))
                    });
                }
            }
        }
        if tag == "a" {
            if inner.starts_with('/') {
                if let Some((start, href)) = open.take().filter(|(start, _)| out.len() > *start) {
                    links.push((start..out.len(), href));
                }
            } else {
                // Only web links: the URL goes to the system's opener on a click.
                open = inner.split_once("href=\"").and_then(|(_, v)| v.split('"').next()).map(decode)
                    .filter(|h| h.starts_with("https://") || h.starts_with("http://"))
                    .map(|h| (out.len(), h));
            }
        }
        rest = &rest[i + j + 1..];
        // Teams formats Shift+Enter as <br> followed by a source newline, not a second break.
        if tag == "br" { rest = rest.trim_start_matches(['\r', '\n']); }
    }
    if !rest.contains('<') {
        out.push_str(&decode(rest));
    }
    let lead = out.len() - out.trim_start().len();
    let text = out.trim().to_string();
    let links = links.into_iter()
        .map(|(r, h)| (r.start.saturating_sub(lead).min(text.len())..r.end.saturating_sub(lead).min(text.len()), h))
        .filter(|(r, _)| !r.is_empty())
        .collect();
    (text, links)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn teams_emoji_images_remain_visible_and_keep_link_offsets() {
        let emoji = r#"<span type="muscle"><img itemtype="http://schema.skype.com/Emoji" itemid="muscle-tone2" src="https://statics.teams.cdn.office.net/muscle.png" alt="💪🏼"></span>"#;
        let raw = format!("<div>{emoji}{emoji}</div>");
        let msgs = parse_messages(&json!({"messages":[{
            "id":"101", "messagetype":"RichText/Html", "content":raw
        }]}), "me");
        assert_eq!(msgs[0].text, "💪🏼💪🏼");
        assert!(msgs[0].images.is_empty());
        assert_eq!(html_to_text(&raw), "💪🏼💪🏼");
        let (text, links) = html_to_rich(&format!("{emoji}<a href=\"https://example.com\">link</a>"));
        assert_eq!(text, "💪🏼link");
        assert_eq!(&text[links[0].0.clone()], "link");
        assert_eq!(html_to_text("<img itemtype='http://schema.skype.com/Emoji' alt='&lt;3'>"), "<3");
        assert_eq!(html_to_text("<img itemtype='http://schema.skype.com/AMSImage' alt='picture'>"), "");
        assert_eq!(html_to_text("<img itemtype='http://schema.skype.com/Emoji'>"), "");
    }

    #[test]
    fn animated_emoji_messages_preserve_duplicates_and_skin_tone() {
        let emoji = r#"<span><img itemtype="http://schema.skype.com/Emoji" src="https://statics.teams.cdn.office.net//evergreen-assets/personal-expressions/v2/assets/emoticons//muscle/default/100_f_s3.png?v=v45" alt="💪🏼"></span>"#;
        let got = animated_emojis(&format!("<div>{emoji}{emoji}</div>"));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], got[1]);
        assert_eq!(got[0].1, "💪🏼");
        assert!(got[0].0.ends_with("/muscle/default/100_anim_f_s3.png"));
        assert!(animated_emojis(&format!("Hello {emoji}")).is_empty());
        assert!(animated_emojis(&emoji.replace("statics.teams.cdn.office.net", "example.com")).is_empty());
        for bad in ["https://statics.teams.cdn.office.net.evil.test/evergreen-assets/personal-expressions/v2/assets/emoticons/muscle/default/100_f_s3.png", "https://statics.teams.cdn.office.net/evergreen-assets/personal-expressions/v2/assets/emoticons/../default/100_f_s3.png"] {
            assert!(super::super::emoji_animation(bad).is_none());
        }
    }

    #[test]
    fn small_blankface_asset_displays_as_a_large_animated_emoji() {
        let raw = r#"<p><span title="Face without mouth" type="(blankface)" class="animated-emoticon-20-blankface"><img itemtype="http://schema.skype.com/Emoji" itemid="blankface" src="https://statics.teams.cdn.office.net/evergreen-assets/personal-expressions/v2/assets/emoticons/blankface/default/20_f.png" title="Face without mouth" alt="😶" style="width:20px; height:20px"></span></p>"#;
        let got = animated_emojis(raw);
        assert_eq!(got, vec![("https://statics.teams.cdn.office.net/evergreen-assets/personal-expressions/v2/assets/emoticons/blankface/default/100_anim_f.png".into(), "😶".into(), 58)]);
        assert_eq!(html_to_text(raw), "😶");
    }

    #[test]
    fn outgoing_links_round_trip_with_safe_html_and_correct_boundaries() {
        let text = "你好 (https://example.com/a_(b)?x=1&y=2),\nhttp://example.org/test. <script>\"quoted\"</script> javascript:alert(1)";
        let links = text_links(text);
        assert_eq!(links.iter().map(|(_, url)| url.as_str()).collect::<Vec<_>>(),
            ["https://example.com/a_(b)?x=1&y=2", "http://example.org/test"]);
        for (range, url) in &links { assert_eq!(&text[range.clone()], url); }
        let html = text_to_html(text);
        assert!(html.contains("href=\"https://example.com/a_(b)?x=1&amp;y=2\""));
        assert!(html.contains("&lt;script&gt;&quot;quoted&quot;&lt;/script&gt;"));
        assert!(!html.contains("href=\"javascript:"));
        assert_eq!(html_to_rich(&html), (text.to_owned(), links));
        assert!(text_links("http:// https:// javascript:https://example.com").is_empty());
        let (shown, links) = html_to_rich(&text_to_html("HTTPS://example.com/path"));
        assert_eq!(shown, "HTTPS://example.com/path");
        assert_eq!(links, vec![(0..shown.len(), "https://example.com/path".into())]);
    }

    #[cfg(feature = "markdown")]
    #[test]
    fn outgoing_links_skip_code_and_work_in_replies_and_edits() {
        let text = "https://example.com/live\n`https://example.com/inline`\n```rs\nhttps://example.com/code\n```";
        let html = message_html(text, &[], &[]);
        assert_eq!(text_links(text).len(), 1);
        assert_eq!(html.matches("<a href=").count(), 1);
        assert_eq!(html_to_rich(&html).1.len(), 1);
        let raw = "<blockquote itemtype=\"http://schema.skype.com/Reply\"><strong>Someone</strong><p>quote</p></blockquote><p>old</p>";
        let edited = edited_html(raw, text, &[], &[]);
        let (reply, (body, links)) = reply_content(&edited);
        assert_eq!(reply.unwrap().text, "quote");
        assert_eq!(body, text);
        assert_eq!(links, text_links(text));
        let reply = format!("{raw}{html}");
        assert_eq!(reply_content(&reply).1.1.len(), 1);
    }

    #[test]
    fn shift_enter_source_newlines_do_not_double_line_breaks() {
        for raw in ["Hello<br>\r\nBye", "<p>Hello<br>\r\nBye</p>", "Hello<br>\nBye"] {
            assert_eq!(html_to_text(raw), "Hello\nBye");
            assert_eq!(html_to_rich(raw).0, "Hello\nBye");
        }
        let (text, links) = html_to_rich("Hello<br>\r\n<a href=\"https://example.com\">Bye</a>");
        assert_eq!(text, "Hello\nBye");
        assert_eq!(links, vec![(6..9, "https://example.com".into())]);
        assert_eq!(html_to_text("<pre>a<br>\r\n<br>\r\nb</pre>"), "a\n\nb");
    }

    #[test]
    fn html_source_newlines_between_blocks_are_not_blank_lines() {
        let raw = "<p>Date: 15th Nov</p>\r\n<p>Time: 4:00</p>\n<p>&nbsp;</p>\n<p>Reason: x</p>";
        assert_eq!(super::html_to_text(raw), "Date: 15th Nov\nTime: 4:00\n \nReason: x");
    }

    #[test]
    fn quoted_reply_keeps_preview_separate_from_body() {
        let raw = r#"<div><blockquote itemscope="" itemtype="http://schema.skype.com/Reply" itemid="1600457867820"><strong itemprop="mri">Bob &amp; Alice</strong><span itemprop="time" itemid="1600457867820"></span><p itemprop="preview">Original &lt;message&gt;</p><p>Second line</p></blockquote><p>My reply</p></div>"#;
        let v = json!({"messages": [{"messagetype": "RichText/Html", "content": raw}]});
        let msgs = parse_messages(&v, "me");
        assert_eq!(msgs[0].text, "My reply");
        let reply = msgs[0].reply.as_ref().unwrap();
        assert_eq!(reply.id, "1600457867820");
        assert_eq!(reply.sender, "Bob & Alice");
        assert_eq!(reply.time, iso(1600457867820));
        assert_eq!(reply.text, "Original <message>\nSecond line");
        let only_quote = raw.replace("<p>My reply</p>", "");
        assert_eq!(reply_content(&only_quote).1.0, "");
        for plain in ["<p>Normal message</p>", "<blockquote>Ordinary quote</blockquote>", "<blockquote itemtype='http://schema.skype.com/Reply'>Broken"] {
            assert_eq!(reply_content(plain), (None, html_to_rich(plain)));
        }
        let missing_time = raw.replace("itemid=\"1600457867820\"", "");
        assert_eq!(reply_content(&missing_time).0.unwrap().time, "");
        let ids = json!({"messages": [{"id": "server-id", "clientmessageid": "client-id", "messagetype": "Text", "content": "Sent"}]});
        let msgs = parse_messages(&ids, "me");
        assert_eq!(msgs[0].id, "client-id");
        assert_eq!(msgs[0].server_id, "server-id");
    }

    #[test]
    fn sent_pictures_parse_back_as_the_same_pictures() {
        let url = "https://in-prod.asyncgw.teams.microsoft.com/v1/objects/0-sin-d2-ab12/views/imgo".to_owned();
        let html = message_html("look <here>", &[(url.clone(), (1170, 2532))], &[]);
        let m = parse_messages(&json!({"messages": [{"messagetype": "RichText/Html", "content": html, "composetime": "1"}]}), "me");
        assert_eq!(m[0].text, "look <here>");
        assert_eq!(m[0].images, [url]);
        assert_eq!(message_html("", &[], &[]), "");
    }

    #[test]
    fn replies_quote_as_teams_does_and_read_back() {
        let m = super::Msg {
            id: "c1".into(), server_id: "1700000000000".into(), sender: "Bob <B>".into(), sender_id: "oid-1".into(),
            text: "first\nsecond".into(), time: "2026-11-14T10:24:00Z".into(), mine: false, reply: None, thread_root: None, images: vec![], links: vec![], files: vec![], raw: String::new(), edited: false, deleted: false, reactions: vec![], mentions: vec![],
        };
        let html = format!("{}{}", super::reply_html(&m), super::message_html("ok", &[], &[]));
        assert!(html.contains(r#"itemid="8:orgid:oid-1""#));
        let (reply, (text, _)) = super::reply_content(&html);
        assert_eq!(reply, Some(super::Reply { id: "1700000000000".into(), sender: "Bob <B>".into(), sender_id: "oid-1".into(), time: "2026-11-14T10:24:00.000Z".into(), text: "first second".into() }));
        assert_eq!(text, "ok");
    }

    #[test]
    fn edits_keep_the_quote_and_pictures() {
        let quote = r#"<blockquote itemscope="" itemtype="http://schema.skype.com/Reply" itemid="1"><strong>A</strong><p>hi</p></blockquote>"#;
        let pic = r#"<img src="https://eu-api.asm.skype.com/v1/objects/0-a/views/imgo" itemtype="http://schema.skype.com/AMSImage" style="width:4px">"#;
        let raw = format!("{quote}<p>old</p><div>{pic}</div><img src=\"https://x/emoji.png\">");
        let new = ("https://eu-api.asm.skype.com/v1/objects/0-b/views/imgo".to_string(), (2, 3));
        let html = super::edited_html(&raw, "new <text>", &[new], &[]);
        assert_eq!(html, format!(r#"{quote}new &lt;text&gt;<div>{pic}</div><div><img src="https://eu-api.asm.skype.com/v1/objects/0-b/views/imgo" itemscope="" itemtype="http://schema.skype.com/AMSImage" style="width:2px; height:3px"></div>"#));
        assert_eq!(super::edited_html("<p>plain</p>", "x", &[], &[]), "x");
    }

    #[test]
    fn html() {
        assert_eq!(html_to_text("<p>a<br>b</p><p>c &lt;d&gt;</p>"), "a\nb\nc <d>");
        assert_eq!(html_to_text("<img src=\"x\">"), "");
        assert_eq!(html_to_text("plain &amp;lt;"), "plain &lt;");
        assert_eq!(html_to_text("<p>a<br><br>b</p><pre><code>c<br><br>d</code></pre>"), "a\nb\nc\n\nd");
    }

    #[cfg(feature = "markdown")]
    #[test]
    fn code_blocks_go_as_teams_code_blocks_and_come_back_fenced() {
        let typed = "hi <you>\n```markdown\n# H1\n\n## H2\n```\nbye";
        let html = text_to_html(typed);
        assert_eq!(html, r#"hi &lt;you&gt;<br><pre class="language-markdown"><code># H1<br><br>## H2</code></pre>bye"#);
        assert_eq!(html_to_rich(&html).0, typed);
        assert_eq!(html_to_text(&html), "hi <you>\n# H1\n\n## H2\nbye");
        // A fence inside a block gets a longer fence around it, so it stays inside.
        let nested = html_to_rich("<pre class=\"language-md x\"><code>a<br>```<br>b<br>```</code></pre>").0;
        assert_eq!(nested, "````md\na\n```\nb\n```\n````");
        let typed = "a <b> & c\nd &lt;";
        assert_eq!(text_to_html(typed), "a &lt;b&gt; &amp; c<br>d &amp;lt;");
        assert_eq!(html_to_text(&text_to_html(typed)), typed);
        assert_eq!(encode("19:a_b@unq.gbl.spaces"), "19%3Aa_b%40unq.gbl.spaces");
    }

    #[test]
    fn links_keep_their_text_range_and_only_web_urls() {
        // A real Teams link: the href's entities decode, and the range covers the shown URL.
        let raw = r#"<p>see <a href="https://x.example/?a=1&amp;b=2" itemtype="http://schema.skype.com/HyperLink" title="t">https://x.example/?a=1&amp;b=2</a> &amp; <a href="javascript:alert(1)">js</a></p>"#;
        let (text, links) = super::html_to_rich(raw);
        assert_eq!(text, "see https://x.example/?a=1&b=2 & js");
        assert_eq!(links.len(), 1);
        assert_eq!(&text[links[0].0.clone()], "https://x.example/?a=1&b=2");
        assert_eq!(links[0].1, "https://x.example/?a=1&b=2");
    }
}
