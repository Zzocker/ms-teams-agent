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
    /// The client message id the sender gave it (the server's id if none), so a message
    /// shown as it's sent keeps its identity when the server's copy replaces it.
    pub id: String,
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
/// `id` is from [`new_msg_id`].
pub fn send(s: &Session, chat_id: &str, id: &str, text: &str) -> Result<(), String> {
    let body = serde_json::json!({
        "content": text_to_html(text),
        "messagetype": "RichText/Html",
        "contenttype": "text",
        "clientmessageid": id,
        "imdisplayname": s.user.name,
    });
    answer(agent().post(&url(s, &format!("conversations/{}/messages", encode(chat_id)))).header("Authentication", &auth(s)).send_json(&body)).map(|_| ())
}

/// A fresh client message id. The service drops a repeat of one, so it must differ per message.
pub fn new_msg_id() -> String {
    // Nanoseconds: two sends in one millisecond still differ.
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()).to_string()
}

/// Now as the service writes `composetime`: `2026-11-14T10:23:45.123Z`.
pub fn now() -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    iso(ms)
}

fn iso(ms: i64) -> String {
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Days since 1970-01-01 to a civil date (Howard Hinnant's days_from_civil, inverted).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let (h, mi, s, milli) = (rem / 3_600_000, rem / 60_000 % 60, rem / 1000 % 60, rem % 1000);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{milli:03}Z")
}

/// `fresh` (the latest page) with the rows of `old` from before it kept above it, so a reload
/// doesn't drop pages scrolled back to.
pub fn merge(old: Page, fresh: Page) -> Page {
    let Some(first) = fresh.msgs.first() else { return fresh };
    let mut msgs: Vec<Msg> = old.msgs.into_iter()
        .filter(|m| m.time < first.time && !fresh.msgs.iter().any(|f| f.id == m.id))
        .collect();
    // Kept rows reach back to `old`'s start, so its link is the one before them.
    let older = if msgs.is_empty() { fresh.older } else { old.older };
    msgs.extend(fresh.msgs);
    Page { msgs, older }
}

/// Plain text as the HTML a Teams message carries; what [`html_to_text`] undoes.
pub fn text_to_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('\n', "<br>")
}

/// The chats with guessed titles; [`name_chats`] gives them their people's names.
pub fn chats(s: &Session) -> Result<Vec<Chat>, String> {
    Ok(parse_chats(&get(s, "conversations?view=msnp24Equivalent&pageSize=100")?, &s.user))
}

/// Names chats after their people. Slow (a request per untitled group chat), so done after the list shows.
pub fn name_chats(s: &Session, mut chats: Vec<Chat>) -> Vec<Chat> {
    let me = format!("8:orgid:{}", s.user.id);
    // Chats carry no names. Who each is named after: a 1:1's other person (in its id), or the
    // members of a group chat without a topic (a request each; there are few).
    // The member lookups run at once: one after another they add up.
    let people: Vec<Vec<String>> = std::thread::scope(|scope| {
        let jobs: Vec<_> = chats.iter().map(|c| match peer(&c.id, &s.user.id) {
            Some(p) => Ok(vec![format!("8:orgid:{p}")]),
            None if c.title == UNTITLED => Err(scope.spawn(|| members(s, &c.id).unwrap_or_default().into_iter().filter(|m| *m != me).collect())),
            None => Ok(vec![]),
        }).collect();
        jobs.into_iter().map(|j| j.unwrap_or_else(|h| h.join().unwrap_or_default())).collect()
    });
    let mut all: Vec<&String> = people.iter().flatten().collect();
    all.sort();
    all.dedup();
    if all.is_empty() {
        return chats;
    }
    // A failed lookup isn't worth losing the chat list over: the guessed titles stay.
    let Ok(v) = answer(agent()
        .post(&format!("{}/beta/users/fetchShortProfile?isMailAddress=false&enableGuest=true&skypeTeamsInfo=true", s.middle_tier))
        .header("Authorization", &format!("Bearer {}", s.access))
        .send_json(&all))
    else {
        return chats;
    };
    let empty = vec![];
    let profiles = v["value"].as_array().unwrap_or(&empty);
    let name = |mri: &str| profiles.iter().find(|p| p["mri"] == mri).and_then(|p| p["displayName"].as_str()).filter(|n| !n.is_empty());
    for (c, people) in chats.iter_mut().zip(&people) {
        let names: Vec<&str> = people.iter().filter_map(|p| name(p)).collect();
        if !names.is_empty() {
            c.title = names.join(", ");
        }
    }
    chats
}

/// A chat's members' ids, e.g. `8:orgid:{oid}`.
fn members(s: &Session, chat_id: &str) -> Result<Vec<String>, String> {
    let v = answer(agent().get(&format!("{}/v1/threads/{}?view=msnp24Equivalent", s.chat_service, encode(chat_id))).header("Authentication", &auth(s)).call())?;
    let empty = vec![];
    Ok(v["members"].as_array().unwrap_or(&empty).iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
}

/// The other person's object id in a 1:1 chat, whose id is `19:{oid}_{oid}@unq.gbl.spaces`.
fn peer<'a>(chat_id: &'a str, me: &str) -> Option<&'a str> {
    let (a, b) = chat_id.strip_prefix("19:")?.strip_suffix("@unq.gbl.spaces")?.split_once('_')?;
    Some(if a == me { b } else { a })
}

/// A run of messages, oldest first, and the link to the page before it (`None` at the start of the chat).
#[derive(Clone)]
pub struct Page {
    pub msgs: Vec<Msg>,
    pub older: Option<String>,
}

pub fn messages(s: &Session, chat_id: &str) -> Result<Page, String> {
    let v = get(s, &format!("conversations/{}/messages?view=msnp24Equivalent&pageSize=50&startTime=1", encode(chat_id)))?;
    Ok(page(&v, &s.user.id))
}

/// The page `link` (a [`Page::older`]) points to.
pub fn older(s: &Session, link: &str) -> Result<Page, String> {
    // The link carries our token, so only follow it to the chat service.
    if !link.starts_with(&s.chat_service) {
        return Err("unexpected link for older messages".into());
    }
    Ok(page(&answer(agent().get(link).header("Authentication", &auth(s)).call())?, &s.user.id))
}

fn page(v: &Value, me: &str) -> Page {
    Page { msgs: parse_messages(v, me), older: v["_metadata"]["backwardLink"].as_str().map(Into::into) }
}

/// The chat with yourself, which Teams shows as "Name (You)".
const NOTES: &str = "48:notes";
/// A chat with no topic and nobody else's last message to name it after.
const UNTITLED: &str = "Chat";

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
                UNTITLED
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
                id: m["clientmessageid"].as_str().or(m["id"].as_str()).unwrap_or("").into(),
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
        assert_eq!(peer("19:bob_me-oid@unq.gbl.spaces", "me-oid"), Some("bob"));
        assert_eq!(peer("19:me-oid_bob@unq.gbl.spaces", "me-oid"), Some("bob"));
        assert_eq!(peer("19:g@thread.v2", "me-oid"), None);
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
    fn iso_dates() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(1_794_673_185_538), "2026-11-14T16:19:45.538Z");
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn merge_keeps_older_rows_and_swaps_the_sent_copy() {
        let m = |id: &str, time: &str| Msg { id: id.into(), sender: String::new(), text: id.into(), time: time.into(), mine: false };
        let old = Page { msgs: vec![m("a", "1"), m("b", "2"), m("c", "3"), m("sent", "9")], older: Some("before-a".into()) };
        let fresh = Page { msgs: vec![m("b", "2"), m("c", "3"), m("sent", "4")], older: Some("before-b".into()) };
        let p = merge(old, fresh);
        assert_eq!(p.msgs.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c", "sent"]);
        assert_eq!(p.older.as_deref(), Some("before-a"));
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
