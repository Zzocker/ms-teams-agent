
//! Teams' private chat service (undocumented; the Teams web client's own API): chats and their messages.

use crate::auth::{Session, User, agent};
use serde_json::Value;
use std::ops::Range;

mod html;
mod media;
mod parse;
pub mod events;

pub use html::*;
pub use media::*;
pub use parse::*;

#[derive(Clone, Debug, PartialEq)]
pub struct Chat {
    pub id: String,
    pub channel: bool,
    pub title: String,
    pub avatar_id: String,
    pub avatar_members: Vec<User>,
    pub preview: String,
    /// ISO 8601, UTC.
    pub time: String,
    pub unread: bool,
    /// Server and client IDs available before message history loads.
    pub latest: Option<ReadHorizon>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadHorizon {
    pub server_id: u64,
    pub client_id: String,
}

impl ReadHorizon {
    pub fn new(server_id: &str, client_id: &str) -> Option<Self> {
        if server_id.is_empty() || !server_id.bytes().all(|b| b.is_ascii_digit()) { return None; }
        Some(Self { server_id: server_id.parse().ok()?, client_id: if client_id.is_empty() { server_id } else { client_id }.into() })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Team {
    pub id: String,
    pub title: String,
    pub channels: Vec<Chat>,
}

pub fn teams(s: &Session) -> Result<Vec<Team>, String> {
    let access = crate::auth::teams_access()?;
    let region = s.middle_tier.rsplit('/').next().unwrap_or("").split('-').next().unwrap_or("");
    if !matches!(region, "amer" | "emea" | "apac") {
        return Err("Unknown Teams service region.".into());
    }
    let v = answer(agent()
        .get(&format!("https://teams.microsoft.com/api/csa/{region}/api/v3/teams/users/me?isPrefetch=false&enableMembershipSummary=true"))
        .header("Authorization", &format!("Bearer {access}"))
        .header("Authentication", &auth(s))
        .header("x-skypetoken", &s.skype_token)
        .call())?;
    parse_teams(&v)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Msg {
    /// The client message id the sender gave it (the server's id if none), so a message
    /// shown as it's sent keeps its identity when the server's copy replaces it.
    pub id: String,
    pub server_id: String,
    pub sender: String,
    pub sender_id: String,
    pub text: String,
    pub time: String,
    pub mine: bool,
    pub reply: Option<Reply>,
    /// Preview URLs of the pictures it carries (pasted screenshots, image files), from [`ams_preview`].
    pub images: Vec<String>,
    /// Web links in `text`: byte ranges and their URLs, from [`html_to_rich`].
    pub links: Vec<(Range<usize>, String)>,
    /// Shared files that aren't pictures (documents, Loop pages, videos), shown as cards.
    pub files: Vec<File>,
    /// The HTML it came as, for an edit to keep what the text doesn't hold (quote, pictures).
    pub raw: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct File {
    pub name: String,
    /// Its extension, lower case: `docx`, `loop`, `pdf`.
    pub kind: String,
    /// Where a click opens it.
    pub url: String,
    /// Its [`sharepoint_thumbnail`], if it's on a SharePoint site.
    pub preview: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub id: String,
    pub sender: String,
    pub time: String,
    pub text: String,
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

fn edit_answer(res: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<(), String> {
    let res = res.map_err(|e| e.to_string())?;
    // Successful edits may have no body; only reads require JSON.
    if res.status().is_success() { return Ok(()) }
    answer(Ok(res)).map(|_| ())
}

/// Sends `text` and pictures (from [`upload_image`], with their pixel sizes) to a chat as the signed-in user.
/// `id` is from [`new_msg_id`].
/// Posts a message; with `quote`, as a reply quoting it.
pub fn send(s: &Session, chat_id: &str, id: &str, text: &str, images: &[(String, (u32, u32))], quote: Option<&Msg>) -> Result<(), String> {
    let body = serde_json::json!({
        "content": format!("{}{}", quote.map(reply_html).unwrap_or_default(), message_html(text, images)),
        "messagetype": "RichText/Html",
        "contenttype": "text",
        "clientmessageid": id,
        "imdisplayname": s.user.name,
    });
    answer(agent().post(&url(s, &format!("conversations/{}/messages", encode(chat_id)))).header("Authentication", &auth(s)).send_json(&body)).map(|_| ())
}

/// Deletes a sent message; Teams keeps a "This message has been deleted" stub.
pub fn delete(s: &Session, chat_id: &str, server_id: &str) -> Result<(), String> {
    let url = url(s, &format!("conversations/{}/messages/{}", encode(chat_id), encode(server_id)));
    let response = agent().delete(&url).header("Authentication", &auth(s)).call().map_err(|e| e.to_string())?;
    match response.status() {
        st if st.is_success() => Ok(()),
        st if st == 401 => Err("Teams session expired; restart the app.".into()),
        st => Err(format!("chat service {st}")),
    }
}

/// Replaces a sent message's text with `text`, keeping its quoted reply and pictures, and adding
/// `images` (from [`upload_image`]) after them.
pub fn edit(s: &Session, chat_id: &str, server_id: &str, raw: &str, text: &str, images: &[(String, (u32, u32))]) -> Result<(), String> {
    let body = serde_json::json!({
        "content": edited_html(raw, text, images),
        "messagetype": "RichText/Html",
        "contenttype": "text",
        "imdisplayname": s.user.name,
    });
    let url = url(s, &format!("conversations/{}/messages/{}", encode(chat_id), encode(server_id)));
    edit_answer(agent().put(&url).header("Authentication", &auth(s)).send_json(&body))
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
    let last = &fresh.msgs.last().unwrap().time;
    let mut newer: Vec<Msg> = old.msgs.iter().filter(|m| m.time > *last
        && !fresh.msgs.iter().any(|f| f.id == m.id)).cloned().collect();
    let mut msgs: Vec<Msg> = old.msgs.into_iter()
        .filter(|m| m.time < first.time && !fresh.msgs.iter().any(|f| f.id == m.id))
        .collect();
    // Kept rows reach back to `old`'s start, so its link is the one before them.
    let older = if msgs.is_empty() { fresh.older } else { old.older };
    msgs.extend(fresh.msgs);
    msgs.append(&mut newer);
    Page { msgs, older }
}

/// The chats with guessed titles; [`name_chats`] gives them their people's names.
pub fn chats(s: &Session) -> Result<Vec<Chat>, String> {
    Ok(parse_chats(&get(s, "conversations?view=msnp24Equivalent&pageSize=100")?, &s.user))
}

fn conversations(s: &Session) -> Result<Vec<Chat>, String> {
    Ok(parse_conversations(&get(s, "conversations?view=msnp24Equivalent&pageSize=100")?, &s.user))
}

/// Resolves names and group avatar members after the initial chat list shows.
pub fn name_chats(s: &Session, mut chats: Vec<Chat>) -> Vec<Chat> {
    let me = format!("8:orgid:{}", s.user.id);
    // Chats carry no names. Who each is named after: a 1:1's other person (in its id), or the
    // group members for their avatar collage and untitled chat names.
    // The member lookups run at once: one after another they add up.
    let people: Vec<Vec<String>> = std::thread::scope(|scope| {
        let jobs: Vec<_> = chats.iter().map(|c| match peer(&c.id, &s.user.id) {
            Some(_) if c.avatar_id.starts_with("28:") => Ok(vec![c.avatar_id.clone()]),
            Some(p) => Ok(vec![format!("8:orgid:{p}")]),
            None if c.id.ends_with("@thread.v2") => Err(scope.spawn(|| members(s, &c.id).unwrap_or_default().into_iter().filter(|m| *m != me).collect())),
            None => Ok(vec![]),
        }).collect();
        jobs.into_iter().map(|j| j.unwrap_or_else(|h| h.join().unwrap_or_default())).collect()
    });
    for (chat, people) in chats.iter_mut().zip(&people) {
        if chat.id.ends_with("@thread.v2") {
            chat.avatar_members = people.iter().take(3).map(|mri| User {
                id: avatar_id(mri), name: "?".into(), ..User::default()
            }).collect();
        }
    }
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
        for member in &mut c.avatar_members {
            let mri = if member.id.starts_with("28:") { member.id.clone() } else { format!("8:orgid:{}", member.id) };
            if let Some(name) = name(&mri) { member.name = name.into(); }
        }
        if !names.is_empty() && (c.title == UNTITLED || peer(&c.id, &s.user.id).is_some()) {
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

pub fn mark_read(s: &Session, chat_id: &str, horizon: &ReadHorizon) -> Result<(), String> {
    let body = read_marker(horizon, std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_millis());
    let response = agent().put(&url(s, &format!("conversations/{}/properties?name=consumptionhorizon", encode(chat_id))))
        .header("Authentication", &auth(s)).send_json(&body).map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Could not save read status: {}", response.status()));
    }
    Ok(())
}

fn read_marker(horizon: &ReadHorizon, read_at: u128) -> Value {
    serde_json::json!({"consumptionhorizon": format!("{};{read_at};{}", horizon.server_id, horizon.client_id)})
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

    #[test]
    fn edit_responses_accept_success_without_requiring_json() {
        let response = |status, body: &str| Ok(ureq::http::Response::builder()
            .status(status).body(ureq::Body::builder().data(body)).unwrap());
        for (status, body) in [(200, ""), (204, ""), (200, "{\"ok\":true}")] {
            assert!(edit_answer(response(status, body)).is_ok());
        }
        assert!(edit_answer(response(401, "")).unwrap_err().contains("session expired"));
        assert!(edit_answer(response(403, "{\"message\":\"Denied\"}")).unwrap_err().contains("Denied"));
        assert!(edit_answer(response(500, "")).is_err());
        assert!(edit_answer(Err(ureq::Error::StatusCode(503))).is_err());
        assert!(answer(response(200, "")).is_err());
    }

    #[test]
    fn read_marker_keeps_server_and_client_message_ids_separate() {
        let message = super::Msg {
            id: "client-123".into(), server_id: "101".into(), sender: "Other".into(), sender_id: String::new(),
            text: "Hello".into(), time: String::new(), mine: false, reply: None, images: vec![], links: vec![], files: vec![], raw: String::new(),
        };
        let marker = super::read_marker(&ReadHorizon::new(&message.server_id, &message.id).unwrap(), 999);
        assert_eq!(marker["consumptionhorizon"], "101;999;client-123");
        let chat = serde_json::json!({
            "properties": marker,
            "lastMessage": {"id": "101", "from": "8:orgid:other"}
        });
        assert!(!super::chat_unread(&chat, "me"));
        for invalid in ["", "invalid", "+1", "18446744073709551616"] {
            assert!(ReadHorizon::new(invalid, "client-123").is_none());
        }
        assert_eq!(ReadHorizon::new("101", "").unwrap().client_id, "101");
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(1_794_673_185_538), "2026-11-14T16:19:45.538Z");
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn merge_keeps_older_rows_and_swaps_the_sent_copy() {
        let m = |id: &str, time: &str| Msg { id: id.into(), server_id: id.into(), sender: String::new(), sender_id: String::new(), text: id.into(), time: time.into(), mine: false, reply: None, images: vec![], links: vec![], files: vec![], raw: String::new() };
        let old = Page { msgs: vec![m("a", "1"), m("b", "2"), m("c", "3"), m("sent", "9")], older: Some("before-a".into()) };
        let fresh = Page { msgs: vec![m("b", "2"), m("c", "3"), m("sent", "4")], older: Some("before-b".into()) };
        let p = merge(old, fresh);
        assert_eq!(p.msgs.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c", "sent"]);
        assert_eq!(p.older.as_deref(), Some("before-a"));
    }

    #[test]
    fn merge_keeps_push_messages_newer_than_a_fetch_in_flight() {
        let m = |id: &str| Msg { id:id.into(), server_id:id.into(), sender:String::new(), sender_id:String::new(),
            text:id.into(), time:id.into(), mine:false, reply:None, images:vec![], links:vec![], files:vec![], raw:String::new() };
        let old = Page { msgs:vec![m("1"), m("3")], older:Some("older".into()) };
        let fetched = Page { msgs:vec![m("1"), m("2")], older:Some("older".into()) };
        assert_eq!(merge(old, fetched).msgs.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["1", "2", "3"]);
    }
}
