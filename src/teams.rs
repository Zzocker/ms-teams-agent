//! Teams' private chat service (undocumented; the Teams web client's own API): chats and their messages.

use crate::auth::{Session, User, agent};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub preview: String,
    /// ISO 8601, UTC.
    pub time: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Msg {
    pub sender: String,
    pub text: String,
    pub time: String,
    pub mine: bool,
}

fn url(s: &Session, path: &str) -> String {
    format!("{}/v1/users/ME/{path}", s.chat_service)
}

fn auth(s: &Session) -> String {
    format!("skypetoken={}", s.skype_token)
}

fn answer(res: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Value, String> {
    let mut res = res.map_err(|e| e.to_string())?;
    let status = res.status();
    if status == 401 {
        return Err("Teams session expired; restart the app.".into());
    }
    let v: Value = res.body_mut().read_json().map_err(|e| format!("chat service {status}: {e}"))?;
    if !status.is_success() {
        return Err(format!("chat service {status}: {}", v["message"].as_str().unwrap_or("")));
    }
    Ok(v)
}

fn get(s: &Session, path: &str) -> Result<Value, String> {
    answer(agent().get(&url(s, path)).header("Authentication", &auth(s)).call())
}

/// Sends `text` to a chat as the signed-in user.
pub fn send(s: &Session, chat_id: &str, text: &str) -> Result<(), String> {
    // The service drops a repeat of a client message id, so it must differ per message.
    let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let body = serde_json::json!({
        "content": text_to_html(text),
        "messagetype": "RichText/Html",
        "contenttype": "text",
        "clientmessageid": id.to_string(),
        "imdisplayname": s.user.name,
    });
    answer(agent().post(&url(s, &format!("conversations/{}/messages", encode(chat_id)))).header("Authentication", &auth(s)).send_json(&body)).map(|_| ())
}

/// Plain text as the HTML a Teams message carries; what [`html_to_text`] undoes.
pub fn text_to_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('\n', "<br>")
}

pub fn chats(s: &Session) -> Result<Vec<Chat>, String> {
    Ok(parse_chats(&get(s, "conversations?view=msnp24Equivalent&pageSize=100")?, &s.user))
}

pub fn messages(s: &Session, chat_id: &str) -> Result<Vec<Msg>, String> {
    let v = get(s, &format!("conversations/{}/messages?view=msnp24Equivalent&pageSize=50&startTime=1", encode(chat_id)))?;
    Ok(parse_messages(&v, &s.user.id))
}

/// The chat with yourself, which Teams shows as "Name (You)".
const NOTES: &str = "48:notes";

pub fn parse_chats(v: &Value, me: &User) -> Vec<Chat> {
    let empty = vec![];
    let mut chats: Vec<Chat> = v["conversations"].as_array().unwrap_or(&empty).iter()
        .filter(|c| {
            let id = c["id"].as_str().unwrap_or("");
            // Other 48: ids are system threads (notifications, call log, drafts); space/topic are team channels.
            !id.is_empty() && (id == NOTES || !id.starts_with("48:")) && !matches!(c["threadProperties"]["threadType"].as_str(), Some("space" | "topic"))
        })
        .map(|c| {
            let last = &c["lastMessage"];
            let topic = c["threadProperties"]["topic"].as_str().unwrap_or("").trim();
            let sender = last["imdisplayname"].as_str().unwrap_or("");
            // shortcut: an untitled chat is named after its last sender who isn't me; the proper name needs a member lookup per chat.
            let you = format!("{} (You)", me.name);
            let title = if c["id"] == NOTES {
                &you
            } else if !topic.is_empty() {
                topic
            } else if !sender.is_empty() && !last["from"].as_str().unwrap_or("").ends_with(&me.id) {
                sender
            } else {
                "Chat"
            };
            Chat {
                id: c["id"].as_str().unwrap_or("").into(),
                title: title.into(),
                preview: html_to_text(last["content"].as_str().unwrap_or("")).lines().next().unwrap_or("").into(),
                time: last["composetime"].as_str().unwrap_or("").into(),
            }
        })
        .collect();
    chats.sort_by(|a, b| b.time.cmp(&a.time));
    chats
}

/// Oldest first; only messages people wrote (not typing, joins, renames, calls).
pub fn parse_messages(v: &Value, me: &str) -> Vec<Msg> {
    let empty = vec![];
    let mut msgs: Vec<Msg> = v["messages"].as_array().unwrap_or(&empty).iter()
        .filter(|m| matches!(m["messagetype"].as_str(), Some(t) if t == "Text" || t.starts_with("RichText")))
        .filter_map(|m| {
            let raw = m["content"].as_str().unwrap_or("");
            if raw.is_empty() {
                return None; // deleted
            }
            let text = html_to_text(raw);
            Some(Msg {
                sender: m["imdisplayname"].as_str().unwrap_or("").into(),
                text: if text.is_empty() { "[attachment]".into() } else { text },
                time: m["composetime"].as_str().unwrap_or("").into(),
                mine: !me.is_empty() && m["from"].as_str().unwrap_or("").ends_with(me),
            })
        })
        .collect();
    msgs.sort_by(|a, b| a.time.cmp(&b.time));
    msgs
}

/// Teams message HTML as plain text: tags dropped, block ends as newlines, common entities decoded.
// shortcut: no formatting, links, mentions or images; render the HTML when the message view gets rich text.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('>') else { break };
        let tag = rest[i + 1..i + j].trim_start_matches('/').split([' ', '/']).next().unwrap_or("").to_ascii_lowercase();
        if matches!(tag.as_str(), "br" | "p" | "div" | "li" | "tr") && !out.ends_with('\n') {
            out.push('\n');
        }
        rest = &rest[i + j + 1..];
    }
    if !rest.contains('<') {
        out.push_str(rest);
    }
    let out = out.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&");
    out.trim().to_string()
}

/// A path segment, percent-encoded as JavaScript's `encodeURIComponent` does.
fn encode(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => (b as char).to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ME: &str = "8:orgid:me-oid";

    #[test]
    fn chats_skip_system_threads_and_channels_and_name_untitled_chats() {
        let v = json!({"conversations": [
            {"id": "48:notifications", "lastMessage": {}},
            {"id": "48:notes", "lastMessage": {"imdisplayname": "Me", "composetime": "2026-11-12T00:00:00Z"}},
            {"id": "19:c@thread.tacv2", "threadProperties": {"threadType": "space"}, "lastMessage": {}},
            {"id": "19:a_b@unq.gbl.spaces", "lastMessage": {"imdisplayname": "Bob", "from": "https://x/contacts/8:orgid:bob",
                "content": "<p>hi &amp; bye</p><p>second</p>", "composetime": "2026-11-14T10:00:00Z"}},
            {"id": "19:g@thread.v2", "threadProperties": {"topic": "Design"}, "lastMessage": {"composetime": "2026-11-14T11:00:00Z"}},
            {"id": "19:m_b@unq.gbl.spaces", "lastMessage": {"imdisplayname": "Me", "from": "https://x/contacts/8:orgid:me-oid", "composetime": "2026-11-13T00:00:00Z"}},
        ]});
        let c = parse_chats(&v, &User { name: "Me".into(), id: "me-oid".into(), ..User::default() });
        assert_eq!(c.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["Design", "Bob", "Chat", "Me (You)"]);
        assert_eq!(c[1].preview, "hi & bye");
    }

    #[test]
    fn messages_keep_only_text_oldest_first() {
        let v = json!({"messages": [
            {"messagetype": "RichText/Html", "content": "<div>two</div>", "imdisplayname": "Me", "from": format!("https://x/contacts/{ME}"), "composetime": "2"},
            {"messagetype": "Control/Typing", "content": "", "composetime": "3"},
            {"messagetype": "ThreadActivity/AddMember", "content": "<addmember/>", "composetime": "1"},
            {"messagetype": "Text", "content": "", "composetime": "4"},
            {"messagetype": "Text", "content": "one", "imdisplayname": "Bob", "from": "https://x/contacts/8:orgid:bob", "composetime": "1"},
        ]});
        let m = parse_messages(&v, "me-oid");
        assert_eq!(m.iter().map(|m| (m.text.as_str(), m.mine)).collect::<Vec<_>>(), [("one", false), ("two", true)]);
    }

    #[test]
    fn html() {
        assert_eq!(html_to_text("<p>a<br>b</p><p>c &lt;d&gt;</p>"), "a\nb\nc <d>");
        assert_eq!(html_to_text("<img src=\"x\">"), "");
        assert_eq!(html_to_text("plain &amp;lt;"), "plain &lt;");
        let typed = "a <b> & c\nd &lt;";
        assert_eq!(text_to_html(typed), "a &lt;b&gt; &amp; c<br>d &amp;lt;");
        assert_eq!(html_to_text(&text_to_html(typed)), typed);
        assert_eq!(encode("19:a_b@unq.gbl.spaces"), "19%3Aa_b%40unq.gbl.spaces");
    }
}
